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
- [ ] Harness measures unary + streaming over UDS, Iroh-direct, Iroh-relay
- [ ] Findings doc committed with numbers, ratios, and GO/NO-GO vs the §10 bars
- [ ] Any encryption-path simplification disclosed in the findings
- [ ] No `TODO`/`FIXME` in the harness
- [ ] Single commit created with the message below

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
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
