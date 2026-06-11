# Task 416 — `Maestro.GetState` RPC (expose `MaestroStateView` on the wire: budget counters + caps + inert + Maestro session id)

| Field | Value |
|---|---|
| Phase | 4 (UI-completion addendum) |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | 414, 412, 403 |
| Touches subsystem(s) | 08 (Maestro), 10 (Local API) |
| Smoke gate | unchanged |

## Goal
Give the Desktop a way to **read the live Maestro state** so it can render the budget meter (80% amber / 100% red), the enabled/inert state, the stale/last-digest cursor, and — critically — discover the **Maestro singleton session id** so the chat can subscribe to that session's write-tool `AwaitingApproval` frames (Task 417's confirmation-chip producer). Today the wire surface is write-only on state: `maestro.proto` has `SendToMaestro`/`GetDigest`/`SetWorkareaVisibility` but **no way to read `MaestroStateView`** — `apps/desktop/src/api/maestro.ts:75` documents this exact gap ("`MaestroState` is NOT yet a `maestro.proto` message nor an exposed `Maestro.*` RPC"), so `<BudgetBanner>` is fed `null` and the amber/red counter path can never fire. The Rust read-model already exists: `MaestroHandle::get_state() -> MaestroStateView` (`crates/core/src/maestro/handle.rs:304`, fields `enabled`/`daily_in_today`/`daily_out_today`/`last_digest_at_ms`), the handle tracks its `inert` reason (`handle.rs:184` `inert_reason`), the budget caps live in 412's `TokenBudget` (200K in / 50K out), and the singleton session is resolvable via `MaestroHandle::maestro_session_id()` (`handle.rs:389`). This task adds an **additive `Maestro.GetState` RPC** returning a new `MaestroState` proto message carrying all of the above, fills the handler (mirroring the existing `get_digest` handler), and regenerates interfaces. The proto change is **purely additive** (a new RPC + new messages; no existing field number or message changes), so it does not violate 401.5's freeze — it extends the surface, it does not re-lock it.

