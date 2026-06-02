# Task 217 — `TransportHandle` Public Rust API (`crates/transport/src/api.rs`)

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 212 |
| Touches subsystem(s) | 11 (Remote Transport & Relay) |
| Smoke gate | unchanged |

## Goal
Lock the **public Rust API of `crates/transport`** — the `TransportHandle` — that the rest of Core calls to drive the Iroh transport. Task 212 produced the `iroh::Endpoint`, the session map, `ConnectionPath`, and the `NatStats` groundwork; this task wraps that machinery behind the **exact `impl TransportHandle` surface `design/11 §5.1` specifies** so the seams the other Phase-2 tasks were written against actually exist. The handle is the single place the API server, revocation, pairing, and notifications reach the transport — `api_server` serves gRPC over the Iroh sessions it opens; revocation (Task 209) severs a stolen device through `close_sessions_for_device`; pairing (Task 207) listens for a pairing handshake through `listen_pairing`; notifications/P5 push a wakeup through `send_wakeup_hint`; diagnostics read `nat_stats`. After this task `crates/transport/src/api.rs` declares the frozen public surface (regen indexes it → `rust-api.md`), each method wraps Task 212's endpoint, and every method has a named downstream consumer. `WakeupPayload` is defined **minimally** here (its fields are fleshed out in P5/`design/14`).

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §5.1 — the **EXACT `impl TransportHandle` signatures to reproduce** (reproduce them verbatim as the frozen surface):
  ```rust
  pub struct TransportHandle { /* opaque */ }
  impl TransportHandle {
      pub async fn start(&self, cfg: TransportConfig) -> Result<()>;
      pub async fn stop(&self) -> Result<()>;
      pub async fn listen_pairing(&self, token_hash: [u8;32]) -> Result<PairingListener>;
      pub async fn close_pairing(&self) -> Result<()>;
      pub async fn current_relay(&self) -> Result<RelayInfo>;
      pub async fn switch_relay(&self, url: Url) -> Result<()>;
      pub async fn nat_stats(&self) -> Result<NatStats>;
      pub async fn close_sessions_for_device(&self, id: DeviceId) -> Result<()>;
      pub async fn send_wakeup_hint(&self, id: DeviceId, payload: WakeupPayload) -> Result<()>;
  }
  ```
