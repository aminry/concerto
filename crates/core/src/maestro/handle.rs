//! The `MaestroHandle` Core-side API surface (Task 401.5 froze the signatures;
//! Task 414 fills the live impl — design/08 §5.2, PHASE4_PLANNING §4.2).
//!
//! ## Drift from 401.5 (path (a), documented in 414's Handoff)
//!
//! 401.5 froze [`MaestroHandle`] as an **opaque** struct (`_opaque: ()`) whose
//! five async signatures returned typed `"unimplemented:"` errors. 414 lights
//! the service up end-to-end, which the task explicitly permits to require
//! filling the handle's real impl here (the alternative — duplicating the
//! routing/digest/visibility logic inside `handlers/maestro.rs` — would make the
//! handler more than the "thin adapter" the design asks for). So this struct now
//! carries the shared Core handles + the deterministic seams it stitches:
//!
//! - 408's [`crate::maestro::routing`] (`pre_parse` + `Router`) for `@workarea`
//!   routing,
//! - 409's [`crate::maestro::digest::generate_digest`] over 404's
//!   [`SummaryCache`] for `GetDigest`,
//! - 413's `set_exclude_from_maestro` toggle for `SetWorkareaVisibility`,
//! - 414's [`MaestroEventSender`] for `maestro.events` publishing.
//!
//! The **five frozen signatures do not change** — only the body + the (private)
//! fields. 415 and the rest of the spine still build against the same surface.
//!
//! The handle is `#[cfg(unix)]` transitively (the whole `maestro` module is),
//! and is constructed once at boot ([`crate::boot`]) gated on
//! `maestro_state.enabled` AND the managed-policy model permission (D1). When
//! the gate is closed the handle is simply never constructed (`None` at the
//! service sites) and the service replies `disabled_by_policy`.

use std::sync::Arc;

use concerto_error::{Error, Result};
use concerto_persist::{Persistence, WorkareaId, WorkspaceId};
use concerto_proto::v1::{
    Digest as ProtoDigest, MaestroAttachment, MaestroChip, MaestroVisibility,
};
use tokio::sync::Mutex;

use crate::agent_supervisor::AgentSupervisorHandle;
use crate::llm::oneshot::OneShotLlm;
use crate::maestro::digest::{generate_digest, Digest as DigestModel};
use crate::maestro::events::{MaestroEvent, MaestroEventSender};
use crate::maestro::routing::{pre_parse, ParseOutcome, Router, SlashDirective};
use crate::maestro::summary::{SummaryCache, GET_DIGEST_STALE_MS};
use crate::workspace_manager::WorkareaManager;

/// A minimal Core-side read-model of the Maestro's live state (design/08 §4.1
/// `maestro_state`). The `get_state` return shape; filled from Task 403's
/// `maestro_state` singleton row. All instants are `i64` unix-ms
/// (PHASE4_PLANNING §2 — NOT `Instant`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaestroStateView {
    /// Whether the Maestro is enabled (vs disabled by the user or by
    /// `enterpriseDataPrivacy` policy — design/08 §3.10).
    pub enabled: bool,
    /// Input tokens spent today (the cumulative-across-backends daily budget,
    /// design/08 §3.9). Owned/wired by Task 403/412.
    pub daily_in_today: i64,
    /// Output tokens spent today.
    pub daily_out_today: i64,
    /// Unix-ms of the last generated digest, or `None` if none yet.
    pub last_digest_at_ms: Option<i64>,
}

/// The outcome of a `SendToMaestro` dispatch, surfaced to the handler so it can
/// shape the gRPC reply + drive the right [`MaestroEvent`]. Kept here (not in
/// the proto) because `SendToMaestro` returns `Empty` on the wire — the dispatch
/// outcome rides `maestro.events`, not the unary response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// Freeform text forwarded to the Maestro session (a `maestro.message`
    /// event carries the streamed assistant output later; here we record that
    /// the input was accepted).
    Forwarded,
    /// `@workarea` routing executed — the body was dispatched to these resolved
    /// composer targets (drives `maestro.routing_executed`).
    Routed { targets: Vec<String> },
    /// `/digest` handled deterministically (drives `maestro.digest_generated`).
    Digested { n_workareas: u32 },
    /// `/pause` / `/new` recognized; the directive marker was handled (no LLM
    /// spend). The body is forwarded to the resolved session if any.
    SlashHandled,
}

/// The budget/policy disable reason a constructed handle may carry. A handle
/// that is `None` at the service sites is the policy-disabled-at-boot case
/// (the service replies `failed_precondition("maestro.disabled_by_policy")`);
/// an *attached* handle that later trips inert carries this so the handler maps
/// it to a typed inert `Status` + emits the matching event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InertReason {
    /// The daily token budget is exhausted (412 owns the counting; dormant
    /// until then). Drives `maestro.budget_exhausted`.
    BudgetExhausted {
        /// Unix-ms the budget resets.
        resets_at_ms: i64,
    },
    /// The `enterpriseDataPrivacy` + external-model gate disabled the LLM at
    /// run time (the boot gate normally catches this; this covers a live policy
    /// flip). Drives `maestro.disabled_by_policy`.
    DisabledByPolicy {
        /// The machine-readable reason.
        reason: String,
    },
}

