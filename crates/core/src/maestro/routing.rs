//! Deterministic, **zero-LLM** routing front-end for the Maestro (Task 408,
//! design/08 §3.5 / §6.3, PHASE4_PLANNING §4.7 — D2).
//!
//! The Maestro's text input is run through [`pre_parse`] **before** the LLM ever
//! sees it: `@workarea` / `@a,@b` fanout / `@all`/`@idle`/`@blocked` /
//! `/digest`/`/pause`/`/new` are parsed and dispatched without spending a single
//! token. The load-bearing property (design/08 §3.5, §6.3): **routing is
//! deterministic** — [`pre_parse`] is a pure non-`async` `fn(&str) ->
//! ParseOutcome` with no I/O, no handles, and no token spend, so the routing
//! path keeps working even when the LLM is inert (budget exhausted /
//! unreachable — design/08 §3.9 / §3.10).
//!
//! Two strictly-separated layers:
//!
//! 1. **Parse** ([`pre_parse`]) — pure string → [`ParseOutcome`]. A reviewer can
//!    see at a glance it cannot reach an LLM: it is not `async` and takes no
//!    handles.
//! 2. **Resolve + dispatch** ([`Router::resolve_targets`] / [`Router::dispatch`])
//!    — touch SQLite (the existing composer-sorted
//!    [`WorkareaManager::list_by_workspace`] +
//!    `started_at DESC` `sessions::list_by_workarea` read APIs) and
//!    [`AgentSupervisorHandle::send_input`], but never an LLM.
//!
//! Every routing failure is a typed [`RoutingError`] variant carrying enough to
//! synthesize the design/08 §8 assistant message (never `todo!()`/`unimplemented!()`,
//! never an empty-success silent no-op — mirrors Task 305's seam discipline).
//!
//! **What this task does NOT do** (these are seams, not omissions):
//! - The `SendToMaestro` wiring, the synthesized assistant message, chat-history
//!   recording, the `maestro.routing_executed` event, and the cross-workspace
//!   `@composer` ask-with-chips are **Task 414**'s. 408 returns typed outcomes;
//!   414 renders them.
//! - `/digest` execution is **Task 409**'s; `/pause`/`/new` execution belongs to
//!   their consuming tasks (406 / the session-creation flow). 408 only parses the
//!   directive into a typed marker.
//! - The `WorkareaSummary` cache + `BlockedReason` taxonomy are **Task 404**'s;
//!   408 classifies `@idle`/`@blocked` **directly from the existing
//!   `Workarea.status` / `Session.status` columns** so it depends only on 402
//!   (NOT 404). If 404/413's richer taxonomy later supersedes this, the
//!   classifier here ([`is_idle_status`] / [`is_blocked_workarea_status`]) is the
//!   seam to upgrade.

use std::sync::Arc;

use async_trait::async_trait;

use concerto_error::{Error, Result};
use concerto_persist::{Persistence, Session, SessionId, Workarea, WorkareaId, WorkspaceId};

use crate::agent_supervisor::AgentSupervisorHandle;
use crate::workspace_manager::WorkareaManager;

// ===========================================================================
// The FROZEN grammar (design/08 §3.5 / §6.3, PHASE4_PLANNING §4.7).
//
// The exact variants / field names below are the contract 409 (`/digest`) and
// 414 (`SendToMaestro` pre-parse) match on — they MUST NOT be re-shaped.
// ===========================================================================

/// Outcome of the pure [`pre_parse`] pass over a Maestro input line.
///
/// - [`ParseOutcome::Freeform`] — no `@`, no `/`: goes to the Maestro LLM
///   normally (design/08 §3.5).
/// - [`ParseOutcome::Routing`] — one or more `@`-targets (the fanout case is a
///   multi-element `targets`).
/// - [`ParseOutcome::Slash`] — one of the three V1.0 directives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    /// Plain text (no `@`, no recognized `/directive`). The agent decides.
    Freeform(String),
    /// `@`-routing: one or more targets + the body after the target span.
    Routing {
        /// The parsed targets (fanout = more than one).
        targets: Vec<RoutingTarget>,
        /// The user's original text after the target span (verbatim).
        body: String,
    },
    /// A recognized slash directive + the body after the directive token.
    Slash {
        /// One of `/digest`, `/pause`, `/new`.
        directive: SlashDirective,
        /// The user's original text after the directive token (verbatim).
        body: String,
    },
}

/// A single `@`-target. `@all`/`@idle`/`@blocked` are dynamic-set markers —
/// they are **resolved later** (at routing time over live state), NOT at parse
/// time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingTarget {
    /// `@bach` — the workarea by composer name (routes to its most-recently
    /// active live session).
    Workarea {
        /// Composer name (raw, as typed; matched case-insensitively at resolve).
        composer: String,
    },
    /// `@bach/claude` — a specific session within a workarea by agent kind.
    ///
    /// `agent_kind` is kept as the raw lowercased string (e.g. `"claude"`); it is
    /// matched against `Session.agent_kind` at resolve time, NOT pre-validated
    /// against the `AgentKind` enum. An unknown kind surfaces as a resolve-time
    /// [`RoutingError::NoMatchingSession`], not a parse error.
    Session {
        /// Composer name (raw, as typed; matched case-insensitively at resolve).
        composer: String,
        /// Raw lowercased agent kind (e.g. `"claude"`).
        agent_kind: String,
    },
    /// `@all` — every workarea with a live session (dynamic set).
    All,
    /// `@idle` — workareas whose newest live session is not actively working
    /// (dynamic set).
    Idle,
    /// `@blocked` — workareas in a blocked status (dynamic set).
    Blocked,
}

