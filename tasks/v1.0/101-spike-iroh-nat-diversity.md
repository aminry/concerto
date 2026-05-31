# Task 101 — Spike: Iroh NAT-Diversity Traversal

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | spike |
| Verification tier | spike |
| Size | spike (~3 engineer-days) |
| Depends on | — |
| Touches subsystem(s) | 11 (Remote Transport & Relay) |
| Smoke gate | unchanged |

## Goal
Establish, with real measurements across diverse real-world networks, whether Iroh's QUIC hole-punching achieves the **>70% direct-connection** rate V1.0's remote story is bet on. This spike is the single biggest gate on V1.0 (`design/00 §11`, `design/11 §10`): a GO unblocks Phases 2 and 5; a NO-GO (direct rate <60%) triggers the tsnet-sidecar contingency, which is an operator decision, not something to build around.

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §3.2 (relay topology), §3.6 (NAT-success target), §3.8 (tsnet contingency, R-1), §10 (the spike definition).
- `design/00_Architecture_Overview.md` §11 (validation spikes — this is spike #1, "run it in week 1–2").
- `tasks/v1.0/README.md` §5.2 (what a `spike` task must produce).

## Scope — in
- A throwaway harness at `spikes/iroh-nat/`: a tiny Rust binary pair (a "core" endpoint and a "client" endpoint) built on the **same vendored Iroh version V1.0 will pin** (see Implementation notes), that:
  - establishes an Iroh connection client→core,
  - reports whether the resulting path is **direct** (hole-punched) or **relayed**,
  - logs NAT type / candidate info Iroh exposes, and round-trip connect time.
- A documented test matrix exercised across **as many real network pairs as you can reach** (the more diverse the NATs, the more meaningful): home NAT ↔ home NAT, home ↔ cellular (CGNAT), home ↔ corporate/VPN, behind symmetric NAT, and at least one ISP known to block UDP (to validate relay-over-TCP fallback, R-8).
- A relay reachable for fallback: stand up `iroh-relay` (or use a temporary throwaway relay) so "relayed" is actually achievable and measured, not just "failed."
- Findings doc `design/spikes/iroh-nat-findings.md`: the matrix, per-pair direct-vs-relayed result, the **aggregate direct %**, UDP-blocking observations, and an explicit **GO / NO-GO** vs the >70% bar with the contingency recommendation if NO-GO.

## Scope — out
- Production transport code (that's Task 212) — this is a throwaway.
- Tonic-over-Iroh throughput (that's the separate spike 102).
- The tsnet sidecar itself (only recommend it if NO-GO; building it is an operator-triggered follow-on).

## Public interface this task locks
- None. Spikes lock nothing; `spikes/iroh-nat/` is throwaway and may be deleted after Task 212 lands.

## Implementation notes
- **Keep the spike out of the root Cargo workspace.** Give `spikes/iroh-nat/Cargo.toml` its own empty `[workspace]` table so it is a standalone workspace. The root `Cargo.toml` `members` list is explicit (no globs) and must NOT be edited — if the spike joined the root workspace, its Iroh dependency would be pulled into every other task's `cargo check --workspace` and `cargo deny check`, which could hard-block the orchestrator (Stop #13) before Iroh is properly vetted in Task 212/214. Build and lint the spike from its own manifest.
- **Pin the Iroh version you intend to vendor for V1.0** (`design/11 §8` mandates pinning Iroh as a vendored sub-crate to avoid wire-compat breaks). Record the exact version in the findings — the GO is only valid for that version.
- Diversity beats sample size here. Five genuinely different NAT pairs tell you more than fifty runs behind one router. Recruit a phone hotspot, a coffee-shop/corporate network, and a VPN if you can.
- Capture Iroh's own connection-type signal (direct vs relay) rather than inferring from latency.
- If you cannot personally reach a symmetric-NAT or UDP-blocking network, say so explicitly in the findings and mark that row "unmeasured" — do not extrapolate.

## Verification
Tier: **spike**. Not a green smoke gate — a measured finding.
1. `cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin core` and `... --bin client` (or the harness's documented invocation) connect successfully on at least the local pair.
2. `design/spikes/iroh-nat-findings.md` exists, contains the network matrix with per-pair direct/relayed results, the aggregate direct %, the pinned Iroh version, and ends with a clearly marked **GO** (≥70% direct) / **MARGINAL** (60–70%) / **NO-GO** (<60%) plus the contingency recommendation.
3. The harness builds clean: `cargo clippy --manifest-path spikes/iroh-nat/Cargo.toml -- -D warnings`.

## Definition of Done
- [ ] Harness runs and reports direct-vs-relayed for each reachable network pair
- [ ] Findings doc committed with the matrix, aggregate %, pinned Iroh version, and GO/NO-GO
- [ ] UDP-blocking / relay-over-TCP fallback observation recorded (or marked unmeasured with reason)
- [ ] No `TODO`/`FIXME` in the harness
- [ ] Single commit created with the message below

## Outputs
- `spikes/iroh-nat/` (new — standalone Cargo crate with its own `[workspace]` table, throwaway)
- `design/spikes/iroh-nat-findings.md` (new)
- Root `Cargo.toml` is NOT modified (the spike is its own workspace).

## Commit message
```
phase-1 spike: iroh NAT-diversity traversal findings

Throwaway harness measuring Iroh direct-vs-relayed connection rate
across real NAT pairs against the >70% V1.0 bar. Findings doc records
the matrix and a GO/NO-GO with the tsnet contingency recommendation.

Refs: tasks/v1.0/101-spike-iroh-nat-diversity.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
