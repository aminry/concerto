# Concerto — V0.1 performance budgets

Source of truth: `design/00 §7.7`. This document tracks the V0.1-relevant
subset, how each budget is measured, and the most recent number observed.
Budgets are FROZEN unless explicitly re-baselined.

Tasks 29 and 50 jointly add the automation. Anything labelled "manual"
must be measured locally before each release ship (Task 52 pre-flight
checklist).

## V0.1 budget table

| Metric | Budget | How measured | Last measured | Where |
|---|---|---|---|---|
| Core idle RSS (0 agents, 10 s after boot) | < 100 MB | `cargo bench -p concerto-core --features mem-bench --bench runtime_memory` | TBD on first nightly run | `crates/core/benches/runtime_memory.rs` + `.github/workflows/perf.yml` `idle-rss` job |
| Core RSS at 8 active agents (peak burst) | < 600 MB | `cargo bench -p concerto-core --features mem-bench --bench runtime_8agents` (manual / nightly) | TBD on first manual run | `crates/core/benches/runtime_8agents.rs` |
| `gix status` on 10k-file repo (V0.1 fixture) | < 100 ms p50 | `cargo bench -p concerto-gix-wrap` | See Task 29 results | `crates/gix-wrap/benches/status.rs` |
| Desktop client cold start | < 2 s to first paint | Manual measurement (protocol below) | TBD per release | `dist/PERF.md` §"Desktop cold-start protocol" |

V1.0-only budgets (split-host RTT, LAN streaming, 2M-file `gix status`)
are explicitly out of V0.1 scope per Task 50.

## How to run the automated benches locally

```sh
# Idle Core (≈3 min wall):
cargo bench -p concerto-core --features mem-bench --bench runtime_memory

# 8-agent burst (heavy; ≈5 min wall; spawns 9 subprocesses per
# iteration). Treat as `--ignored` semantics — manual / nightly only:
cargo bench -p concerto-core --features mem-bench --bench runtime_8agents

# gix status hot path (Task 29; builds a 10k-file fixture, ≈2 min wall):
cargo bench -p concerto-gix-wrap
```

Each bench prints a `CONCERTO_PERF <metric>_kb=<value> budget_kb=<budget>`
line per iteration. The bench itself `assert!`s the budget — `cargo
bench` fails with a panic if the live measurement exceeds the locked
ceiling. CI greps for the line as a second-layer check that the print
itself fired (so a silent regression where the println is dropped can't
slip through).

## Why `ps -o rss=` rather than `procfs` / `mach2`

Per Task 50 pre-decision 1. `procfs` is Linux-only; `mach2` is
Darwin-only. Shelling out to `ps -o rss= -p <pid>` works on both hosts
without a platform branch, and the column is reported in 1 KiB units on
both platforms — same semantics, single integer parse. The cost is one
fork-per-sample, which is negligible against the multi-second settle
intervals the benches sleep through.

## CI gating

`.github/workflows/perf.yml`:

- `bench-compile`: macOS + Linux, validates `cargo bench --no-run -p
  concerto-core --features mem-bench` on every PR that touches the
  bench surface.
- `idle-rss`: Linux only, runs `runtime_memory` and asserts the
  `CONCERTO_PERF idle_rss_kb=` line was printed (the bench's internal
  `assert!` does the budget gate itself).
- `runtime_8agents` is **not** invoked in routine CI. It runs manually
  or in a nightly job — see Task 50 spec §"Implementation notes".

Per Task 50: `gix status` (Task 29) is covered by `bench.yml`, which
runs `cargo bench --no-run -p concerto-gix-wrap` on every PR. Full
status timings are measured manually — the 10k-file fixture takes
several minutes to build.

## Desktop cold-start protocol (manual)

Per Task 50 pre-decision 4 + spec §"This may be impractical to fully
automate": Playwright / tauri-driver automation is brittle and slow to
install in CI for V0.1. Measure manually before each release ship using
the procedure below; record the result in this file alongside the
release tag.

### Steps

1. Build the Desktop in release mode:
   ```sh
   cd apps/desktop && pnpm install --frozen-lockfile && pnpm tauri build
   ```
2. Quit any running Concerto Desktop instance and wait 30 s so the OS
   evicts the binary's page cache (so the measurement reflects a true
   cold start, not a warm relaunch).
3. Open Activity Monitor / `top` in a side window to observe process
   creation, then double-click the built `.app` bundle (macOS) or run
   the binary (Linux).
4. Start a stopwatch when the dock icon appears (macOS) or the binary
   is invoked (Linux); stop it when the first window paint completes
   (the three-panel layout from Task 49 is visible and the loading
   placeholder has cleared).
5. Repeat 3 times after a 30 s wait between runs. Record the median.

### Budget

Median time-to-first-paint < **2000 ms**. If a release run exceeds the
budget, file a perf regression issue against the Phase 3 desktop tasks
(48, 49, 50, 51, 52) and block the ship until resolved or the budget is
explicitly re-baselined with a follow-up task.

### Reporting

Add a row to the "Recent measurements" log below with the date, the
commit SHA, the hardware (model + chip + RAM), and the three samples
plus the median. Treat the log as append-only.

## Recent measurements

| Date | SHA | Bench | Host | Samples (KB) | Median | Status |
|---|---|---|---|---|---|---|
| TBD | TBD | idle Core RSS | TBD | TBD | TBD | TBD |
| TBD | TBD | 8-agent RSS | TBD | TBD | TBD | TBD |
| TBD | TBD | Desktop cold start | TBD | TBD ms | TBD ms | TBD |