## Inputs to read before starting
- `crates/proto/proto/concerto/v1/maestro.proto` — the `service Maestro` block (3 RPCs) + the `MaestroChip`/`Digest` message style to mirror; append the new RPC + messages after `SetWorkareaVisibility` / the last message.
- `crates/core/src/maestro/handle.rs:48` (`MaestroStateView`), `:304` (`get_state`), `:184` (`inert_reason`), `:389` (`maestro_session_id`) — the read-model + the inert + session-id sources the handler assembles. **If `get_state`/`MaestroStateView` does not already surface the budget caps or the inert reason or the session id, extend the handler's assembly (read them from the handle) — do NOT change the existing `MaestroStateView` field set unless needed; if you do extend it, keep it additive.**
- `crates/core/src/llm/provider.rs` (Task 412's `TokenBudget`) — the daily caps (`200_000` in / `50_000` out) + the inert/`InertReason` shape, so `GetState` reports `in_cap`/`out_cap` + `inert`/`inert_reason` consistently with what 412 enforces and 414 emits on `maestro.budget_exhausted`/`maestro.disabled_by_policy`.
- `crates/core/src/handlers/maestro.rs:89` (`get_digest` handler — mirror its shape: take `&self.maestro` handle, call the handle method, `map_err(error_to_status)`, return the proto). The handler is `#[cfg(unix)]`-gated; mirror that. When the handle is `None` (policy-disabled boot) `GetState` returns `failed_precondition("maestro.disabled_by_policy")` exactly like the other RPCs (`handlers/maestro.rs` `error_to_status` / the disabled path) — never `unimplemented!()`.
- `apps/desktop/src/api/maestro.ts:69-89` — the TS `MaestroState` read-model the Desktop already declares; the proto `MaestroState` field names should map cleanly onto it (snake_case on the wire). 417 adds the `Maestro.GetState` binding consuming this RPC.

## Scope — in
- **`crates/proto/proto/concerto/v1/maestro.proto`** (additive):
  - `message GetStateRequest {}`.
  - `message MaestroState { bool enabled = 1; int64 daily_in_today = 2; int64 daily_out_today = 3; int64 in_cap = 4; int64 out_cap = 5; int64 last_digest_at_ms = 6; bool inert = 7; string inert_reason = 8; string maestro_session_id = 9; }` — `last_digest_at_ms = 0` ⇒ never; `inert_reason` ∈ `"" | "budget_exhausted" | "disabled_by_policy"`; `maestro_session_id = ""` when no live Maestro session. Field numbers FROZEN as of this task.
  - `rpc GetState(GetStateRequest) returns (MaestroState);` appended to `service Maestro`.
- **`crates/core/src/handlers/maestro.rs`** — fill the `get_state` handler: when `self.maestro` is `Some(handle)`, assemble the `MaestroState` proto from `handle.get_state()` (enabled/counts/last_digest) + the budget caps (412) + `handle.inert_reason()` (→ `inert` + `inert_reason` string) + `handle.maestro_session_id()` (→ `maestro_session_id`, empty on `Err`/none); when `None`, `failed_precondition("maestro.disabled_by_policy")`. `#[cfg(unix)]`-gated.
- **`crates/core/src/maestro/handle.rs`** (only if needed) — if assembling the caps/inert/session-id for the handler needs a small accessor, add it additively (do not change the frozen `MaestroStateView` shape unless you extend it additively + note in Handoff).
- **Regen:** `./scripts/regen-interfaces.sh` updates `docs/interfaces/proto.md` (gains `GetState` + `MaestroState`/`GetStateRequest`). Commit it.
- Tests (Tier 1): a handler test asserting `GetState` returns the populated `MaestroState` for an attached handle (enabled, the counters, the caps, `maestro_session_id` non-empty when a session exists), and returns `failed_precondition("maestro.disabled_by_policy")` when the handle is `None`.

## Scope — out
- The Desktop binding + the budget-meter render — **Task 417** (consumes this RPC).
- Live token counting / the budget tripwire — **Task 412** (already merged); this RPC only *reports* the current counters/caps/inert.
- Any change to the existing `SendToMaestro`/`GetDigest`/`SetWorkareaVisibility` field numbers or messages — forbidden (this task is purely additive).

## Public interface this task locks
- **`Maestro.GetState` + `MaestroState` proto message** (FROZEN by this task; additive to 401.5's surface). The field set above is the contract 417's binding mirrors.

## Implementation notes
- **Additive-only proto change.** Appending a new RPC + new messages to an existing service is forward-compatible and does not re-lock 401.5's frozen fields — call this out in the proto comment ("Task 416: additive GetState; existing field numbers unchanged").
- **Mirror `get_digest`'s handler exactly** — same `#[cfg(unix)]` gate, same `error_to_status` mapping, same `Option<handle>` `disabled_by_policy` path. No new business logic; this is a read projection.
- **`maestro_session_id` is the load-bearing add for 417** — without it the Desktop cannot subscribe to the Maestro session's `session.events.<sid>` to surface write-tool confirmation chips. Source it from `MaestroHandle::maestro_session_id()`; empty string when there is no live session.
- Two-site registration does NOT apply — `GetState` is a new RPC on the already-registered `Maestro` service (served at both `add_core_services` + `connect_bridge`).

## Verification
**Tier 1.** `rust` §5.3 set.
1. `cargo check --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --all -- --check` — clean.
2. `cargo test -p concerto-core maestro` — the `GetState` handler test (attached ⇒ populated `MaestroState`; `None` ⇒ `disabled_by_policy`).
3. `cargo test --workspace --no-fail-fast` — all pass.
4. `cargo deny check` — green (no new deps).
5. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` — commit the regen (`proto.md` gains `GetState`/`MaestroState`).
6. `scripts/smoke.sh` — unchanged.

## Definition of Done
- [ ] `Maestro.GetState` RPC + `MaestroState`/`GetStateRequest` messages added (additive; existing field numbers unchanged)
- [ ] `get_state` handler filled (attached ⇒ enabled/counts/caps/last_digest/inert/inert_reason/maestro_session_id; `None` ⇒ `failed_precondition("maestro.disabled_by_policy")`); `#[cfg(unix)]`-gated, `error_to_status`
- [ ] Tier-1 tests pass; smoke unchanged; interfaces regenerated + committed
- [ ] No TODO/FIXME/unimplemented!()/todo!() in new code
- [ ] No files outside Outputs modified
- [ ] Single commit with the message below

## Outputs
- `crates/proto/proto/concerto/v1/maestro.proto` (modified — `GetState` RPC + `MaestroState`/`GetStateRequest`)
- `crates/core/src/handlers/maestro.rs` (modified — `get_state` handler)
- `crates/core/src/maestro/handle.rs` (modified only if a small additive accessor is needed)
- `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-4: Maestro.GetState RPC (expose budget/inert/session-id read-model)

Adds an additive Maestro.GetState RPC returning a new MaestroState message
(enabled, daily_in/out_today, in/out caps, last_digest_at_ms, inert +
reason, maestro_session_id) so the Desktop can render the live budget meter
and subscribe to the Maestro session's write-tool approvals (Task 417).
Handler mirrors get_digest (cfg(unix), error_to_status, disabled_by_policy
when the handle is None). Purely additive — 401.5's frozen field numbers
are unchanged.

Refs: tasks/v1.0/416-maestro-getstate-rpc.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** — (417 adds the `Maestro.GetState` binding + feeds `<BudgetBanner>`/`<DigestPanel>` and subscribes to `maestro_session_id`'s `session.events` for the confirmation-chip producer.)
- **Deliberate debt:** —
- **Smoke-gate state:** —