/// The three V1.0 slash directives (design/08 §3.5 / §2). An unrecognized
/// `/foo` is NOT a directive — it parses as [`ParseOutcome::Freeform`] (the
/// agent decides what to do with literal slash text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashDirective {
    /// `/digest` — consumed by Task 409.
    Digest,
    /// `/pause` — consumed by Task 406 (`set_workarea_paused`).
    Pause,
    /// `/new` — consumed by the session-creation flow.
    New,
}

/// One resolved routing target: a concrete `(workarea, session)` the body will
/// be dispatched to, with the composer / agent-kind labels the caller (414) uses
/// to synthesize the "Routed to bach / Claude" assistant message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    /// The resolved workarea.
    pub workarea_id: WorkareaId,
    /// The resolved (live) session within that workarea.
    pub session_id: SessionId,
    /// The workarea's composer name (for the synthesized message).
    pub composer: String,
    /// The session's agent kind (for the synthesized message).
    pub agent_kind: String,
}

/// A cross-workspace `@composer` ambiguity candidate.
///
/// Within one `workspace_id` composer names are unique, so [`Router::resolve_targets`]
/// (single-workspace) NEVER constructs this — it is defined here for the
/// **caller** (Task 414) to construct when it has to choose a workspace before
/// calling `resolve_targets` (the cross-workspace ask-with-chips branch,
/// PHASE4_PLANNING §2 / design/08 §3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkareaRef {
    /// The workspace the candidate workarea lives in.
    pub workspace_id: WorkspaceId,
    /// The candidate workarea.
    pub workarea_id: WorkareaId,
    /// The candidate workarea's composer name.
    pub composer: String,
}

/// A typed routing failure carrying enough to synthesize the design/08 §8
/// assistant message. Every failure is one of these variants — never a
/// `todo!()`/`unimplemented!()` macro, never an empty-success silent no-op
/// (mirrors Task 305's `ConeSuggestError::Unwired` seam discipline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingError {
    /// `@nonexistent` — no workarea by that composer in the workspace.
    /// `suggestions` is the composer-sorted name list ("did you mean bach /
    /// mozart?").
    NoSuchWorkarea {
        /// The composer that did not match.
        composer: String,
        /// Composer-sorted names of the workspace's non-archived workareas.
        suggestions: Vec<String>,
    },
    /// Cross-workspace `@composer` ambiguity. **Constructed by the caller (414),
    /// never returned by single-workspace [`Router::resolve_targets`]** — within
    /// one workspace composer names are unique (see [`WorkareaRef`]).
    AmbiguousComposer {
        /// The ambiguous composer.
        composer: String,
        /// The matching candidates across workspaces (caller offers as chips).
        candidates: Vec<WorkareaRef>,
    },
    /// The workarea exists but has no live session to route to ("`<target>` has
    /// no active agent. Start one?"). Also the mapping for `send_input`'s
    /// `Error::NotFound` at dispatch time.
    NoActiveAgent {
        /// The composer whose workarea has no live session.
        composer: String,
    },
    /// `@bach/claude` resolved a workarea but no live session matched the
    /// requested agent kind (e.g. an unknown kind, or no Claude session running).
    NoMatchingSession {
        /// The composer.
        composer: String,
        /// The requested (unmatched) agent kind.
        agent_kind: String,
    },
    /// A dynamic set (`@all`/`@idle`/`@blocked`) resolved to zero routes — NOT
    /// an empty-success silent no-op; the caller renders "no workareas are
    /// currently <set>".
    EmptyDynamicSet {
        /// `"all"` | `"idle"` | `"blocked"`.
        set: String,
    },
}

/// Per-route dispatch outcome (one per [`ResolvedRoute`]). [`Router::dispatch`]
/// does NOT synthesize the assistant message or touch chat history (that is
/// 414's job per the design/08 §3.5 3-step flow) — it returns the per-route
/// outcomes so 414 can record "Routed to …" and surface failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchResult {
    /// The route the body was sent to.
    pub route: ResolvedRoute,
    /// `Ok(())` if `send_input` accepted the bytes; `Err` (currently always
    /// [`RoutingError::NoActiveAgent`], from `send_input`'s `Error::NotFound`)
    /// otherwise.
    pub outcome: std::result::Result<(), RoutingError>,
}

// ===========================================================================
// pre_parse — the pure, deterministic, ZERO-LLM front-end.
//
// THE LOAD-BEARING RULE: this is a pure non-`async` `fn(&str) -> ParseOutcome`
// with no handles, no I/O, no token spend. That signature *is* the
// "routing is deterministic / zero-LLM" guarantee (design/08 §3.5, §6.3). A
// reviewer must be able to see at a glance that the parse path cannot reach an
// LLM. Resolution (`resolve_targets`/`dispatch`, which touch SQLite +
// `send_input`) is kept strictly separate.
// ===========================================================================

/// Pure, deterministic, **zero-LLM** pre-parse of a Maestro input line.
///
/// Order (design/08 §6.3): try `parse_slash` first (leading `/`), then
/// `parse_at` (leading `@`), else [`ParseOutcome::Freeform`]. No I/O, no async,
/// no token spend — that is the contract 409/414 build on.
pub fn pre_parse(input: &str) -> ParseOutcome {
    let trimmed = input.trim_start();
    if let Some((directive, body)) = parse_slash(trimmed) {
        return ParseOutcome::Slash { directive, body };
    }
    if let Some((targets, body)) = parse_at(trimmed) {
        return ParseOutcome::Routing { targets, body };
    }
    ParseOutcome::Freeform(input.to_owned())
}

