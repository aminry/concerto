# Task 204 — Connect-Web Bridge: Core Loopback `hyper` Server (gRPC-Web + SSE), Path A Only

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 201 |
| Touches subsystem(s) | 10 (Client API Protocol), 11 (Remote Transport — Path A) |
| Smoke gate | new:connect-web-bridge |

## Goal
Give a browser a way to reach the Core's gRPC services without speaking Iroh or raw HTTP/2-gRPC: a tiny **`hyper` server bound to loopback/LAN** that serves **gRPC-Web** (with SSE fallback for server-streaming) over the **same Tonic services** the UDS server already hosts. Today the Core listens only on UDS (`crates/core/src/api_server.rs`); there is no HTTP front door for a Connect-Web SPA. This task builds `design/11 §3.4` **Path A** (LAN-reachable loopback HTTP) — and **only** Path A; the Path B WSS-via-relay path is explicitly Task 215. The bridge tags every connection it accepts as `TransportKind::WSS_BRIDGE` **through Task 201's `ConnTransport` request-extension seam** — it does not edit `RuntimeHandler` (201 froze that the handler only *reads* the tag, listeners *write* it). After this task a headless Connect-Web client can hit `http://127.0.0.1:<port>` and drive the same RPCs/streams the Desktop drives over UDS, and `GetServerCapabilities` reports `transport_kind = WSS_BRIDGE` so the SPA suppresses local-only affordances (`15 §3.11`). This is the loopback data path the Web client (Phase 5, Task 519/520) builds on.

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §3.4 **Path A** (Core spawns a tiny `hyper` server bound to loopback/LAN interfaces; Connect-Web requests go directly to it; it runs the **same Tonic services** via Connect-Web's HTTP transport). **Path B (WSS-via-relay) is Task 215 — explicitly out of scope here.** §3.9 (LAN-direct is the strongest-trust path; `disable_remote` eliminates relay involvement — informs what Path A means for trust).
- `design/10_Local_API_Protocol.md` §2 (the V1.0 Connect-Web bridge translates browser HTTP+SSE into the same gRPC services), §3.4 (the two auth paths land in the **same** handlers; `TRANSPORT_KIND_WSS_BRIDGE` is "browser"; this bridge tags its connections via the Task-201 `ConnTransport` seam), §12 R-2 (**server-streaming + unary `AckOffset` by default**; bidi only where Connect supports it natively over HTTP/2 — the SPA uses `Streams.Subscribe` server-stream + the unary `AckOffset` from Task 202), §4.2 (`ServerCapabilities.transport_kind`).
- `crates/core/src/api_server.rs` — the **live** UDS server: `run_uds(...)` builds `tonic::transport::Server::builder().add_service(...)` for each handler and serves over `UnixListenerStream`. **You reuse the exact same handler construction** for the hyper server — same `RuntimeServer`/`WorkspacesServer`/`StreamsServer`/… instances, a different front door. Study how the `#[cfg(unix)]` gating and the `ApiServerActor` handle-passing work; the new bridge is a sibling actor (or a second serve path) built from the same handles.
- `tasks/v1.0/201-capability-negotiation.md` (the **dependency**) → read the whole file, especially `## Public interface this task locks` and `## Scope — out`: 201 defines `ConnTransport(pub TransportKind)` as a request-extension carrier that **each listener tags** (UDS now, Iroh in 212, **WSS bridge in 204** — that's this task), and the contract that the handler never branches on transport, it only reads the tag. This task is the WSS-bridge tag site: inject `ConnTransport(TransportKind::WssBridge)` into every request the hyper server accepts. Read its Handoff Notes for the exact carrier name/location as merged.
- `crates/core/src/handlers/runtime.rs` + `crates/proto/proto/concerto/v1/runtime.proto` — `ServerCapabilities.transport_kind` + the `TRANSPORT_KIND_WSS_BRIDGE = 3` enum value already exist; the handler reads the `ConnTransport` tag (after 201). You do **not** change the proto or the handler.
- root `Cargo.toml` `[workspace.dependencies]` — note what's already pinned: `tonic = "0.12"`, `prost = "0.13"`, `tower = "0.5"`, `hyper-util = "0.1"`, `tokio-stream`, `futures`. There is **no** `tonic-web` and **no** direct `hyper` pin yet — you add them (see Implementation notes for the crate decision + the deny.toml check).

## Scope — in
- A **loopback hyper server** in `crates/core` (a new module, e.g. `crates/core/src/connect_bridge.rs`, exposed as a supervised actor sibling to `ApiServerActor` or an opt-in branch of it) that:
  - Binds a `tokio::net::TcpListener` on `127.0.0.1:<port>` (loopback by default; LAN-bind is a documented config knob, default loopback-only per the trust note in `§3.9`). Port from config with a sane default; 0 ⇒ OS-assigned (report it for tests).
  - Serves the **same** Tonic service set as `run_uds`, wrapped for browser reachability: **gRPC-Web** for unary + server-streaming, with **SSE** as the server-streaming fallback where the client can't read gRPC-Web trailers (Connect-Web negotiates this).
  - **Tags every accepted connection** with `ConnTransport(TransportKind::WssBridge)` via the 201 seam (a `tower` layer / interceptor on this listener — mirror however 201's UDS listener injects its tag).
- Cross-platform: the hyper loopback server **must build on Windows** (Task 113 lane) — it's TCP, not UDS, so this is the one transport that is natively cross-platform; keep it free of `#[cfg(unix)]`.
- Wire the bridge into boot/registration alongside the UDS server (behind a config flag so a pure-co-located install can leave it off; default decision documented).
- Tests (Tier 2 — see below): a headless **Connect-Web / gRPC-Web client** against the loopback server exercises (a) a unary RPC (`GetServerCapabilities`) and asserts `transport_kind == WSS_BRIDGE`, (b) a server-streaming RPC (`Streams.Subscribe`) delivering events, (c) the unary `AckOffset` (Task 202) path. The client runs in-process/headless (no real browser).

## Scope — out
- **Path B — WSS-via-relay** (`§3.4` Path B): the relay bridges WSS↔Iroh with browser-side Noise IK. That is **Task 215** entirely. This task is loopback-only.
- The `apps/web` SPA, the TS `DataClient`, the Connect-Web TS client, ephemeral pairing — Phase 5 (Tasks 519–522). This task ships the **server** the SPA later calls.
- A real browser via Playwright — that is the Phase-5 web client's Tier-2 surface (Task 519/520); this task's double is a headless gRPC-Web client only (state plainly below).
- Auth/peer-uid/device-cert gating on the bridge — Task 210 (auth middleware) and the browser ephemeral-pairing (Task 522) own that. This task tags the transport kind but does not gate.
- TLS / cert-pinning on the loopback socket — LAN-direct TLS pinned to Core identity is Task 521; loopback here is plain HTTP (it never leaves the host).
- bidi streaming over Connect — `R-2`: server-streaming + unary `AckOffset` only.

## Public interface this task locks
- The bridge **tags `transport_kind = WSS_BRIDGE`** for all connections it accepts (consuming, not redefining, 201's `ConnTransport` seam) — FROZEN behavior: a browser-reached connection reports `WSS_BRIDGE`.
- The loopback bind contract: **loopback-only by default**; LAN-bind is opt-in config; the served surface is byte-identical to the UDS surface (same services, same proto) — no Connect-Web-specific RPCs.
- No proto change. No new handler. The seam is internal `crates/core` wiring (likely **no** `docs/interfaces/` diff — confirm and note, cf. Task 112's regen note).

## Implementation notes
- **Crate choice — decide and freeze.** The two viable routes: (a) **`tonic-web`** (`tonic_web::enable(service)` / `GrpcWebLayer`) layered onto a `tonic::transport::Server` for gRPC-Web framing — the smallest delta from `run_uds`, since it reuses the Tonic server builder and just adds a layer + a TCP incoming; (b) a hand-rolled `hyper` 1.x service with `hyper-util` (already pinned) routing into the Tonic services. Prefer **`tonic-web` on top of the existing Tonic server builder over a `TcpListenerStream`** — it gives gRPC-Web (incl. server-streaming) with minimal new surface and stays cross-platform; the design says "tiny hyper server" but `tonic-web`'s server *is* a hyper server underneath, so this satisfies `§3.4` Path A. If `tonic-web`'s SSE story is insufficient for the headless client, fall back to the hand-rolled hyper route and document why. Pin the chosen crate(s) in `[workspace.dependencies]` (`tonic-web = "0.12"` tracks the `tonic = "0.12"` pin), run **`cargo deny check`**, and ratify any new SPDX in `deny.toml` in the existing dated-comment house style (flag in Handoff). A copyleft/SSPL/BSL transitive dep is a Stop-and-ask.
- **Reuse the handler construction verbatim.** Refactor `run_uds`'s service-building block so both the UDS path and the hyper path consume one `fn build_services(handles…) -> Router/Server` (or build the same handler structs twice from cloned handles). Do **not** fork the handler logic — the whole point of `§6.3` is one handler set, two front doors.
- **The 201 tag is the load-bearing detail.** A `tower` layer on the hyper/TCP path inserts `ConnTransport(TransportKind::WssBridge)` into `request.extensions_mut()` before dispatch, exactly as 201's UDS listener inserts `Uds`. The `RuntimeHandler` (post-201) reads it and reports `WSS_BRIDGE`. Verify against 201's merged carrier name.
- **Loopback default for trust.** Bind `127.0.0.1` by default (`§3.9`: LAN-direct is high-trust but loopback is the conservative default; the SPA in Phase 5 is served same-host first). Make the bind address a config field; document that LAN-bind widens exposure and is gated by managed settings later (Task 211).
- **Cross-platform**: TCP + hyper + tonic-web are all cross-platform; this module must compile on the Windows lane with **no** `#[cfg(unix)]`. (It is, in fact, the transport that works on Windows where UDS needs named-pipe glue.)

## Verification
Tier 2. **Test double:** an **in-process headless Connect-Web / gRPC-Web client** (e.g. a `tonic-web`-compatible client, or a minimal HTTP gRPC-Web request harness) connected to the loopback hyper server on an OS-assigned port. It proves the SPA *data path*: unary RPC, server-streaming, and the unary `AckOffset` all work over gRPC-Web, and that `transport_kind == WSS_BRIDGE` flows through the 201 seam.
**What the double does NOT cover (→ Phase-2 manual checklist lines):** (1) a **real browser** driving the bridge via Playwright against the actual `apps/web` SPA (that is Task 519/520's Tier-2 surface); (2) the **remote WSS-via-relay path (Path B / Task 215)** with browser-side Noise IK and the relay seeing ciphertext only. Add both as Phase-2 Tier-3/manual lines.

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core connect_bridge` → the headless gRPC-Web client tests (unary + server-stream + `transport_kind == WSS_BRIDGE` + `AckOffset`) pass.
4. `cargo test --workspace --no-fail-fast` → all pass (incl. the Windows-build-relevant compile; the lane runs in CI per Task 113).
5. `cargo deny check` → green (new `tonic-web`/`hyper` SPDX cleared + ratified in `deny.toml` if needed).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → clean (no proto/SQL/api.rs surface change expected — confirm and note in Handoff).
7. `scripts/smoke.sh` → add a `connect-web-bridge` capability (`scripts/smoke.d/<NN>-connect-web-bridge.sh` defining `check_connect_web_bridge`, appended to `scripts/smoke.manifest`): start a Core with the bridge enabled, `curl`/headless-call `GetServerCapabilities` over loopback gRPC-Web, assert `transport_kind = WSS_BRIDGE`. Exits 0.

## Definition of Done
- [x] Loopback `hyper`/`tonic-web` server serves the same Tonic service set over gRPC-Web (+ SSE fallback for server-streaming), bound to `127.0.0.1` by default
- [x] Every accepted connection tagged `ConnTransport(TransportKind::WssBridge)` via the 201 seam; `GetServerCapabilities` reports `WSS_BRIDGE`
- [x] Handler construction shared with `run_uds` (one service-build path, two front doors)
- [x] Builds on the Windows CI lane (no `#[cfg(unix)]` in the bridge)
- [x] Crate choice (`tonic-web` vs hand-rolled hyper) decided + frozen; deps pinned; `cargo deny check` green + ratified
- [x] Tier-2 headless gRPC-Web client tests pass; what the double does NOT cover recorded as Phase-2 checklist lines (real browser/Playwright; Path B WSS-relay = Task 215)
- [x] Verification commands pass; new `connect-web-bridge` smoke green; interfaces clean (or noted)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/connect_bridge.rs` (new — the loopback hyper/tonic-web server + the WSS_BRIDGE tag layer)
- `crates/core/src/api_server.rs` (modified — extract shared `build_services`; wire the bridge actor/branch)
- `crates/core/src/lib.rs` *(or the module tree root — `mod connect_bridge;`)*
- `crates/core/Cargo.toml` + root `Cargo.toml` (modified — `tonic-web` / `hyper` pins)
- `deny.toml` (modified only if a new SPDX needs ratification)
- `crates/core/tests/connect_web_bridge.rs` (new — headless gRPC-Web client tests)
- `scripts/smoke.d/<NN>-connect-web-bridge.sh` (new) + `scripts/smoke.manifest` (modified)

## Commit message
```
phase-2: Connect-Web bridge — loopback hyper/gRPC-Web server (Path A)

Adds a loopback (127.0.0.1) hyper server serving the same Tonic
services over gRPC-Web (+ SSE fallback), tagging every connection
WSS_BRIDGE through the Task-201 ConnTransport seam. Path A (LAN/loopback)
only; the WSS-via-relay Path B is Task 215. Headless gRPC-Web client
proves the SPA data path; real-browser/Playwright + relay are manual.

Refs: tasks/v1.0/204-connect-web-bridge.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** Three deliberate choices, none functional drift. (1) **Crate choice = `tonic-web` 0.12.3** (NOT hand-rolled hyper), layered as `Server::builder().accept_http1(true).layer(interceptor(tag_wss_bridge)).layer(GrpcWebLayer::new()).add_service(...)` over a `TcpListenerStream` — `tonic-web`'s server is a hyper server underneath, so this satisfies `§3.4` Path A's "tiny hyper server" with the smallest delta from `run_uds`. **No direct `hyper` pin added** (tonic re-exports what's needed). (2) **Config is read from process env inside the bridge module** (`CONCERTO_CONNECT_BRIDGE` to enable — default OFF; `CONCERTO_CONNECT_BRIDGE_ADDR` for bind, default `127.0.0.1:0`), and the bridge is wired as an **opt-in second serve-loop inside `ApiServerActor::run`** (the task's permitted "opt-in branch of it"), running concurrently with `run_uds` under the shared shutdown token via `tokio::join!`. This keeps all wiring inside the Outputs (`api_server.rs` + `connect_bridge.rs`) and avoids touching `boot.rs`/`runtime.rs` (NOT in Outputs). (3) Shared service construction is a `BridgeServices` struct + `build_and_serve` in `connect_bridge.rs` that mirrors `run_uds`'s service-registration order and `Some(..)` gating verbatim (same handler structs, second front door); the build + serve are fused in one async fn because the layered `Router` carries an unnameable interceptor-closure generic. **One unexpected dep needed, added to Outputs:** `reqwest` gains the `stream` dev-feature (`crates/core/Cargo.toml` `[dev-dependencies]`) so the integration test can read the `Subscribe` server-stream body chunk-by-chunk — feature unification only; production reqwest is unchanged. Flagged here per the rules.
- **Open questions for next task (Phase-2 Tier-3 / manual-checklist lines — what the headless double does NOT cover):** (1) **Real browser via Playwright** against the actual `apps/web` SPA — that is Task 519/520's Tier-2 surface; the double here is an in-process `reqwest` gRPC-Web client, not a browser (no real CORS preflight, no browser fetch/SSE EventSource semantics, no base64 `application/grpc-web-text` variant — only `application/grpc-web+proto`). (2) **Remote WSS-via-relay Path B (Task 215)** with browser-side Noise IK and the relay seeing ciphertext only — this task is loopback plain-HTTP Path A only. Also for **Task 215**: it tags `WssBridge` the same way *only if* it re-injects `ConnTransport` at the relay-bridged listener; the `WssBridge` tag is shared between Path A (this task) and Path B (215), so the SPA can't distinguish loopback-direct from relayed by `transport_kind` alone — if 215 needs that distinction it's a new field, not a re-use of this enum. **Auth gating (Task 210) and browser ephemeral pairing (Task 522) are NOT applied here** — the bridge tags transport but does not authenticate; Task 210 must add its middleware to *both* front doors. **LAN-bind exposure (Task 211 managed settings)**: `CONCERTO_CONNECT_BRIDGE_ADDR=0.0.0.0:<port>` widens exposure beyond loopback and is currently ungated — 211 should gate it via `managed.json`. **Cross-platform note:** the bridge module has no `#[cfg(unix)]` on its serve path and builds for the Windows target in principle; I could not fully cross-compile locally (the sandbox's rustup/cargo toolchain wiring rejected the added `x86_64-pc-windows-gnu` std), so the **Windows lane (Task 113 CI) is the authoritative confirmation** — the `#[cfg(not(unix))]` branch in `build_and_serve` is small and reviewed (it serves Runtime + Files/Projects/Repositories/Workspaces/Workareas/Skills/Vcs; Sessions/Streams/Schedules/Suggestions arrive on Windows when the supervisor ports do, same gating as `run_uds`).
- **Deliberate debt:** — None. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code.
- **License ratifications:** — None needed. `tonic-web` 0.12.3 is MIT; its transitive additions (`tower-http` 0.5.2, `base64`, `http-body-util`, `pin-project`) are all MIT/Apache-2.0, already in `deny.toml`'s allow-list. `cargo deny check` is green (`advisories ok, bans ok, licenses ok, sources ok`) — **`deny.toml` unchanged**.
- **Smoke-gate state:** **new capability `connect-web-bridge` added and green.** `scripts/smoke.d/97-connect-web-bridge.sh` (defines `check_connect_web_bridge`) + appended to `scripts/smoke.manifest`. It boots a **dedicated** Core with the bridge enabled under a separate config dir (so it never contends for the shared smoke Core's single-instance lock), reserves a loopback port, and drives a headless `curl` gRPC-Web `GetServerCapabilities`, asserting the wire bytes `28 03` (`transport_kind` field-5 varint = `WSS_BRIDGE` value 3), the `concerto.v1` schema marker, and the `grpc-status:0` success trailer. Full `scripts/smoke.sh` passes end-to-end (28s, all checks incl. `PASS connect-web-bridge`). `regen-interfaces.sh` produced **no diff** (the bridge is internal `crates/core` wiring — no proto/SQL/`api.rs` surface change, as the task predicted). All `rust` verification commands green: `cargo check`/`clippy -D warnings`/`test --workspace --no-fail-fast` (70 test-result blocks ok, 0 failed)/`deny check`/`fmt --all --check`.