/// The shared inner state of a [`MaestroHandle`]. `Arc`-wrapped so the handle is
/// cheap to `Clone` across the factory closure, the Iroh `CoreServiceSet`, and
/// the bridge `BridgeServices`.
struct Inner {
    persistence: Arc<Persistence>,
    workareas: WorkareaManager,
    supervisor: AgentSupervisorHandle,
    /// 404's summary cache (force-refresh-if-stale-60s feeds `GetDigest`).
    summary_cache: Arc<Mutex<SummaryCache>>,
    /// 409/312's one-shot LLM seam (DeterministicOneShot is the LIVE P4 path;
    /// 412 swaps the real provider behind this `Arc`).
    oneshot: Arc<dyn OneShotLlm>,
    /// The `maestro.events` producer.
    events: MaestroEventSender,
    /// `Some(reason)` when the handle is attached but inert (budget/policy);
    /// `None` is the normal live state. 412 flips the budget arm when its
    /// counter trips; the boot gate handles the policy-disabled-at-boot case by
    /// not constructing the handle at all.
    inert: Mutex<Option<InertReason>>,
}

/// The Core-side Maestro API (design/08 §5.2). Filled by Task 414 (path (a))
/// over the shared handles + the deterministic 408/409/413 seams. The five
/// signatures are FROZEN (401.5); the impl is live.
#[derive(Clone)]
pub struct MaestroHandle {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for MaestroHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaestroHandle").finish_non_exhaustive()
    }
}