/// Parse a leading `/directive`. Returns `None` for anything that is not one of
/// the three recognized directives — an unrecognized `/foo` is NOT a directive
/// (it falls through to [`ParseOutcome::Freeform`]; the agent decides what to do
/// with literal slash text). A bare `/` also returns `None`.
fn parse_slash(input: &str) -> Option<(SlashDirective, String)> {
    let rest = input.strip_prefix('/')?;
    // The directive token is the first whitespace-delimited word; the body is
    // everything after it (verbatim, leading whitespace stripped once).
    let (token, body) = split_first_token(rest);
    let directive = match token.to_ascii_lowercase().as_str() {
        "digest" => SlashDirective::Digest,
        "pause" => SlashDirective::Pause,
        "new" => SlashDirective::New,
        // Unrecognized `/foo` (or bare `/`, where token == "") is NOT a
        // directive — caller falls through to Freeform.
        _ => return None,
    };
    Some((directive, body.to_owned()))
}

/// Parse a leading `@`-target run: `@bach`, `@bach/claude`, or a comma-separated
/// fanout `@a,@b` (`@a,@b/claude`). Returns `None` if there is no `@` prefix or
/// no parseable target (a bare `@` with no token → `None` → caller falls through
/// to [`ParseOutcome::Freeform`]).
///
/// The target run is the first whitespace-delimited word; the body is everything
/// after it (verbatim).
fn parse_at(input: &str) -> Option<(Vec<RoutingTarget>, String)> {
    if !input.starts_with('@') {
        return None;
    }
    let (target_run, body) = split_first_token(input);

    let mut targets = Vec::new();
    for spec in target_run.split(',') {
        let spec = spec.trim();
        // Each comma-separated spec must itself start with `@`.
        let inner = spec.strip_prefix('@')?;
        if inner.is_empty() {
            // A bare `@` (e.g. "@" or "@,@bach") is not a valid target.
            return None;
        }
        targets.push(parse_one_target(inner)?);
    }
    if targets.is_empty() {
        return None;
    }
    Some((targets, body.to_owned()))
}

/// Parse the body of one `@`-spec (the text after the `@`): `bach` →
/// [`RoutingTarget::Workarea`], `bach/claude` → [`RoutingTarget::Session`],
/// `all`/`idle`/`blocked` → the dynamic-set markers. Composer / agent-kind are
/// kept raw (lowercased for agent kind) for case-insensitive resolve-time match.
fn parse_one_target(inner: &str) -> Option<RoutingTarget> {
    // Dynamic-set markers are matched case-insensitively and have no `/session`
    // suffix.
    match inner.to_ascii_lowercase().as_str() {
        "all" => return Some(RoutingTarget::All),
        "idle" => return Some(RoutingTarget::Idle),
        "blocked" => return Some(RoutingTarget::Blocked),
        _ => {}
    }
    match inner.split_once('/') {
        Some((composer, agent_kind)) => {
            if composer.is_empty() || agent_kind.is_empty() {
                return None;
            }
            Some(RoutingTarget::Session {
                composer: composer.to_owned(),
                agent_kind: agent_kind.to_ascii_lowercase(),
            })
        }
        None => Some(RoutingTarget::Workarea {
            composer: inner.to_owned(),
        }),
    }
}

/// Split off the first whitespace-delimited token; return `(token, body)` where
/// `body` is the remainder with its leading whitespace stripped (the user's
/// original text after the token span, preserved verbatim).
fn split_first_token(s: &str) -> (&str, &str) {
    match s.find(char::is_whitespace) {
        Some(idx) => (&s[..idx], s[idx..].trim_start()),
        None => (s, ""),
    }
}

// ===========================================================================
// Resolve + dispatch — the SQLite-backed, NEVER-LLM resolution layer.
//
// The `Router` holds handle clones of WorkareaManager + AgentSupervisorHandle +
// the readers pool (PHASE4_PLANNING §2). To keep the SQLite-backed resolution
// path unit-testable against an in-process fixture (and the dispatch path
// testable without a live PTY host), the two effects the Router needs are
// captured behind narrow async traits that the live handles implement.
// ===========================================================================

/// Read side the resolver needs: the existing composer-sorted
/// `list_by_workspace` + `started_at DESC` `list_by_workarea` APIs. Implemented
/// for the live [`WorkareaManager`] (over the readers pool); a fake implements
/// it for the resolver unit tests.
#[async_trait]
pub trait WorkareaReader: Send + Sync {
    /// Non-archived workareas in the workspace, **composer-sorted** (the
    /// `suggestions` list is just this sorted by `composer_name`).
    async fn list_workareas(&self, workspace_id: &WorkspaceId) -> Result<Vec<Workarea>>;
    /// Sessions for a workarea, **`started_at DESC`** (the first row is the
    /// most-recently-started session — the `newest_agent_kind` convention,
    /// workarea.rs:1585).
    async fn list_sessions(&self, workarea_id: &WorkareaId) -> Result<Vec<Session>>;
}

/// Send side dispatch needs: the only send-prompt path,
/// [`AgentSupervisorHandle::send_input`]. Implemented for the live handle; a
/// fake captures sends for the dispatch unit test.
#[async_trait]
pub trait InputSink: Send + Sync {
    /// Send `data` as the session's stdin. Returns `Error::NotFound` if the
    /// session is not running (mapped by [`Router::dispatch`] to
    /// [`RoutingError::NoActiveAgent`]).
    async fn send_input(&self, session_id: &SessionId, data: Vec<u8>) -> Result<()>;
}

/// Live read side: [`WorkareaManager::list_by_workspace`] (composer-sorted) +
/// `sessions::list_by_workarea` (`started_at DESC`) over the readers pool. No
/// new persist query is added — the two existing read APIs are reused verbatim.
struct LiveWorkareaReader {
    workareas: Arc<WorkareaManager>,
    persistence: Arc<Persistence>,
}

