# Task 104 — Spike: gix Sparse-Cone Status Latency

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | spike |
| Verification tier | spike |
| Size | spike (~2 engineer-days) |
| Depends on | — |
| Touches subsystem(s) | 02 (Repository Manager) |
| Smoke gate | unchanged |

## Goal
Measure whether `git status` (via `gix` and/or shell-out) stays **<100 ms on a 2M-file repo with a ~100k-file sparse cone** with fsmonitor + untracked-cache active (`design/00 §7.7`, `design/02 §7.2`). V0.1's Task 29 settled `status` as a shell-out on a 10k-file fixture; this spike extends the measurement to the real monorepo scale and sparse-cone case that Phase 3 (Tasks 302/303) depends on, and decides whether `gix`-native status is worth pursuing or shell-out remains the hot path.

## Inputs to read before starting
- `design/02_Repository_Manager.md` §3.1 (gix on the hot path, hybrid routing), §6.2 (shared `.git/objects` across cones), §7.2 (status hot-path sequence).
- `design/00_Architecture_Overview.md` §7.7 (`gix status` <100 ms target) and §11 (gix-vs-shell-out spike).
- `tasks/29-gix-status-hot-path.md` → "Handoff Notes" (V0.1 chose shell-out `git status --porcelain=v1 -z` on a 10k fixture; the `gix::status` API was churning).
- `crates/gix-wrap/src/status.rs` and `benches/status.rs` (the existing V0.1 implementation + bench to extend).

## Scope — in
- A fixture generator for a **large synthetic repo** (target ~2M files; if a runner can't build that cheaply, build the largest you can — e.g. 500k — and **extrapolate explicitly** in the findings, noting the gap) with a sparse cone of ~100k files, fsmonitor + `core.untrackedCache` + `feature.manyFiles` + commit-graph configured.
- A benchmark comparing, on that fixture: (a) the V0.1 shell-out path, (b) a `gix`-native status attempt against the **current `gix` version** the workspace pins (`gix 0.77`), for both cold and fsmonitor-warm cases.
- A findings doc `design/spikes/gix-sparse-cone-findings.md`: fixture size actually used, per-path p50/p95 numbers (warm + cold), the shell-out-vs-gix comparison, and an explicit **GO / NO-GO** vs the <100 ms bar plus a recommendation (keep shell-out / pursue gix-native / fixture-image needed for CI).

## Scope — out
- Production sparse-checkout lifecycle (Task 302) and the production status path (Task 303) — this only measures.
- A pre-built multi-million-file CI fixture image (note in findings if one is needed; building it is a Phase-3 concern).

## Public interface this task locks
- None (spike). It may live alongside the existing `crates/gix-wrap/benches/` or under `spikes/gix-sparse-cone/` — state which in Handoff.

## Implementation notes
- **Placement & workspace:** preferred is a Criterion bench in the existing `crates/gix-wrap` (it is already a root-workspace member and already depends on `gix`, so no `cargo deny`/workspace pollution). If you instead use a standalone `spikes/gix-sparse-cone/` crate, give it its own empty `[workspace]` table and do NOT edit the root `Cargo.toml`. State which you chose in Handoff; the verification commands below assume the gix-wrap-bench placement — adjust to `--manifest-path` if standalone.
- The fixture is the hard part. Generating 2M files takes real disk + time; a tarball-restore or a sparse synthetic tree (many small files in a deep dir structure) is acceptable as long as the cone size is realistic. Document exactly what you generated.
- Warm-vs-cold matters enormously with fsmonitor; report both, and ensure the fsmonitor daemon is actually running for the warm case (verify via `git fsmonitor--daemon status`).
- If `gix::status` (gix 0.77) still can't match `git status` correctness on the cone, that itself is the finding — record it and recommend shell-out, consistent with the V0.1 outcome.

## Verification
Tier: **spike**.
1. The fixture generator + bench run and print warm/cold p50/p95 for shell-out and gix paths.
2. `design/spikes/gix-sparse-cone-findings.md` exists with the fixture size used (and extrapolation note if <2M), the numbers, the comparison, and a clear **GO / NO-GO** + recommendation.
3. The new bench/harness builds clean: `cargo clippy -p concerto-gix-wrap --all-targets -- -D warnings` (gix-wrap-bench placement) — or `cargo clippy --manifest-path spikes/gix-sparse-cone/Cargo.toml -- -D warnings` if standalone.

## Definition of Done
- [ ] Large-repo + sparse-cone fixture generated (size documented; extrapolation noted if under 2M)
- [ ] Warm + cold p50/p95 measured for shell-out and gix-native paths
- [ ] Findings doc committed with numbers and GO/NO-GO vs the <100 ms bar
- [ ] Recommendation recorded (shell-out vs gix-native vs CI-fixture-image)
- [ ] Single commit created with the message below

## Outputs
- `spikes/gix-sparse-cone/` OR `crates/gix-wrap/benches/sparse_cone.rs` (new — state which in Handoff)
- `design/spikes/gix-sparse-cone-findings.md` (new)

## Commit message
```
phase-1 spike: gix sparse-cone status latency findings

Benchmarks git status (shell-out vs gix-native) on a large repo with a
~100k-file sparse cone, fsmonitor warm and cold, against the <100ms
V1.0 bar. Findings doc records numbers and a keep-shell-out vs
pursue-gix recommendation for Phase 3.

Refs: tasks/v1.0/104-spike-gix-sparse-cone-latency.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
