# Task 220 — Split-Host Loopback Smoke: two Iroh endpoints, end-to-end RPC + stream + Files transfer

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | infra-ops |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 217 |
| Touches subsystem(s) | 11 (Transport — Iroh loopback), 10 (Client API Protocol over Iroh) |
| Smoke gate | new:split-host-loopback |

## Goal
Add the **Tier-2 capstone of the Phase-2 transport spine**: a smoke-gate capability that brings up **two Iroh endpoints on one host (relays disabled → direct hole-punch, the spike's model)** and drives the full remote client path over the Iroh transport + Noise IK — **pair → unary RPC → `Streams.Subscribe` stream → `Files.Upload` + `Files.Download`** — entirely in CI on one machine, no NAT and no second host. Today the smoke gate (`scripts/smoke.sh` + `scripts/smoke.d/*`) only ever dials the Core over **UDS** (`tools/smoke-client` is UDS-only — `connect_to_socket` wraps `UnixStream`). This task proves the same Tonic service surface answers correctly when reached over Iroh, closing the loop on Tasks 207/208/209/212/217's individual unit/integration tests with one end-to-end check. It is the automatable floor under the Phase-2 Tier-3 line "pair a real second machine over LAN / transfer a file split-host."

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §10 (testing strategy — the **"Two-process Iroh loopback (Core + client)" E2E integration** row this task automates; plus the throughput/resilience rows that stay Tier-3), §3.1 (Iroh as the single non-browser remote transport), §3.3 (the three logical channels per pairing; one gRPC connection == one Iroh bidi stream — the adapter shape), §7.1 (the direct hole-punch sequence the loopback models with relays off).
- `design/10_Local_API_Protocol.md` §10 (testing — the **"Same [full RPC round-trips] over Iroh (loopback Iroh node pair)" E2E** row), §6.3 (UDS vs Iroh — **same Tonic server, two transports**; the Iroh listener path. **Note:** §6.3 may still cite `tonic-iroh-transport`; Task 200 amends it to the hand-rolled tonic-0.12 adapter (B1). The loopback drives whatever Task 212 built — **never re-add `tonic-iroh-transport`**).
- `design/spikes/iroh-nat-findings.md` §7 (the pinned trio `iroh 0.98.2` / `iroh-relay 0.98.0`) + `design/spikes/tonic-iroh-findings.md` §2 (the hand-rolled adapter + its four gotchas; the spike's loopback harness under `spikes/` is the **closest existing prior art for two endpoints on one host with relays disabled** — read it for the endpoint-bring-up + direct-connection pattern to reuse).
- `scripts/smoke.manifest` — the capability list + **run order** (PROJECT→REPO→WS→WA→SID chain). Your check appends `split-host-loopback`; pick its position **after the transport-relevant checks** (it needs a workarea + session to subscribe to + a file to transfer — i.e. after `workspace-workarea`/`echo-session`/`streams-subscribe`; before or after `cli`/`backup` is fine — justify the slot).
- `scripts/smoke.sh` — the driver: how it sources each `scripts/smoke.d/NN-<cap>.sh`, the shared `fail`/`wait_for_file` helpers, the `--ci-mode` flag (`CI_MODE` env, currently reserved), the `CONCERTO_HOME`/`SOCKET`/`SMOKE_CLIENT`/`CORE_LOG` exports, and the single-Core boot in `00-core-boot.sh` (this task brings up the **second, Iroh-facing** Core/endpoint itself, inside the check or a helper, and tears it down).
- `scripts/smoke.d/00-core-boot.sh` (the boot + socket-wait + caps-grep pattern to mirror for the Iroh endpoint), `scripts/smoke.d/40-streams-subscribe.sh` (the stream-then-assert pattern), `scripts/smoke.d/95-cli.sh` (a check that builds + runs a binary against the live Core and greps output).
- `tools/smoke-client/src/connect.rs` (**UDS-only** — `connect_to_socket` wraps `UnixStream`; it **cannot** speak Iroh), `tools/smoke-client/src/cmd/{caps,clone,stream_session_io}.rs` + `cmd/mod.rs` (the subcommand + 30s-timeout + id-on-stdout conventions). **Decide: extend `smoke-client` with an Iroh transport (a `--iroh-endpoint`/`--device-cert` flag selecting an Iroh `Channel` instead of UDS) vs. ship a tiny new Iroh-only driver — see Implementation notes.**
- `tasks/v1.0/217-transport-handle-api.md` → "Handoff Notes" — the `TransportHandle` (`start`/`listen_pairing`/etc.) the second endpoint uses to listen; **how to bring up an Iroh endpoint with relays disabled** (direct-only). **Hard dependency.**
- `tasks/v1.0/{207,208,209,212,203}-*.md` → "Handoff Notes" — the pairing RPCs (`Devices.StartPairing`/`CompletePairing`, 207/209), Noise IK session layer (208), the Core Iroh endpoint + adapter (212), and the **`Files.Upload`/`Files.Download` streaming RPCs (203)** this check exercises. Read whichever have landed; the check asserts against their real surface.
- `tasks/v1.0/README.md` §5.3 (the **`infra-ops`** row: "task-specific … always states its exact gate") + §5.1/§5.2 (Tier-2 loopback-Iroh definition + what the double does not cover).

## Scope — in
- A new `scripts/smoke.d/NN-split-host-loopback.sh` defining `check_split_host_loopback` (capability `split-host-loopback`), appended to `scripts/smoke.manifest` in a justified run position (after the chain that produces a workarea + session).
- The check: bring up a **second Iroh endpoint** on the same host with **relays disabled (direct)**, **pair** a synthetic device against the Core (32-byte token / Noise XX per `design/12 §3.3`, driven through the Iroh transport), then over the Iroh + Noise IK channel:
  1. **unary RPC** — e.g. `Runtime.GetServerCapabilities` and assert `transport_kind == TRANSPORT_KIND_IROH` (proves Task 201's per-connection tag fires on the Iroh listener).
  2. **stream** — `Streams.Subscribe(session.io.<sid>)` (or `session.events`) on the workarea/session from earlier chain links, assert output captured.
  3. **Files transfer** — `Files.Upload` a fixture blob then `Files.Download` it back and assert the bytes + blake2b checksum round-trip (per Task 203).
- The **Iroh-capable driver** (extend `smoke-client` or a tiny new tool — decide): dials the Core's Iroh endpoint via the hand-rolled adapter, presents the device cert in metadata, runs the three steps. Reuse `smoke-client`'s 30s-timeout + id-on-stdout conventions.
- `shellcheck`-clean shell; the check green under `scripts/smoke.sh --ci-mode`; clean teardown of the second endpoint (no leaked processes/sockets/ports) via the driver's cleanup.

## Scope — out
- **Building** the Iroh endpoint / adapter / `TransportHandle` / pairing / Files RPCs — Tasks 212/217/207/208/209/203. This task is a **consumer** that exercises them end-to-end; it does not implement transport, crypto, or RPC logic.
- **Real cross-machine split-host** (Core on one box, client on another), **real NAT traversal / relay fallback**, **throughput/migration/`tc netem` shaping** — all **Tier-3** Phase-2 checklist lines (`design/11 §10` performance/resilience/NAT-diversity rows). The loopback is direct-only on one host.
- The **WSS bridge** path (browser transport) — Task 215; not exercised here.
- Any Desktop/renderer UI (Tasks 218/219); this is a headless driver-level smoke.
- Editing `00-core-boot.sh` or other existing checks (the chain stays intact; this check is additive). If the existing Core must be (re)started with its Iroh endpoint enabled, prefer doing that in **this** check's setup or via an env toggle, not by rewriting `00-core-boot.sh` — note the decision.

## Public interface this task locks
- The **`split-host-loopback` smoke capability contract**: the capability name, that `check_split_host_loopback` lives in `scripts/smoke.d/NN-split-host-loopback.sh`, its manifest position, and that it asserts the **pair → IROH-tagged unary → stream → Files up/down round-trip over loopback Iroh + Noise IK**. FROZEN — later tasks (711 full V1.0 gate) compose this capability by name.
- If `smoke-client` is extended: the **Iroh-transport flag surface** (e.g. `--iroh-endpoint <id> --device-cert <path>`) — the seam mobile/web driver tests or Task 711 may reuse.

## Implementation notes
- **Driver decision (flag every choice in the report + Handoff).** `tools/smoke-client` is UDS-only; its `connect.rs` cannot reach Iroh. Two paths: **(a)** add an Iroh transport to `smoke-client` — a `--iroh-endpoint`/`--device-cert` option that builds a `tonic::Channel` over Task 212's hand-rolled adapter (via the `concerto-transport` crate / Task 217's client surface) instead of `connect_to_socket`, reusing every existing subcommand; or **(b)** a tiny purpose-built `tools/split-host-loopback/` driver that stands up both endpoints + runs the three steps in one process. **(a) is preferred** if 212/217 expose a clean client `Channel` builder (it reuses the `caps`/`stream-session-io`/`Files` subcommands and the locked conventions); fall back to **(b)** only if standing up the *second listening endpoint* (the pairing target) doesn't fit the subcommand model. The **pairing target endpoint** is the new piece either way — it needs `TransportHandle::start` + `listen_pairing` (217) and the Core's Iroh listener (212); a Rust helper is almost certainly required for that, so a small new bin under `tools/` (or a `smoke-client serve-loopback`/`pair-loopback` subcommand) is likely needed even under path (a).
- **Relays disabled = direct only.** Configure both endpoints with no relay (the iroh-nat spike's loopback model) so the check is hermetic and fast — no network, no relay binary (that's Task 214/215's territory). Two endpoints on `127.0.0.1`/localhost discovery.
- **Reuse the chain, don't rebuild it.** Slot the check after `workspace-workarea`/`echo-session` so `$WA_ID`/`$SID` already exist for the stream step; the Files step can upload into the workarea's `.context/`. Mirror `40-streams-subscribe.sh` for the stream assertion and `95-cli.sh` for the build-and-run-a-binary pattern.
- **`--ci-mode`.** The gate must pass under `scripts/smoke.sh --ci-mode` (the orchestrator's invocation). If Iroh loopback proves flaky/slow in unattended CI, the correct lever is a generous timeout + direct-only config, **not** skipping the check under `CI_MODE` — this is the Phase-2 capstone and must run in CI. If you do gate any sub-step on `CI_MODE`, justify it loudly.
- **Pre-build binaries** in the check (like `00-core-boot.sh`/`95-cli.sh` do with `cargo build --quiet`) so compile time stays out of the assertion wall-clock; cap each step with a timeout and dump the Core log + driver stderr on failure (the existing checks' diagnostic pattern).

## Verification
**Tier 2. `infra-ops` gate (exact):**
1. `shellcheck scripts/*.sh scripts/smoke.d/*.sh` → clean (the new `NN-split-host-loopback.sh` included; mirror the `# shellcheck shell=bash` header + the `check_<cap>` convention).
2. If the driver is Rust (extended `smoke-client` or a new `tools/` bin): `cargo check --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo build --quiet` for the driver succeeds.
3. `scripts/smoke.sh --list` → shows `split-host-loopback` in the manifest at its chosen position.
4. `scripts/smoke.sh --ci-mode` → exits 0 with `PASS split-host-loopback` (and every prior check still `PASS`); the check brings up the second Iroh endpoint, pairs, runs the IROH-tagged unary + stream + Files up/down round-trip, and tears the endpoint down with no leaked processes.
5. `scripts/smoke.sh --only split-host-loopback` → runs the manifest prefix up to and including this check and passes (exercises the chain dependency).

**Tier-2 double + what it does NOT cover.** The double is **two Iroh endpoints on one host with relays disabled (direct hole-punch only)** + a synthetic paired device. It proves: gRPC-over-Iroh dispatch, the Noise IK session, the per-connection `IROH` transport tag (201), pairing (207/208/209), and the `Files` round-trip (203) — all in CI on one machine. It does **NOT** cover: real cross-machine split-host, real NAT diversity / direct-connection % on real networks, relay fallback, Wi-Fi↔LTE migration, or throughput-vs-UDS budgets — those are the **Tier-3 Phase-2 checklist** lines (`design/11 §10`) the operator signs off at the phase gate.

## Definition of Done
- [ ] `scripts/smoke.d/NN-split-host-loopback.sh` (`check_split_host_loopback`) added; `split-host-loopback` appended to `scripts/smoke.manifest` at a justified position
- [ ] Iroh-capable driver decided + built (extended `smoke-client` Iroh flag **or** a new `tools/` bin; choice recorded)
- [ ] Second Iroh endpoint (relays disabled, direct) brought up + torn down cleanly; pair → IROH-tagged unary → stream → `Files.Upload`/`Download` round-trip all asserted
- [ ] No `tonic-iroh-transport` introduced (uses Task 212's hand-rolled tonic-0.12 adapter via 217)
- [ ] `shellcheck` clean; `scripts/smoke.sh --ci-mode` exits 0 with `PASS split-host-loopback`; `--list` shows it; `--only` works
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code
- [ ] No files outside Outputs modified (esp. `00-core-boot.sh` left intact)
- [ ] Single commit with the message below

## Outputs
- `scripts/smoke.d/NN-split-host-loopback.sh` (new — the check)
- `scripts/smoke.manifest` (modified — append `split-host-loopback`)
- `tools/smoke-client/src/` (modified — Iroh transport flag + the loopback-pairing helper) **OR** `tools/split-host-loopback/` (new bin) — per the driver decision
- `Cargo.toml` (modified only if a new `tools/` bin crate is added to `[workspace.members]`)

## Commit message
```
phase-2: split-host loopback smoke (Iroh RPC + stream + Files)

Adds the split-host-loopback smoke capability: two Iroh endpoints on one
host (relays off, direct) exercising pair -> IROH-tagged unary RPC ->
Streams.Subscribe -> Files.Upload/Download over Iroh + Noise IK. The
Tier-2 capstone of the Phase-2 transport spine; real cross-machine
split-host stays a Tier-3 checklist line.

Refs: tasks/v1.0/220-split-host-loopback-smoke.md
```

## Handoff Notes (fill in when finishing)
- Drift from plan / driver decision (extended smoke-client vs new bin) + the Iroh-flag surface / manifest slot + rationale / how the second endpoint is brought up (217 surface used) / any `CI_MODE` gating + justification / Open questions / Smoke-gate state