impl MaestroHandle {
    /// Construct the live handle from the shared Core handles + seams (Task
    /// 414's boot wiring). The handle starts live (`inert = None`); 412 flips it
    /// inert when the budget trips.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        persistence: Arc<Persistence>,
        workareas: WorkareaManager,
        supervisor: AgentSupervisorHandle,
        summary_cache: Arc<Mutex<SummaryCache>>,
        oneshot: Arc<dyn OneShotLlm>,
        events: MaestroEventSender,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                persistence,
                workareas,
                supervisor,
                summary_cache,
                oneshot,
                events,
                inert: Mutex::new(None),
            }),
        }
    }

    /// The `maestro.events` producer's carrier sender, for `boot.rs` to hand
    /// into `StreamsHandler::with_maestro_events`.
    pub fn events_sender(
        &self,
    ) -> tokio::sync::broadcast::Sender<crate::handlers::streams::MaestroEvent> {
        self.inner.events.frame_sender()
    }

    /// Mark the handle inert (412's budget tripwire / a live policy flip). Idem-
    /// potent; the next RPC surfaces the typed inert `Status` + the matching
    /// event. Dormant until 412 wires the live token counter.
    pub async fn set_inert(&self, reason: Option<InertReason>) {
        *self.inner.inert.lock().await = reason;
    }

    /// The current inert reason, if any. `pub` so `handlers/maestro.rs` can
    /// project it into the `MaestroState.inert`/`inert_reason` wire fields
    /// (Task 416); also used internally by `guard_llm`.
    pub async fn inert_reason(&self) -> Option<InertReason> {
        self.inner.inert.lock().await.clone()
    }

    /// Insert-or-get the singleton `chats(kind='maestro')` row and return its id
    /// (a raw `String` — there is no `ChatId` newtype in `concerto_persist`).
    ///
    /// Idempotent: `maestro_state::ensure_maestro_chat` only INSERTs when no
    /// `kind='maestro'` row exists (CHECK allows the NULL `session_id`), so a
    /// second call is a no-op and re-reads the existing id. The Maestro spawn
    /// path binds its long-lived session to this row, which is why
    /// `maestro_session_id()` (joining `sessions` to `chats WHERE kind='maestro'`)
    /// resolves it — closing Seam 4b's chat-kind mismatch.
    async fn ensure_maestro_chat(&self) -> Result<String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        {
            let mut w = self.inner.persistence.writer().await;
            // Caller-supplied id is honored only on first bootstrap; if a maestro
            // chat already exists this is a no-op and the SELECT below returns the
            // existing id.
            concerto_persist::maestro_state::ensure_maestro_chat(
                &mut w,
                &uuid::Uuid::now_v7().to_string(),
                now_ms,
            )
            .await?;
        }
        let id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM chats WHERE kind = 'maestro' ORDER BY created_at ASC LIMIT 1",
        )
        .fetch_optional(self.inner.persistence.readers())
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        id.ok_or_else(|| {
            Error::Internal("maestro chat missing immediately after ensure".to_string())
        })
    }

    /// Spawn (or no-op if already live) the long-lived Maestro session.
    ///
    /// The Maestro runs as ONE long-lived agent session under the supervisor.
    /// This ensures the singleton `chats(kind='maestro')` row + the reserved
    /// system workspace/workarea exist, then `start_session`s an
    /// `AgentKind::Maestro` session bound to that chat (via the Task-6
    /// `StartSessionRequest.chat_id` seam). Without the binding the supervisor
    /// would create a fresh `kind='session'` chat and `maestro_session_id()`
    /// would never find the session (Seam 4b). Idempotent: a live session short-
    /// circuits to its id, so boot can call this unconditionally.
    pub async fn spawn_maestro_session(&self) -> Result<concerto_persist::SessionId> {
        match self.maestro_session_id().await {
            Ok(existing) => return Ok(existing),
            Err(Error::NotFound(_)) => {} // no live session yet — proceed to spawn
            Err(e) => return Err(e),       // surface real DB/internal errors
        }
        let chat_id = self.ensure_maestro_chat().await?;
        let (_ws, wa_id) = crate::maestro::system_workarea::ensure_system_workspace_and_workarea(
            &self.inner.persistence,
        )
        .await?;
        let scratch = crate::maestro::ensure_maestro_scratch_dir()?;
        // Best-effort folder-trust preseed so the strict Maestro session never
        // blocks on Claude's interactive "trust this folder?" dialog. A failure
        // here must NOT fail the spawn (the supervisor also preseeds trust).
        if let Err(e) = crate::maestro::ensure_maestro_scratch_trusted(&scratch) {
            tracing::warn!(error = %e, "maestro: folder-trust preseed failed (best-effort)");
        }
        let mut req = crate::maestro::maestro_start_request(wa_id, scratch);
        req.chat_id = Some(chat_id);
        self.inner.supervisor.start_session(req).await
    }

    /// Send the user's chat input to the Maestro (design/08 §5.2 / §6.1). Runs
    /// 408's deterministic [`pre_parse`] first; routes `@workarea` mentions,
    /// handles slash directives, or forwards freeform text to the Maestro
    /// session. Routing is deterministic and fires **even when the LLM is
    /// inert** (design/08 §3.5) — only the freeform/LLM path is gated.
    ///
    /// `attachments` is a V1.0 text-only seam (R-9): frozen, currently empty.
    pub async fn send_to_maestro(
        &self,
        text: String,
        attachments: Vec<MaestroAttachment>,
    ) -> Result<SendOutcome> {
        let _ = attachments; // V1.0 text-only (R-9).
        match pre_parse(&text) {
            ParseOutcome::Routing { targets, body } => {
                // Deterministic routing — NEVER gated on the LLM inert state.
                let workspace_id = self.default_workspace_id().await?;
                let router = Router::new(
                    Arc::new(self.inner.workareas.clone()),
                    self.inner.supervisor.clone(),
                    Arc::clone(&self.inner.persistence),
                );
                let routes = router
                    .resolve_targets(&workspace_id, &targets)
                    .await
                    .map_err(routing_error_to_internal)?;
                router.dispatch(&routes, &body).await;
                let resolved: Vec<String> = routes.iter().map(|r| r.composer.clone()).collect();
                self.inner.events.emit(MaestroEvent::RoutingExecuted {
                    targets: resolved.clone(),
                    body: body.clone(),
                });
                Ok(SendOutcome::Routed { targets: resolved })
            }
            ParseOutcome::Slash { directive, body } => match directive {
                SlashDirective::Digest => {
                    // `/digest` is the same path as `GetDigest` — compose the
                    // digest AND emit `maestro.digest_generated` (Scope — in:
                    // `/digest` ⇒ 409's digest + `DigestGenerated`), so the chat
                    // top bar refreshes exactly as the explicit `GetDigest` RPC
                    // does.
                    let digest = self.get_digest_model().await?;
                    let n = digest_workarea_count(&digest);
                    self.inner.events.emit(MaestroEvent::DigestGenerated {
                        at_ms: digest.generated_at,
                        n_workareas: n,
                    });
                    Ok(SendOutcome::Digested { n_workareas: n })
                }
                SlashDirective::Pause | SlashDirective::New => {
                    // `/pause` (406) / `/new` (session-creation flow) are
                    // recognized markers; forward the body to the Maestro
                    // session so the agent can act on the directive context.
                    self.forward_freeform(&body).await?;
                    Ok(SendOutcome::SlashHandled)
                }
            },
            ParseOutcome::Freeform(body) => {
                // Freeform goes to the Maestro LLM — gated on the inert state.
                self.guard_llm().await?;
                self.forward_freeform(&body).await?;
                Ok(SendOutcome::Forwarded)
            }
        }
    }

    /// Return the current digest (design/08 §3.6 / §5.2), mapped onto the proto
    /// `Digest` 401.5 froze. 409's `generate_digest` force-refreshes stale-60s
    /// summaries then composes (`<5s p50`, §4.4); the groups + next-step fold
    /// into the wire `text`, the chips map 1:1.
    pub async fn get_digest(&self) -> Result<ProtoDigest> {
        let digest = self.get_digest_model().await?;
        let n = digest_workarea_count(&digest);
        self.inner.events.emit(MaestroEvent::DigestGenerated {
            at_ms: digest.generated_at,
            n_workareas: n,
        });
        Ok(digest_to_proto(digest))
    }

    /// Set the per-workarea Maestro visibility (design/08 §3.3 / §5.2). Applies
    /// 413's `exclude_from_maestro` toggle: `HardFactsOnly` ⇒ excluded,
    /// `Full` ⇒ not excluded. `Unspecified` is rejected as a typed validation
    /// error.
    pub async fn set_workarea_visibility(
        &self,
        wa: WorkareaId,
        vis: MaestroVisibility,
    ) -> Result<()> {
        let exclude = match vis {
            MaestroVisibility::Full => false,
            MaestroVisibility::HardFactsOnly => true,
            MaestroVisibility::Unspecified => {
                return Err(Error::Validation(
                    "maestro.visibility_unspecified: visibility must be FULL or HARD_FACTS_ONLY"
                        .to_string(),
                ));
            }
        };
        self.inner
            .workareas
            .set_exclude_from_maestro(&wa, exclude)
            .await?;
        Ok(())
    }

    /// Enable or disable the Maestro (design/08 §5.2). Persists the flag on the
    /// `maestro_state` singleton (403).
    pub async fn set_enabled(&self, on: bool) -> Result<()> {
        let mut w = self.inner.persistence.writer().await;
        concerto_persist::maestro_state::set_enabled(&mut w, on).await?;
        Ok(())
    }

    /// Read the Maestro's live state view (design/08 §5.2) from the
    /// `maestro_state` singleton (403).
    pub async fn get_state(&self) -> Result<MaestroStateView> {
        let state = concerto_persist::maestro_state::get(self.inner.persistence.readers())
            .await?
            .ok_or_else(|| {
                Error::NotFound("maestro_state singleton not initialized".to_string())
            })?;
        Ok(MaestroStateView {
            enabled: state.enabled,
            daily_in_today: state.daily_in_today,
            daily_out_today: state.daily_out_today,
            last_digest_at_ms: state.last_digest_at,
        })
    }

    /// The live Maestro session id as a wire string, or `""` when there is no
    /// live session (Task 416). Wraps the internal [`Self::maestro_session_id`]
    /// (which returns a typed `NotFound` for the forward path) and flattens
    /// `Err`/none to the empty string the `MaestroState.maestro_session_id`
    /// field carries — 417 keys "no live session" off the empty string.
    pub async fn maestro_session_id_str(&self) -> String {
        self.maestro_session_id()
            .await
            .map(|s| s.0)
            .unwrap_or_default()
    }

    // -- internals ----------------------------------------------------------

    /// Generate the digest model (shared by `GetDigest` + the `/digest` slash
    /// route). Runs 409's `generate_digest` over 404's [`SummaryCache`] snapshot,
    /// scoped to the default workspace.
    ///
    /// ## Staleness (§4.4)
    ///
    /// 404's `force_refresh_if_stale` is a pure **in-place rebuild** seam (it
    /// takes a `rebuild` closure, not a from-disk fetch) — the from-disk
    /// summary rebuild is owned by 404's refresh path, not by this handle. So
    /// `GetDigest` reads the current cache snapshot; 409 records any summary
    /// older than the 60s `GET_DIGEST_STALE_MS` window into the digest's
    /// `degraded`/`stale` flag (R-7 badge) rather than silently building on
    /// stale facts. The `GET_DIGEST_STALE_MS` constant is the shared window.
    async fn get_digest_model(&self) -> Result<DigestModel> {
        let _stale_window = GET_DIGEST_STALE_MS; // the shared §4.4 window (409 applies it).
        let workspace_id = self.default_workspace_id().await?;
        let last_seen_at = self.last_seen_at().await?;
        let cache = self.inner.summary_cache.lock().await;
        generate_digest(
            &workspace_id,
            last_seen_at,
            &cache,
            &self.inner.oneshot,
            &self.inner.persistence,
        )
        .await
    }

    /// Forward freeform text to the live Maestro session's stdin via the agent
    /// supervisor. The Maestro session is `AgentKind::Maestro`; its streamed
    /// assistant output is surfaced as `maestro.message` events by the session
    /// pump (the handle records acceptance here).
    async fn forward_freeform(&self, body: &str) -> Result<()> {
        if body.trim().is_empty() {
            return Ok(());
        }
        let session_id = self.maestro_session_id().await?;
        self.inner
            .supervisor
            .send_input(&session_id, body.as_bytes().to_vec())
            .await
    }

    /// Guard the LLM path against the inert state. Routing/tools never call
    /// this; only the freeform/LLM forward does (design/08 §3.5/§3.9/§3.10).
    /// Emits the matching event + returns a typed `Err` the handler maps to the
    /// inert `Status`.
    async fn guard_llm(&self) -> Result<()> {
        match self.inert_reason().await {
            None => Ok(()),
            Some(InertReason::BudgetExhausted { resets_at_ms }) => {
                self.inner
                    .events
                    .emit(MaestroEvent::BudgetExhausted { resets_at_ms });
                Err(Error::Policy("maestro.budget_exhausted".to_string()))
            }
            Some(InertReason::DisabledByPolicy { reason }) => {
                self.inner.events.emit(MaestroEvent::DisabledByPolicy {
                    reason: reason.clone(),
                });
                Err(Error::Policy("maestro.disabled_by_policy".to_string()))
            }
        }
    }

    /// The singleton `chats(kind='maestro')` id → the live Maestro session id.
    /// The Maestro runs as one long-lived session under the supervisor; we look
    /// up the most-recent live session on the maestro chat. A missing session
    /// is a typed `NotFound` the handler surfaces (rather than a silent no-op).
    async fn maestro_session_id(&self) -> Result<concerto_persist::SessionId> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT s.id FROM sessions s
             JOIN chats c ON c.id = s.chat_id
             WHERE c.kind = 'maestro' AND s.ended_at IS NULL
             ORDER BY s.started_at DESC LIMIT 1",
        )
        .fetch_optional(self.inner.persistence.readers())
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        row.map(concerto_persist::SessionId)
            .ok_or_else(|| Error::NotFound("no live Maestro session to forward to".to_string()))
    }

    /// The workspace the Maestro message is scoped to. V1.0 has no server-side
    /// active-workspace; the Maestro is global, so we resolve the single (or
    /// most-recent) non-archived workspace. A multi-workspace ask-with-chips is
    /// a follow-on (design/08 §3.5 cross-workspace branch).
    async fn default_workspace_id(&self) -> Result<WorkspaceId> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT id FROM workspaces WHERE archived_at IS NULL
             ORDER BY created_at DESC LIMIT 1",
        )
        .fetch_optional(self.inner.persistence.readers())
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        row.map(WorkspaceId)
            .ok_or_else(|| Error::NotFound("no active workspace for the Maestro".to_string()))
    }

    /// The `last_seen_at` the digest computes deltas since: the last digest
    /// instant, or 0 (everything is "new") when none yet (403's
    /// `last_digest_at`).
    async fn last_seen_at(&self) -> Result<i64> {
        let state = concerto_persist::maestro_state::get(self.inner.persistence.readers()).await?;
        Ok(state.and_then(|s| s.last_digest_at).unwrap_or(0))
    }
}