#[async_trait]
impl WorkareaReader for LiveWorkareaReader {
    async fn list_workareas(&self, workspace_id: &WorkspaceId) -> Result<Vec<Workarea>> {
        self.workareas.list_by_workspace(workspace_id, false).await
    }
    async fn list_sessions(&self, workarea_id: &WorkareaId) -> Result<Vec<Session>> {
        concerto_persist::sessions::list_by_workarea(self.persistence.readers(), workarea_id).await
    }
}

#[async_trait]
impl InputSink for AgentSupervisorHandle {
    async fn send_input(&self, session_id: &SessionId, data: Vec<u8>) -> Result<()> {
        AgentSupervisorHandle::send_input(self, session_id, data).await
    }
}

/// The deterministic, zero-LLM router. Resolves [`RoutingTarget`]s to concrete
/// `(workarea, session)` routes within one explicit workspace and dispatches a
/// body to each via `send_input`. Holds handle clones of the read + send sides
/// (PHASE4_PLANNING §2) behind [`WorkareaReader`] / [`InputSink`].
#[derive(Clone)]
pub struct Router {
    reader: Arc<dyn WorkareaReader>,
    sink: Arc<dyn InputSink>,
}

impl Router {
    /// Build a live router from the Workarea Manager + Agent Supervisor handle +
    /// the readers pool (PHASE4_PLANNING §2). No server-side active-workspace
    /// exists — [`Router::resolve_targets`] takes an explicit `workspace_id`.
    pub fn new(
        workareas: Arc<WorkareaManager>,
        supervisor: AgentSupervisorHandle,
        persistence: Arc<Persistence>,
    ) -> Self {
        Self {
            reader: Arc::new(LiveWorkareaReader {
                workareas,
                persistence,
            }),
            sink: Arc::new(supervisor),
        }
    }

    /// Construct a router from explicit read + send sides (for tests / for 406's
    /// write-tools to reuse with the live handles wrapped).
    pub fn from_parts(reader: Arc<dyn WorkareaReader>, sink: Arc<dyn InputSink>) -> Self {
        Self { reader, sink }
    }

    /// Resolve every target **within one explicit `workspace_id`** (there is no
    /// server-side active-workspace; the caller — 414 — passes the workspace the
    /// Maestro message is scoped to).
    ///
    /// Static targets (`Workarea`/`Session`) resolve via the composer-sorted
    /// `list_by_workspace` + `started_at DESC` `list_by_workarea`, picking the
    /// most-recently-active **live** session. Dynamic sets
    /// (`All`/`Idle`/`Blocked`) fan out over all non-archived workareas,
    /// classified deterministically from the existing `Workarea.status` /
    /// `Session.status` columns (NOT the 404 summary cache).
    ///
    /// Single-workspace resolution NEVER returns
    /// [`RoutingError::AmbiguousComposer`] — within one workspace composer names
    /// are unique; that variant is the caller's (414) to construct for the
    /// cross-workspace branch (see [`WorkareaRef`]).
    pub async fn resolve_targets(
        &self,
        workspace_id: &WorkspaceId,
        targets: &[RoutingTarget],
    ) -> std::result::Result<Vec<ResolvedRoute>, RoutingError> {
        // One snapshot of the workspace's non-archived workareas (composer-
        // sorted) feeds every target in this batch.
        let workareas = self
            .reader
            .list_workareas(workspace_id)
            .await
            .map_err(|_| RoutingError::EmptyDynamicSet {
                set: "all".to_string(),
            })?;

        let mut routes = Vec::new();
        for target in targets {
            match target {
                RoutingTarget::Workarea { composer } => {
                    routes.push(self.resolve_workarea(&workareas, composer).await?);
                }
                RoutingTarget::Session {
                    composer,
                    agent_kind,
                } => {
                    routes.push(
                        self.resolve_session(&workareas, composer, agent_kind)
                            .await?,
                    );
                }
                RoutingTarget::All => {
                    routes.extend(self.resolve_dynamic(&workareas, DynamicSet::All).await?);
                }
                RoutingTarget::Idle => {
                    routes.extend(self.resolve_dynamic(&workareas, DynamicSet::Idle).await?);
                }
                RoutingTarget::Blocked => {
                    routes.extend(
                        self.resolve_dynamic(&workareas, DynamicSet::Blocked)
                            .await?,
                    );
                }
            }
        }
        Ok(routes)
    }

    /// Resolve `@composer` → the workarea's most-recently-active live session.
    async fn resolve_workarea(
        &self,
        workareas: &[Workarea],
        composer: &str,
    ) -> std::result::Result<ResolvedRoute, RoutingError> {
        let wa = find_workarea(workareas, composer)?;
        let sessions = self.list_sessions(&wa.id).await;
        // "Most-recently-active" = the first `started_at DESC` row that is still
        // live (`ended_at IS NULL` / status not ended) — the `newest_agent_kind`
        // convention (workarea.rs:1585).
        let session = sessions
            .iter()
            .find(|s| is_live_session(s))
            .ok_or_else(|| RoutingError::NoActiveAgent {
                composer: wa.composer_name.clone(),
            })?;
        Ok(ResolvedRoute {
            workarea_id: wa.id.clone(),
            session_id: session.id.clone(),
            composer: wa.composer_name.clone(),
            agent_kind: session.agent_kind.clone(),
        })
    }

    /// Resolve `@composer/agent_kind` → the live session of that agent kind.
    async fn resolve_session(
        &self,
        workareas: &[Workarea],
        composer: &str,
        agent_kind: &str,
    ) -> std::result::Result<ResolvedRoute, RoutingError> {
        let wa = find_workarea(workareas, composer)?;
        let sessions = self.list_sessions(&wa.id).await;
        let session = sessions
            .iter()
            .find(|s| is_live_session(s) && s.agent_kind.eq_ignore_ascii_case(agent_kind))
            .ok_or_else(|| RoutingError::NoMatchingSession {
                composer: wa.composer_name.clone(),
                agent_kind: agent_kind.to_string(),
            })?;
        Ok(ResolvedRoute {
            workarea_id: wa.id.clone(),
            session_id: session.id.clone(),
            composer: wa.composer_name.clone(),
            agent_kind: session.agent_kind.clone(),
        })
    }

