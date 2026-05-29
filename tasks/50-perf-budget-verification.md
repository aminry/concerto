# Task 50 — Performance Budget Verification

| Field | Value |
|---|---|
| Phase | 4 |
| Size | medium (1–3d) |
| Depends on | 11, 18, 22, 29 |
| Touches subsystem(s) | 01 (Runtime), 02 (Repository Manager), 15 (Desktop) |
| Smoke gate | unchanged |

## Goal
Add automated benchmarks that gate against the V0.1-relevant performance targets from `design/00 §7.7`: Core idle RSS < 100 MB, Core at 8 active agents RSS < 600 MB, `gix status` < 100 ms (already covered by Task 29's bench), Desktop cold start < 2 s. After this task, CI fails if any V0.1-applicable budget regresses.

## Inputs to read before starting
- `design/00_Architecture_Overview.md` §7.7 (full performance budgets table).
- `tasks/29-gix-status-hot-path.md` (existing bench infrastructure).

## Scope — in
- Add a Criterion bench `crates/core/benches/runtime_memory.rs`:
  - Starts Core via `test-harness`.
  - Idles for 10 seconds.
  - Reads RSS via `/proc/self/status` on Linux, `mach_task_basic_info` on macOS (use `procfs` and `mach2` crates respectively, or shell out to `ps -o rss= -p <pid>`).
  - Asserts RSS < 100 MB.
- Add a bench `crates/core/benches/runtime_8agents.rs`:
  - Starts Core; spawns 8 `echo`-kind sessions with a long-running echo (use a sleeping loop instead of plain `echo`).
  - Polls RSS.
  - Asserts < 600 MB.
- Add a Vitest perf test in `apps/desktop/`:
  - Cold-start the Desktop in headless mode via Playwright (or Tauri's bundled webdriver if simpler).
  - Measure time-to-first-paint.
  - Assert < 2000 ms.
  - This may be impractical to fully automate in CI for V0.1 — if so, document a manual measurement protocol in `dist/PERF.md` and run it in Task 52's pre-ship checklist.
- Add `.github/workflows/perf.yml` running the benches on macOS and Linux (Desktop perf only on macOS).
- Document the budget table + current-measured values in `dist/PERF.md`.

## Scope — out
- p50 mobile / split-host latency budgets (V1.0 — those subsystems aren't in V0.1).
- LAN streaming throughput (V1.0).
- `gix status` on a 2M-file repo (V1.0 — V0.1 uses the 10k-file fixture from Task 29).
- Continuous perf-tracking dashboards (V2.0).

## Public interface this task locks
- Budget thresholds: 100 MB idle, 600 MB at 8 agents, 2 s cold start, 100 ms `gix status`. Frozen unless explicitly re-baselined.
- Bench locations and CI workflow paths.

## Implementation notes
- For RSS measurement on macOS, use the `mach2 = "0.4"` crate's `task_info` (or shell out to `ps -o rss=`). Document the platform branch.
- The 8-agent bench may stress the test environment heavily — gate it behind a `--ignored` flag and run only in nightly CI rather than every PR. Document.
- For Desktop cold-start: `tauri-driver` is the standard way to run E2E tests against Tauri 2. It's slow to install in CI — a manual measurement protocol may be more practical for V0.1.

## Verification
1. `cargo bench -p concerto-core` (with `--features mem-bench`) → runs and reports RSS within budget on a clean machine.
2. `cargo clippy --workspace -- -D warnings` → clean.
3. CI workflow `perf.yml` runs and gates the build.
4. `dist/PERF.md` lists each V0.1 budget + the most recent measured value.
5. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass.
- [x] CI perf workflow runs green.
- [x] All V0.1 budgets have an automated or documented-manual measurement.
- [x] `dist/PERF.md` exists and is up to date.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/core/Cargo.toml` (modified — feature `mem-bench`, optional deps procfs/mach2)
- `crates/core/benches/runtime_memory.rs` (new)
- `crates/core/benches/runtime_8agents.rs` (new)
- `apps/desktop/perf/coldstart.spec.ts` (new — optional Playwright test)
- `.github/workflows/perf.yml` (new)
- `dist/PERF.md` (new)

## Commit message
```
phase-4: performance budget verification

Criterion benches for Core idle RSS (<100MB) and 8-agent RSS
(<600MB). gix status bench from Task 29 already gates <100ms.
Desktop cold-start measurement (manual protocol if not automatable
in CI). dist/PERF.md tracks current measured values per design/00
§7.7.

Refs: tasks/50-perf-budget-verification.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** RSS measurement uses `ps -o rss= -p <pid>` shell-out on both macOS and Linux (no `procfs` / `mach2` deps) — same KB column semantics on both platforms; documented in `dist/PERF.md` and the bench module docs. Benches gated behind a new `mem-bench` Cargo feature on `concerto-core` plus `required-features` on each `[[bench]]` entry so `cargo test` / `cargo check` are unaffected.
- **Open questions for next task:** Task 52 pre-ship checklist should run the manual Desktop cold-start protocol from `dist/PERF.md` and append the median + sha to the "Recent measurements" table.
- **Deliberate debt:** Desktop cold-start is measured manually per `dist/PERF.md`; the 8-agent bench is treated as `--ignored` (compile-checked in CI, run manually / nightly — `perf.yml` only runs `runtime_memory` on Linux). 2M-file `gix status` bench is V1.0.
- **Smoke-gate state:** unchanged (smoke.sh PASSED post-changes).
