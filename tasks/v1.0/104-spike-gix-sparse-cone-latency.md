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
- [x] Large-repo + sparse-cone fixture generated (size documented; extrapolation noted if under 2M)
- [x] Warm + cold p50/p95 measured for shell-out and gix-native paths
- [x] Findings doc committed with numbers and GO/NO-GO vs the <100 ms bar
- [x] Recommendation recorded (shell-out vs gix-native vs CI-fixture-image)
- [x] Single commit created with the message below

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
  - **Placement = the existing gix-wrap Criterion bench** (the task's preferred option), not a standalone `spikes/gix-sparse-cone/` crate. New file `crates/gix-wrap/benches/sparse_cone.rs` + one `[[bench]] name = "sparse_cone"` stanza in `crates/gix-wrap/Cargo.toml`. Zero root-`Cargo.toml`, dependency-tree, or `cargo deny` impact — `gix-wrap` already pins `gix 0.77` + `criterion 0.5`.
  - **Fixture size actually built + measured end-to-end: `medium` (500k files / 25k cone), with a `quick` (100k/10k) corroborating point** — both real, on this machine (Apple M5 Pro, 64 GB). The **2M/100k design target is EXTRAPOLATED**, not directly measured: a `full` 2M build was attempted but its multi-million-file `git add` (loose-object hashing, ~25 min in) did not complete cleanly here — exactly the cost the task flagged, and why the findings recommend a pre-packed CI fixture image (§7). The two measured points establish the scaling law (latency tracks the **cone**, not the repo); §4a of the findings extrapolates to 100k cone explicitly and shows the verdict does NOT hinge on the extrapolation (shell-out is GO at the measured 25k cone and stays GO extrapolated; only the gix-native *naïve scan* extrapolates over the bar — and we recommend against it anyway). The cone is held at a fixed 5% of the repo so the points line up with the target. Fixtures self-delete and are never committed.
  - **gix-native cell is the *reachable* gix path, not full `gix::status`.** The pinned feature set reaches `gix-index` (open_index + entry iteration) but NOT the `status` feature (it would pull new crates `gix-status` + `gix-dir`, requiring a root-`Cargo.toml` bump + a fresh `cargo deny` pass — out of scope). So the gix cell measures open-repo + decode-(sparse)-index + stat-every-cone-entry-vs-index, which is the dominant cost of a real status; it omits the untracked dirwalk + rename/blob-diff. That feature-gating is a first-class finding, consistent with V0.1 Task 29's "`gix::status` churning" outcome.
- **Open questions for next task:**
  - **Verdict: GO for the shell-out path** vs the <100 ms bar. The V0.1 shell-out (`git status --porcelain=v1 -z`) is GO at the measured 500k/25k (p50 25 ms, p95 34 ms cold / 26 ms warm) and stays GO extrapolated to 2M/100k (~75 ms p50 on a pessimistic *linear* extrapolation; lower warm because fsmonitor skips the full re-stat). Latency tracks the **cone**, not the repo (sparse-checkout + sparse-index). **gix-native is NO-GO**: feature-gated (§5) *and* its single-threaded, fsmonitor-blind full-cone scan extrapolates to ~170 ms. **Recommendation: keep the V0.1 shell-out as the production status hot path** for Tasks 302/303 — faster than the reachable gix path at every measured scale *and* already correct. This corrects `design/02 §3.1` (which routes `git status` → gix) for V1.0, the same correction V0.1 already applied for the non-sparse case.
  - **Do NOT pursue gix-native status** on the strength of this spike: it is feature-gated (new crates + deny pass) and slower here. The locked `concerto_gix_wrap::status` seam lets the body be swapped later with no downstream change if a future gix release is both correctness-complete and faster.
  - **A pre-built multi-million-file CI fixture image IS worth building for Phase 3 — as a convenience, not a blocker.** Generating 2M files per CI run is too slow (loose-object hashing). Tasks 302/303's bench gate should restore a pre-packed tarball (objects in a pack, sparse-index pre-written) so the gate times `status`, not fixture construction. `sparse_cone.rs` is the recipe to bake that image; building the image is the Phase-3 follow-on the task scope flagged.
  - **fsmonitor stays the default** for cones at this scale and larger (Task 304 already plans restart-if-dead). warm≈cold on a warm M5 SSD at these cone sizes; the benefit grows with cone size and on cold/networked disks.
- **Deliberate debt:** the 2M/100k target is extrapolated, not measured (the 2M fixture is too slow to build per-spike on a loose-object layout — the bench supports `SPARSE_CONE_SCALE=full` and would measure it directly on a faster/pre-packed fixture; this is the Phase-3 CI fixture-image follow-on). gix-native cell omits the untracked dirwalk + rename/blob-diff (feature-gated; see Drift). The `medium` measurement uses `SPARSE_CONE_SAMPLES=25` (real latency; spike tier permits reduced sample/measurement time). No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` in the bench.
- **Smoke-gate state:** unchanged. This is a spike — `scripts/smoke.sh` is untouched and the bench is not on the smoke path; CI only validates the bench compiles (`cargo bench --no-run`), like the Task 29 bench.
