# Task 212 — `crates/transport`: Iroh Endpoint in Core + Hand-Rolled Tonic-0.12 Adapter + 3 Logical Channels

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) — **acknowledge this is the heaviest task of the phase**; budget the high end |
| Depends on | 102, 208 |
| Touches subsystem(s) | 11 (Remote Transport & Relay), 10 (Client API Protocol — `serve_iroh` feeds the same handlers) |
| Smoke gate | new:transport-loopback |

## Goal
Fill the empty `crates/transport` with the **production** Iroh transport for the Core: one long-lived Iroh QUIC endpoint (hole-punch with relay fallback), the **hand-rolled tonic-0.12 ↔ Iroh-bidi-stream duplex adapter** spike 102 proved (NEVER `tonic-iroh-transport`, which forces tonic 0.14), and the three logical channels (API / push-hint / pairing) multiplexed over that one endpoint. After this task the SAME Tonic server that serves UDS today (`crates/core/src/api_server.rs`) also accepts Iroh callers via `serve_iroh`, with the Noise IK layer from Task 208 layered inside each stream and each connection tagged `ConnTransport(TransportKind::Iroh)` through the Task 201 seam — so the 210 auth path and 201 capability negotiation see `IROH` with no per-transport handler branching. This is the dependency root for every remote feature in Phases 2/5 (mDNS 213, relay 214, WSS 215, migration 216, the `TransportHandle` API 217, and the mobile/web clients).

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §3.1 (one long-lived Iroh **endpoint** per Core; endpoint key generated + stored by Iroh in its own state dir, **separate** from the Core Ed25519 identity per `12 §3.1`; Core registers its endpoint with the configured relay; clients connect by endpoint id → direct hole-punch, then relay fallback; the same Tonic server handles UDS + Iroh callers identically), §3.3 (the **three logical channels** — API / push-hint / pairing — all Iroh QUIC streams over one endpoint, different stream-multiplexing IDs / Noise IK sessions), §4 (reproduce `TransportState` / `ActiveSession` / `ConnectionPath` — `Direct | Relayed | Lan`), §6.1 (the Core-side flow `Iroh → adapter → Noise(208) → Tonic`), §8 (failure modes: relay-unreachable backoff, hole-punch→relay fallback, endpoint-key mismatch → re-pair).
- `design/10_Local_API_Protocol.md` §6.3 (`serve_iroh` uses the same builder + handlers as `serve_uds`; auth middleware distinguishes UDS peer-uid vs Iroh device-cert metadata). **Note:** §6.3 names `tonic-iroh-transport::IrohListener` — this is the reference Task 200 amends to the hand-rolled adapter. Treat the **hand-rolled adapter as canonical** (see Task 200 + the spike); the design wording is corrected by 200, not by you.
- `design/12_Security_Identity.md` §3.4 (Noise IK handshake **inside** the QUIC stream; AES-256-GCM session keys; rekey 1 GB / 1 h; `NoiseSession` comes from Task 208).
- `design/spikes/tonic-iroh-findings.md` §2 + §2.1–§2.4 + §7 — the load-bearing input. **REUSE the `IrohDuplex` / `IrohConnector` pattern** from `spikes/tonic-iroh/src/iroh_adapter.rs` and apply the four gotchas: (1) fully-qualified `AsyncRead` / `AsyncWrite` `poll_*` trait syntax (inherent-vs-trait shadowing on `iroh::endpoint::{Send,Recv}Stream`); (2) **one gRPC connection == one Iroh bidi stream**, many bidi streams per Iroh `Connection`; (3) **acceptor priming** — the connector sends a zero-byte `flush()` so the server's `accept_bi()` wakes promptly; (4) lift Tonic's 4 MiB decode/encode ceiling explicitly (the spike used 64 MiB).
- `design/spikes/iroh-nat-findings.md` §5 Note A (the relay is **load-bearing** — a meaningful fraction of real clients land on relay fallback; provision for it) and Note B (DNS/pkarr discovery can fail independently of NAT traversal → the transport **must allow a Core address / relay to be supplied directly** when DNS discovery is blocked). Confirms the pin: `iroh = 0.98.2`.
- `crates/core/src/api_server.rs` — the existing UDS `ApiServer` actor + `run_uds`: the `Server::builder().add_service(...).serve_with_incoming_shutdown(...)` shape you mirror for `serve_iroh`, the `#[cfg(unix)]` gating pattern, the shutdown-via-`ctx.shutdown.cancelled()` wiring, and the full list of `add_service(...)` registrations Iroh must reuse verbatim.
- `crates/transport/Cargo.toml` + `crates/transport/src/lib.rs` — the empty 3-line stub you fill (already a `[workspace.members]` entry; lib `concerto_transport`).
- `spikes/tonic-iroh/Cargo.toml` — the exact production-coherent pin set to lift: `tonic = "=0.12.3"`, `prost = "=0.13.5"`, `iroh = "=0.98.2"`, `iroh-relay = "=0.98.0"` (relay needed here only transitively / for the loopback test double).
- `deny.toml` — the `[licenses] allow` list + the dated **operator-ratification comment** house style; iroh/iroh-base/iroh-relay pull a large tree — run `cargo deny check` and ratify any new SPDX (see Implementation notes).
- `tasks/v1.0/201-capability-negotiation.md` → "Handoff Notes" — the `ConnTransport(TransportKind)` request-extension seam; the Iroh listener tags `ConnTransport(TransportKind::Iroh)`.
- `tasks/v1.0/208-noise-ik-session.md` → "Handoff Notes" — the `NoiseSession` / handshake surface this task layers inside each Iroh stream (init responder side on accept, initiator side on connect).
- `tasks/v1.0/211-managed-settings-enforcement.md` → "Handoff Notes" — the `disable_remote` enforcement predicate (`crates/core/src/security/managed.rs`); read it **before** registering with a relay or accepting a remote connection.
- `tasks/v1.0/200-adapter-reconciliation.md` → "Handoff Notes" — the canonical hand-rolled-adapter decision + the four-gotcha notes lifted into `design/11`.

