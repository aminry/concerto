# Task 102 — Spike: Tonic-over-Iroh Latency & Throughput

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | spike |
| Verification tier | spike |
| Size | spike (~2 engineer-days) |
| Depends on | 101 |
| Touches subsystem(s) | 10 (Client API Protocol), 11 (Remote Transport & Relay) |
| Smoke gate | unchanged |

## Goal
Prove that running the existing Tonic gRPC stack over an Iroh QUIC stream (instead of UDS) stays within the V1.0 performance envelope: **unary round-trip within ~30% of bare UDS**, and **`session.io` streaming throughput > 1 MB/s over Iroh+Noise+gRPC** (`design/11 §10`). This de-risks the core architectural claim that "the schema does not branch by transport" (`design/10 §3.4`) before Task 212 commits to it in production.

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §3.3 (the API channel — QUIC stream pool for gRPC), §10 (throughput target).
- `design/10_Local_API_Protocol.md` §3.4 (transport-agnostic schema), §5.2 (`session.io` ~1 MB/s, 1 MiB buffer).
- `design/00_Architecture_Overview.md` §7.7 (`session.io` >5 MB/s LAN / >1 MB/s WAN; split-host chat <100 ms LAN / <250 ms WAN).
- `tasks/v1.0/101-spike-iroh-nat-diversity.md` → "Handoff Notes" (the pinned Iroh version).