    /// Resolve a dynamic set (`@all`/`@idle`/`@blocked`) to zero-or-more routes
    /// (a fanout) over the non-archived workareas, classified from the existing
    /// status columns. An empty result is [`RoutingError::EmptyDynamicSet`], NOT
    /// an empty-success.
    async fn resolve_dynamic(
        &self,
        workareas: &[Workarea],
        set: DynamicSet,
    ) -> std::result::Result<Vec<ResolvedRoute>, RoutingError> {
        let mut routes = Vec::new();
        for wa in workareas {
            // `@blocked` classifies off the workarea status alone (no session
            // needed); `@all`/`@idle` need the newest live session.
            if set == DynamicSet::Blocked {
                if is_blocked_workarea_status(&wa.status) {
                    if let Some(session) = self
                        .list_sessions(&wa.id)
                        .await
                        .iter()
                        .find(|s| is_live_session(s))
                    {
                        routes.push(make_route(wa, session));
                    }
                }
                continue;
            }
            let sessions = self.list_sessions(&wa.id).await;
            let Some(session) = sessions.iter().find(|s| is_live_session(s)) else {
                continue; // `@all`/`@idle` require a live session.
            };
            let include = match set {
                DynamicSet::All => true,
                DynamicSet::Idle => is_idle_status(&wa.status, &session.status),
                DynamicSet::Blocked => unreachable!("handled above"),
            };
            if include {
                routes.push(make_route(wa, session));
            }
        }
        if routes.is_empty() {
            return Err(RoutingError::EmptyDynamicSet {
                set: set.label().to_string(),
            });
        }
        Ok(routes)
    }

    /// Send `body` to each resolved session via [`InputSink::send_input`]
    /// (`AgentSupervisorHandle::send_input` live). Maps `send_input`'s
    /// `Error::NotFound` → [`RoutingError::NoActiveAgent`]. Does NOT synthesize
    /// the assistant message or touch chat history (that is 414's job) — returns
    /// the per-route outcomes so 414 can render "Routed to …" + surface failures.
    pub async fn dispatch(&self, routes: &[ResolvedRoute], body: &str) -> Vec<DispatchResult> {
        let bytes = body.as_bytes();
        let mut results = Vec::with_capacity(routes.len());
        for route in routes {
            let outcome = match self
                .sink
                .send_input(&route.session_id, bytes.to_vec())
                .await
            {
                Ok(()) => Ok(()),
                Err(Error::NotFound(_)) => Err(RoutingError::NoActiveAgent {
                    composer: route.composer.clone(),
                }),
                // Any other transport error also means the route could not be
                // delivered to a live agent; surface it as NoActiveAgent so the
                // caller renders a single "no active agent" message rather than
                // leaking a raw internal error to the user.
                Err(_) => Err(RoutingError::NoActiveAgent {
                    composer: route.composer.clone(),
                }),
            };
            results.push(DispatchResult {
                route: route.clone(),
                outcome,
            });
        }
        results
    }

    /// Sessions for a workarea (`started_at DESC`), or empty on a read error
    /// (a read failure surfaces downstream as `NoActiveAgent`/empty-set, never a
    /// panic).
    async fn list_sessions(&self, workarea_id: &WorkareaId) -> Vec<Session> {
        self.reader
            .list_sessions(workarea_id)
            .await
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DynamicSet {
    All,
    Idle,
    Blocked,
}

impl DynamicSet {
    fn label(self) -> &'static str {
        match self {
            DynamicSet::All => "all",
            DynamicSet::Idle => "idle",
            DynamicSet::Blocked => "blocked",
        }
    }
}

/// Find a workarea by composer name (case-insensitive). On miss, returns
/// [`RoutingError::NoSuchWorkarea`] with the composer-sorted name list as
/// `suggestions` ("did you mean bach / mozart?"). `workareas` is already
/// composer-sorted by `list_by_workspace`.
fn find_workarea<'a>(
    workareas: &'a [Workarea],
    composer: &str,
) -> std::result::Result<&'a Workarea, RoutingError> {
    workareas
        .iter()
        .find(|wa| wa.composer_name.eq_ignore_ascii_case(composer))
        .ok_or_else(|| RoutingError::NoSuchWorkarea {
            composer: composer.to_string(),
            suggestions: workareas
                .iter()
                .map(|wa| wa.composer_name.clone())
                .collect(),
        })
}

fn make_route(wa: &Workarea, session: &Session) -> ResolvedRoute {
    ResolvedRoute {
        workarea_id: wa.id.clone(),
        session_id: session.id.clone(),
        composer: wa.composer_name.clone(),
        agent_kind: session.agent_kind.clone(),
    }
}

/// A session is "live" (routable) when it has not ended and is not in a terminal
/// status. Mirrors the `Session.status` taxonomy (`starting|running|awaiting|
/// finished|crashed`, migration 0001).
fn is_live_session(s: &Session) -> bool {
    s.ended_at.is_none() && !matches!(s.status.as_str(), "finished" | "crashed")
}

/// `@idle` classifier — **from the existing status columns, NOT the 404 summary
/// cache** (keeps 408 dependent only on 402). A workarea is idle when its newest
/// live session is not actively working: the workarea is paused/idle/awaiting, or
/// the session is awaiting/idle. If 404/413's richer taxonomy later supersedes
/// this, this is the seam to upgrade (Handoff).
fn is_idle_status(workarea_status: &str, session_status: &str) -> bool {
    matches!(workarea_status, "paused" | "idle" | "awaiting")
        || matches!(session_status, "awaiting" | "idle")
}

