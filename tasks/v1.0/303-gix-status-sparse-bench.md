# Task 303 — `git status` Hot Path on a Sparse Cone + Bench Gate (builds on spike 104)

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | 104, 302 |
| Touches subsystem(s) | 02 (Repository Manager) |
| Smoke gate | unchanged |

## Goal
Wire the FROZEN V0.1 shell-out `concerto_gix_wrap::status` seam through a per-(workarea, repo) **sparse cone** worktree (the one Task 302 materializes) and add a **Criterion bench gate** that proves `git status` stays under the `design/00 §7.7` **<100 ms** bar at sparse-cone scale — without a `gix`-native rewrite. Spike 104 (`design/spikes/gix-sparse-cone-findings.md`) already returned **GO for shell-out** (25 ms p50 measured at 500k/25k, ~75 ms extrapolated at 2M/100k) and **NO-GO for gix-native** (feature-gated + slower). Today `crates/gix-wrap/src/status.rs::status()` runs `git status --porcelain=v1 -z` against an arbitrary worktree path, and the spike's `crates/gix-wrap/benches/sparse_cone.rs` builds synthetic fixtures but is a throwaway spike harness (CI only `--no-run`-checks it). This task: (a) confirms `status()` is invoked against the sparse-cone worktree (it already takes a `&Path`; the work is ensuring 302's `--sparse-index` reapply stays in the lifecycle so latency tracks cone size); (b) promotes a **status bench gate** from the spike harness that restores a **pre-packed fixture tarball** (objects already in a pack, sparse-index pre-written) so the gate measures `status`, not fixture construction (spike §7 rec 3); (c) asserts `<100 ms p50` against the design bar. After this task the status hot path has a regression gate; the real 2M-file-monorepo number is a Phase-3 Tier-3 checklist line.