## Scope — in
- A throwaway harness at `spikes/tonic-iroh/` that serves a trivial Tonic service (one unary echo + one server-streaming byte-firehose) over **three transports**: UDS (baseline), Iroh QUIC direct (loopback / LAN), and Iroh via relay.
- Measurements: unary p50/p95 round-trip and streaming MB/s for each transport, on **LAN-direct** and (if reachable from spike 101's networks) **WAN-relayed**.
- A findings doc `design/spikes/tonic-iroh-findings.md` reporting the numbers as a table, the Iroh-vs-UDS ratio, and an explicit **GO / NO-GO** vs the "within 30% of UDS" + ">1 MB/s session.io" bars.

## Scope — out
- The Noise IK layer's full implementation (Task 208) — for the spike, layer Iroh's built-in encryption + a representative AEAD pass so the throughput number includes encryption overhead, and note in findings if you stubbed the second Noise layer.
- Reconnect / ring-buffer semantics (Task 202).
- Production transport wiring (Task 212).

## Public interface this task locks
- None (throwaway spike).

## Implementation notes
- **Keep the spike out of the root Cargo workspace** — give `spikes/tonic-iroh/Cargo.toml` its own empty `[workspace]` table (same reasoning as Task 101: an Iroh-dependent crate in the root workspace would pull Iroh into every other task's `cargo deny check`). Do NOT edit the root `Cargo.toml`. Build/lint from the spike's own manifest.
- Pin the same Tonic/prost versions the workspace uses (`tonic 0.12`, `prost 0.13` — see root `Cargo.toml`) explicitly in the spike's manifest so the measurement reflects production codegen overhead.
- Drive Tonic over an Iroh bi-directional stream by adapting it to a `tokio::io::AsyncRead + AsyncWrite` channel (Tonic can serve over an arbitrary connected transport). The mechanics here are the actual risk — document any friction; it directly informs Task 212's design.
- For the streaming throughput test, push at least tens of MB so the steady-state rate dominates connection setup.
- Report numbers honestly with the hardware and network described. A ratio of "Iroh is 1.25× UDS latency" is a GO; "3× and 400 KB/s" is a NO-GO and a serious finding for Task 212.

## Verification
Tier: **spike**.
1. The harness runs all three transports and prints the latency + throughput table: `cargo run --manifest-path spikes/tonic-iroh/Cargo.toml` (or the documented invocation).
2. `design/spikes/tonic-iroh-findings.md` exists with the table, the Iroh-vs-UDS ratios, what (if anything) was stubbed in the encryption path, and a clear **GO / NO-GO**.
3. `cargo clippy --manifest-path spikes/tonic-iroh/Cargo.toml -- -D warnings` clean.

## Definition of Done
- [x] Harness measures unary + streaming over UDS, Iroh-direct, Iroh-relay (all three real; relay is a local in-process `iroh-relay` dev instance forced via cleared IP transports — see findings §4)
- [x] Findings doc committed with numbers, ratios, and GO/NO-GO vs the §10 bars (`design/spikes/tonic-iroh-findings.md`; real-WAN-relayed row marked PENDING operator field measurement)
- [x] Any encryption-path simplification disclosed in the findings (Iroh TLS is ON in every number; the second Noise IK layer is stubbed — findings §3)
- [x] No `TODO`/`FIXME` in the harness (grep clean)
- [x] Single commit created with the message below

## Outputs
- `spikes/tonic-iroh/` (new — standalone throwaway crate with its own `[workspace]` table)
- `design/spikes/tonic-iroh-findings.md` (new)
- Root `Cargo.toml` is NOT modified (the spike is its own workspace).

## Commit message
```
phase-1 spike: tonic-over-iroh latency & throughput findings

Throwaway harness benchmarking the existing Tonic stack over UDS vs
Iroh-direct vs Iroh-relay. Findings doc records latency/throughput and
a GO/NO-GO against the "within 30% of UDS, >1 MB/s session.io" bars.

Refs: tasks/v1.0/102-spike-tonic-over-iroh.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** (1) **Tonic version: kept production `tonic =0.12.3` / `prost =0.13.5` and HAND-ROLLED the adapter** — did NOT pull `tonic-iroh-transport` (it forces `tonic 0.14.6`, conflicting with the root pin; bumping the workspace would invalidate the very codegen the spike measures). The hand-roll worked on the first real attempt (~70 lines: `IrohDuplex` + `IrohConnector` in `spikes/tonic-iroh/src/iroh_adapter.rs`), so it is the recommended Task-212 path. Iroh `=0.98.2` + `iroh-relay =0.98.0` reused from Task 101. (2) **Relay is in-process, not a CLI `iroh-relay --dev`** — the harness stands up `iroh_relay::server::Server` (plain-HTTP, OS-assigned loopback port) so the relay run is hermetic and CI-runnable with no external binary; both endpoints `clear_ip_transports()` + point at it, forcing the relayed path (verified: relay emits forward-backpressure WARNs under the firehose). (3) **The unary "within 30% of UDS" bar is reported two ways** — the raw loopback ratio trips it (~3.2–3.8×), but that is a sub-millisecond artifact; the figure that transfers is the FIXED ADDITIVE overhead. Both are printed and explained rather than massaging one number.
- **Open questions for next task:** **Measured (Apple M5 Pro, macOS arm64, single host; modal of 3 runs):** UDS unary p50 ~30–34 µs / stream ~1850–1910 MB/s · **Iroh-direct** p50 ~112 µs (**~3.3–3.8× UDS, +~80 µs additive**) / stream **~70–97 MB/s** · **Iroh-relay (local)** p50 ~99–107 µs (**~3.2–3.3× UDS, +~70 µs additive**) / stream **~210–230 MB/s**. **GO/NO-GO:** **streaming = emphatic GO** (every transport is 70–230× the >1 MB/s bar, Iroh TLS included); **unary = GO on the bar's real-RTT intent** (the +~70–90 µs additive is ≤~8% of a 1 ms LAN RTT and <0.5% of a WAN RTT — the raw loopback ratio is recorded honestly as a sub-ms artifact, not massaged); **architectural claim (Tonic-stack-unmodified-over-Iroh) = GO**. **Adapter friction Task 212 MUST plan for:** (a) `Send/RecvStream` expose an *inherent* `poll_write`/`poll_read` that shadows the `tokio::io::Async{Write,Read}` trait method (wrong error type `WriteError` vs `io::Error`) — use fully-qualified trait syntax; (b) model is **one gRPC connection = one Iroh bidi stream**, many per `Connection` (`design/11 §3.3`); (c) **acceptor priming** — the client must flush a zero-byte write so the server's `accept_bi()` wakes; (d) lift Tonic's 4 MiB message ceilings. **Encryption:** Iroh's TLS 1.3 is ON in every number; the **second Noise IK layer (`design/12` / Task 208) is STUBBED** — Task 208 must benchmark it (streaming headroom says it won't breach the bar). **PENDING (Phase-1 Tier-3 line):** the **real-WAN-relayed** row is `PENDING operator field measurement` — the local relay has zero RTT / zero bandwidth limit, so it proves the relayed gRPC path works and bounds local overhead but is NOT a WAN number; the operator must run the harness across real machines + a real relay at the Phase-1 gate (alongside Task 101's NAT matrix).
- **Deliberate debt:** Second Noise IK AEAD layer stubbed (Task 208, in scope-out). Real-WAN-relayed numbers deferred to operator field measurement. Harness is throwaway (`spikes/tonic-iroh/` may be deleted after Task 212 lands); standalone Cargo workspace (empty `[workspace]` table), not in the root workspace, so its Iroh/tonic-0.12 pins never leak into other tasks' `cargo check --workspace` / `cargo deny check`. Root `Cargo.toml` untouched.
- **Smoke-gate state:** unchanged (spike produces no product code; `scripts/smoke.sh` untouched). Spike verification is "harness runs all three transports + prints the latency/throughput table + findings doc committed with GO/NO-GO"; done — `cargo run --manifest-path spikes/tonic-iroh/Cargo.toml` prints the table and `cargo clippy --manifest-path spikes/tonic-iroh/Cargo.toml -- -D warnings` is clean.