## Scope — in
- **Fill `crates/transport`** (lib `concerto_transport`): add the pinned deps (`iroh = "=0.98.2"`, the tonic/prost workspace pins, tokio io-util, plus `iroh-relay = "=0.98.0"` behind a `dev`/`test` feature for the loopback double only). The crate owns the Iroh endpoint, the adapter, the channel model, and `TransportState` / `ActiveSession` / `ConnectionPath` (`design/11 §4`).
- **Endpoint lifecycle:** build one long-lived `iroh::Endpoint` (Iroh generates + persists its own endpoint key in its state dir — do NOT reuse the Core Ed25519 identity). Register the endpoint with the configured relay; on relay-unreachable, exponential-backoff retry while the LAN path stays usable (`design/11 §8`). Honor a **directly-supplied Core address / relay** so a blocked-DNS client (spike Note B) can still connect.
- **The hand-rolled adapter** (`src/adapter.rs` or similar): productionize `IrohDuplex` (`SendStream` + `RecvStream` → one `AsyncRead + AsyncWrite` duplex implementing `tonic::transport::server::Connected`) + `IrohConnector` (per-call `connect_with_connector` opening a fresh bidi stream with acceptor-priming `flush()`). Server side feeds a `Stream<Item = Result<IrohDuplex, _>>` of accepted bidi streams into `serve_with_incoming`. Apply all four gotchas; set explicit ≥64 MiB gRPC message limits on both ends.
- **`serve_iroh`:** add an Iroh listener path that constructs the **same** Tonic `Server::builder()` with the **same** `add_service(...)` set as `run_uds` and serves it over the adapter's incoming stream. Wire it into the `ApiServer` actor (or a sibling transport task the actor owns) so it shuts down on `ctx.shutdown.cancelled()` exactly like the UDS path. Do not duplicate handler logic — both transports dispatch into the identical handler set.
- **Noise IK inside the stream** (`design/12 §3.4`, Task 208): on each accepted API/push-hint stream, run the Noise IK responder handshake (Core static key); on connect, run the initiator. Subsequent gRPC bytes flow through the established `NoiseSession`. One Noise session per Iroh connection; rekey is 208's concern but the transport must surface a rekey/replay-failure → drop-connection path (`design/12 §3.4`).
- **Three logical channels** (`design/11 §3.3`): API (long-lived QUIC stream pool, the gRPC traffic), push-hint (lightweight, opt-in; the wakeup-fetch channel 217's `send_wakeup_hint` + 14 use), pairing (short-lived, gated by the pairing token from `12 §3.3` / Task 207 — surface a `listen_pairing(token_hash)` hook that 217 / 207 drive). Each channel is an Iroh QUIC stream over the one endpoint, distinguished by an initial channel tag the acceptor reads before handing the stream to Tonic / pairing / push-hint handling.
- **Tag `ConnTransport(TransportKind::Iroh)`** (Task 201 seam): the Iroh listener injects the extension on every request so the handler reports `IROH` and 210 auth / 201 caps branch correctly — never edit the handler.
- **`disable_remote` gate** (Task 211): before registering with a relay or accepting any remote (non-LAN) connection, consult the `managed.rs` predicate. When `disable_remote = true`, do not register with a relay and accept LAN-discovered connections only (`design/11 §6.4`); mDNS publication (213) is unaffected by this flag.
- **`ConnectionPath` classification** (`design/11 §4`): read Iroh's own per-path signal (the spike's `selected().is_ip()` → `Direct`/`Lan`, `is_relay()` → `Relayed`) to populate `ActiveSession::path`; this feeds 216's NAT telemetry. Distinguish `Lan` (LAN-direct, mDNS) from `Direct` (hole-punched) where Iroh's signal allows; otherwise document the heuristic.
- **Tests** (the Tier-2 double, see Verification): two Iroh endpoints on one host with relays disabled, forced onto the direct loopback path; a full gRPC round-trip (e.g. `GetServerCapabilities`) + a streaming subject over the adapter + Noise; assert the handler reports `transport_kind = IROH`; assert `disable_remote = true` refuses relay registration; assert the four-gotcha behaviors (large-message round-trip > 4 MiB; first-RPC-no-stall via acceptor priming).