## Inputs to read before starting
- `design/spikes/gix-sparse-cone-findings.md` — **the entire doc**: the GO/NO-GO verdict, §4 measured numbers (25 ms p50 shell-out at 500k/25k; ~75 ms extrapolated at 2M/100k), §4a the cone-not-repo scaling law, §5 the gix-native feature-gating caveat (why NOT to enable `gix status`), and §7 the four Phase-3 recommendations — **rec 1 (shell-out, no rewrite), rec 2 (fsmonitor stays default), rec 3 (pre-packed CI fixture image), rec 4 (do not enable the gix `status` feature)** are this task's marching orders.
- `design/02_Repository_Manager.md` §3.1 — the **V1.0 amendment (2026-06-02)**: `git status` stays shell-out (`git status --porcelain=v1 -z`), NOT gix-native, per spike 104. Read the `git status` row's footnote in full.
- `design/02_Repository_Manager.md` §7.2 — the hot-path sequence (read the diagram's `gix` participant as "the status backend selected by §3.1," which for V1.0 is shell-out, per the §7.2 amendment box). Target: `<100 ms` on a 2M-file repo with a 100k-file sparse cone.
- `design/00_Architecture_Overview.md` §7.7 — the performance budget table: `gix status on 2M-file repo with sparse cone < 100 ms` (the literal bar the gate asserts against).
- `crates/gix-wrap/src/status.rs` — the **FROZEN `pub async fn status(worktree_path: &Path) -> Result<StatusReport>`** seam (Task 29). Do NOT change its signature or body backend; 303 only wires + benches it. Note it already shells out + parses porcelain v1 `-z`.
- `crates/gix-wrap/benches/sparse_cone.rs` (the spike harness) + `crates/gix-wrap/benches/status.rs` (Task 29's status bench) + `crates/gix-wrap/Cargo.toml` (the two `[[bench]]` stanzas + `criterion 0.5` dev-dep). The spike harness's fixture *generator* is the recipe to bake the pre-packed image from (`sparse_cone.rs` §3 in the findings doc describes the layout: `top/<t>/sub/<s>/f<i>.txt`, cone-mode + sparse-index + `feature.manyFiles` + `core.untrackedCache` + commit-graph).
- `tasks/v1.0/302-sparse-cone-lifecycle.md` → "Handoff Notes" — the sparse-cone worktree layout + the `--sparse-index` reapply guarantee (303 depends on 302 keeping reapply in the lifecycle).
- `tasks/v1.0/104-spike-gix-sparse-cone-latency.md` → "Handoff Notes" — the spike's own notes on fixture build cost (2M loose-object `git add` does not finish quickly → the pre-packed image is mandatory for a CI gate).
- `tasks/v1.0/PHASE3_PLANNING.md` §2 (303 row: **keep the V0.1 shell-out `status()` seam** — spike-104 GO; 303 wires it through a per-workarea cone + adds a Criterion bench gate; **no gix-native rewrite**).

## Scope — in
- **Confirm + lock the wiring:** `status()` is called against the (workarea, repo) sparse-cone worktree (the path 302 produces). If the Repo-Mgr / Workspace-Mgr status read path does not yet route through the sparse worktree, wire it (the function already takes a `&Path` — the change is in the caller, not `status()`). Add a small integration test that `status()` on a sparse-cone worktree returns only in-cone changes.
- **The bench gate:** a Criterion bench `crates/gix-wrap/benches/status_sparse_gate.rs` (or fold a `gate` group into the existing `sparse_cone.rs`) that:
  - restores a **pre-packed fixture tarball** (objects in a pack, sparse index pre-written) from a checked-in/CI-fetched artifact, rather than regenerating loose objects per run (spike §7 rec 3). The fixture has a known sparse cone with a handful of in-cone modifications + untracked files so `status` reports real work.
  - measures p50 over N runs of `concerto_gix_wrap::status(cone_worktree)`.
  - **asserts `<100 ms p50`** (the §7.7 bar). On regression the bench fails the gate (Criterion alone does not fail on a threshold — add an explicit `assert!(p50 < Duration::from_millis(100))` after the measurement, or a tiny harness that runs the closure, computes nearest-rank p50, and exits non-zero over budget).
- **The pre-packed fixture image:** a build step (a script + the spike generator) that produces the tarball once and a documented way to restore it in CI. The image build itself was flagged by the spike as a Phase-3 follow-on; this task owns it (or a small `quick`-scale fixture if a full 2M image is impractical to commit — then the gate runs at the largest scale that fits CI time/size, and the 2M number stays Tier-3, documented).
- A `[[bench]]` stanza for the new gate bench (if a separate file) in `gix-wrap/Cargo.toml`.

## Scope — out
- **Any `gix`-native status rewrite** — explicitly NO-GO (spike §5/§6/§7 rec 4). Do not enable the `gix` `status` feature (it adds `gix-status` + `gix-dir` → a `cargo deny` pass, for a *slower* path).
- **Changing the `status()` signature or backend** — FROZEN (Task 29). 303 wires + benches only.
- **The sparse-cone lifecycle itself** (`sparse_init_cone`/`set`/`reapply --sparse-index`) — **Task 302**. 303 depends on 302's reapply staying in place but does not implement it.
- **fsmonitor supervision / prewarm** — **Task 304** (the spike notes fsmonitor stays default; 304 supervises it).
- **The real 2M-file-monorepo confirmation on real hardware** — **Tier-3** Phase-3 checklist; the CI gate runs against the pre-packed image at the largest scale that fits CI.
- **Generating 2M loose objects per CI run** — forbidden (minutes-long; the whole point of the pre-packed image).

## Public interface this task locks
- **No new public Rust/proto surface.** `status()` stays FROZEN; this task adds a bench + a fixture, neither of which is a library API. The locked artifacts are: the bench name (`status_sparse_gate` or the `gate` group in `sparse_cone`), the `<100 ms p50` threshold assertion, and the pre-packed fixture format (a tarball of a git repo with a written sparse index + packed objects + a documented cone). FREEZE the threshold (100 ms p50, per `design/00 §7.7`) and the fixture-restore contract in the bench's doc-comment.

## Implementation notes
- **`--sparse-index` is the whole game.** The gate only stays under budget because the sparse index collapses out-of-cone paths to directory entries (spike §4a). Assert in the fixture-build that the restored repo's index is a *sparse* index (`git ls-files --sparse` shows directory entries, or `test -f .git/index` + the index version/extension check) — a fixture that lost its sparse index would silently make the gate measure full-repo status and either fail spuriously or (worse) pass against the wrong thing.
- **fsmonitor in the bench:** the spike ran cold (daemon stopped) and warm (daemon primed) cells. For a deterministic CI gate, run **cold** (fsmonitor stopped) so the number is reproducible and pessimistic — cold shell-out was already 25 ms at 25k / ~75 ms extrapolated at 100k, both under 100 ms. Document that warm is faster and is what production uses; the gate proves the floor.
- **CI must not regenerate the fixture.** Restore the tarball (decompress + `git` is happy with the pre-written index). If the 2M image is too large to commit, use Git LFS or a CI cache, OR ship a smaller `medium`/`quick`-scale committed fixture and assert the *scaled* budget, documenting that the 2M number is corroborated at the Tier-3 checklist (the spike's extrapolation already covers it). Pick the option that keeps the repo + CI healthy and FREEZE the choice in the bench doc-comment.
- **`cargo deny` must stay green** (`rust` §5.3) — no new crates. The gate uses the existing `criterion 0.5` dev-dep (or a hand-rolled timing loop with no new dep). Do NOT pull in `gix-status`/`gix-dir`.
- **Cross-platform:** the bench fixture + `git status` shell-out must work on the Windows + Linux CI lanes (Task 113). A pre-packed tarball restores identically on every OS; sparse-checkout + `--sparse-index` are git-version features present in the matrix. If the fixture build (not restore) is mac-only, gate the *build* script behind an OS check and ship the restore path cross-platform.
- The bench is a **gate**, not just a benchmark: CI must observe a failure when p50 regresses over 100 ms. Decide how the orchestrator invokes it — either a dedicated `cargo bench --bench status_sparse_gate` step that exits non-zero over budget (the harness does the assertion + `std::process::exit`), or a `#[test]`-shaped gate that runs the timing loop under `cargo test`. Prefer the test-shaped gate so the standard `cargo test --workspace` run enforces it; document the choice.

## Verification
Tier 1. The `rust` §5.3 set + the bench gate.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean (the new bench is `--all-targets`; keep it warning-clean).
3. `cargo test --workspace --no-fail-fast` → all pass, INCLUDING the status-on-sparse-cone integration test and (if test-shaped) the `<100 ms p50` gate.
4. `cargo bench -p concerto-gix-wrap --bench status_sparse_gate` (or the `gate` group) → measures status on the restored pre-packed sparse-cone fixture, prints p50/p95, and **exits non-zero if p50 ≥ 100 ms** (the gate assertion). At the committed fixture scale this passes with margin (spike: 25 ms at 25k cone).
5. `cargo deny check` → green (no new crates; gix `status` feature NOT enabled).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → **no change** (303 adds no public API).
7. `scripts/smoke.sh` → **unchanged** (303 touches no smoke capability).

**Tier-1 scope + what it does NOT cover.** The CI gate proves `status` stays under 100 ms p50 on the **pre-packed fixture** at the committed scale, cold. It does **not** prove the number on a **real 2M-file monorepo on real hardware** — that is the Phase-3 Tier-3 checklist (corroborated by spike 104's extrapolation, ~75 ms p50 linear). The bench's scaled assertion + the spike doc together back the bar; the live confirmation is the operator's at the phase gate.

## Definition of Done
- [ ] `status()` confirmed/wired through the (workarea, repo) sparse-cone worktree; integration test asserts in-cone-only status
- [ ] No `gix`-native rewrite; the `gix` `status` feature is NOT enabled; `cargo deny` green
- [ ] A `<100 ms p50` bench gate over a **pre-packed** sparse-cone fixture (no per-run loose-object regen); fails non-zero on regression
- [ ] The fixture asserts its index is a sparse index; the cone + restore contract documented in the bench doc-comment
- [ ] Bench runs cold (deterministic floor); warm-is-faster + the 2M-is-Tier-3 caveat documented
- [ ] All Verification commands pass on a clean checkout; no interface change
- [ ] No TODO/FIXME/unimplemented!()/todo!() in new code
- [ ] No files outside Outputs modified
- [ ] Single commit with the message below

## Outputs
- `crates/gix-wrap/benches/status_sparse_gate.rs` (new — the bench gate) OR `crates/gix-wrap/benches/sparse_cone.rs` (modified — add a `gate` group); `crates/gix-wrap/Cargo.toml` (modified — `[[bench]]` stanza if a new file)
- `crates/gix-wrap/src/status.rs` (UNCHANGED — verify; only its caller wiring changes)
- `crates/core/src/repo_manager/actor.rs` and/or the workspace-status read path (modified only if the status read is not already routed through the sparse worktree)
- `crates/gix-wrap/tests/status_sparse.rs` (new — status-on-sparse-cone integration test) and/or the `<100 ms p50` test-shaped gate
- `scripts/build-sparse-fixture.sh` (new — bakes the pre-packed tarball from the spike generator) + the committed/CI-fetched fixture artifact (path documented)
- (no `docs/interfaces/` change)

## Commit message
```
phase-3: gix status sparse-cone wiring + <100ms bench gate

Wires the FROZEN shell-out status() seam through the per-workarea sparse
cone (Task 302) and adds a Criterion bench gate over a pre-packed
sparse-index fixture asserting <100ms p50 (design/00 §7.7). No gix-native
rewrite (spike 104 NO-GO); the gix status feature stays off. Real 2M-file
monorepo confirmation is the Tier-3 checklist line.

Refs: tasks/v1.0/303-gix-status-sparse-bench.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan / Open questions for next task / Deliberate debt / Smoke-gate state —
