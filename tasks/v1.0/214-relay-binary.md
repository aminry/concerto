# Task 214 — `crates/relay`: Self-Hosted Relay Binary (iroh-relay + env config + Prometheus)

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 212 |
| Touches subsystem(s) | 11 (Remote Transport & Relay) |
| Smoke gate | new:relay-route |

## Goal
Fill the empty `crates/relay` with the self-hosted `concerto-relay` binary: a single static Rust binary that **embeds Iroh's `iroh-relay 0.98.0` as a library** (do NOT define a new relay protocol — R-7), wraps it with Twelve-Factor env-var config and a Prometheus metrics endpoint, and runs the minimal relay job from `design/11 §3.2` — accept the Iroh relay protocol, keep an in-memory `endpoint_id → addr` routing table (TTL 90 s, refreshed ~per-minute), assist hole-punch, and forward relayed QUIC. This is the load-bearing fallback the NAT spike proved is **required, not optional** (a meaningful fraction of real clients — cellular, symmetric-NAT, corporate — land on it). The spike already ran an in-process `iroh-relay` dev server hermetically, proving the library-embed approach works for both the binary core and the tests.

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §3.2 (the **minimal** relay job: accept Iroh's relay protocol; routing table `endpoint_id → current public IP+port` refreshed ~every minute; STUN-like hole-punch address exchange; forward relayed QUIC; per-endpoint in-memory state, **TTL 90 s**; stateless except routing entries), §12 R-7 (**use Iroh's `iroh-relay` with our config — do NOT define a new protocol**), §6.2 (the relay-side mermaid: Iroh relay protocol + routes table + WSS bridge + bandwidth counters + health endpoint), §6.3 (**single static binary**; Docker image; **env-var-only config (Twelve-Factor)**: `RELAY_LISTEN_ADDR` / `WSS_LISTEN_ADDR` / `MAX_ROUTES` / `BANDWIDTH_CAP_PER_ENDPOINT` / `PROMETHEUS_LISTEN_ADDR`; **Prometheus metrics**: routes count, bytes forwarded, hole-punch success rate per region; a node handles **10k–50k routes** + ~1 Gbps), §3.9 (relay observability is **ciphertext-only** — the relay sees source IP / endpoint id / byte counts / timestamps, never payload, certs, names, or tokens), §4 (reproduce `RelayState` / `EndpointRoute`).
- `design/spikes/tonic-iroh-findings.md` §6 (the spike stood up an **in-process `iroh-relay` dev server** — plain-HTTP, OS-assigned loopback port, no external binary install — proving `iroh-relay 0.98.0` embeds as a **library** hermetically; **reuse this for the binary core and the tests**) and the §1 "Iroh-relay" transport description.
- `design/spikes/iroh-nat-findings.md` §5 Note A (the relay is **load-bearing** — provision/operate it accordingly; row 5 cellular-CGNAT → public-cloud fell back to relay and the relay carried it). Confirms the pin `iroh-relay 0.98.0` / `iroh 0.98.2`.
- `crates/relay/Cargo.toml` + `crates/relay/src/lib.rs` + `crates/relay/src/main.rs` — the empty 3-line lib + trivial `main.rs` placeholder you fill (already a `[workspace.members]` entry with both a `[lib]` `concerto_relay` and a `[[bin]]` `concerto-relay`).
- `spikes/tonic-iroh/Cargo.toml` — the pin to lift: `iroh-relay = { version = "=0.98.0", features = ["server"] }` (the `server` feature is what exposes the embeddable dev/relay server), `iroh = "=0.98.2"`. Read how the spike's harness constructs the in-process relay (its `src/` — the relay-transport setup) to reuse the embed shape.
- `crates/transport/src/*` (Task 212) — how the Core **registers** its endpoint with a relay (the client side of the protocol this binary serves), so the binary's relay surface matches what 212 expects to talk to.
- `Cargo.toml` `[workspace.dependencies]` — where the `iroh-relay` pin lands (212 may already have added `iroh`); the Prometheus/metrics crate pin.
- `deny.toml` — `[licenses] allow` + dated **operator-ratification comment** style; the `iroh-relay` server-feature tree + the Prometheus crate must clear `cargo deny check`.
- `tasks/v1.0/212-transport-iroh-endpoint.md` → "Handoff Notes" — the iroh pin, the relay-registration client surface, and any `iroh-relay` deps already added.