- `design/11_Remote_Transport_Relay.md` §3.3 — the **three logical channels** (API channel, push-hint channel, pairing channel) the handle's methods front: `listen_pairing`/`close_pairing` gate the pairing channel; `send_wakeup_hint` the push-hint channel; the API channel is served by `api_server` over the sessions `start` brings up.
- `design/11_Remote_Transport_Relay.md` §4 — `TransportState` / `ActiveSession` / `NatStats` / `ConnectionPath` (the in-memory state the handle wraps; Task 212 owns these — `nat_stats()` returns the §4 `NatStats`, extended by-kind in Task 216).
- `design/11_Remote_Transport_Relay.md` §5.3 — the emitted events (`session_opened`/`session_closed`/`relay_switched`/`nat_success_changed`); `switch_relay`/`close_sessions_for_device` cause some of them. Event *emission* is Task 216's `TransportEvent`; the handle just triggers the underlying state change.
- `crates/transport/src/` (filled by **Task 212**) + `tasks/v1.0/212-transport-iroh-endpoint.md` → "Handoff Notes" — the live `iroh::Endpoint`, `TransportConfig`, `ConnectionPath`, `NatStats` groundwork, the `sessions`/`ActiveSession` map, the `PairingListener` type (if 212 created it or 207 did — reconcile), and **how 212 structured the crate** (where `api.rs` lives, what's already `pub`). The handle wraps exactly this.
- `crates/keychain/src/api.rs` (head) — the locked `api.rs` convention: this file declares the public surface **directly** (literal `pub struct`/`fn` decls, NOT `pub use` re-exports) because `regen-interfaces.sh` reads `crates/<crate>/src/api.rs` at depth exactly 3; impl bodies live in sibling modules. Mirror this for `TransportHandle`.
- `tasks/v1.0/209-devices-service.md` (full + Handoff) — Task 209 defines a **narrow local `SessionCloser` trait** (`fn close_sessions_for_device(&self, id: [u8;32])`) in `crates/core` that the revocation coordinator depends on, and injects a stub in tests; **217's `TransportHandle::close_sessions_for_device` is the production implementation that satisfies it.** Note the type mismatch to reconcile: 209's trait uses `[u8;32]`, §5.1 uses `DeviceId` (see Implementation notes).
- `tasks/v1.0/207-pairing-noise-xx.md` (full + Handoff) — Task 207's pairing coordinator calls `listen_pairing(token_hash)` to open the pairing channel; reconcile the `PairingListener` type ownership (207 vs 212 vs here) and the `token_hash: [u8;32]` shape (the 32-byte pairing-token hash, `12 §3.3`).
- `tasks/v1.0/216-quic-migration-telemetry.md` — Task 216 extends `NatStats` by client kind and surfaces it via Runtime/Devices; `nat_stats()` here returns that extended shape. (216 depends on 212, like this task; they touch the same crate — coordinate if both land near each other, noted in Handoff.)
- `tasks/v1.0/README.md` §5.3 (`rust`) + §6 row 217.

## Scope — in
- **`crates/transport/src/api.rs`** declaring the public surface **directly** (keychain convention): `pub struct TransportHandle` + the nine `impl TransportHandle` methods with the **exact §5.1 signatures**, plus the public types they reference that `crates/transport` owns: `TransportConfig`, `RelayInfo`, `WakeupPayload` (minimal — see below), and re-/co-locating `NatStats` / `PairingListener` / `DeviceId` per how Task 212 already declared them (do not duplicate a type 212 owns — reference it). Impl bodies live in sibling modules (`src/handle.rs` or similar); `api.rs` holds the decls the generator reads.
- **Method implementations**, each wrapping Task 212's endpoint/state:
  - `start(cfg)` / `stop()` — bring the Iroh endpoint up/down (register/deregister with the relay, start/stop the accept loop). If 212 already starts the endpoint at construction, `start`/`stop` are the explicit lifecycle controls layered on top — reconcile with 212's model.
  - `listen_pairing(token_hash)` / `close_pairing()` — open/close the pairing channel gated by the 32-byte token hash, returning the `PairingListener` Task 207 consumes.
  - `current_relay()` / `switch_relay(url)` — read the active relay (`RelayInfo`) / point the endpoint at a new relay URL (triggers the underlying `relay_switched`).
  - `nat_stats()` — return the current `NatStats` (the §4 shape extended by-kind in 216).
  - `close_sessions_for_device(id)` — terminate all open sessions/streams for a device (the revocation sever, < 1 s budget per `12 §10`; Task 209 measures the latency against its stub, this is the real teardown).
  - `send_wakeup_hint(id, payload)` — send a `WakeupPayload` over the push-hint channel to a device (the live wiring of the side that P5 notifications drive).
- **`WakeupPayload` defined minimally**: the smallest shape that lets `send_wakeup_hint` compile and route (e.g. an opaque/ID-only carrier — the **ID-only wakeup payload** principle is locked in `design/14`/P5; the privacy invariant is *no PII in the payload*, Task 506). Declare it here with a doc note that P5/`design/14` fleshes the fields; **do not** speculatively add notification semantics.
- **Tier-1 unit tests**: each method is exercised against the in-process endpoint (the handle wraps 212's machinery, unit-testable without a real NAT) — `start`→`stop` lifecycle is idempotent/clean; `listen_pairing`→`close_pairing` opens then releases the pairing channel; `switch_relay` updates what `current_relay` returns; `nat_stats` returns the live counters; `close_sessions_for_device` removes the targeted device's sessions (assert against an in-process session); `send_wakeup_hint` routes to the right device's push-hint channel (or errors cleanly for an unknown device). These are the **internal Rust API surface** tests; the real cross-device behaviors are downstream tasks' Tier-2/Tier-3.

## Scope — out
- The **Iroh endpoint / hole-punch / relay-fallback / `ConnectionPath` / `NatStats` groundwork** — **Task 212** (this task wraps it; it does not build the endpoint).
- **Migration handling + `TransportEvent` emission + by-kind `NatStats` increments** — **Task 216** (`nat_stats()` here just *returns* the stats 216 populates).
- The **`SessionCloser` trait declaration + the revocation coordinator** — **Task 209** (this task provides the `TransportHandle` that satisfies the trait; the wiring is a one-line construction noted below).
- The **pairing Noise XX handshake + token store** — **Task 207** (this task exposes `listen_pairing`/`close_pairing`; 207 drives them).
- The **push-hint backend / Expo / the full `WakeupPayload` fields / notification fan-out** — **P5, Tasks 503/504/507** (this task defines `WakeupPayload` minimally + the `send_wakeup_hint` seam).
- The **WSS bridge** (Task 215) and the **mDNS responder** (Task 213) — separate surfaces; the handle does not front them.
- Any **gRPC/proto** — the handle is a pure Rust API; the gRPC the API channel carries is `10`'s Tonic services served by `api_server`, not a method here.

## Public interface this task locks
- **The `TransportHandle` method set** — the nine §5.1 signatures (`start`, `stop`, `listen_pairing`, `close_pairing`, `current_relay`, `switch_relay`, `nat_stats`, `close_sessions_for_device`, `send_wakeup_hint`) with their exact argument/return types — FROZEN. This is the contract `api_server` / 207 / 209 / P5-notifications call; renaming or re-shaping a method breaks a named downstream consumer.
- **`TransportConfig`, `RelayInfo`, `WakeupPayload`** (the transport-owned public types in the signatures) — FROZEN at the surface level; `WakeupPayload`'s **fields** are explicitly *not* frozen (P5/`design/14` fleshes them) but its existence + that it is the `send_wakeup_hint` payload **is** frozen.
- **`close_sessions_for_device` as the `SessionCloser` realization** — its signature is the contract Task 209's narrow trait targets; the device-id type is reconciled (see notes) and frozen so 209's wiring lands without a rename.

## Implementation notes
- **Reproduce §5.1 verbatim, don't improve it.** The signatures are the locked contract other tasks were authored against (209 named `close_sessions_for_device`, 207 named `listen_pairing` with `[u8;32]`, P5 named `send_wakeup_hint`). Match names, async-ness, arg types, and `Result` returns exactly. The struct is `/* opaque */` — keep the internals private; only the methods are public.
- **`DeviceId` vs `[u8;32]` — reconcile and freeze.** §5.1 uses `DeviceId` for `close_sessions_for_device`/`send_wakeup_hint`; Task 209's `SessionCloser` trait uses `[u8;32]`. Resolve to **one** type: either `DeviceId` is a transparent newtype/alias over `[u8;32]` (preferred — keeps §5.1's name and 209's bytes compatible via `From`/`Into` or by `DeviceId` *being* `[u8;32]`), or 209's trait is adapted at the wiring site. Pick the conversion that lets 209's `Arc<dyn SessionCloser>` accept this handle **without changing 209's frozen trait**; document the choice and note it for 209/210 in Handoff. Whatever 212 already chose for its session-map key wins — align to it.
- **`PairingListener` ownership.** 212 and/or 207 may already define `PairingListener`. Do **not** create a third. Reference the existing one; if neither created it, define it here minimally (the handle returns it; 207 consumes it). Note where it lives in Handoff.
- **Methods are thin wrappers.** Each method delegates to Task 212's endpoint/state behind the handle's `Arc<...>` (or whatever 212 exposed). Keep logic in 212's modules; `handle.rs` is glue. If 212 didn't expose a needed hook, add the minimal accessor on 212's side **without** re-architecting it, and note the touch.
- **`start`/`stop` reconcile with 212's lifecycle.** If 212 starts the endpoint eagerly, make `start`/`stop` explicit idempotent controls (double-`start` is a clean no-op or error — pick + test). Don't duplicate endpoint construction.
- **`api.rs` discipline.** Declarations in `crates/transport/src/api.rs` (depth 3 → indexed by `regen-interfaces.sh` into `rust-api.md`), impls in siblings. This is the **public-surface file the brief calls out** as where regen indexes the transport surface. Commit the regen diff (this task *adds* the transport public surface to `rust-api.md`).
- **Cross-platform.** No `std::os::unix`-only types in the handle or its signatures (`DeviceId`/`[u8;32]`/`Url`/`NatStats` are portable); the transport crate builds on the Linux + Windows CI lanes (Task 113).

## Verification
**Tier 1** — internal Rust API surface; the methods wrap Task 212's endpoint and are unit-testable in-process (no real NAT, no second machine). The tests prove each method's contract against the in-process transport; the **real** cross-device behaviors (a stolen device severed over the wire, a real pairing handshake, a real push wakeup) are downstream tasks' Tier-2 (209/207) and the **Phase-2 Tier-3 checklist**.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-transport` (handle) → the nine-method contract tests (lifecycle, pairing open/close, relay switch reflected in `current_relay`, `nat_stats` read, `close_sessions_for_device` removes the targeted sessions, `send_wakeup_hint` routes/errors cleanly) pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new external deps expected beyond 212's; confirm).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`rust-api.md` gains the `crates/transport/src/api.rs` `TransportHandle` surface).
7. `scripts/smoke.sh` → unchanged (the smoke Core is co-located/UDS; the Iroh `TransportHandle` is the remote path). Exits 0.

