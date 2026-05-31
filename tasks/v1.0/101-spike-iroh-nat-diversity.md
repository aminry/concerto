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
- [x] Harness runs and reports direct-vs-relayed for each reachable network pair (only the loopback / same-host pair is reachable from this isolated single-network env; it was run and reports DIRECT — see findings §4.1. All diverse-NAT rows are PENDING operator field measurement.)
- [x] Findings doc committed with the matrix, aggregate %, pinned Iroh version, and GO/NO-GO — **with the Option-A caveat:** per the operator's Option A decision the final GO/NO-GO is **deferred to the operator at the phase gate**; the doc carries a clearly-labeled **PENDING OPERATOR FIELD MEASUREMENT** provisional verdict and a PENDING aggregate-% instead of a faked verdict (see Handoff Notes). Matrix, pinned Iroh version (0.98.2), and the local-pair real numbers are present.
- [x] UDP-blocking / relay-over-TCP fallback observation recorded — marked **unmeasured-with-reason** (no UDP-blocking network reachable from this env); findings §5 records what the operator must validate (row 6).
- [x] No `TODO`/`FIXME` in the harness (grep clean)
- [x] Single commit created with the message below

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
- **Drift from plan:** (1) **The GO/NO-GO is PENDING operator field measurement** — per the operator's explicit **Option A** decision for this physically-gated spike, the harness was built now and the field verdict is deferred to the operator at the Phase-1 gate. The findings doc ends in a clearly-labeled `PENDING OPERATOR FIELD MEASUREMENT` provisional verdict, NOT a final GO/NO-GO, and the aggregate direct-% is PENDING (only the loopback row is measured). This is the honest outcome, not a downgrade. (2) **Pinned Iroh is `0.98.2`, not the 0.35 line.** The plan said "pin the version V1.0 will vendor"; investigation showed (a) the current *stable* Iroh is 0.98.2 (`cargo info` reports 1.0.0-rc.1 only as a pre-release that sorts higher), and (b) `tonic-iroh-transport 0.9.2` — the Tonic-over-Iroh adapter Task 102 needs — resolves against `iroh 0.98.2` + `tonic 0.14.6`. Pinning 0.98.2 keeps the whole transport stack coherent. The 0.98 API differs substantially from older docs (`NodeId`→`EndpointId`, `Endpoint::node_id()`→`id()`, the single `conn_type` signal replaced by multipath `Connection::paths()` with per-`PathInfo` `is_ip()`/`is_relay()`/`is_selected()`); the harness reads the *selected* path as the direct-vs-relay verdict. (3) A transient build break (transitive `netwatch`/`socket2` major mismatch) was resolved by regenerating the lockfile cleanly; no manifest hack needed.
- **Open questions for next task:** **The aggregate direct-% bar is UNRESOLVED and these network rows remain unmeasured — they become Phase-1 Tier-3 checklist lines the operator must run before the Phase-1 gate clears:** row 1 home↔home same-router (two machines); row 2 home↔home two different ISPs; row 3 home↔cellular/CGNAT (phone hotspot); row 4 home↔corporate/VPN; row 5 behind symmetric NAT; row 6 UDP-blocking ISP (relay-over-TCP / port-443 fallback, R-8). Per V4 in `tasks/v1.0/README.md`, **Phases 5 and the relay tasks do not start until this Iroh spike clears its >70%-direct bar** — so the operator's field run is the actual gate. If aggregate <60% → tsnet-sidecar contingency (operator decision, not pre-built). **For Task 102 (Tonic-over-Iroh, depends on 101):** the pinned Iroh is **`iroh = 0.98.2`** (exact), pinned in `spikes/iroh-nat/Cargo.toml`; companion `iroh-relay 0.98.0`; the Tonic adapter to use is `tonic-iroh-transport 0.9.2` (pulls `tonic 0.14.6`), which resolves against this same iroh — reuse this trio. Findings §7 records the same.
- **Deliberate debt:** `--relay disabled` (direct-only) mode can't dial a bare `EndpointId` across hosts because n0 discovery's relay/DNS bootstrap is gone in that mode; it's documented as for "no-relay-at-all" confirmation only, not for the matrix (default mode is the matrix path). The throwaway-relay setup is *documented* (README) rather than scripted — standing up `iroh-relay --dev` is a one-liner and the product relay is Task 214. Harness is throwaway (`spikes/iroh-nat/` may be deleted after Task 212 lands); not in the root workspace.
- **Smoke-gate state:** unchanged (spike produces no product code; `scripts/smoke.sh` untouched). Spike verification is "harness runs + findings doc committed with verdict," and the verdict is the labeled PENDING per Option A.