## Scope — in
- **Fill `crates/relay`**: embed `iroh-relay = "=0.98.0"` (`server` feature) as a **library** — the `concerto_relay` lib owns the relay core (build/run the iroh-relay server from config, the routing-table lifecycle, bandwidth counters), and the `concerto-relay` bin is a thin wrapper that reads env config, starts the relay + the Prometheus endpoint, and handles signals/shutdown. **No new protocol** — the wire protocol is iroh-relay's (R-7).
- **`RelayState` / `EndpointRoute`** (`design/11 §4`): the in-memory `routes: HashMap<IrohEndpointId, EndpointRoute>` with `public_addr` / `last_seen` / `expires_at` (90 s TTL refreshed by keep-alive), plus `bandwidth_counters`. Reserve a `wss_bridges` field/seam (Task 215 fills it; see Scope — out).
- **Env-var config** (`design/11 §6.3`, Twelve-Factor — env only, no config file): parse `RELAY_LISTEN_ADDR`, `WSS_LISTEN_ADDR` (**reserved** — parsed/validated but the WSS bridge itself is Task 215), `MAX_ROUTES` (enforce the cap; reject/evict beyond it), `BANDWIDTH_CAP_PER_ENDPOINT` (enforce per-endpoint), `PROMETHEUS_LISTEN_ADDR`. Sensible defaults; clear error on malformed input; document each var.
- **Prometheus metrics** (`design/11 §6.3`): expose at `PROMETHEUS_LISTEN_ADDR` the metrics named in the design — **routes count**, **bytes forwarded**, **hole-punch success rate / count by region** — plus a basic health/up signal. Freeze the metric names (see Public interface).
- **Routing-table lifecycle:** register endpoints on relay-protocol registration, refresh `expires_at` on keep-alive, evict on TTL expiry; cap at `MAX_ROUTES`. Hole-punch address-exchange assist + relayed-QUIC forwarding are provided by `iroh-relay` itself — wire its config, don't reimplement.
- **Ciphertext-only posture** (`design/11 §3.9`): the relay forwards encrypted QUIC; it must not log or expose payload. Metrics/logs carry only metadata (source IP, endpoint id, byte counts, timestamps, region). Add a test/assertion that the observable surface is metadata-only.
- **Docker image** (`design/11 §6.3`): a `Dockerfile` producing the single static binary image (the deploy artifact; Fly.io is the operator's default but the binary is cloud-agnostic). Keep it minimal; the binary is the deliverable, the image wraps it.
- **Tests** (the Tier-2 double, see Verification): stand up the relay **in-process** (the spike §6 embed), have a Core (212) register its endpoint, and route a relayed QUIC stream through it from a loopback client (IP transports cleared so the only path is relayed); assert the route appears in `RelayState` with a refreshing TTL, the Prometheus endpoint reports `routes count` ≥ 1 and `bytes forwarded` > 0 after the transfer, `MAX_ROUTES`/bandwidth caps are enforced, and the relay never surfaces plaintext.

## Scope — out
- **The WSS bridge** (WSS↔Iroh, ciphertext-only — Task 215). `WSS_LISTEN_ADDR` is **reserved** here (parsed + the `wss_bridges` field stubbed) but the bridge is built in 215. Do not implement WSS framing.
- **Multi-tenant / per-org isolation, Redis persistence, dynamic geographic relay selection** (all V2.0 per `design/11 §2` — keep `RelayState` in-memory, single-tenant; reserve no schema).
- **The Core/client side of registration + fallback** (Task 212 — this binary is the *server* they talk to).
- **Real Fly.io / anycast deployment** and the real-WAN-relayed throughput measurement (Tier-3 — Phase-2 manual checklist + the spike's PENDING real-WAN-relayed row; the Docker image + binary are the artifacts the operator deploys).
- **Defining any relay wire protocol** — R-7 forbids it; embed iroh-relay's.
- **Proto changes** — the relay speaks iroh-relay's protocol, not gRPC; no proto.

## Public interface this task locks
- **The env-var config surface** — FROZEN. The exact names + meaning: `RELAY_LISTEN_ADDR`, `WSS_LISTEN_ADDR` (reserved for 215), `MAX_ROUTES`, `BANDWIDTH_CAP_PER_ENDPOINT`, `PROMETHEUS_LISTEN_ADDR` (`design/11 §6.3`). Operators script against these; new vars are additive.
- **The Prometheus metric names** — FROZEN. The metrics from `design/11 §6.3`: routes count, bytes forwarded, hole-punch success (count/rate, labelled by region). Dashboards/alerts key off these names; additions are append-only.
- **`RelayState` / `EndpointRoute`** field layout (`design/11 §4`) as the in-memory model (215 extends `wss_bridges`).
- **The `concerto_relay` library entry point** (build + run the relay from a config struct) so 215 wraps it to add the WSS bridge in the same binary.

## Implementation notes
- **Embed, don't fork (R-7).** `iroh-relay`'s `server` feature exposes the relay server; the spike (§6) already constructs one in-process on an OS-assigned loopback port with plain HTTP. Lift that construction into the `concerto_relay` lib, then drive its listen addr / limits from env. The hole-punch assist and QUIC forwarding are iroh-relay's; your code is config + routing-table observability + metrics + caps.
- **Twelve-Factor strictness:** env only — no config file, no flags beyond `--help`/`--version`. Parse once at startup, validate, fail fast with a precise message naming the bad var. This is what makes the Docker image and Fly.io deploy clean.
- **Metric naming:** use the design's metric *concepts* and pick conventional Prometheus names (`concerto_relay_routes`, `concerto_relay_bytes_forwarded_total`, `concerto_relay_holepunch_success_total{region=...}` — exact spelling is yours to set but **freeze it in Handoff + the lib** since dashboards depend on it). Use a maintained metrics crate (`prometheus` or `metrics` + `metrics-exporter-prometheus`); pin it and clear `cargo deny`.
- **TTL discipline:** `EndpointRoute::expires_at` is 90 s, refreshed by the per-minute keep-alive (`design/11 §3.2`/§4); evict lazily on access + sweep periodically. A node targets 10k–50k routes — keep the table allocation-sane (no per-route task; one sweep timer).
- **Cross-platform.** The relay binary should build on the Linux + Windows CI lanes (Task 113) — it's the primary Linux/Docker artifact. No `std::os::unix`-only types in the lib's public surface. The Docker image targets Linux x64/arm64 (the deploy target); the Windows lane just needs it to compile.
- **License:** the `iroh-relay` `server` feature pulls more than the client (an HTTP server stack); plus the metrics crate. Run `cargo deny check`; ratify new SPDX with a dated comment; copyleft / SSPL / BSL = **Stop-and-ask**.
- **Pin exactly** (`iroh-relay = "=0.98.0"`, the metrics crate) in `[workspace.dependencies]` with a rationale comment citing R-7 + the spike.

## Verification
Tier 2.

The Tier-2 test double is **the relay running in-process inside a test** (the spike §6 embed) **+ a Core (212) registering with it + a loopback client routing a relayed QUIC stream through it** (IP transports cleared so the path is forced relayed). It proves the **embed + routing-table TTL + forwarding + env-config + Prometheus + caps + ciphertext-only** behavior hermetically in CI. It does **NOT** cover the **real WAN-relayed** path — real relay-server distance, real bandwidth limits, real RTT, anycast routing — which is **Tier-3**: the spike's PENDING real-WAN-relayed row and the Phase-2 manual checklist (deploy the relay on real infra and route a remote client through it).

Per README §5.3 (`rust`):
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-relay` → in-process relay + registration + relayed-stream round-trip, TTL refresh/evict, `MAX_ROUTES` + bandwidth-cap enforcement, Prometheus metric values after transfer, ciphertext-only assertion, env-config parse/validate tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (iroh-relay server tree + metrics crate cleared; `deny.toml` ratified if needed).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen if `concerto_relay`'s public lib surface is indexed (`src/api.rs` or lib root).
7. `scripts/smoke.sh` → the new `relay-route` capability starts the relay in-process, registers a loopback endpoint, routes one relayed stream, and asserts the Prometheus `routes count`/`bytes forwarded`; existing caps stay green. Exits 0.

## Definition of Done
- [ ] `crates/relay` filled: `iroh-relay 0.98.0` embedded as a library (no new protocol — R-7); `concerto_relay` lib + thin `concerto-relay` bin
- [ ] `RelayState`/`EndpointRoute` (90 s TTL, ~per-min refresh, `MAX_ROUTES` cap); bandwidth caps enforced; routing/forwarding via embedded iroh-relay
- [ ] Twelve-Factor env config: `RELAY_LISTEN_ADDR`/`WSS_LISTEN_ADDR`(reserved)/`MAX_ROUTES`/`BANDWIDTH_CAP_PER_ENDPOINT`/`PROMETHEUS_LISTEN_ADDR`, validated, fail-fast
- [ ] Prometheus endpoint: routes count + bytes forwarded + hole-punch success (by region); ciphertext-only posture (metadata-only logs/metrics, asserted)
- [ ] Dockerfile producing the single static binary image
- [ ] FROZEN: env-var config surface + Prometheus metric names + `RelayState`/`EndpointRoute` + the `concerto_relay` lib entry point
- [ ] `cargo deny check` green; any new SPDX ratified in `deny.toml` with a dated comment
- [ ] Tier-2 in-process-relay double tests pass; Verification commands pass; interfaces clean/regenerated; smoke `relay-route` green
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate debt in Handoff)
- [ ] Single commit with the message below

## Outputs
- `Cargo.toml` (modified — `[workspace.dependencies]` += `iroh-relay = "=0.98.0"` + the metrics crate, with rationale)
- `crates/relay/Cargo.toml` (modified — deps + the `server` feature)
- `crates/relay/src/lib.rs` (filled — relay core + `RelayState`/`EndpointRoute` + config struct + metrics), `crates/relay/src/main.rs` (filled — env parse + start + signals), `crates/relay/src/api.rs` (new — frozen lib entry point if surfaced)
- `crates/relay/Dockerfile` (new — single static binary image)
- `crates/relay/tests/relay_route.rs` (new — the in-process relay double)
- `deny.toml` (modified only if a new SPDX needs ratification)
- `scripts/smoke.d/<NN>-relay-route.sh` + `scripts/smoke.manifest` (new capability)
- `docs/interfaces/rust-api.md` (regenerated if the relay lib surface is indexed)

## Commit message
```
phase-2: crates/relay — self-hosted relay binary (iroh-relay + Prometheus)

Fills crates/relay with concerto-relay: embeds iroh-relay 0.98.0 as a
library (no new protocol — R-7), wraps it with Twelve-Factor env config
(RELAY_LISTEN_ADDR/WSS_LISTEN_ADDR[reserved]/MAX_ROUTES/
BANDWIDTH_CAP_PER_ENDPOINT/PROMETHEUS_LISTEN_ADDR) and a Prometheus
endpoint (routes/bytes-forwarded/hole-punch). In-memory RelayState with
90s-TTL routes; ciphertext-only. Tier-2 double: in-process relay + Core
registration + a relayed loopback stream. Real WAN relay = Tier-3.

Refs: tasks/v1.0/214-relay-binary.md
```

## Handoff Notes (fill in when finishing)
- Drift from plan / Frozen Prometheus metric names (exact spelling) / Open questions for Task 215 (WSS bridge embed point) / Deliberate debt / License ratifications / Smoke-gate state