/// Map a 408 `RoutingError` to a typed `Error::Internal` carrying the routing
/// failure description (the handler maps it to a `Status`). Routing failures are
/// not panics — every variant carries enough to synthesize the user message.
fn routing_error_to_internal(err: crate::maestro::routing::RoutingError) -> Error {
    Error::NotFound(format!("maestro.routing: {err:?}"))
}

/// How many workareas the digest covered (the three groups summed).
fn digest_workarea_count(d: &DigestModel) -> u32 {
    (d.finished.len() + d.blocked.len() + d.working.len()) as u32
}

/// Map the in-process [`DigestModel`] onto the proto `Digest` 401.5 froze. The
/// groups + next-step fold into the wire `text` (415 renders `text` + `chips`);
/// the chips map 1:1 onto `MaestroChip`; `degraded` ⇒ `stale` (R-7 badge).
fn digest_to_proto(d: DigestModel) -> ProtoDigest {
    ProtoDigest {
        text: compose_digest_text(&d),
        chips: d.chips.into_iter().map(chip_to_proto).collect(),
        generated_at_ms: d.generated_at,
        stale: d.degraded,
    }
}

/// Fold the LLM prose + the one-line next step into the single wire `text` the
/// proto carries (the groups already live in the structured `chips`; 415 renders
/// `text` as the digest body and the chips as the next-step affordances).
fn compose_digest_text(d: &DigestModel) -> String {
    if d.next_step.trim().is_empty() {
        d.text.clone()
    } else {
        format!("{}\n\n{}", d.text.trim_end(), d.next_step)
    }
}