## Scope — out
- **mDNS** responder/browser (Task 213 — this task only exposes the `Lan` `ConnectionPath` and the endpoint others advertise).
- **The relay binary** (`crates/relay`, Task 214) and the **WSS bridge** (Task 215). This task *registers with* and *falls back to* a relay but does not implement one; the loopback test double disables relays entirely.
- **QUIC connection migration** Wi-Fi↔LTE + the NAT-success telemetry aggregation (Task 216 — this task records the per-session `path`; 216 aggregates `NatStats` and emits `transport.nat_success_changed`).
- **The `TransportHandle` public API surface** (`start`/`stop`/`listen_pairing`/`current_relay`/`switch_relay`/`nat_stats`/`send_wakeup_hint`/`close_sessions_for_device` — Task 217). This task builds the internals 217 wraps; expose them as `pub` Rust on `crates/transport` (see the FROZEN surface below) but do not build the 217 façade or its events.
- **Pairing handshake logic** (Noise XX over the token — Task 207) and **device-cert auth** (Task 210). This task provides the *pairing channel* and *tags Iroh*; 207/210 own what flows through them.
- **Proto changes.** There is NO `transport.proto` (decision D1): `TransportHandle` is internal Rust; transport events become a `TransportEvent` arm in `streams.proto`'s `Event` oneof (added by 217, not here); `nat_stats` surfaces via Runtime/Devices. Do **not** add proto here.
- A real relay, real NAT traversal, or real connection migration (all Tier-3 — the spike 101 field matrix).

