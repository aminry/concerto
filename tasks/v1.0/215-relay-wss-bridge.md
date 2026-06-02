# Task 215 — WSS Bridge at the Relay (WSS ↔ Iroh, Ciphertext-Only)

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 214 |
| Touches subsystem(s) | 11 (Remote Transport & Relay) |
| Smoke gate | unchanged |

## Goal
Add the **WSS bridge** to the `concerto-relay` binary: a WebSocket-Secure listener that, per browser connection, opens an Iroh bidi stream to the addressed Core endpoint and **pumps bytes both ways** — WSS frame payloads → Iroh stream, Iroh stream bytes → WSS frames. This is the only non-Iroh path the relay runs (`design/11 §3.4 Path B`): it exists because browsers cannot speak Iroh natively in V1.0. The load-bearing invariant is that the **browser establishes Noise IK *inside* the WSS stream using its device cert** (`12 §3.4`) and the **relay sees CIPHERTEXT ONLY** — it forwards opaque bytes and can decrypt nothing (`11 §3.9`: the relay observes source IP, byte counts, the addressed endpoint ID, and nothing else). Task 214 produced the relay binary embedding `iroh-relay 0.98.0` with env-var config including a reserved `WSS_LISTEN_ADDR`; this task **binds it** and stands up the bridge listener. After this task the relay can carry a browser's encrypted gRPC-over-WSS to a Core and back over Iroh, with a byte-pump that is structurally incapable of reading plaintext. The web *client* that drives this is Phase 5 (Tasks 519–522) — **215 is the relay-side bridge ONLY**.

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §3.4 **Path B** — the exact bridge contract: web client points at `https://relay.concerto.app/wss/<endpoint_id>`; the relay opens an Iroh stream to the Core's endpoint and bridges WSS frames ↔ Iroh stream frames; the browser does Noise IK inside the WSS stream with its device cert; **the relay sees ciphertext only**. This is the FROZEN behavior you implement.
- `design/11_Remote_Transport_Relay.md` §6.2 — the relay-side architecture mermaid: the `Wss` node (`WSS Bridge (Connect-Web ↔ Iroh)`) sits beside `IrohRelay`; `Web --wss--> Wss --Iroh stream--> IrohRelay`. Your listener IS that node.
- `design/11_Remote_Transport_Relay.md` §7.3 — the web-client-via-WSS-bridge sequence (`WSS upgrade with device-cert metadata` → relay opens Iroh stream → gRPC over WSS forwarded frame-for-frame → Core validates the device cert, **not the relay**). This is the order your bridge realizes; note the relay never inspects the cert (it rides in encrypted gRPC metadata, §3.9).
- `design/11_Remote_Transport_Relay.md` §3.9 — relay observability / the trust table: what a relay operator can observe (source IP, endpoint ID, ciphertext byte counts, timestamps) vs. **cannot** (plaintext payload, device-cert contents, workspace/repo/file names, pairing tokens). The ciphertext-only property test you write asserts this boundary.
- `design/11_Remote_Transport_Relay.md` §4 — `RelayState.wss_bridges: HashMap<BridgeId, WssBridge>` — the in-memory per-bridge state shape this task populates; bridges are ephemeral (one per live browser connection), torn down on disconnect.
- `crates/relay/src/` (filled by **Task 214**) + `tasks/v1.0/214-relay-binary.md` → "Handoff Notes" — the relay binary structure, the env-var config loader, the **reserved `WSS_LISTEN_ADDR`** you bind, how the embedded `iroh-relay 0.98.0` endpoint/router is constructed (you open Iroh bidi streams to Cores **through the same endpoint** the relay already owns — do not create a second Iroh endpoint), the Prometheus registry to add a bridge metric to, and the `deny.toml` posture 214 left.
- `design/spikes/tonic-iroh-findings.md` §2 — the four adapter gotchas Task 200 lifted into `design/11`; the relevant ones here: **fully-qualified `AsyncRead`/`AsyncWrite` syntax** on `iroh::endpoint::{Send,Recv}Stream` (gotcha 1), and **one logical connection == one Iroh bidi stream** (gotcha 2) — a WSS connection maps to exactly one Iroh bidi stream.
- `deny.toml` — the `[licenses] allow` list + the dated operator-ratification comment style; the WSS server crate is a NEW dep (see Implementation notes) and must clear `cargo deny check`.
- `tasks/v1.0/README.md` §5.3 (`rust` command set) + §5.1 Tier-2 (the test-double rules) + §6 row 215.

