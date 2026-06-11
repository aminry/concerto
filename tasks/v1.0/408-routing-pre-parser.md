# Task 408 — Deterministic routing pre-parser (`pre_parse` → `ParseOutcome`) + dynamic `@all`/`@idle`/`@blocked` sets + composer→workarea→session resolver (zero-LLM routing)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium |
| Depends on | 402 |
| Touches subsystem(s) | 08 (Maestro), 03 (Workspace/Workarea/Session Mgr) |
| Smoke gate | unchanged |

## Goal
Give the Maestro a **purely deterministic, zero-LLM routing front-end** so that `@workarea` / `@a,@b` fanout / `@all`/`@idle`/`@blocked` / `/digest`/`/pause`/`/new` inputs are parsed and dispatched without ever spending a token (`design/08 §3.5`, §6.3 — "routing is deterministic … the Maestro only spends tokens on the questions that actually need its reasoning"). Today there is **no routing code at all**: there is no `route_prompt_to_session`, no `@workarea` pre-parser, and **no server-side active-workspace** (workspace selection lives only in the Desktop's client-side Zustand store), so nothing translates `@bach …` into a session prompt; the only send-prompt path that exists is `AgentSupervisorHandle::send_input(&SessionId, Vec<u8>)` (`crates/core/src/agent_supervisor/actor.rs:930`), reached today only through per-session handlers, and the only resolution primitives are `WorkareaManager::list_by_workspace(workspace_id, include_archived)` (composer-sorted via `concerto_persist::workareas::list_by_workspace`, `crates/core/src/workspace_manager/workarea.rs:2403` / `crates/persist/src/workareas.rs:206` `ORDER BY composer_name`) and `concerto_persist::sessions::list_by_workarea(pool, workarea_id)` (`crates/persist/src/sessions.rs:320`, `ORDER BY started_at DESC`). This task creates **`crates/core/src/maestro/routing.rs`** and **FREEZES** the routing grammar: `pre_parse(&str) -> ParseOutcome` where `ParseOutcome ∈ { Freeform(String) | Routing { targets: Vec<RoutingTarget>, body: String } | Slash { directive: SlashDirective, body: String } }`, plus the `RoutingTarget`/`SlashDirective`/`RoutingError` types and the composer→workarea→session **resolver** (`resolve_targets`) layered over the two existing list APIs and dispatching through `send_input` (**FROZEN, design/08 §3.5 / §6.3, PHASE4_PLANNING §4.7**). The load-bearing property: **routing spends ZERO LLM tokens** — `pre_parse` is a pure string function and the resolver only reads SQLite + calls `send_input`. After this task, **409** (`/digest`) consumes `SlashDirective::Digest` and **414** (`SendToMaestro`) runs every inbound message through `pre_parse` before the agent ever sees freeform text; both build on this frozen grammar with no re-shape. What stays out of this Tier-1 task: the live end-to-end "real Maestro routes to a real workarea session and the response is surfaced back" demonstration is the Phase-4 Tier-3 checklist's job, not CI's.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md §4.7` — **AUTHORITATIVE.** "The routing grammar + `ParseOutcome` — FROZEN by 408 (D2)": `pre_parse(&str) -> ParseOutcome` (`Freeform` | `Routing{targets, body}` | `Slash{directive, body}`) covering `@workarea`, `@a,@b` fanout, `@all`/`@idle`/`@blocked`, `/digest`/`/pause`/`/new`, and the composer→session resolver; 409 (`/digest`) + 414 (`SendToMaestro` pre-parse) consume it.
- `tasks/v1.0/PHASE4_PLANNING.md §2` row 408 — **AUTHORITATIVE** sub-decision D2: "`maestro/routing.rs`: a **pure deterministic pre-parser** … + a composer→workarea→session resolver over `workareas::list_by_workspace` (composer-sorted) + `sessions::list_by_workarea`. **No server-side active-workspace exists** — the Maestro takes an explicit `workspace_id`; cross-workspace `@composer` disambiguation is the Maestro's job (ask-with-chips). Routing spends **zero** LLM tokens."
- `tasks/v1.0/PHASE4_PLANNING.md §6` (dep row 408 ← 402) + `§8.1` (write-set: `crates/core/src/maestro/routing.rs`, `crates/core/src/maestro/mod.rs`; hard seam shared with 401/402/404 on the maestro `mod.rs` soft seam — add your `pub mod routing;` in a distinct region) + the migration author-check: **confirm the highest `crates/persist/migrations/NNNN_*.sql` on `main` is still `0014`** (it is, as of this authoring — `0014_pull_requests_merge_order.sql`); this task adds **no migration**, but if a higher one landed note it in Handoff. (No `crates/persist` change at all here — routing is in-memory over existing read APIs.)
- `design/08_Maestro_Agent.md §3.5` — the routing syntax spec (the `@bach` / `@bach/claude` / `@bach,@mozart` / `@all`/`@idle`/`@blocked` examples + the cross-workspace disambiguation rule: "composer names are unique within a workspace but not across workspaces … If ambiguous, the Maestro asks"; the 3-step "pre-parser handles it directly … records a synthesized assistant message … user sees the session's response surfaced" flow; "Free-form text (no `@`, no `/`) goes to the Maestro LLM normally").
- `design/08_Maestro_Agent.md §6.3` — the canonical `pre_parse` skeleton (`parse_slash` first, then `parse_at`, else `Freeform`) + "Targets like `@all` / `@idle` / `@blocked` resolve to dynamic workarea (or session) sets at routing time, scoped to the currently-active workspace." Transcribe the control flow; the **built** resolver takes an explicit `workspace_id` (no server-side active-workspace), per PHASE4_PLANNING §2.
- `design/08_Maestro_Agent.md §7.1` + §8 — the `@bach run the e2e suite` hot path (pre-parse → resolve within active workspace → `send_input` → synthesize assistant message) and the **error-handling table** (the four routing failure rows: bad target `@nonexistent` → "I don't see a workarea named X …"; ambiguous `@composer` → ask-with-chips; routing target with no active agent → "<target> has no active agent. Start one?"; these become typed `RoutingError` variants, NOT silent failures).
- `crates/core/src/agent_supervisor/actor.rs:930` — `pub async fn send_input(&self, session_id: &SessionId, data: Vec<u8>) -> Result<()>` — **the only send-prompt path**; the resolver dispatches each resolved target's prompt through this (returns `Error::NotFound` if the session is not running — map that to the "no active agent" `RoutingError`). `start_session` (actor.rs:368) is NOT called by routing (the Maestro/user starts sessions; routing only sends to existing ones).
- `crates/core/src/workspace_manager/workarea.rs:2403` — `WorkareaManager::list_by_workspace(workspace_id, include_archived) -> Result<Vec<Workarea>>` (composer-sorted); `crates/persist/src/sessions.rs:320` — `sessions::list_by_workarea(pool, workarea_id) -> Result<Vec<Session>>` (`ORDER BY started_at DESC` — **the first row is the most-recently-started session**, the `newest_agent_kind` helper at workarea.rs:1585 documents this exact "first is the most recent" reliance; reuse the same ordering as the "most-recently-active" tiebreak).
- `tasks/v1.0/305-cone-stats-suggest-seam.md` (Handoff Notes) — the seam discipline this task mirrors: unwired/uncertain paths return a **typed error** (there `ConeSuggestError::Unwired`), never `todo!()`/`unimplemented!()`, never empty-success; here every routing failure is a typed `RoutingError` variant the caller renders as a synthesized assistant message.

## Scope — in

**`crates/core/src/maestro/routing.rs` (new) — the FROZEN grammar:**
- `pub fn pre_parse(input: &str) -> ParseOutcome` — a **pure, allocation-only, zero-LLM** function. Order per `design/08 §6.3`: try `parse_slash` first (leading `/`), then `parse_at` (leading `@`), else `Freeform(input.to_owned())`. Leading/trailing whitespace trimmed for the directive detection; the `body` preserves the user's original text after the directive/target span.
- `pub enum ParseOutcome { Freeform(String), Routing { targets: Vec<RoutingTarget>, body: String }, Slash { directive: SlashDirective, body: String } }`.
- `pub enum RoutingTarget { Workarea { composer: String }, Session { composer: String, agent_kind: String }, All, Idle, Blocked }` — `@bach` → `Workarea`; `@bach/claude` → `Session`; `@all`/`@idle`/`@blocked` → the dynamic-set markers (resolved later, NOT at parse time). A comma-separated `@a,@b` (or `@a,@b/claude`) parses into a multi-element `targets` vec (the **fanout** case). `agent_kind` is kept as the raw lowercased string (e.g. `"claude"`); it is matched against `Session.agent_kind` at resolve time, NOT pre-validated against the `AgentKind` enum (an unknown kind surfaces as a resolve-time `RoutingError::NoMatchingSession`, not a parse error).
- `pub enum SlashDirective { Digest, Pause, New }` — exactly the three `design/08 §3.5`/§2 V1.0 directives. An unrecognized `/foo` parses as `Freeform` (it is NOT a directive; the agent decides what to do with literal slash text), documented inline.
- `parse_slash`/`parse_at` are private helpers; only `pre_parse` + the types are `pub`.

**`crates/core/src/maestro/routing.rs` — the composer→workarea→session resolver:**
- `pub async fn resolve_targets(&self, workspace_id: &WorkspaceId, targets: &[RoutingTarget]) -> Result<Vec<ResolvedRoute>, RoutingError>` (on a `Router` struct holding `Arc`/handle clones of `WorkareaManager` + `AgentSupervisorHandle` + the readers pool) — resolves each `RoutingTarget` **within the explicit `workspace_id`** (there is **no server-side active-workspace**; the caller — 414 — passes the workspace the Maestro message is scoped to).
- **Static targets** (`Workarea`/`Session`): look up the workarea by `composer` via `WorkareaManager::list_by_workspace(workspace_id, false)` (composer-sorted; exact case-insensitive composer match). For `Workarea`, pick the **most-recently-active session** = the first row of `sessions::list_by_workarea` that is still live (`ended_at IS NULL` / status not ended) — reusing the `started_at DESC` "first is most recent" convention. For `Session`, additionally filter to `agent_kind == requested` (case-insensitive). Resolve to a `ResolvedRoute { workarea_id, session_id, composer, agent_kind }`.
- **Dynamic sets** (`All`/`Idle`/`Blocked`): resolve at routing time over all non-archived workareas in `workspace_id`: `All` = every workarea with a live session; `Idle` = workareas whose newest live session is not actively working (status idle/awaiting — derive from `Session.status`); `Blocked` = workareas in a blocked status (`awaiting_approval` / `test_failure` / `merge_conflict` — read `Workarea.status` / session status; mirror the `BlockedReason` notion from `design/08 §3.3`, but classify deterministically from the existing status columns, NOT from the 404 summary cache, to keep 408 independent of 404). Each resolves to **zero or more** `ResolvedRoute`s (a fanout).
- `pub enum RoutingError { NoSuchWorkarea { composer: String, suggestions: Vec<String> }, AmbiguousComposer { composer: String, candidates: Vec<WorkareaRef> }, NoActiveAgent { composer: String }, NoMatchingSession { composer: String, agent_kind: String }, EmptyDynamicSet { set: String } }` — each carries enough to synthesize the `design/08 §8` assistant message (e.g. `NoSuchWorkarea.suggestions` = the composer-sorted names for "did you mean bach / mozart?"; `AmbiguousComposer.candidates` for the ask-with-chips). **Cross-workspace `@composer` ambiguity is the Maestro's job** (PHASE4_PLANNING §2): within a single `workspace_id`, composer names are unique, so `AmbiguousComposer` is surfaced by the **caller** (414) when it has to choose a workspace before calling `resolve_targets` — this enum variant is defined here for the caller to construct, and `resolve_targets` itself (single-workspace) never returns it. Document this split inline.

**`crates/core/src/maestro/routing.rs` — the dispatch:**
- `pub async fn dispatch(&self, routes: &[ResolvedRoute], body: &str) -> Vec<DispatchResult>` — for each `ResolvedRoute`, call `AgentSupervisorHandle::send_input(&route.session_id, body.as_bytes().to_vec())`; collect a `DispatchResult { route: ResolvedRoute, outcome: Result<(), RoutingError> }` (mapping `send_input`'s `Error::NotFound` → `RoutingError::NoActiveAgent`). Dispatch does **not** synthesize the assistant message or touch chat history (that is 414's job per the §3.5 3-step flow) — it returns the per-route outcomes so 414 can record "Routed to bach / Claude" and surface failures.

**`crates/core/src/maestro/mod.rs` (modified):**
- Add `pub mod routing;` in a **distinct additive region** (the maestro `mod.rs` is the Phase-4 soft seam — 401 owns the initial skeleton; add your line where it auto-merges, per PHASE4_PLANNING §8.1). Re-export `pre_parse`, `ParseOutcome`, `RoutingTarget`, `SlashDirective`, `RoutingError` if the module's re-export convention (set by 401) does so for siblings; otherwise leave them addressed as `maestro::routing::*`.

- Tests (Tier 1): a **table-driven** `pre_parse` suite covering every directive shape — `@bach …` → `Routing{[Workarea{bach}]}`; `@bach/claude …` → `Routing{[Session{bach,claude}]}`; `@bach,@mozart …` → fanout (two targets); `@all`/`@idle`/`@blocked …` → the dynamic markers; `/digest`/`/pause`/`/new` → the three `SlashDirective`s; `/foo` → `Freeform`; plain text → `Freeform`; an `@` with no token / a bare `/` → `Freeform`; body-span correctness (the text after the directive is preserved verbatim). A **resolver** suite against an in-process `WorkareaManager` fixture (two workareas `bach`/`mozart`, each with sessions): `@bach` picks the most-recently-started **live** session; `@bach/claude` filters to the Claude session; an unknown `@nonexistent` → `RoutingError::NoSuchWorkarea` with composer-sorted `suggestions`; a workarea with no live session → `RoutingError::NoActiveAgent`; `@all` fans out to every workarea-with-a-live-session; `@idle`/`@blocked` classify by status; an empty dynamic set → `RoutingError::EmptyDynamicSet`. A **dispatch** test using a stub/fake supervisor (or asserting the `send_input` call shape) that proves the body bytes route to the resolved session id and a `NotFound` maps to `NoActiveAgent`. **A property/assertion test that `pre_parse` performs no I/O and no LLM call** (it is a pure `fn(&str)->ParseOutcome` — enforced by it not being `async` and taking no handles).

## Scope — out
- **`SendToMaestro` wiring + synthesized assistant message + chat-history recording + the cross-workspace `@composer` ask-with-chips** — owned by **Task 414** (the `MaestroServer` impl). 408 leaves `pre_parse` + `resolve_targets` + `dispatch` as the frozen library; 414 runs every inbound message through them, picks the workspace (constructing `RoutingError::AmbiguousComposer` when needed), records the synthesized message, and publishes `maestro.routing_executed`. This is a **seam**: 408 returns typed outcomes, 414 renders them.
- **`/digest` execution (the actual digest generation + latency proof)** — owned by **Task 409**. 408 only defines `SlashDirective::Digest`; 409 matches on it and generates the digest over 404's summaries. `/pause`/`/new` execution likewise belongs to their consuming tasks (pause → `set_workarea_paused` write tool 406; `/new` → session creation flow) — 408 only parses the directive into a typed marker.
- **The `WorkareaSummary` cache + `BlockedReason`** — owned by **Task 404** (D9/§4.4). 408 classifies `@idle`/`@blocked` **directly from the existing `Workarea.status` / `Session.status` columns** so it does NOT depend on 404; if 413/404's richer blocked-reason taxonomy later supersedes this, the classifier is the seam to upgrade (note in Handoff). 408 must NOT consume the 404 cache (it would create a false dep — 408 only depends on 402).
- **Token accounting / budget gating of routing** — owned by **Task 412**/403. Routing is the path that **survives** budget exhaustion (`design/08 §3.9`/§3.10: "routing and tool calls (deterministic) still work"); 408 spends zero tokens by construction, so there is nothing for the budget to gate here. Do not add a budget check to `pre_parse`/`resolve`/`dispatch`.
- **The `route_prompt_to_session` / `fanout_to_sessions` MCP write-tools** — owned by **Task 406** (`maestro/tools/write.rs`). Those are the **LLM-invoked** routing tools (the agent decides to route); 408 is the **deterministic pre-parse** that fires *before* the LLM and is the more common path. 406's tools may call 408's `resolve_targets`/`dispatch` (a pure addition); 408 does not implement any MCP tool.
- **Real-world Tier-3:** the live end-to-end demonstration — "type `@bach run the e2e suite` in the real Maestro chat, confirm it routes to bach's live session, the session runs, and the response is surfaced back as quoted lines in the Maestro chat" — is the **Phase-4 Tier-3 operator checklist** line, run at the phase gate against a real multi-workarea state; CI proves only the deterministic parse + resolve + dispatch-shape against an in-process fixture.

## Public interface this task locks
- **The routing grammar + resolver (FROZEN, design/08 §3.5/§6.3, PHASE4_PLANNING §4.7), `crates/core/src/maestro/routing.rs`:**
  ```rust
  /// Pure, deterministic, ZERO-LLM pre-parse. No I/O, no async, no token spend.
  /// design/08 §6.3: try slash, then @, else freeform.
  pub fn pre_parse(input: &str) -> ParseOutcome;

  pub enum ParseOutcome {
      Freeform(String),
      Routing { targets: Vec<RoutingTarget>, body: String },
      Slash { directive: SlashDirective, body: String },
  }

  pub enum RoutingTarget {
      Workarea { composer: String },                 // @bach
      Session  { composer: String, agent_kind: String }, // @bach/claude
      All,                                            // @all   (dynamic set)
      Idle,                                           // @idle  (dynamic set)
      Blocked,                                        // @blocked (dynamic set)
  }

  pub enum SlashDirective { Digest, Pause, New }      // /digest /pause /new

  pub struct ResolvedRoute {
      pub workarea_id: WorkareaId,
      pub session_id: SessionId,
      pub composer: String,
      pub agent_kind: String,
  }

  pub enum RoutingError {
      NoSuchWorkarea     { composer: String, suggestions: Vec<String> },
      AmbiguousComposer  { composer: String, candidates: Vec<WorkareaRef> }, // constructed by the caller (414); cross-workspace only
      NoActiveAgent      { composer: String },
      NoMatchingSession  { composer: String, agent_kind: String },
      EmptyDynamicSet    { set: String },
  }

  pub struct WorkareaRef { pub workspace_id: WorkspaceId, pub workarea_id: WorkareaId, pub composer: String }

  pub struct DispatchResult { pub route: ResolvedRoute, pub outcome: Result<(), RoutingError> }

  impl Router {
      /// Resolve targets WITHIN one explicit workspace (no server-side active-workspace).
      pub async fn resolve_targets(
          &self, workspace_id: &WorkspaceId, targets: &[RoutingTarget],
      ) -> Result<Vec<ResolvedRoute>, RoutingError>;

      /// Send `body` to each resolved session via AgentSupervisorHandle::send_input.
      pub async fn dispatch(&self, routes: &[ResolvedRoute], body: &str) -> Vec<DispatchResult>;
  }
  ```
  The exact field names/variants above are FROZEN; the enum is `#[non_exhaustive]`-friendly only if 401's module convention already uses it (else plain) — but the five `RoutingTarget` variants + three `SlashDirective` variants + the `ParseOutcome` shape are the contract 409/414 match on and **must not be re-shaped**.
- **Consumes (does NOT re-lock):** `AgentSupervisorHandle::send_input(&SessionId, Vec<u8>)` as frozen by V0.1 (`crates/core/src/agent_supervisor/actor.rs:930`); `WorkareaManager::list_by_workspace` + `concerto_persist::sessions::list_by_workarea` as the existing composer-sorted / `started_at DESC` read APIs; `WorkspaceId`/`WorkareaId`/`SessionId`/`Workarea`/`Session` types from `concerto-persist`; the `maestro` module path + `mod.rs` skeleton as frozen by Task 401 (PHASE4_PLANNING §4.1/§2). `AgentKind` is NOT consumed at parse time (agent_kind stays a raw string until resolve).

## Implementation notes
- **The load-bearing rule: `pre_parse` is a pure non-`async` `fn(&str) -> ParseOutcome` with no handles, no I/O, no token spend.** That signature *is* the "routing is deterministic / zero-LLM" guarantee (`design/08 §3.5`, §6.3) — keep resolution (`resolve_targets`/`dispatch`, which touch SQLite + `send_input`) strictly separate from parsing. A reviewer must be able to see at a glance that the parse path cannot reach an LLM. The budget-exhausted path (`design/08 §3.9`/§3.10) depends on this: routing keeps working when the LLM is inert.
- **Reuse, don't reinvent, the resolution primitives.** Do NOT add a new persist query or a new `list_*` API — resolve over the existing `WorkareaManager::list_by_workspace` (already composer-sorted, so the "did you mean …" `suggestions` is just the sorted composer list) and `sessions::list_by_workarea` (already `started_at DESC`, so "most-recently-active" is `.iter().find(|s| live)`). The `newest_agent_kind` helper (`workarea.rs:1585`) is the precedent for the "first is the most recent" reliance — match it.
- **Honest typed seams, never the macro.** Every routing failure is a `RoutingError` variant the caller renders (`design/08 §8` table → synthesized assistant message); an unresolvable dynamic set is `EmptyDynamicSet`, not an empty-success silent no-op. Mirror 305's seam discipline: no `todo!()`/`unimplemented!()` in new code. `AmbiguousComposer` is **defined but not returned** by the single-workspace `resolve_targets` (it is the caller's to construct when choosing a workspace) — document that explicitly so 414 knows it owns the cross-workspace branch.
- **`@idle`/`@blocked` classify from existing status columns, not the 404 cache.** This keeps 408 dependent only on 402 (per the §6 edge-list) and parallel-safe with 404. Read `Workarea.status` + the newest live `Session.status`; map blocked statuses (`awaiting_approval`/`test_failure`/`merge_conflict`) deterministically. Document the classification as the seam 404/413 may later refine.
- **No `#[cfg(unix)]` needed here.** Unlike the agent-supervisor sessions/streams handlers, `routing.rs` only *calls* the supervisor handle (`send_input`) — it has no platform-specific syscalls; the cross-platform gating is already inside the supervisor. Keep `routing.rs` platform-neutral so it builds on the Windows/Linux lanes (Task 113).
- **No proto, no gRPC, no two-site registration in this task.** 408 is a pure Rust library inside the `maestro` module; the `Maestro` service + its two-site registration are 401.5/414's. There is no new service to register here.
- **Regen:** no proto/schema/SQL change. `pub` Rust API is added (`routing` module). Run `./scripts/regen-interfaces.sh`; if it captures the new `pub` types into `docs/interfaces/rust-api.md`, commit the diff (note 305's Handoff observed `regen-interfaces.sh` captures struct/enum *definitions* from `crates/*/src/api.rs` but not free `pub fn`s nor `src/maestro/*` modules — so `routing.rs` may produce **no** diff; if so, the `git diff --exit-code` step is trivially clean — record which in Handoff).
- **Parallel build hint:** three disjoint fan-out sub-parts (DAG `fanout`): (1) **parse-grammar + `ParseOutcome`/`RoutingTarget`/`SlashDirective`** (the pure `pre_parse` + table-driven tests — zero handles, fully standalone); (2) **dynamic-set resolution** (`@all`/`@idle`/`@blocked` classifier over `list_by_workspace` + status columns); (3) **composer→workarea→session static resolver + `dispatch`** (over `sessions::list_by_workarea` + `send_input`). (1) has no dependency on the handles and can be built/tested first; (2) and (3) share the `Router` struct + the fixture but touch disjoint match arms.

## Verification
**Tier 1.** The `rust` §5.3 set.
1. `cargo check --workspace` clean (the new `maestro::routing` module compiles; `mod.rs` line added).
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo fmt --all -- --check` clean.
4. `cargo test -p concerto-core routing` (+ `pre_parse`/`resolve`/`dispatch` filters) → the table-driven `pre_parse` suite (every `@`/`/` shape + fanout + `/foo`→`Freeform` + bare `@`/`/`→`Freeform` + body-span correctness), the resolver suite (most-recently-active pick, `@bach/claude` agent-kind filter, `NoSuchWorkarea` with composer-sorted suggestions, `NoActiveAgent`, `@all`/`@idle`/`@blocked` dynamic sets, `EmptyDynamicSet`), and the dispatch test (body bytes → resolved session id; `send_input` `NotFound` → `NoActiveAgent`) all pass. This proves the FROZEN grammar parses correctly and resolution picks the most-recently-active session + surfaces typed ambiguity/failure — the §4.7 contract 409/414 build on.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → green (no new crates; routing is pure-Rust over existing deps).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit any regen (likely no diff for a `src/maestro/*` module per 305's observation; if `rust-api.md` gains the routing enums, commit it).
8. `scripts/smoke.sh` → **unchanged** (408 adds no smoke capability; the maestro-digest smoke check is 409/414's; routing is CI-provable in-process).

**Tier-1 scope + what it does NOT cover.** Tier-1 proves the deterministic grammar + resolver + dispatch-shape against an in-process `WorkareaManager`/fixture-session state — the parse table, the most-recently-active pick, the typed `RoutingError` ambiguity/failure surface, and that the body bytes reach the resolved session via `send_input`. It does **NOT** cover the live end-to-end route (a real Maestro chat sending `@bach …` to a real running workarea session and surfacing the response back) — that is the **Phase-4 Tier-3 operator checklist** line "route a prompt through the Maestro to a live workarea session and confirm the response is surfaced", verified at the phase gate. No Tier-2 double is needed (the resolution is deterministic over real persist read APIs on a fixture, not a mocked external service).

## Definition of Done
- [x] `crates/core/src/maestro/routing.rs` created with the FROZEN `pre_parse(&str) -> ParseOutcome` + `ParseOutcome`/`RoutingTarget`/`SlashDirective`/`ResolvedRoute`/`RoutingError`/`WorkareaRef`/`DispatchResult` types (design/08 §3.5/§6.3, PHASE4_PLANNING §4.7)
- [x] `pre_parse` is a pure non-`async` `fn(&str)` — no I/O, no handles, no LLM call (the zero-token guarantee) — asserted by a test
- [x] Composer→workarea→session `resolve_targets(workspace_id, targets)` over `WorkareaManager::list_by_workspace` (composer-sorted) + `sessions::list_by_workarea` (`started_at DESC`); picks the most-recently-active live session; `@all`/`@idle`/`@blocked` resolve dynamically from existing status columns (NOT the 404 cache)
- [x] `dispatch` routes the body to each resolved session via `AgentSupervisorHandle::send_input`; `send_input` `NotFound` → `RoutingError::NoActiveAgent`
- [x] Bad target / no-active-agent / ambiguous / empty-set surface as typed `RoutingError` variants carrying the data to synthesize the design/08 §8 message (409/414 render); `AmbiguousComposer` documented as caller-constructed (cross-workspace)
- [x] `pub mod routing;` added to `crates/core/src/maestro/mod.rs` in an additive region (soft seam)
- [x] Tests (Tier 1): table-driven `pre_parse` (all shapes), resolver suite, dispatch test, purity assertion — all pass
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams return a typed `RoutingError`, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any schema/contract changed (no proto/SQL change; commit any `rust-api.md` diff)
- [x] All Verification commands pass on a clean checkout; smoke gate unchanged (green)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/routing.rs` (new — `pre_parse` + `ParseOutcome`/`RoutingTarget`/`SlashDirective`/`ResolvedRoute`/`RoutingError`/`WorkareaRef`/`DispatchResult` + the `Router` `resolve_targets`/`dispatch` + the Tier-1 tests)
- `crates/core/src/maestro/mod.rs` (modified — `pub mod routing;` in an additive region; optional sibling re-exports per 401's convention)
- `docs/interfaces/rust-api.md` (regenerated — only if `regen-interfaces.sh` captures the new routing enums; likely no diff for a `src/maestro/*` module, record in Handoff)

## Commit message
```
phase-4: deterministic routing pre-parser + composer→session resolver

Adds maestro/routing.rs: the FROZEN zero-LLM pre_parse(&str)->ParseOutcome
grammar (@workarea / @a,@b fanout / @all/@idle/@blocked / /digest //pause
//new, design/08 §3.5/§6.3) plus a composer→workarea→session resolver over
the existing composer-sorted list_by_workspace + started_at-DESC
list_by_workarea, dispatching via AgentSupervisorHandle::send_input. Bad
target / ambiguous / no-active-agent surface as typed RoutingError variants
(no macro stubs). Tier-1 table-driven; 409 (/digest) + 414 (SendToMaestro
pre-parse) consume the frozen grammar. Live end-to-end routing is the
Phase-4 Tier-3 checklist line.

Refs: tasks/v1.0/408-routing-pre-parser.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** Minimal. (1) **Migration high-water moved 0014→0015.** The Inputs §8.1 author-check expected `0014_pull_requests_merge_order.sql` as the highest migration on `main`; the base now also carries `0015_maestro_state.sql` (landed by Task 401's `bf12839`). 408 adds **no** migration and touches no `crates/persist` file, so this is a benign high-water advance — no conflict, recorded here per the author-check note. (2) To keep the FROZEN `Router` method signatures exact **and** make resolve/dispatch unit-testable against an in-process fixture (no PTY host), the live `WorkareaManager::list_by_workspace` + `sessions::list_by_workarea` + `AgentSupervisorHandle::send_input` are reached through two **narrow internal async traits** (`WorkareaReader` / `InputSink`) the live handles impl — `Router::new(workareas, supervisor, persistence)` is the production constructor; `Router::from_parts` is the test/reuse seam. No new persist query was added; the two existing read APIs are reused verbatim. `pre_parse` stays a pure non-`async` `fn(&str)->ParseOutcome` (the zero-LLM guarantee), asserted by `pre_parse_is_pure_no_io_no_async`.
- **Open questions for next task:** **409** matches `SlashDirective::Digest` to generate the digest; **414** runs every `SendToMaestro` input through `pre_parse` → `resolve_targets` → `dispatch`, owns the cross-workspace `AmbiguousComposer` branch (single-workspace `resolve_targets` never returns it — documented inline + on the `WorkareaRef`/`RoutingError::AmbiguousComposer` doc-comments) + the synthesized assistant message + the `maestro.routing_executed` event; **406**'s `route_prompt_to_session`/`fanout_to_sessions` write-tools may reuse `Router::resolve_targets`/`dispatch` (or `Router::from_parts`). All build on the FROZEN `ParseOutcome`/`RoutingTarget`/`SlashDirective`/`RoutingError` surface, re-exported from `maestro::{pre_parse, ParseOutcome, RoutingTarget, SlashDirective, RoutingError, Router, ResolvedRoute, DispatchResult, WorkareaRef}`.
- **Deliberate debt:** `@idle`/`@blocked` classify from raw `Workarea.status` / `Session.status` columns (`is_idle_status` → `paused`/`idle`/`awaiting` or session `awaiting`/`idle`; `is_blocked_workarea_status` → `awaiting_approval`/`test_failure`/`merge_conflict`/`blocked`), **NOT** the 404 `WorkareaSummary`/`BlockedReason` cache — keeps 408 dependent only on 402 and parallel-safe with 404. If 404/413's richer blocked-reason taxonomy later supersedes this, `is_idle_status`/`is_blocked_workarea_status` are the single seam to upgrade. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code — every failure path is a typed `RoutingError` (`send_input` `NotFound`/any transport error → `NoActiveAgent`; read error → empty-set/`NoActiveAgent`, never a panic).
- **Smoke-gate state:** Unchanged (green). Routing adds no smoke capability — the maestro-digest smoke check is owned by 409/414; routing is fully CI-provable in-process (22 `maestro::routing::*` unit tests).