## Public interface this task locks
- **`crates/transport` public surface consumed by Task 217** — FROZEN. The endpoint/session types and entry points 217 wraps: an opaque transport handle/struct exposing (at minimum) `start(cfg)` / `stop()`, an Iroh-`serve` entry the Core actor drives, `listen_pairing(token_hash: [u8;32])`, relay query/switch, per-session `ConnectionPath`, a `close_sessions_for_device(DeviceId)` hook, and a `send_wakeup_hint`-able push-hint channel. Name them so 217's `TransportHandle` is a thin façade; freeze the names + signatures in `src/api.rs` (keychain/identity convention so `regen-interfaces.sh` indexes them).
- **`TransportState` / `ActiveSession` / `ConnectionPath`** field layout per `design/11 §4` (`ConnectionPath = Direct | Relayed | Lan`) — FROZEN as the in-memory model 216/217 read.
- **The adapter contract** — `IrohDuplex` (one Iroh bidi stream ⇒ one `AsyncRead + AsyncWrite + Connected`) and `IrohConnector` (one gRPC connection ⇒ one fresh primed bidi stream), with the ≥64 MiB message-limit setting and the channel-tag framing that distinguishes API / push-hint / pairing streams. FROZEN — 213/214/215/216/218 build against this shape.
- **The pin trio** `iroh 0.98.2` / `iroh-relay 0.98.0` / `tonic 0.12.3` + `prost 0.13.5` — the validated stack; do not bump within V1.0 without a re-spike.

## Implementation notes
- **Lift the spike adapter verbatim, then harden.** `spikes/tonic-iroh/src/iroh_adapter.rs` is ~141 lines and already correct on the four gotchas; the production deltas are: real error types (not `anyhow`), connection-pool bookkeeping into `ActiveSession`, the channel-tag handshake, the Noise layer, and `Connected` connection-info that carries the `TransportKind::Iroh` tag. Keep the fully-qualified `AsyncWrite::poll_write(Pin::new(&mut s), ..)` syntax — a bare call silently binds the inherent `poll_write` (wrong error type) and won't compile against tonic.
- **Layering order (`design/11 §6.1`):** `Iroh QUIC stream → channel-tag read → Noise IK (208) → tonic adapter → shared dispatch`. Noise wraps the byte duplex *before* it reaches Tonic, so `IrohDuplex` either wraps the post-Noise transport or composes with the `NoiseSession`'s read/write halves — decide in-task and document; the spike's stub has Noise OUT, so this is net-new integration the spike explicitly deferred (§3).
- **Endpoint key vs Core identity.** Iroh owns its endpoint keypair and persists it in its own state directory; the Core's Ed25519 identity (Task 206) is separate and travels in the device cert inside Noise/gRPC metadata. Do not cross them. On endpoint-key mismatch after a reinstall, the correct behavior is force-re-pair (`design/11 §8`).
- **`serve_iroh` reuses `run_uds`'s service list.** Factor the `Server::builder().add_service(...)` chain so both `run_uds` and `serve_iroh` register the identical services (extract a helper if it reduces drift). The only per-transport difference is the incoming stream + the injected `ConnTransport` tag; handlers are untouched.
- **Cross-platform.** Iroh + the adapter are portable; the UDS path stays `#[cfg(unix)]`. Do not introduce `std::os::unix`-only types into the transport's public signatures so the Windows CI lane (Task 113) stays green. Iroh's endpoint runs on Windows/Linux/macOS.
- **License clearance is part of this task.** iroh / iroh-base / iroh-relay pull a substantial tree (quinn/rustls/ring/webpki, possibly new SPDX). Run `cargo deny check`; ratify any new license in `deny.toml`'s allow-list with a **dated operator-ratification comment** in the house style and flag it in Handoff. A copyleft / SSPL / BSL transitive dep is a **Stop-and-ask**, not a silent allow.
- **Pin exactly** in `[workspace.dependencies]` with a rationale comment mirroring the tonic/sqlx pins (`iroh = "=0.98.2"` etc.), citing the spike verdicts as the validation source.

## Verification
Tier 2.