## Scope — in
- **Bind `WSS_LISTEN_ADDR`** in the relay binary: when set, the relay starts a WSS listener on that address (TLS terminated at the relay — the inner Noise IK is what protects payload confidentiality end-to-end, so relay TLS is the outer transport hop the browser's `wss://` requires). When unset, the relay runs Iroh-only exactly as Task 214 shipped it (the bridge is opt-in, additive).
- **URL/path routing**: accept WebSocket upgrades on `/wss/<endpoint_id>` (`§3.4`). Parse `<endpoint_id>` from the path into an Iroh endpoint ID; reject malformed/oversized IDs with an HTTP 4xx before any upgrade. This path scheme is FROZEN (the Phase-5 web client constructs exactly this URL).
- **Per-connection bridge**: on a successful upgrade, open **one Iroh bidi stream** to `<endpoint_id>` through the relay's existing Iroh endpoint, register a `WssBridge` entry in `RelayState.wss_bridges` keyed by a fresh `BridgeId`, and run a **bidirectional byte pump**: WSS binary-frame payloads → Iroh `SendStream`; Iroh `RecvStream` bytes → WSS binary frames. The pump is **opaque** — it copies bytes; it never parses, decodes, decrypts, or interprets frame contents.
- **Lifecycle/teardown**: either side closing (WSS close frame, Iroh stream end, error, or idle timeout) tears down the other and removes the `wss_bridges` entry. A Core that refuses the Iroh stream (e.g. `disable_remote`, unknown endpoint) closes the WSS with a clean status. No bridge state survives the connection.
- **Observability** (`§3.9`): one Prometheus counter/gauge for live bridges + bytes forwarded per direction (byte *counts* only — the data this task already legitimately sees). Structured `tracing` on bridge open/close with `endpoint_id` and `BridgeId`, **never** any frame payload.
- **Tests** (Tier 2):
  - A `tokio-tungstenite` **WSS client** connects to `/wss/<endpoint_id>` on a relay bound to a loopback addr, where `<endpoint_id>` is a **loopback "Core" Iroh endpoint** spun up in-process (two endpoints on one host, the §5.1 loopback-Iroh double). A ciphertext blob round-trips browser→relay→Core and Core→relay→browser **byte-identical** — proving the pump forwards faithfully.
  - A **ciphertext-only property/invariant test**: feed the bridge a stream of pre-encrypted (or simply opaque random) frames and assert the relay-side code path retains/derives **no plaintext** — the pump operates only on `&[u8]`, never deserializes a Noise/gRPC frame, and the only relay-observable derivations are the §3.9-permitted metadata (byte counts, endpoint ID, timestamps). Encode the invariant as a test that would fail if any code path attempted to decode the inner frame.
  - Path routing: a malformed `<endpoint_id>` is rejected pre-upgrade; an unknown endpoint yields a clean WSS close, not a hang.
  - Teardown: closing the WSS client removes the `wss_bridges` entry and ends the Iroh stream (assert the map is empty after disconnect).

## Scope — out
- **The web CLIENT** that consumes this bridge (the browser-side Connect-Web/WSS transport, Noise IK in JS, ephemeral pairing) — **Phase 5, Tasks 519–522**. 215 is the relay-side bridge only; the browser is a Tier-2/P5 concern.
- **Path A** (LAN-direct Connect-Web over the Core's own `127.0.0.1:<port>` `hyper` server) — that is **Task 204** on the *Core*, not the relay. The bridge here is Path B only.
- The **Core's** Iroh endpoint, its Noise IK acceptance, gRPC dispatch, and device-cert validation — **Tasks 208/210/212**; the Core is the bridge's downstream peer, reached through the loopback double in tests.
- Bandwidth quotas / per-endpoint abuse caps (`§3.9`: default 1 Gbps burst, 50 GB/day) — a relay-policy concern; not built here (note in Handoff if 214 left a hook).
- Iroh-in-browser / eliminating the WSS bridge — **V2.0** (`§3.4`, R-4).
- Multi-region relay selection — **V1.5** (R-6).

## Public interface this task locks
- **The WSS bridge URL/path scheme: `/wss/<endpoint_id>`** (`§3.4`) — FROZEN. The Phase-5 web client builds exactly `wss://<relay-host>/wss/<endpoint_id>`; changing the path is a wire break across the relay↔web-client boundary.
- **The framing contract**: a WSS **binary** message payload maps 1:1 onto bytes written to the Iroh bidi stream, and vice versa — the bridge is a transparent opaque byte pump, **one WSS connection == one Iroh bidi stream** (spike-102 gotcha 2). No relay-imposed envelope, length-prefix, or re-framing. FROZEN.
- **The ciphertext-only invariant**: the relay's bridge code reads, derives, or logs **no plaintext and no inner-frame structure** — only §3.9-permitted metadata. This is a security contract, enforced by the property test.
- **`WSS_LISTEN_ADDR` semantics**: set ⇒ bridge enabled on that addr; unset ⇒ Iroh-only relay. FROZEN as the bridge's on/off switch.

## Implementation notes
- **Choose the WSS server and clear its license.** Two candidates per the brief: (a) `tokio-tungstenite` (a focused async WebSocket lib, MIT) layered over the relay's existing async runtime, or (b) a `hyper` HTTP-upgrade path (`hyper` already in the tree via the Core's Connect-Web bridge / reqwest chain) handling the `Upgrade: websocket` handshake directly. Prefer **`tokio-tungstenite`** for a clean WSS framing surface unless 214 already stood up a `hyper` server you can hang an upgrade route on — decide in-task. Either way: **run `cargo deny check`**; `tokio-tungstenite` + its `tungstenite` core are **MIT** (already on the allow-list) but **confirm the full transitive set** (the TLS backend especially — pin **rustls**, not native-tls/openssl, matching the `reqwest`-rustls posture Task 112 ratified). If any new SPDX surfaces, add it to `deny.toml` with a **dated operator-ratification comment** in the house style and flag it in Handoff. Copyleft/SSPL/BSL = **Stop-and-ask**.
- **One Iroh endpoint.** Open the per-bridge Iroh bidi stream **through the relay's existing `iroh-relay`-embedded endpoint** (Task 214) — do not construct a second `iroh::Endpoint`. Use **fully-qualified `AsyncRead`/`AsyncWrite` trait syntax** on `iroh::endpoint::{Send,Recv}Stream` (spike-102 gotcha 1) to avoid the inherent-vs-trait `poll_read`/`poll_write` shadowing.
- **The pump must be dumb on purpose.** Implement the two directions as `tokio::io::copy`-style loops over `&[u8]` (or `tokio::select!` over the two halves), copying frame payloads verbatim. Resist any temptation to parse a length, peek a gRPC header, or buffer-by-message-boundary beyond what WSS framing already gives you — every such hook is a place plaintext could leak into a log or metric. The ciphertext-only test exists to catch exactly that regression.
- **Map WSS framing to stream bytes faithfully.** Use **binary** WebSocket frames (not text); a fragmented WSS message reassembles to the same byte run on the Iroh side. Backpressure: if the Iroh stream is slow, apply WSS backpressure (don't unboundedly buffer) — bounded copy buffers, drop nothing silently without a metric.
- **Cross-platform.** The relay binary builds on the Linux + Windows CI lanes (Task 113); keep `std::os::unix`-only types out of the bridge. TLS via rustls is portable.
- **`api.rs` surface.** If the relay exposes a public bridge type, `crates/relay/src/api.rs` is indexed by `regen-interfaces.sh` (depth-3 rule) → `rust-api.md`; most of the bridge is internal to the binary, so the regen diff may be empty — confirm with the verification step and note it in Handoff (cf. Task 201's regen note).

## Verification
**Tier 2.** The test double is a **loopback Iroh "Core" endpoint** (two Iroh endpoints on one host, the §5.1 loopback-Iroh double) plus a `tokio-tungstenite` in-process WSS client. It proves: the `/wss/<endpoint_id>` route, the one-WSS-to-one-Iroh-bidi mapping, **byte-identical bidirectional forwarding**, the **ciphertext-only invariant** (the pump derives no plaintext), and clean teardown of `wss_bridges`. It does **NOT** cover: a **real browser** establishing a real Noise IK over a **real WSS connection across a real network** to a Core behind a real NAT — that is the **Phase-5 web-client** work (519–522) and the **Phase-2/Phase-5 Tier-3 checklist** line ("open the web client on a borrowed laptop … LAN-direct + relayed"), and it depends on the still-`PENDING` real-WAN-relayed datapoint in `design/spikes/tonic-iroh-findings.md §5`.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-relay` (wss/bridge) → the loopback round-trip, ciphertext-only invariant, path-routing, and teardown tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → advisories/bans/licenses/sources all green (the WSS server crate + its rustls TLS backend cleared; `deny.toml` updated + dated-ratified if a new SPDX surfaced).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → clean, or commit the regen if `crates/relay/src/api.rs` gained a public bridge type.
7. `scripts/smoke.sh` → unchanged (the smoke Core is co-located/UDS; the WSS bridge is relay-side and not on the co-located happy path). Exits 0.

## Definition of Done
- [ ] `WSS_LISTEN_ADDR` bound: set ⇒ WSS bridge listener up; unset ⇒ Iroh-only relay unchanged from Task 214
- [ ] `/wss/<endpoint_id>` route parses the endpoint ID and upgrades; malformed IDs rejected pre-upgrade
- [ ] Per-connection bridge opens **one** Iroh bidi stream through the relay's existing endpoint; registers/removes a `WssBridge` in `RelayState.wss_bridges`
- [ ] Opaque bidirectional byte pump (WSS binary frames ↔ Iroh stream bytes), no parsing/decoding of frame contents
- [ ] Ciphertext-only invariant enforced + property-tested; bridge logs/metrics expose only §3.9-permitted metadata
- [ ] Clean teardown on either-side close/idle/error; no surviving bridge state
- [ ] WSS server crate license cleared via `cargo deny check`; any new SPDX dated-ratified in `deny.toml`
- [ ] Verification commands pass; smoke unchanged (exits 0); interfaces clean or regenerated
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate ones in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/relay/src/` (modified/new — the WSS bridge listener + per-connection pump + `wss_bridges` wiring; module placement matches Task 214's layout)
- `crates/relay/Cargo.toml` (modified — WSS server + rustls TLS deps)
- `Cargo.toml` (modified only if the WSS server dep is pinned in `[workspace.dependencies]`)
- `deny.toml` (modified only if a new SPDX needs dated ratification)
- `crates/relay/tests/wss_bridge.rs` (new — loopback round-trip + ciphertext-only + routing + teardown tests)
- `docs/interfaces/rust-api.md` (regenerated only if `crates/relay/src/api.rs` gained a public type)

## Commit message
```
phase-2: relay WSS bridge (WSS <-> Iroh, ciphertext-only)

Binds WSS_LISTEN_ADDR and adds the design/11 §3.4 Path-B bridge to
concerto-relay: per browser connection, opens one Iroh bidi stream to
the addressed Core and pumps opaque bytes both ways on /wss/<endpoint_id>.
The relay sees ciphertext only — the pump never decodes the inner Noise
frame, enforced by a property test. Loopback-Iroh Tier-2 double; the real
browser over a real network is Phase-5/Tier-3.

Refs: tasks/v1.0/215-relay-wss-bridge.md
```

## Handoff Notes (fill in when finishing)
- Drift from plan / WSS server crate chosen + its license clearance / `WSS_LISTEN_ADDR` + `/wss/<endpoint_id>` framing as frozen / ciphertext-only test shape / regen-interfaces diff state / Tier-3 lines deferred to P5 (real browser, real-WAN-relayed) / Smoke-gate state (unchanged)
```