/// `@blocked` classifier — **from the existing `Workarea.status` column, NOT the
/// 404 `BlockedReason` cache**. Mirrors the `BlockedReason` notion from
/// design/08 §3.3 (`awaiting_approval` / `test_failure` / `merge_conflict`) plus
/// the generic `blocked` status, classified deterministically. Seam for 404/413
/// to refine (Handoff).
fn is_blocked_workarea_status(workarea_status: &str) -> bool {
    matches!(
        workarea_status,
        "awaiting_approval" | "test_failure" | "merge_conflict" | "blocked"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // (1) pre_parse — the table-driven grammar suite (standalone, no handles).
    // -----------------------------------------------------------------------

    fn workarea(composer: &str) -> RoutingTarget {
        RoutingTarget::Workarea {
            composer: composer.to_string(),
        }
    }

    #[test]
    fn pre_parse_workarea_target() {
        assert_eq!(
            pre_parse("@bach apply the migration pattern"),
            ParseOutcome::Routing {
                targets: vec![workarea("bach")],
                body: "apply the migration pattern".to_string(),
            }
        );
    }

    #[test]
    fn pre_parse_session_target() {
        assert_eq!(
            pre_parse("@bach/claude run the suite"),
            ParseOutcome::Routing {
                targets: vec![RoutingTarget::Session {
                    composer: "bach".to_string(),
                    agent_kind: "claude".to_string(),
                }],
                body: "run the suite".to_string(),
            }
        );
    }

    #[test]
    fn pre_parse_fanout_two_targets() {
        assert_eq!(
            pre_parse("@bach,@mozart status please"),
            ParseOutcome::Routing {
                targets: vec![workarea("bach"), workarea("mozart")],
                body: "status please".to_string(),
            }
        );
    }

    #[test]
    fn pre_parse_fanout_mixed_session() {
        assert_eq!(
            pre_parse("@bach,@mozart/codex go"),
            ParseOutcome::Routing {
                targets: vec![
                    workarea("bach"),
                    RoutingTarget::Session {
                        composer: "mozart".to_string(),
                        agent_kind: "codex".to_string(),
                    },
                ],
                body: "go".to_string(),
            }
        );
    }

    #[test]
    fn pre_parse_dynamic_sets() {
        assert!(matches!(
            pre_parse("@all status"),
            ParseOutcome::Routing { targets, body } if targets == vec![RoutingTarget::All] && body == "status"
        ));
        assert!(matches!(
            pre_parse("@idle nudge"),
            ParseOutcome::Routing { targets, .. } if targets == vec![RoutingTarget::Idle]
        ));
        assert!(matches!(
            pre_parse("@blocked why"),
            ParseOutcome::Routing { targets, .. } if targets == vec![RoutingTarget::Blocked]
        ));
        // Dynamic-set markers are case-insensitive.
        assert!(matches!(
            pre_parse("@ALL ping"),
            ParseOutcome::Routing { targets, .. } if targets == vec![RoutingTarget::All]
        ));
    }

    #[test]
    fn pre_parse_slash_directives() {
        assert_eq!(
            pre_parse("/digest"),
            ParseOutcome::Slash {
                directive: SlashDirective::Digest,
                body: String::new(),
            }
        );
        assert_eq!(
            pre_parse("/pause bach"),
            ParseOutcome::Slash {
                directive: SlashDirective::Pause,
                body: "bach".to_string(),
            }
        );
        assert_eq!(
            pre_parse("/new"),
            ParseOutcome::Slash {
                directive: SlashDirective::New,
                body: String::new(),
            }
        );
        // Case-insensitive directive token.
        assert_eq!(
            pre_parse("/DIGEST"),
            ParseOutcome::Slash {
                directive: SlashDirective::Digest,
                body: String::new(),
            }
        );
    }

    #[test]
    fn pre_parse_unknown_slash_is_freeform() {
        // `/foo` is NOT a directive — it goes to the agent as literal text.
        assert_eq!(
            pre_parse("/foo bar"),
            ParseOutcome::Freeform("/foo bar".to_string())
        );
    }

    #[test]
    fn pre_parse_plain_text_is_freeform() {
        assert_eq!(
            pre_parse("just talking to the maestro"),
            ParseOutcome::Freeform("just talking to the maestro".to_string())
        );
    }

    #[test]
    fn pre_parse_bare_at_and_slash_are_freeform() {
        assert_eq!(pre_parse("@"), ParseOutcome::Freeform("@".to_string()));
        assert_eq!(
            pre_parse("@ hi"),
            ParseOutcome::Freeform("@ hi".to_string())
        );
        assert_eq!(pre_parse("/"), ParseOutcome::Freeform("/".to_string()));
        // An empty agent-kind suffix is not a valid target → freeform.
        assert_eq!(
            pre_parse("@bach/ hi"),
            ParseOutcome::Freeform("@bach/ hi".to_string())
        );
        // A malformed fanout member voids the whole parse → freeform.
        assert_eq!(
            pre_parse("@,@bach hi"),
            ParseOutcome::Freeform("@,@bach hi".to_string())
        );
    }

    #[test]
    fn pre_parse_body_span_preserved_verbatim() {
        // Inner punctuation / casing / extra spaces in the body are preserved
        // verbatim; only the single run of whitespace after the target span is
        // consumed.
        assert_eq!(
            pre_parse("@bach   Apply: the FIX, please."),
            ParseOutcome::Routing {
                targets: vec![workarea("bach")],
                body: "Apply: the FIX, please.".to_string(),
            }
        );
        // A target with no body yields an empty body.
        assert_eq!(
            pre_parse("@bach"),
            ParseOutcome::Routing {
                targets: vec![workarea("bach")],
                body: String::new(),
            }
        );
    }

    #[test]
    fn pre_parse_is_pure_no_io_no_async() {
        // The zero-LLM / zero-I/O guarantee is enforced structurally:
        // `pre_parse` is a non-`async` `fn(&str) -> ParseOutcome` taking no
        // handles. This test pins the signature via a function pointer (it would
        // not compile if `pre_parse` were `async` or took a handle) and asserts
        // calling it many times has no observable side effect beyond its return.
        let f: fn(&str) -> ParseOutcome = pre_parse;
        for _ in 0..1000 {
            let _ = f("@bach run");
            let _ = f("/digest");
            let _ = f("plain");
        }
        // Determinism: same input → same output, every time.
        assert_eq!(f("@bach run"), f("@bach run"));
    }

    // -----------------------------------------------------------------------
    // (2)+(3) resolver + dispatch — against in-process fakes (no PTY host).
    // -----------------------------------------------------------------------

    fn wa(id: &str, composer: &str, status: &str) -> Workarea {
        Workarea {
            id: WorkareaId(id.to_string()),
            workspace_id: WorkspaceId("ws".to_string()),
            composer_name: composer.to_string(),
            branch_name: "b".to_string(),
            worktree_root: "/tmp".to_string(),
            status: status.to_string(),
            permission_mode: None,
            created_at: 0,
            archived_at: None,
            last_activity_at: None,
            settings_json: "{}".to_string(),
        }
    }

    fn sess(id: &str, workarea: &str, agent_kind: &str, started_at: i64, status: &str) -> Session {
        Session {
            id: SessionId(id.to_string()),
            workarea_id: WorkareaId(workarea.to_string()),
            chat_id: "c".to_string(),
            agent_kind: agent_kind.to_string(),
            agent_version: None,
            model: None,
            mode: None,
            host_pid: None,
            host_socket: None,
            pty_cookie: None,
            external_session_id: None,
            permission_mode: "normal".to_string(),
            bypass_destructive_guard: false,
            started_at,
            ended_at: if matches!(status, "finished" | "crashed") {
                Some(started_at + 1)
            } else {
                None
            },
            last_heartbeat: None,
            status: status.to_string(),
            last_acked_seq: 0,
        }
    }

    /// In-process reader: workareas (kept composer-sorted, like
    /// `list_by_workspace`) + per-workarea sessions (kept `started_at DESC`,
    /// like `list_by_workarea`).
    struct FakeReader {
        workareas: Vec<Workarea>,
        sessions: HashMap<String, Vec<Session>>,
    }

    impl FakeReader {
        fn new(mut workareas: Vec<Workarea>, mut sessions: HashMap<String, Vec<Session>>) -> Self {
            workareas.sort_by(|a, b| a.composer_name.cmp(&b.composer_name));
            for v in sessions.values_mut() {
                v.sort_by_key(|s| std::cmp::Reverse(s.started_at));
            }
            Self {
                workareas,
                sessions,
            }
        }
    }

    #[async_trait]
    impl WorkareaReader for FakeReader {
        async fn list_workareas(&self, _workspace_id: &WorkspaceId) -> Result<Vec<Workarea>> {
            Ok(self.workareas.clone())
        }
        async fn list_sessions(&self, workarea_id: &WorkareaId) -> Result<Vec<Session>> {
            Ok(self
                .sessions
                .get(&workarea_id.0)
                .cloned()
                .unwrap_or_default())
        }
    }

    /// Capturing sink: records `(session_id, body)` sends; optionally returns
    /// `NotFound` for designated session ids (to exercise the
    /// `NotFound → NoActiveAgent` mapping).
    #[derive(Default)]
    struct FakeSink {
        sent: Mutex<Vec<(String, Vec<u8>)>>,
        not_found: Vec<String>,
    }

    #[async_trait]
    impl InputSink for FakeSink {
        async fn send_input(&self, session_id: &SessionId, data: Vec<u8>) -> Result<()> {
            if self.not_found.iter().any(|s| s == &session_id.0) {
                return Err(Error::NotFound(format!(
                    "session {} not running",
                    session_id.0
                )));
            }
            self.sent.lock().unwrap().push((session_id.0.clone(), data));
            Ok(())
        }
    }

    fn ws() -> WorkspaceId {
        WorkspaceId("ws".to_string())
    }

    /// Two workareas `bach`/`mozart`; `bach` has an older finished claude
    /// session + a newer live codex session + an even-newer live claude session.
    fn fixture() -> FakeReader {
        let workareas = vec![
            wa("wa-bach", "bach", "running"),
            wa("wa-mozart", "mozart", "paused"),
        ];
        let mut sessions = HashMap::new();
        sessions.insert(
            "wa-bach".to_string(),
            vec![
                sess("s-old-claude", "wa-bach", "claude", 100, "finished"),
                sess("s-codex", "wa-bach", "codex", 200, "running"),
                sess("s-claude", "wa-bach", "claude", 300, "running"),
            ],
        );
        sessions.insert(
            "wa-mozart".to_string(),
            vec![sess("s-mozart", "wa-mozart", "gemini", 150, "awaiting")],
        );
        FakeReader::new(workareas, sessions)
    }

    fn router_with(reader: FakeReader, sink: FakeSink) -> (Router, Arc<FakeSink>) {
        let sink = Arc::new(sink);
        let router = Router::from_parts(Arc::new(reader), sink.clone());
        (router, sink)
    }

    #[tokio::test]
    async fn resolve_workarea_picks_most_recently_active_live_session() {
        let (router, _) = router_with(fixture(), FakeSink::default());
        let routes = router
            .resolve_targets(&ws(), &[workarea("bach")])
            .await
            .expect("resolve");
        assert_eq!(routes.len(), 1);
        // The newest *live* session is s-claude (started_at 300); the older
        // finished claude (100) is skipped.
        assert_eq!(routes[0].session_id.0, "s-claude");
        assert_eq!(routes[0].composer, "bach");
        assert_eq!(routes[0].agent_kind, "claude");
    }

    #[tokio::test]
    async fn resolve_session_filters_by_agent_kind() {
        let (router, _) = router_with(fixture(), FakeSink::default());
        let target = RoutingTarget::Session {
            composer: "bach".to_string(),
            agent_kind: "codex".to_string(),
        };
        let routes = router
            .resolve_targets(&ws(), &[target])
            .await
            .expect("resolve");
        assert_eq!(routes[0].session_id.0, "s-codex");
        assert_eq!(routes[0].agent_kind, "codex");
    }

    #[tokio::test]
    async fn resolve_session_unknown_kind_is_no_matching_session() {
        let (router, _) = router_with(fixture(), FakeSink::default());
        let target = RoutingTarget::Session {
            composer: "bach".to_string(),
            agent_kind: "llama".to_string(),
        };
        let err = router.resolve_targets(&ws(), &[target]).await.unwrap_err();
        assert_eq!(
            err,
            RoutingError::NoMatchingSession {
                composer: "bach".to_string(),
                agent_kind: "llama".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn resolve_unknown_workarea_is_no_such_workarea_with_sorted_suggestions() {
        let (router, _) = router_with(fixture(), FakeSink::default());
        let err = router
            .resolve_targets(&ws(), &[workarea("nonexistent")])
            .await
            .unwrap_err();
        assert_eq!(
            err,
            RoutingError::NoSuchWorkarea {
                composer: "nonexistent".to_string(),
                // Composer-sorted (bach < mozart).
                suggestions: vec!["bach".to_string(), "mozart".to_string()],
            }
        );
    }

    #[tokio::test]
    async fn resolve_workarea_with_no_live_session_is_no_active_agent() {
        // mozart's only session is finished → no live session.
        let workareas = vec![wa("wa-mozart", "mozart", "running")];
        let mut sessions = HashMap::new();
        sessions.insert(
            "wa-mozart".to_string(),
            vec![sess("s-dead", "wa-mozart", "claude", 10, "finished")],
        );
        let (router, _) = router_with(FakeReader::new(workareas, sessions), FakeSink::default());
        let err = router
            .resolve_targets(&ws(), &[workarea("mozart")])
            .await
            .unwrap_err();
        assert_eq!(
            err,
            RoutingError::NoActiveAgent {
                composer: "mozart".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn resolve_all_fans_out_to_every_workarea_with_a_live_session() {
        let (router, _) = router_with(fixture(), FakeSink::default());
        let routes = router
            .resolve_targets(&ws(), &[RoutingTarget::All])
            .await
            .expect("resolve");
        // Both bach (live) and mozart (awaiting = live) have a live session.
        let mut composers: Vec<_> = routes.iter().map(|r| r.composer.clone()).collect();
        composers.sort();
        assert_eq!(composers, vec!["bach".to_string(), "mozart".to_string()]);
    }

    #[tokio::test]
    async fn resolve_idle_classifies_by_status() {
        let (router, _) = router_with(fixture(), FakeSink::default());
        let routes = router
            .resolve_targets(&ws(), &[RoutingTarget::Idle])
            .await
            .expect("resolve");
        // mozart is paused (idle); bach is running (not idle).
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].composer, "mozart");
    }

    #[tokio::test]
    async fn resolve_blocked_classifies_by_workarea_status() {
        let workareas = vec![
            wa("wa-bach", "bach", "test_failure"),
            wa("wa-mozart", "mozart", "running"),
        ];
        let mut sessions = HashMap::new();
        sessions.insert(
            "wa-bach".to_string(),
            vec![sess("s-bach", "wa-bach", "claude", 100, "running")],
        );
        sessions.insert(
            "wa-mozart".to_string(),
            vec![sess("s-mozart", "wa-mozart", "claude", 100, "running")],
        );
        let (router, _) = router_with(FakeReader::new(workareas, sessions), FakeSink::default());
        let routes = router
            .resolve_targets(&ws(), &[RoutingTarget::Blocked])
            .await
            .expect("resolve");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].composer, "bach");
    }

    #[tokio::test]
    async fn resolve_empty_dynamic_set_is_typed_error() {
        // No workarea is blocked → EmptyDynamicSet, NOT an empty success.
        let (router, _) = router_with(fixture(), FakeSink::default());
        let err = router
            .resolve_targets(&ws(), &[RoutingTarget::Blocked])
            .await
            .unwrap_err();
        assert_eq!(
            err,
            RoutingError::EmptyDynamicSet {
                set: "blocked".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn dispatch_routes_body_bytes_to_resolved_session() {
        let (router, sink) = router_with(fixture(), FakeSink::default());
        let routes = router
            .resolve_targets(&ws(), &[workarea("bach")])
            .await
            .expect("resolve");
        let results = router.dispatch(&routes, "run the e2e suite").await;
        assert_eq!(results.len(), 1);
        assert!(results[0].outcome.is_ok());
        let sent = sink.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].0, "s-claude");
        assert_eq!(sent[0].1, b"run the e2e suite".to_vec());
    }

    #[tokio::test]
    async fn dispatch_not_found_maps_to_no_active_agent() {
        let sink = FakeSink {
            not_found: vec!["s-claude".to_string()],
            ..Default::default()
        };
        let (router, _) = router_with(fixture(), sink);
        let routes = router
            .resolve_targets(&ws(), &[workarea("bach")])
            .await
            .expect("resolve");
        let results = router.dispatch(&routes, "go").await;
        assert_eq!(
            results[0].outcome,
            Err(RoutingError::NoActiveAgent {
                composer: "bach".to_string(),
            })
        );
    }
}
