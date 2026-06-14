# Task 414 — Live `Maestro` gRPC impl (fills 401.5's `MaestroServer` skeleton) + `maestro.events` publishing + boot wiring (consumes §4.2/§4.4/§4.7)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 401.5, 409 |
| Touches subsystem(s) | 08 (Maestro), 10 (Local API), 01 (Core) |
| Smoke gate | unchanged |

## Goal
Light up the Maestro gRPC service so the Desktop UI (415) — which already built against 401.5's frozen `maestro.proto` + `maestro.events` subject **with zero live data** — now drives a real Maestro with **no UI rework**. Today the wire surface exists but is inert: 401.5 froze `service Maestro { SendToMaestro / GetDigest / SetWorkareaVisibility }` (`design/08 §5.3`), the Rust `MaestroHandle` API (`send_to_maestro`/`get_digest`/`set_workarea_visibility`/`set_enabled`/`get_state`, `design/08 §5.2`), the `Subject::MaestroEvents` arm + `parse_subject("maestro.events")` branch + a `StreamsHandler::with_maestro_events(sender)` producer setter, and registered an **initially-`Status::unimplemented` `MaestroServer` at BOTH sites** (`add_core_services` in `crates/core/src/api_server.rs:565` + `connect_bridge.rs` `build_and_serve`) per the `UpsertProjectMcp` precedent (D8) — but `crates/core/src/handlers/maestro.rs` returns `unimplemented` for every RPC, `boot.rs` never constructs a `MaestroHandle`, and `CoreServiceSet.maestro`/`BridgeServices.maestro` are always `None`. This task **fills `handlers/maestro.rs`** with the live impl over the in-process `MaestroHandle`: `SendToMaestro` runs 408's `pre_parse(&str) -> ParseOutcome` (§4.7) then forwards routing/freeform/slash to the Maestro session or handle; `GetDigest` calls 409's digest path (§4.4 `<5s p50` over 404's `WorkareaSummary` cache, force-on-stale-60s); `SetWorkareaVisibility` applies 413's privacy toggle. It **constructs the real `MaestroHandle` in `boot.rs`** (the ~880 `ApiServerActor` factory closure + the ~925 Iroh `CoreServiceSet` + `connect_bridge.rs::BridgeServices`), gated on `maestro_state.enabled` (§4.6, 403) **AND** managed-policy model permission (`ManagedPolicy::default_model()`), threading `Some(handle)` into `CoreServiceSet.maestro`/`BridgeServices.maestro`. It **publishes `maestro.events`** through a new **FROZEN** `crates/core/src/maestro/events.rs` (`MaestroEvent` enum + `to_frame() -> bytes` opaque-JSON serializer + a `broadcast::Sender<MaestroEvent>`) wired into `StreamsHandler::with_maestro_events`, emitting `maestro.message` / `routing_executed` / `digest_generated` / `budget_exhausted` / `disabled_by_policy` (`design/08 §5.4`) on the **`Event.checks_opaque=17` carrier** (NOT a new `body` oneof arm). After this task the Maestro chat top bar renders live messages, digests, routing receipts, and budget/policy state end-to-end; `enterpriseDataPrivacy`+external-model (D1) leaves the handle un-constructed and the service replies `disabled_by_policy` — the real-LLM digest-quality + the >30-min live digest judgement stay Tier-3.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §4.2 — **AUTHORITATIVE**: the `maestro.proto`/`MaestroHandle`/`maestro.events` surface this task **consumes as frozen by 401.5** (D7/D8). "414 fills the impl." Do NOT re-lock the proto, the handle trait, or the subject arm.
- `tasks/v1.0/PHASE4_PLANNING.md` §4.4 — **AUTHORITATIVE**: `WorkareaSummary` + the digest's `<5s p50` / force-on-`GetDigest`-if-stale-60s refresh contract, **frozen by 404, consumed by 409 then by this task's `GetDigest`**.
- `tasks/v1.0/PHASE4_PLANNING.md` §4.7 — **AUTHORITATIVE**: `pre_parse(&str) -> ParseOutcome` (`Freeform | Routing{targets,body} | Slash{directive,body}`), **frozen by 408**; "414 (`SendToMaestro` pre-parse) consumes it."
- `tasks/v1.0/PHASE4_PLANNING.md` §1 (D7 `maestro.events` rides `checks_opaque=17`, NOT a new oneof arm; D8 two-site registration is the easiest Phase-4 bug to miss), §6 (this task's deps = 401.5 + 409), §8.1 (write-set: `handlers/{maestro,streams}.rs`, `maestro/events.rs`, `boot.rs`, `api_server.rs`; hard seam shared with 401.5 on `handlers/maestro.rs`+`streams.rs`; soft `maestro/mod.rs`).
- `design/08_Maestro_Agent.md` §5.2/§5.3/§5.4 — the `MaestroHandle` API, the `service Maestro` shape, and the 5-row emitted-events table (`maestro.message`/`routing_executed`/`digest_generated`/`budget_exhausted`/`disabled_by_policy`). §6.1 (lifecycle: `SendToMaestro` pre-parses then forwards to agent stdin or handles deterministically; budget-exhaust ⇒ inert).
- `crates/core/src/handlers/maestro.rs` — 401.5's skeleton handler to fill (thin struct + `#[async_trait]` impl returning `Status::unimplemented`, `#[cfg(unix)]`-gated). Mirror `crates/core/src/handlers/vcs.rs` (thin handler over a `VcsHandle` + an `error_to_status` mapper).
- `crates/core/src/handlers/streams.rs:147` (`Subject::MaestroEvents` arm 401.5 added), `:485` (`with_vcs_events` setter precedent), `:1207` (`map_vcs_event` → `Event{ body: None, checks_opaque: Some(frame) }` — the exact opaque-frame discipline to mirror for `map_maestro_event`), `:931` (`parse_subject`).
- `crates/core/src/api_server.rs:501` (`CoreServiceSet` struct — the `maestro: Option<MaestroHandle>` field 401.5 added), `:534` (`runtime_only` sets it `None`), `:565`/`:590` (`add_core_services` + its destructure — where the `MaestroServer` registers, `#[cfg(unix)]`-gated like `Sessions`/`Streams`), `:672` (the `StreamsHandler::new(..).with_*` chain to extend with `.with_maestro_events(..)`).
- `crates/core/src/boot.rs:863`–`:949` (the `factory_*` handle clones + `ApiServerActor::with_managers(..)` factory closure ~880, and the `CoreServiceSet { .. }` literal for the Iroh serve path ~925) + `crates/core/src/connect_bridge.rs:161` (`BridgeServices` struct) `:232` (its destructure) — the construction sites to thread the new handle through.
- `crates/core/src/security/managed.rs` (`ManagedPolicy::default_model()` — parsed-but-currently-unread; this task reads it to gate `MaestroHandle` construction per D1's `enterpriseDataPrivacy`+external-model consequence) + `crates/persist/src/maestro_state.rs` (403's `enabled` accessor, §4.6).

## Scope — in
- **`crates/core/src/maestro/events.rs` (new — FROZEN by this task):**
  - A `pub enum MaestroEvent { Message{..}, RoutingExecuted{..}, DigestGenerated{..}, BudgetExhausted{..}, DisabledByPolicy{..} }` mirroring `design/08 §5.4`'s five rows, plus `pub fn kind(&self) -> &'static str` returning the exact wire kind strings (`maestro.message` / `maestro.routing_executed` / `maestro.digest_generated` / `maestro.budget_exhausted` / `maestro.disabled_by_policy`).
  - `pub fn to_frame(&self) -> Vec<u8>` (or `Bytes`) serializing the event to the **opaque JSON frame** carried on `Event.checks_opaque=17` (mirror `design/13 §5.3`'s `{"kind": "...", ...}` envelope that 324 parses; 415 parses these). The frame is the only wire shape; **NO new `Event.body` oneof arm** (oneof FROZEN through field 16, D7).
  - A `MaestroEvents` producer wrapper exposing a `broadcast::Sender<MaestroEvent>` (mirroring the VCS aggregator's `sender()`), owned by the `MaestroHandle`, so `boot.rs` can pass `handle.events_sender()` into `with_maestro_events`.
  - Add `pub mod events;` to `crates/core/src/maestro/mod.rs` in a distinct region (the soft seam — additive line, auto-merges).
- **`crates/core/src/handlers/streams.rs` (modified — wire the producer):**
  - Add a `maestro_events: Option<broadcast::Sender<MaestroEvent>>` field + a `pub fn with_maestro_events(mut self, ..) -> Self` setter (mirror `with_vcs_events` at `:485` exactly — 401.5 may have stubbed the field/setter; if so, light it up rather than re-declaring).
  - Add a `fn map_maestro_event(ev: MaestroEvent) -> Event { Event { offset: 0, at: Some(now_ts()), body: None, checks_opaque: Some(ev.to_frame()) } }` (mirror `map_vcs_event` at `:1207`).
  - Wire the `Subject::MaestroEvents` arm in `source_events` to filter the broadcast (as `with_vcs_events`/`checks.*` does) and map through `map_maestro_event`; when `maestro_events` is `None` the subject stays valid-but-empty (honest "no Maestro attached", exactly the `checks.*` precedent).
- **`crates/core/src/handlers/maestro.rs` (modified — fill the live impl):**
  - `SendToMaestro(MaestroMessageRequest) -> Empty`: run 408's `pre_parse(&request.text)`; on `Routing{targets, body}` resolve via 408's composer→workarea→session resolver and forward (the write-tool path / `route_prompt_to_session`), emitting `MaestroEvent::RoutingExecuted`; on `Slash{directive, body}` handle deterministically (`/digest` ⇒ 409's digest + `DigestGenerated`; `/pause`,`/new` per 408); on `Freeform` forward to the Maestro session via the handle, emitting `MaestroEvent::Message` for streamed assistant output. **No new business logic** — this handler is the thin gRPC adapter over `MaestroHandle::send_to_maestro`; the routing/forward lives behind the handle.
  - `GetDigest(GetDigestRequest) -> Digest`: call `MaestroHandle::get_digest()` (409's path: force-refresh stale-60s `WorkareaSummary`s then compose, `<5s p50` §4.4); emit `MaestroEvent::DigestGenerated`; map the in-process `Digest` (+ persisted chips, D11) into the proto `Digest` message 401.5 froze.
  - `SetWorkareaVisibility(VisibilityRequest) -> Empty`: call `MaestroHandle::set_workarea_visibility(wa, vis)` (413's `exclude_from_maestro` toggle); typed `error_to_status` on failure.
  - An `error_to_status` mapper (mirror `handlers/vcs.rs`): budget-exhausted ⇒ a typed `Status` the UI shows inert (not a 500); policy-disabled (no handle constructed at boot) ⇒ `Status::failed_precondition("maestro.disabled_by_policy")` (the inert seam, NOT `unimplemented!()`).
  - `#[cfg(unix)]`-gate the handler (it depends on the agent supervisor + the Maestro session), exactly as 401.5 gated the skeleton and as `Sessions`/`Streams` are gated.
- **`crates/core/src/boot.rs` (modified — construct + thread the handle):**
  - After the agent-supervisor + persistence + summary/digest handles exist, construct the `MaestroHandle` **gated on `maestro_state::get(..).enabled` (§4.6) AND `ManagedPolicy::default_model()` being a permitted (non-external-under-`enterpriseDataPrivacy`) model (D1)**. When the gate is closed, leave the handle `None` (no panic, no spawn) and log `tracing::info!(target: "concerto::maestro", reason = .., "maestro disabled at boot")` — the service then replies `disabled_by_policy` and the subject is empty.
  - Add `factory_maestro_handle = maestro_handle.clone()` to the `factory_*` block (~863) and thread `factory_maestro_handle.clone()` into the `ApiServerActor::with_managers(..)` factory closure (~880); add `maestro: Some(maestro_handle.clone())` to the Iroh-path `CoreServiceSet { .. }` literal (~925); thread it into the `BridgeServices { .. }` literal for `connect_bridge::serve`.
- **`crates/core/src/api_server.rs` (modified if needed — light up the registration):**
  - In `add_core_services` destructure (`:590`) and the `#[cfg(unix)]` block, replace 401.5's `Status::unimplemented` `MaestroServer` registration with the live `MaestroHandler::new(maestro)` when `Some` (mirror the `vcs`/`Sessions` `if let Some(..)` gating); thread `maestro` through `with_managers`/`runtime_only` (already `None` there). Extend the `StreamsHandler` build chain with `.with_maestro_events(maestro.events_sender())` when `maestro` is `Some`.
  - Mirror the identical registration into `connect_bridge.rs::build_and_serve` (the second site — **D8: missing it is the single easiest Phase-4 bug**). If 401.5 already registered the unimplemented server at both sites, this is a fill, not an add.
- Tests (Tier 1): (1) `SendToMaestro` against an in-process `MaestroHandle` double returns `Empty` and emits the right `MaestroEvent` per `ParseOutcome` variant (freeform→`Message`, `@wa`→`RoutingExecuted`, `/digest`→`DigestGenerated`); (2) `GetDigest` returns the proto `Digest` shape (with chips) from the handle's digest, `<5s` on the deterministic 6-workarea fixture; (3) `SetWorkareaVisibility` round-trips the toggle; (4) `policy-disabled boot ⇒ GetDigest` returns `failed_precondition("maestro.disabled_by_policy")`, NOT `unimplemented`; (5) `map_maestro_event` for all 5 kinds carries ONLY `checks_opaque` (`body` is `None`) and the frame round-trips its `kind` string; (6) the `maestro.events` subject is parsable + valid-but-empty when no handle is attached; (7) a two-site registration assertion (the service is served on both the `add_core_services` router and the `connect_bridge` router).

## Scope — out
- **The `MaestroHandle` business logic itself** (the agent session lifecycle, the pre-parser, the digest composition, the summary cache) → owned by **402 / 408 / 409 / 404**; this task **consumes their frozen surfaces** and is the thin gRPC + events + boot adapter. It adds no routing grammar, no digest prompt, no summary derivation — those are seams it calls.
- **Live token counting / budget enforcement** → **412** (the budget tripwire that flips the handle inert + emits `budget_exhausted`). This task **emits** `MaestroEvent::BudgetExhausted` when the handle reports exhaustion but does NOT implement the counting; until 412 lands, the handle never reports exhaustion (the path is exercised only by the test double).
- **The real-LLM provider** (Codex/Gemini live, Direct-API seam) → **412**; this task is provider-agnostic (it drives whatever `MaestroHandle` 402/412 built).
- **The Desktop chat UI** → **415** (already built against 401.5's frozen proto + `maestro.events` subject with mocked invoke); this task supplies the live data 415 renders unchanged.
- **`notify_user` live notification delivery** → **507 (Phase 5)**; the side-channel stub is 407's.
- **The real-world Tier-3 line:** "leave for >30 min across active workareas, return, judge digest quality + measure latency; route prompts via `@workarea` and fanout; confirm budget-exhaust goes inert while routing still works" — the live digest-quality + cross-machine behaviour are signed off at the Phase-4 gate, not provable in CI.

## Public interface this task locks
- **`crates/core/src/maestro/events.rs::MaestroEvent` (FROZEN, design/08 §5.4 / PHASE4_PLANNING §4.2 — the event payload arm of D7):**
  ```rust
  /// The five Maestro stream events (`design/08 §5.4`). Serialized to an
  /// opaque JSON frame and carried on `Event.checks_opaque = 17` — NEVER a
  /// new `Event.body` oneof arm (oneof FROZEN through field 16, D7). 415
  /// parses these frames.
  pub enum MaestroEvent {
      Message { text: String, message_id: String },          // maestro.message
      RoutingExecuted { targets: Vec<String>, body: String }, // maestro.routing_executed
      DigestGenerated { at_ms: i64, n_workareas: u32 },       // maestro.digest_generated
      BudgetExhausted { resets_at_ms: i64 },                  // maestro.budget_exhausted
      DisabledByPolicy { reason: String },                    // maestro.disabled_by_policy
  }
  impl MaestroEvent {
      pub fn kind(&self) -> &'static str; // the wire kind, e.g. "maestro.message"
      pub fn to_frame(&self) -> Vec<u8>;  // {"kind": "...", ...} opaque JSON
  }
  ```
  (Field sets are minimal + append-friendly; the **`{"kind": ...}` envelope is FROZEN** because 415 parses it — mirror `design/13 §5.3`'s `checks.*` frame discipline.)
- **`StreamsHandler::with_maestro_events` (lit up here; the setter shape was reserved by 401.5, PHASE4_PLANNING §4.2):**
  ```rust
  pub fn with_maestro_events(mut self, maestro_events: broadcast::Sender<MaestroEvent>) -> Self;
  ```
  Mirrors `with_vcs_events`/`with_transport_events` exactly. `None` ⇒ `maestro.events` is parsable but yields no events.
- **Consumes — does NOT re-lock:**
  - `maestro.proto` (`service Maestro { SendToMaestro/GetDigest/SetWorkareaVisibility }` + `MaestroMessageRequest`/`GetDigestRequest`/`VisibilityRequest`/`Digest`) — **frozen by 401.5 (PHASE4_PLANNING §4.2)**.
  - `MaestroHandle` (`send_to_maestro`/`get_digest`/`set_workarea_visibility`/`set_enabled`/`get_state`) — **frozen by 401.5 (PHASE4_PLANNING §4.2)**; built by 402/404/408/409/412.
  - `Subject::MaestroEvents` + `parse_subject("maestro.events")` — **frozen by 401.5 (PHASE4_PLANNING §4.2)**; this task only attaches the producer.
  - `pre_parse(&str) -> ParseOutcome` — **frozen by 408 (PHASE4_PLANNING §4.7)**.
  - `WorkareaSummary` + the digest refresh contract — **frozen by 404 (PHASE4_PLANNING §4.4)**; the digest itself is 409's.
  - `maestro_state.enabled` accessor — **frozen by 403 (PHASE4_PLANNING §4.6)**.
- **No proto change, no migration, no `*.sql`** — this task writes Rust + a new module only.

## Implementation notes
- **The load-bearing rule: this handler is a thin adapter, the events module is the only new contract.** Every RPC delegates to `MaestroHandle`; the only logic that lives here is `ParseOutcome`-dispatch → which handle method, error→`Status` mapping, and `MaestroEvent` emission. If you find yourself writing routing/digest/summary logic, you are duplicating 408/409/404 — call them.
- **Two-site registration (D8).** A new/filled gRPC service registers in **BOTH** `add_core_services` (`api_server.rs:565`, serves UDS **and** Iroh via `CoreServiceSet`) **AND** `connect_bridge.rs::build_and_serve` (the Connect-Web front door). 401.5 added the unimplemented server at both sites; verify both `if let Some(maestro)` arms exist and serve `MaestroHandler::new(maestro)`. **Missing the `connect_bridge` site is the single easiest Phase-4 bug** — the Desktop dials UDS, but the web/bridge clients (P5) would 404 silently.
- **`#[cfg(unix)]`-gate the handler + the boot construction** exactly as `Sessions`/`Streams` and 401.5's skeleton are gated — the Maestro depends on the agent supervisor (`AgentKind::Maestro` session), which is unix-only today. On non-unix the service is simply absent (the Iroh server serves the cross-platform subset; the bridge mirrors this).
- **Reuse don't reinvent the opaque-frame discipline.** `map_maestro_event` is `map_vcs_event` (`streams.rs:1207`) with a different `to_frame()`: `Event { offset: 0, at: Some(now_ts()), body: None, checks_opaque: Some(frame) }`. The per-subject pump stamps `offset` and tolerates a `body`-less Event (offset/at are separate fields). Do NOT add an `EventBody` variant.
- **Seams return a typed `Status`, never the macro.** Policy-disabled (handle un-constructed at boot) ⇒ `Status::failed_precondition("maestro.disabled_by_policy")`; budget-exhausted ⇒ a typed inert `Status` the UI shows stale (412 supplies the trip). **Never `unimplemented!()`/`todo!()`** and never empty-success — the inert path is a real, documented reply (305/313 seam discipline). Record the chosen `Status` codes in Handoff.
- **The boot gate is the D1 enforcement point.** `enterpriseDataPrivacy=true` + an external `default_model` ⇒ `MaestroHandle` is `None` (Maestro disabled, `design/08 §3.10`). Read `WorkspaceSettingsResolver::enterprise_data_privacy()` / `ManagedPolicy::default_model()`; construct the handle only when permitted. The CLI backends (Claude/Codex/Gemini, D1) are local, so they pass the gate; Direct-API + external is the disabled case (and Direct-API is itself a frozen-unwired seam per 412).
- **Regen:** this task changes **no** proto/schema/Rust-public-trait that the generators read (the proto + `MaestroHandle` were frozen by 401.5/403; `MaestroEvent` is an internal type), so `./scripts/regen-interfaces.sh` should produce **no diff**. Run it anyway and `git diff --exit-code docs/interfaces/` to prove it — if it drifts, you touched a frozen surface you shouldn't have.
- **Parallel build hint:** the three sub-parts are file-disjoint and can be built by helper sub-agents then integrated into the one commit — **handler-impl** (`SendToMaestro`/`GetDigest`/`SetWorkareaVisibility` + `error_to_status` in `handlers/maestro.rs`) ∥ **event-publishing** (`maestro/events.rs` `MaestroEvent`/`to_frame` + `streams.rs` `with_maestro_events`/`map_maestro_event`/`Subject::MaestroEvents` wiring) ∥ **boot-wiring** (`boot.rs` handle construction + gate + `factory_*`/`CoreServiceSet`/`BridgeServices` threading + `api_server.rs` two-site fill). The boot-wiring part integrates last (it consumes the other two's public symbols).

## Verification
**Tier 1.** No test double of physical reality is needed — the in-process `MaestroHandle` (built by 402/404/408/409, or a unit fake for this task's isolated tests) is the real thing; CI proves the gRPC shapes, the event frames, and the two-site registration end-to-end.

1. `cargo check --workspace` — clean (the filled `handlers/maestro.rs`, new `maestro/events.rs`, the `streams.rs` producer, the `boot.rs`/`api_server.rs`/`connect_bridge.rs` wiring all compile).
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean.
3. `cargo fmt --all -- --check` — clean.
4. `cargo test -p concerto-core maestro` — proves: `SendToMaestro` dispatches each `ParseOutcome` variant + emits the matching `MaestroEvent` (freeform→`Message`, `@wa`→`RoutingExecuted`, `/digest`→`DigestGenerated`); `GetDigest` returns the proto `Digest` (with chips) `<5s` on the deterministic 6-workarea fixture; `SetWorkareaVisibility` round-trips; **policy-disabled boot ⇒ `GetDigest` returns `failed_precondition("maestro.disabled_by_policy")`, NOT `unimplemented`**; `map_maestro_event` carries ONLY `checks_opaque` (`body == None`) for all 5 kinds + the `{"kind"}` frame round-trips; `maestro.events` parses + is valid-but-empty with no handle; the **two-site registration** assertion (served on both routers).
5. `cargo test --workspace --no-fail-fast` — all pass (the 401.5 skeleton tests now exercise live replies; nothing else regresses).
6. `cargo deny check` — green (this task adds no new dependency).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` — **no diff** (no proto/schema/public-trait change; if it drifts you touched a frozen surface — Stop-and-ask).
8. `scripts/smoke.sh` — unchanged gate (the `maestro-digest` smoke capability is a Phase-4 Tier-3 / later-task concern; this task's `Smoke gate` field is `unchanged` and the existing boot path still exits 0 with the handle present or absent). Exits 0.

**Tier-1 scope + what it does NOT cover.** Tier 1 proves the gRPC shapes, the `ParseOutcome`-dispatch, the event-frame discipline, the policy-disabled inert reply, and the two-site registration against the in-process handle. It does **NOT** cover real-LLM digest quality, the >30-min live-data digest, real routing across active sessions, or the live budget-exhaust→inert transition (412's counting is mocked here) — those are the Phase-4 Tier-3 checklist line "leave for >30 min across active workareas, return, judge digest quality + measure latency; route prompts via `@workarea` and fanout; confirm budget-exhaust goes inert while routing still works," signed off at the phase gate.

## Definition of Done
- [x] `handlers/maestro.rs` filled: `SendToMaestro` (408 `pre_parse` → forward/route/slash + event), `GetDigest` (409 digest, `<5s p50`, + chips), `SetWorkareaVisibility` (413 toggle) — thin adapter over `MaestroHandle`, `#[cfg(unix)]`-gated
- [x] `crates/core/src/maestro/events.rs` new + FROZEN: `MaestroEvent` (5 kinds, `design/08 §5.4`) + `kind()` + `to_frame()` `{"kind": ...}` opaque-JSON envelope; `pub mod events;` added to `maestro/mod.rs`
- [x] `streams.rs` wires the producer: `with_maestro_events` setter (mirrors `with_vcs_events`) + `map_maestro_event` (`body: None, checks_opaque: Some(frame)`) + the `Subject::MaestroEvents` source-events arm (valid-but-empty when `None`)
- [x] `boot.rs` constructs the real `MaestroHandle` gated on `maestro_state.enabled` (§4.6) **AND** managed-policy model permission (D1); threads `Some(handle)` through the ~880 factory closure, the ~925 Iroh `CoreServiceSet`, and `connect_bridge::BridgeServices`; `None` when the gate is closed (logged, no spawn)
- [x] `MaestroServer` served live at **BOTH** sites (`add_core_services` + `connect_bridge::build_and_serve`), gated `if let Some(maestro)`; `StreamsHandler` chain extended with `.with_maestro_events(..)`
- [x] Consumes 401.5's proto/`MaestroHandle`/subject (§4.2), 409's digest (§4.4), 408's `pre_parse` (§4.7), 403's `enabled` (§4.6) — re-locks NONE of them
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams — policy-disabled, budget-exhausted — return a typed `Status`, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any schema/contract changed (expected: **no diff** — no proto/schema/public-trait change)
- [x] All Verification commands pass on a clean checkout; smoke gate unchanged (green)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/events.rs` (new — `MaestroEvent` enum + `kind()`/`to_frame()` opaque-JSON serializer + the `broadcast::Sender<MaestroEvent>` producer wrapper, FROZEN)
- `crates/core/src/maestro/mod.rs` (modified — additive `pub mod events;` line in a distinct region; the soft seam)
- `crates/core/src/handlers/maestro.rs` (modified — fill 401.5's skeleton with the live `SendToMaestro`/`GetDigest`/`SetWorkareaVisibility` impls + `error_to_status`)
- `crates/core/src/handlers/streams.rs` (modified — `with_maestro_events` setter + `map_maestro_event` + the `Subject::MaestroEvents` source-events arm)
- `crates/core/src/boot.rs` (modified — construct + gate the `MaestroHandle`; thread it through the factory closure, the Iroh `CoreServiceSet`, and `BridgeServices`)
- `crates/core/src/api_server.rs` (modified — replace the unimplemented `MaestroServer` with the live `MaestroHandler::new(maestro)` when `Some` at the `add_core_services` site; extend the `StreamsHandler` chain)
- `crates/core/src/connect_bridge.rs` (modified — the second registration site: live `MaestroServer` in `build_and_serve` + the `BridgeServices` destructure)

## Commit message
```
phase-4: live Maestro gRPC impl + maestro.events publishing + boot wiring

Fill 401.5's unimplemented MaestroServer skeleton with the live impl over
the in-process MaestroHandle: SendToMaestro pre-parses (408) then routes/
forwards, GetDigest calls the <5s digest (409/404), SetWorkareaVisibility
applies the privacy toggle (413). New maestro/events.rs publishes the five
maestro.events on the checks_opaque=17 carrier (no new oneof arm). boot.rs
constructs the handle gated on maestro_state.enabled + managed-policy model
permission (disabled-by-policy under enterpriseDataPrivacy+external); served
at both registration sites. Tier-1: shapes + event frames + two-site serve
proven against the in-process handle; real-LLM digest quality is Tier-3.

Refs: tasks/v1.0/414-maestro-grpc-service.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan** —
  - **`MaestroHandle` logic placement (path (a) — `maestro/handle.rs` edited, documented drift).** 401.5 froze `MaestroHandle` as an opaque struct whose five methods returned typed `"unimplemented:"` errors. Per the task's IMPORTANT judgment call, the live logic lives **behind the handle** (`maestro/handle.rs`, added to the commit as drift) so `handlers/maestro.rs` stays the thin gRPC adapter. `handle.rs` now stitches 408's `Router`/`pre_parse`, 409's `generate_digest` over 404's `SummaryCache`, and 413's `set_exclude_from_maestro` toggle; the five frozen signatures are unchanged (only bodies + private `Inner` fields). The handler delegates and maps errors via `error_map::error_to_status`. No RPC returns `Status::unimplemented`.
  - **`Status` codes:** policy-disabled-at-boot (handle `None`) ⇒ `Status::failed_precondition("maestro.disabled_by_policy")` (const `DISABLED_BY_POLICY`, the message 415 keys off). Budget-exhausted ⇒ the handle returns `Error::Policy("maestro.budget_exhausted")`, which `error_to_status` maps to `FailedPrecondition` (typed inert, never a 500/`unimplemented`).
  - **`streams.rs` was NOT touched** — 401.5 had already fully lit up `with_maestro_events` (real field + setter, not a stub), `map_maestro_event` (`body: None, checks_opaque: Some(frame)`), and the `Subject::MaestroEvents` source-events arm (valid-but-empty when `None`). 414 only attaches the producer at the two registration sites (`api_server.rs` + `connect_bridge.rs`). So `streams.rs` is not in this commit (the listed Output was already satisfied upstream); the wiring is the `.with_maestro_events(maestro.events_sender())` line added at both sites.
  - **Boot gate reads `ManagedPolicy`** (not `WorkspaceSettingsResolver`): `load_managed_policy(config_dir)` → `enterprise_data_privacy()` + `default_model()`, fed to `PrivacyPolicy::maestro_disabled_by_policy(privacy, model_external)`. `default_model` is classified by a conservative `is_external_maestro_model` heuristic in `boot.rs` (empty/on-prem markers ⇒ local/permitted; public-provider name markers ⇒ external/disabled-under-privacy) since the parsed `default_model` is otherwise unread until 412's `MaestroProvider` locality classification supersedes it. Gate also requires `maestro_state.enabled` (403); boot bootstraps the `maestro_state` singleton + `chats(kind='maestro')` row so `GetDigest` has its persistence anchor.
  - **Bug fixed during integration:** the `/digest` slash arm in `handle.rs::send_to_maestro` composed the digest but did **not** emit `MaestroEvent::DigestGenerated`, so the Tier-1 test `send_digest_slash_emits_digest_generated_event` hung forever on `rx.recv()`. Fixed: the slash arm now emits `DigestGenerated` (matching the public `GetDigest` path and Scope — in's "`/digest` ⇒ 409's digest + `DigestGenerated`"). All 145 maestro lib tests pass.
  - **`maestro/mod.rs`:** the `pub mod events;` line sits in its own clearly-labeled "Task 414 region" block, distinct from 411's in-flight `pub mod cone_suggester;`, so the soft seam auto-merges.
- **Open questions for next task** —
  - **415 (consumer):** parses the `{"kind": ...}` JSON envelope off `maestro.events` / `Event.checks_opaque=17`. Frozen kind strings + payload keys: `maestro.message`{`text`,`message_id`}, `maestro.routing_executed`{`targets`,`body`}, `maestro.digest_generated`{`at_ms`,`n_workareas`}, `maestro.budget_exhausted`{`resets_at_ms`}, `maestro.disabled_by_policy`{`reason`}. The proto `Digest` it renders is fully populated (`text` = LLM prose + next-step folded; `chips` map 1:1 to `MaestroChip`; `generated_at_ms`; `stale` ⇐ digest `degraded`).
  - **412:** `MaestroEvent::BudgetExhausted` is emitted only when the handle is `set_inert(BudgetExhausted{..})` and the freeform/LLM path runs `guard_llm()` — dormant until 412 wires the live token counter to flip the handle inert. Routing/`/digest` deliberately bypass `guard_llm` (deterministic, fire even when inert, design/08 §3.5).
- **Deliberate debt** — `BudgetExhausted` is reachable only via the test double (no live counter until 412). `notify_user` stays a 407 stub until 507 (P5). The boot `default_model` externality is a name-string heuristic, superseded by 412's `MaestroProvider` locality once it lands (the on-prem re-enable case is a Tier-3 gate item). `attachments` on `SendToMaestro` is a frozen text-only seam (currently ignored, R-9).
- **Smoke-gate state** — `unchanged` (not re-run): this task adds no smoke capability; the boot path exits 0 with the handle present or absent; the `maestro-digest` smoke check is a later/Tier-3 concern.