## Definition of Done
- [ ] `crates/transport/src/api.rs` declares `TransportHandle` + the nine §5.1 methods with exact signatures (decls in `api.rs`, impls in a sibling)
- [ ] `TransportConfig` / `RelayInfo` / `WakeupPayload` (minimal) declared; `NatStats` / `PairingListener` / `DeviceId` referenced from Task 212 (not duplicated)
- [ ] Each method wraps Task 212's endpoint/state; downstream consumer named in code/Handoff for each seam
- [ ] `DeviceId` ↔ `[u8;32]` reconciled so Task 209's `SessionCloser` is satisfied without changing 209's frozen trait
- [ ] `WakeupPayload` minimal + doc-noted as P5/`design/14`-fleshed; no speculative notification fields
- [ ] Tier-1 contract tests for all nine methods pass
- [ ] Verification commands pass; smoke unchanged (exits 0); `rust-api.md` regenerated + committed
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate ones in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/transport/src/api.rs` (new/modified — the `TransportHandle` public surface declarations)
- `crates/transport/src/handle.rs` *(or sibling — match Task 212's layout)* (new/modified — method impls wrapping 212's endpoint) + `crates/transport/src/lib.rs` (modified — module wiring)
- `crates/transport/tests/transport_handle.rs` (new — the nine-method Tier-1 contract tests)
- `docs/interfaces/rust-api.md` (regenerated — gains the transport public surface)

## Commit message
```
phase-2: TransportHandle public API (crates/transport)

Locks the design/11 §5.1 TransportHandle surface over Task 212's Iroh
endpoint: start/stop, listen_pairing/close_pairing, current/switch_relay,
nat_stats, close_sessions_for_device, send_wakeup_hint. Declared in
src/api.rs (regen -> rust-api.md), impls wrap 212's machinery. Each
method is a named seam: api_server serves over the sessions, 209's
SessionCloser is satisfied by close_sessions_for_device, 207 drives
listen_pairing, P5 notifications drive send_wakeup_hint. WakeupPayload
defined minimally (fleshed in P5/design/14). Tier-1 unit-tested.

Refs: tasks/v1.0/217-transport-handle-api.md
```

## Handoff Notes (fill in when finishing)
- `DeviceId` vs `[u8;32]` reconciliation chosen (how 209's `SessionCloser` is satisfied) / `PairingListener` ownership (212 vs 207 vs here) / `WakeupPayload` minimal shape + P5 fields deferred / `start`/`stop` reconciliation with 212's lifecycle / any minimal accessor added on 212's side / each method's named downstream consumer / regen-interfaces diff (transport surface added to rust-api.md) / Open questions / Deliberate debt / Smoke-gate state (unchanged)
```