/// Map an in-process [`crate::suggestions::chip::Chip`] onto the proto
/// `MaestroChip` (same field set as 409's persisted chips).
fn chip_to_proto(c: crate::suggestions::chip::Chip) -> MaestroChip {
    MaestroChip {
        rule_id: c.rule_id,
        workarea_id: c.workarea_id.0,
        title: c.title,
        priority: c.priority,
        created_at_ms: c.created_at,
        action: c.action.as_wire_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    use concerto_persist::{PersistenceConfig, RepositoryId, SessionId};

    use crate::agent_supervisor::actor::AgentKind;
    use crate::maestro::summary::{RepoSummary, SessionSummary, WorkareaSummary};
    use crate::repo_manager::RepoManager;

    /// Build a live `MaestroHandle` over a tempdir DB with the `maestro_state`
    /// singleton + the `kind='maestro'` chat bootstrapped (409's anchor) and one
    /// non-archived workspace `ws`. The supervisor is the lightweight test
    /// handle (no live PTY — visibility/digest paths never `send_input`).
    async fn live_handle() -> (tempfile::TempDir, MaestroHandle, Arc<Persistence>) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path = tmp.path().join("test.db");
        let persistence = Arc::new(
            Persistence::open(PersistenceConfig {
                db_path,
                max_readers: 4,
            })
            .await
            .expect("open"),
        );
        {
            let mut w = persistence.writer().await;
            concerto_persist::maestro_state::ensure_initialized(&mut w, 0)
                .await
                .expect("init maestro_state");
            concerto_persist::maestro_state::ensure_maestro_chat(&mut w, "maestro-chat", 0)
                .await
                .expect("bootstrap maestro chat");
            // One non-archived workspace so `default_workspace_id` resolves.
            sqlx::query(
                "INSERT INTO workspaces (id, name, slug, settings_json, created_at, archived_at) \
                 VALUES ('ws', 'WS', 'ws', '{}', 0, NULL)",
            )
            .execute(&mut *w)
            .await
            .expect("insert workspace");
        }

        let handle = handle_over(Arc::clone(&persistence), tmp.path()).await;
        (tmp, handle, persistence)
    }

    /// Build a `MaestroHandle` over an already-open `persistence` rooted at
    /// `base` (for `data`/`config` subdirs). Does NOT seed the DB — callers
    /// that need the maestro-state/chat/workspace fixtures use `live_handle()`;
    /// the Task-6 insert-arm test drives a near-bare DB to exercise the
    /// `ensure_maestro_chat` INSERT path. The supervisor uses `/bin/true` (no
    /// live PTY): the Task-6 unit tests never reach a real first-spawn.
    async fn handle_over(persistence: Arc<Persistence>, base: &std::path::Path) -> MaestroHandle {
        let data_dir = base.join("data");
        let config_dir = base.join("config");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();
        let repo_manager = RepoManager::new(Arc::clone(&persistence), data_dir.join("repos"));
        let workareas = WorkareaManager::new(
            Arc::clone(&persistence),
            repo_manager,
            Arc::new(data_dir.clone()),
            Arc::new(config_dir.clone()),
        );
        let supervisor = AgentSupervisorHandle::new(
            Arc::clone(&persistence),
            Arc::new(data_dir),
            Arc::new(config_dir),
            PathBuf::from("/bin/true"),
        );
        let cache = Arc::new(Mutex::new(SummaryCache::with_system_clock()));
        let oneshot = crate::maestro::digest::default_oneshot();
        let events = MaestroEventSender::new();
        MaestroHandle::new(persistence, workareas, supervisor, cache, oneshot, events)
    }

    /// A workarea row in workspace `ws`, so visibility toggles + routing have a
    /// target.
    async fn insert_workarea(persistence: &Persistence, id: &str, composer: &str) {
        let mut w = persistence.writer().await;
        sqlx::query(
            "INSERT INTO workareas (id, workspace_id, composer_name, branch_name, \
             worktree_root, status, created_at, archived_at, last_activity_at, settings_json) \
             VALUES (?, 'ws', ?, ?, '/tmp', 'running', 0, NULL, NULL, '{}')",
        )
        .bind(id)
        .bind(composer)
        .bind(format!("concerto/{composer}"))
        .execute(&mut *w)
        .await
        .expect("insert workarea");
    }

    fn seed_summary(id: &str, composer: &str, status: &str) -> WorkareaSummary {
        WorkareaSummary {
            workarea_id: WorkareaId(id.into()),
            workspace_id: WorkspaceId("ws".into()),
            workspace_name: "WS".into(),
            composer_name: composer.into(),
            branch_name: format!("concerto/{composer}"),
            status: status.into(),
            last_activity_at: 0,
            sessions: vec![SessionSummary {
                session_id: SessionId(format!("sess-{id}")),
                agent_kind: AgentKind::Claude,
                model: "claude".into(),
                status: status.into(),
                last_turn_summary: format!("{composer} worked"),
            }],
            last_turn_summary: format!("{composer} worked"),
            last_3_turn_summaries: vec![format!("{composer} worked")],
            repos: vec![RepoSummary {
                repository_id: RepositoryId(format!("repo-{id}")),
                repo_name: format!("{composer}-repo"),
                commits_ahead: 2,
                files_changed: 3,
                lines_added: 4,
                lines_removed: 1,
                pr_state: Some("open".into()),
                ci_state: Some("success".into()),
            }],
            blocked_on: None,
            generated_at: 0,
            generation: 0,
        }
    }

    // -- digest mapping (pure) ---------------------------------------------

    #[test]
    fn digest_to_proto_maps_chips_text_and_stale() {
        use crate::suggestions::chip::{Chip, ChipAction};
        let model = DigestModel {
            text: "the prose".into(),
            finished: vec![],
            blocked: vec![],
            working: vec![],
            next_step: "Do the thing.".into(),
            chips: vec![Chip {
                rule_id: "maestro_digest".into(),
                workarea_id: WorkareaId("wa-1".into()),
                title: "Resume bach".into(),
                priority: 50,
                created_at: 7,
                action: ChipAction::Resume,
            }],
            generated_at: 123,
            degraded: true,
        };
        let proto = digest_to_proto(model);
        assert_eq!(proto.chips.len(), 1);
        assert_eq!(proto.chips[0].rule_id, "maestro_digest");
        assert_eq!(proto.chips[0].workarea_id, "wa-1");
        assert_eq!(proto.chips[0].priority, 50);
        assert_eq!(proto.chips[0].created_at_ms, 7);
        assert_eq!(proto.generated_at_ms, 123);
        assert!(proto.stale, "degraded ⇒ stale (R-7)");
        assert!(proto.text.contains("the prose"));
        assert!(proto.text.contains("Do the thing."));
    }

    // -- visibility round-trip ---------------------------------------------

    #[tokio::test]
    async fn set_visibility_round_trips_exclude_flag() {
        let (_tmp, handle, persistence) = live_handle().await;
        insert_workarea(&persistence, "wa-1", "bach").await;

        handle
            .set_workarea_visibility(WorkareaId("wa-1".into()), MaestroVisibility::HardFactsOnly)
            .await
            .expect("set hard facts only");
        let wa = handle
            .inner
            .workareas
            .get(&WorkareaId("wa-1".into()))
            .await
            .expect("get")
            .expect("present");
        let settings: serde_json::Value = serde_json::from_str(&wa.settings_json).unwrap();
        assert_eq!(settings["exclude_from_maestro"], true);

        handle
            .set_workarea_visibility(WorkareaId("wa-1".into()), MaestroVisibility::Full)
            .await
            .expect("set full");
        let wa = handle
            .inner
            .workareas
            .get(&WorkareaId("wa-1".into()))
            .await
            .expect("get")
            .expect("present");
        let settings: serde_json::Value = serde_json::from_str(&wa.settings_json).unwrap();
        assert_eq!(settings["exclude_from_maestro"], false);
    }

    #[tokio::test]
    async fn set_visibility_unspecified_is_validation_error() {
        let (_tmp, handle, persistence) = live_handle().await;
        insert_workarea(&persistence, "wa-1", "bach").await;
        let err = handle
            .set_workarea_visibility(WorkareaId("wa-1".into()), MaestroVisibility::Unspecified)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Validation(_)));
    }

    // -- GetDigest: proto shape + chips + <5s ------------------------------

    #[tokio::test]
    async fn get_digest_returns_proto_with_chips_under_5s() {
        let (_tmp, handle, _persistence) = live_handle().await;
        // Seed the summary cache with two workareas so the digest is non-empty.
        {
            let mut cache = handle.inner.summary_cache.lock().await;
            cache.upsert(seed_summary("wa-1", "bach", "finished"));
            cache.upsert(seed_summary("wa-2", "mozart", "running"));
        }
        let start = Instant::now();
        let digest = handle.get_digest().await.expect("digest");
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "GetDigest p50 budget <5s (got {elapsed:?})"
        );
        assert!(!digest.text.trim().is_empty(), "digest text non-empty");
        assert!(!digest.chips.is_empty(), "digest carries next-step chips");
        assert_eq!(digest.chips.len(), 2, "one chip per seeded workarea");
        assert!(digest.generated_at_ms >= 0);
    }

    // -- SendToMaestro: ParseOutcome dispatch + events ---------------------

    #[tokio::test]
    async fn send_freeform_with_no_session_is_typed_not_found() {
        let (_tmp, handle, _persistence) = live_handle().await;
        // Freeform forwards to the Maestro session; none is running, so the
        // handle returns a typed NotFound (NOT a silent no-op / panic).
        let err = handle
            .send_to_maestro("just chatting".into(), vec![])
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn send_digest_slash_emits_digest_generated_event() {
        let (_tmp, handle, _persistence) = live_handle().await;
        {
            let mut cache = handle.inner.summary_cache.lock().await;
            cache.upsert(seed_summary("wa-1", "bach", "finished"));
        }
        let mut rx = handle.inner.events.frame_sender().subscribe();
        let outcome = handle
            .send_to_maestro("/digest".into(), vec![])
            .await
            .expect("digest slash");
        assert_eq!(outcome, SendOutcome::Digested { n_workareas: 1 });
        let frame = rx.recv().await.expect("event");
        let v: serde_json::Value = serde_json::from_slice(&frame.frame).unwrap();
        assert_eq!(v["kind"], "maestro.digest_generated");
    }

    #[tokio::test]
    async fn send_routing_to_unknown_workarea_is_typed_not_found() {
        let (_tmp, handle, _persistence) = live_handle().await;
        // `@ghost` resolves nothing → typed routing NotFound (not a panic);
        // routing is deterministic and runs even with no LLM.
        let err = handle
            .send_to_maestro("@ghost do it".into(), vec![])
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn send_routing_executed_emits_event_and_dispatches() {
        let (_tmp, handle, persistence) = live_handle().await;
        insert_workarea(&persistence, "wa-1", "bach").await;
        // A live session for `bach` so resolve picks it; the test supervisor's
        // `send_input` finds no running PTY and maps to NoActiveAgent per-route,
        // but resolution + the RoutingExecuted event still fire.
        {
            let mut w = persistence.writer().await;
            // Reuse the bootstrapped maestro chat as the FK target (the routing
            // resolver reads the `sessions` row by workarea, not via the chat).
            sqlx::query(
                "INSERT INTO sessions (id, workarea_id, chat_id, agent_kind, permission_mode, \
                 bypass_destructive_guard, started_at, ended_at, status, last_acked_seq) \
                 VALUES ('s-1', 'wa-1', 'maestro-chat', 'claude', 'normal', 0, 1, NULL, 'running', 0)",
            )
            .execute(&mut *w)
            .await
            .expect("insert session");
        }
        let mut rx = handle.inner.events.frame_sender().subscribe();
        let outcome = handle
            .send_to_maestro("@bach run the suite".into(), vec![])
            .await
            .expect("routing");
        assert_eq!(
            outcome,
            SendOutcome::Routed {
                targets: vec!["bach".into()]
            }
        );
        let frame = rx.recv().await.expect("event");
        let v: serde_json::Value = serde_json::from_slice(&frame.frame).unwrap();
        assert_eq!(v["kind"], "maestro.routing_executed");
        assert_eq!(v["targets"][0], "bach");
        assert_eq!(v["body"], "run the suite");
    }

    // -- get_state round-trips the maestro_state singleton ------------------

    #[tokio::test]
    async fn get_state_reads_singleton() {
        let (_tmp, handle, _persistence) = live_handle().await;
        let state = handle.get_state().await.expect("state");
        assert!(state.enabled, "default enabled");
        assert_eq!(state.last_digest_at_ms, None);
    }

    // -- Task 416: maestro_session_id_str flattens to "" / the live id -------

    #[tokio::test]
    async fn maestro_session_id_str_empty_when_no_live_session() {
        let (_tmp, handle, _persistence) = live_handle().await;
        assert_eq!(
            handle.maestro_session_id_str().await,
            "",
            "no live Maestro session ⇒ empty string"
        );
    }

    #[tokio::test]
    async fn maestro_session_id_str_returns_live_session_id() {
        let (_tmp, handle, persistence) = live_handle().await;
        insert_workarea(&persistence, "wa-1", "bach").await;
        // A live (`ended_at IS NULL`) session on the bootstrapped maestro chat.
        {
            let mut w = persistence.writer().await;
            sqlx::query(
                "INSERT INTO sessions (id, workarea_id, chat_id, agent_kind, permission_mode, \
                 bypass_destructive_guard, started_at, ended_at, status, last_acked_seq) \
                 VALUES ('maestro-sess', 'wa-1', 'maestro-chat', 'maestro', 'normal', 0, 1, NULL, 'running', 0)",
            )
            .execute(&mut *w)
            .await
            .expect("insert maestro session");
        }
        assert_eq!(handle.maestro_session_id_str().await, "maestro-sess");
    }

    // -- Task 6: ensure_maestro_chat + spawn binding (Seam 4b) --------------

    /// `ensure_maestro_chat` returns the singleton `kind='maestro'` chat id,
    /// the row's kind is `'maestro'`, and a second call is a no-op (same id, no
    /// second row). `live_handle()` bootstraps `"maestro-chat"` so we assert the
    /// existing row is reused (insert-or-GET, not insert-or-error).
    #[tokio::test]
    async fn ensure_maestro_chat_is_idempotent_and_kind_maestro() {
        let (_tmp, handle, persistence) = live_handle().await;
        let id1 = handle.ensure_maestro_chat().await.expect("ensure 1");
        let id2 = handle.ensure_maestro_chat().await.expect("ensure 2");
        assert_eq!(id1, id2, "idempotent: same chat id");
        assert_eq!(id1, "maestro-chat", "reuses the bootstrapped maestro chat");

        let kind: String =
            sqlx::query_scalar("SELECT kind FROM chats WHERE id = ?")
                .bind(&id1)
                .fetch_one(persistence.readers())
                .await
                .expect("kind");
        assert_eq!(kind, "maestro");

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chats WHERE kind = 'maestro'")
            .fetch_one(persistence.readers())
            .await
            .expect("count");
        assert_eq!(n, 1, "never creates a second maestro chat");
    }

    /// `ensure_maestro_chat` INSERTs the singleton on a DB that has none yet
    /// (the `live_handle()` harness pre-bootstraps it, so this drives a bare DB
    /// to exercise the insert arm — proving the chat the spawn binds to is the
    /// one `maestro_session_id()` later joins on).
    #[tokio::test]
    async fn ensure_maestro_chat_inserts_when_absent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let persistence = Arc::new(
            Persistence::open(PersistenceConfig {
                db_path: tmp.path().join("test.db"),
                max_readers: 2,
            })
            .await
            .expect("open"),
        );
        {
            let mut w = persistence.writer().await;
            concerto_persist::maestro_state::ensure_initialized(&mut w, 0)
                .await
                .expect("init");
        }
        // No maestro chat yet.
        let pre: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chats WHERE kind = 'maestro'")
            .fetch_one(persistence.readers())
            .await
            .expect("pre count");
        assert_eq!(pre, 0);

        let handle = handle_over(Arc::clone(&persistence), tmp.path()).await;
        let id = handle.ensure_maestro_chat().await.expect("ensure");
        let kind: String = sqlx::query_scalar("SELECT kind FROM chats WHERE id = ?")
            .bind(&id)
            .fetch_one(persistence.readers())
            .await
            .expect("kind");
        assert_eq!(kind, "maestro", "freshly-inserted chat is kind='maestro'");
    }

    /// `spawn_maestro_session` short-circuits to the already-live session when
    /// one exists. This proves the end-state Task 6 targets: a Maestro session
    /// bound to a `kind='maestro'` chat is resolvable by `maestro_session_id()`
    /// (Seam 4b closed), and the spawn is idempotent — a second call returns the
    /// same id without spawning. (The first-spawn PTY path uses
    /// `AgentKind::Maestro` → the real `claude` CLI and is integration-tested in
    /// Task 10; the seam it depends on — `StartSessionRequest.chat_id` binding —
    /// is covered by `chat_id_binds_session_to_existing_chat` in
    /// `tests/agent_spawn.rs`.)
    #[tokio::test]
    async fn spawn_short_circuits_to_live_maestro_session() {
        let (_tmp, handle, persistence) = live_handle().await;
        insert_workarea(&persistence, "wa-1", "bach").await;
        // A live (`ended_at IS NULL`) Maestro session on the bootstrapped
        // `kind='maestro'` chat — i.e. exactly the row a normal bound spawn
        // produces.
        {
            let mut w = persistence.writer().await;
            sqlx::query(
                "INSERT INTO sessions (id, workarea_id, chat_id, agent_kind, permission_mode, \
                 bypass_destructive_guard, started_at, ended_at, status, last_acked_seq) \
                 VALUES ('maestro-sess', 'wa-1', 'maestro-chat', 'maestro', 'strict', 0, 1, NULL, 'running', 0)",
            )
            .execute(&mut *w)
            .await
            .expect("insert maestro session");
        }
        let sid = handle.spawn_maestro_session().await.expect("spawn (short-circuit)");
        assert_eq!(sid.0, "maestro-sess", "resolves the live bound session");

        // The backing chat is kind='maestro' (Seam 4b).
        let kind: String = sqlx::query_scalar(
            "SELECT c.kind FROM sessions s JOIN chats c ON c.id = s.chat_id WHERE s.id = ?",
        )
        .bind(&sid.0)
        .fetch_one(persistence.readers())
        .await
        .expect("kind");
        assert_eq!(kind, "maestro");

        // Idempotency: a second spawn returns the same session.
        let sid2 = handle.spawn_maestro_session().await.expect("spawn 2");
        assert_eq!(sid, sid2);
    }

    // -- Task 416: GetState handler projects the full MaestroState ----------

    #[tokio::test]
    async fn get_state_handler_projects_full_state_with_session_id() {
        use crate::handlers::maestro::MaestroHandler;
        use crate::llm::{DEFAULT_DAILY_IN_CAP, DEFAULT_DAILY_OUT_CAP};
        use concerto_proto::v1::maestro_server::Maestro as MaestroService;
        use concerto_proto::v1::GetStateRequest;
        use tonic::Request;

        let (_tmp, handle, persistence) = live_handle().await;
        insert_workarea(&persistence, "wa-1", "bach").await;
        // A live Maestro session so `maestro_session_id` is non-empty (417's
        // load-bearing field).
        {
            let mut w = persistence.writer().await;
            sqlx::query(
                "INSERT INTO sessions (id, workarea_id, chat_id, agent_kind, permission_mode, \
                 bypass_destructive_guard, started_at, ended_at, status, last_acked_seq) \
                 VALUES ('maestro-sess', 'wa-1', 'maestro-chat', 'maestro', 'normal', 0, 1, NULL, 'running', 0)",
            )
            .execute(&mut *w)
            .await
            .expect("insert maestro session");
        }

        let svc = MaestroHandler::new(Some(handle));
        let state = svc
            .get_state(Request::new(GetStateRequest::default()))
            .await
            .expect("get_state")
            .into_inner();

        assert!(state.enabled, "default enabled");
        assert_eq!(state.daily_in_today, 0);
        assert_eq!(state.daily_out_today, 0);
        assert_eq!(state.in_cap, DEFAULT_DAILY_IN_CAP as i64);
        assert_eq!(state.out_cap, DEFAULT_DAILY_OUT_CAP as i64);
        assert_eq!(state.last_digest_at_ms, 0, "0 ⇒ never");
        assert!(!state.inert, "live handle is not inert");
        assert_eq!(state.inert_reason, "");
        assert_eq!(
            state.maestro_session_id, "maestro-sess",
            "live session id is load-bearing for 417"
        );
    }

    #[tokio::test]
    async fn get_state_handler_reports_inert_budget_exhausted() {
        use crate::handlers::maestro::MaestroHandler;
        use concerto_proto::v1::maestro_server::Maestro as MaestroService;
        use concerto_proto::v1::GetStateRequest;
        use tonic::Request;

        let (_tmp, handle, _persistence) = live_handle().await;
        handle
            .set_inert(Some(InertReason::BudgetExhausted { resets_at_ms: 9 }))
            .await;

        let svc = MaestroHandler::new(Some(handle));
        let state = svc
            .get_state(Request::new(GetStateRequest::default()))
            .await
            .expect("get_state")
            .into_inner();
        assert!(state.inert);
        assert_eq!(state.inert_reason, "budget_exhausted");
    }
}