The Tier-2 test double is **two Iroh endpoints on one host with relays disabled, forced onto the direct loopback IP path** (the spike's Tier-2 model — `tonic-iroh-findings.md §1`). It proves the full **gRPC-over-Iroh + adapter + Noise IK + `serve_iroh` shared-dispatch + `ConnTransport(Iroh)` tagging + channel multiplexing** path end to end in CI with no network. It does **NOT** cover real-NAT hole-punch traversal, a real relay over a real WAN, or real connection migration — those are **Tier-3**, on the Phase-2 manual checklist and the spike 101 field matrix (real-NAT direct-%, real LTE↔Wi-Fi, relay-on-real-infra).

Per README §5.3 (`rust`):
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-transport` → adapter (four gotchas), loopback round-trip + streaming, Noise-inside-stream, `transport_kind = IROH`, `disable_remote` refusal, and `ConnectionPath` classification tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → advisories/bans/licenses/sources green (iroh tree cleared; `deny.toml` updated + ratified if needed).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`rust-api.md` gains the `concerto-transport` `src/api.rs` surface).
7. `scripts/smoke.sh` → the new `transport-loopback` capability brings up two in-process Iroh endpoints (relays disabled) and runs one RPC + one stream + asserts `IROH`; existing caps stay green. Exits 0.

## Definition of Done
- [ ] `crates/transport` filled: pinned Iroh endpoint, hand-rolled tonic-0.12 adapter (four gotchas, ≥64 MiB limits), three logical channels, `TransportState`/`ActiveSession`/`ConnectionPath`
- [ ] `serve_iroh` wired into the Core api server reusing the UDS service set + shutdown; Noise IK (208) layered inside each stream; `ConnTransport(Iroh)` tagged via the 201 seam
- [ ] `disable_remote` (211) consulted before relay registration / remote accept; directly-supplied address/relay path honored (spike Note B); LAN path usable when relay is down
- [ ] FROZEN: the `crates/transport` surface 217 wraps + the adapter contract, declared in `src/api.rs`
- [ ] `cargo deny check` green; any new SPDX ratified in `deny.toml` with a dated comment
- [ ] Tier-2 loopback double tests pass; Verification commands pass; interfaces regenerated; smoke `transport-loopback` green
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate debt in Handoff)
- [ ] Single commit with the message below

## Outputs
- `Cargo.toml` (modified — `[workspace.dependencies]` += `iroh = "=0.98.2"` / `iroh-relay = "=0.98.0"` pins with rationale)
- `crates/transport/Cargo.toml` (modified — deps + a `dev`/`test` feature for the in-process relay double)
- `crates/transport/src/lib.rs`, `crates/transport/src/endpoint.rs`, `crates/transport/src/adapter.rs`, `crates/transport/src/channels.rs`, `crates/transport/src/state.rs`, `crates/transport/src/api.rs` (new — names indicative; freeze the public surface in `api.rs`)
- `crates/transport/tests/loopback.rs` (new — the two-endpoints-relays-disabled double)
- `crates/core/src/api_server.rs` (modified — `serve_iroh` listener + `ConnTransport(Iroh)` tag, reusing the shared service set)
- `deny.toml` (modified only if a new SPDX needs ratification)
- `scripts/smoke.d/<NN>-transport-loopback.sh` + `scripts/smoke.manifest` (new capability)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-2: crates/transport — Iroh endpoint + hand-rolled tonic-0.12 adapter

Fills crates/transport with the production Iroh QUIC endpoint
(hole-punch + relay fallback), the hand-rolled tonic-0.12 / Iroh-bidi
duplex adapter (the four spike-102 gotchas), and the three logical
channels. serve_iroh feeds the same Tonic handlers as UDS, with Noise IK
(208) inside each stream and ConnTransport(Iroh) tagged via the 201 seam;
disable_remote (211) gates relay registration. Tier-2 loopback double:
two endpoints, relays disabled, forced direct path.

Refs: tasks/v1.0/212-transport-iroh-endpoint.md
```

## Handoff Notes (fill in when finishing)
- Drift from plan / Open questions for next task (217 surface boundary, Noise-vs-adapter layering) / Deliberate debt / License ratifications / Smoke-gate state
