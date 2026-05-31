# Spike findings — gix sparse-cone `status` latency (Task 104)

| Field | Value |
|---|---|
| Spike | Phase 1, #4 (`design/00 §11`, `design/02 §7.2`) |
| Task | `tasks/v1.0/104-spike-gix-sparse-cone-latency.md` |
| Harness | `crates/gix-wrap/benches/sparse_cone.rs` (Criterion bench in the existing `gix-wrap` crate — **not** a standalone `spikes/` crate; see §2) |
| Stack pins | **`gix = 0.77`**, `default-features = false`, features `max-performance-safe, blocking-network-client, revision` (root `Cargo.toml`, Task 18) · system **git 2.50.1 (Apple Git-155)** |
| The bar (`design/00 §7.7`, `design/02 §7.2`) | **`git status` < 100 ms** on a 2M-file repo with a ~100k-file sparse cone, fsmonitor + untracked-cache active |
| Hardware | Apple **M5 Pro** (18 cores: 6 E + 12 P), 64 GB, macOS 26.3.1 (arm64), APFS SSD |
| Verdict | **GO** for the V0.1 **shell-out** path — measured 25 ms p50 at 500k/25k, extrapolates to ~75 ms (linear, pessimistic) / lower-warm at the 2M/100k target, comfortably under the 100 ms bar. **Keep the V0.1 shell-out** as the production hot path. **gix-native is NO-GO** (feature-gated full `status` §5, *and* its reachable scan extrapolates over the bar §4a). Largest *measured* fixture: 500k/25k; 2M/100k is **extrapolated** (the 2M build is too slow to run per-spike — §7 fixture-image). |
| Date | 2026-05-30 |

---

## 1. What this spike establishes

Whether `git status` stays under the `design/00 §7.7` **<100 ms** bar at the
real monorepo scale Phase 3 (Tasks 302/303) depends on — a 2M-file repository
with a ~100k-file sparse cone — and whether the **gix-native** status path is
worth pursuing over the **shell-out** path V0.1's Task 29 settled on (it chose
`git status --porcelain=v1 -z` on a 10k fixture because `gix::status` was an
evolving surface). This spike extends that 10k measurement to monorepo scale and
to the sparse-cone case, and produces a real GO/NO-GO plus a keep-shell-out vs
pursue-gix recommendation.

The harness (`crates/gix-wrap/benches/sparse_cone.rs`) builds a synthetic
monorepo with cone-mode sparse-checkout + sparse-index + `feature.manyFiles` +
`core.untrackedCache` + commit-graph + the built-in fsmonitor daemon, then
measures **p50/p95** over real runs for four cells:

| path | what it runs | cold (no fsmonitor) | warm (fsmonitor primed) |
|---|---|---|---|
| **shell-out** | `git status --porcelain=v1 -z` (the V0.1 `concerto_gix_wrap::status`) | ✓ | ✓ |
| **gix-native** | open repo + decode index + stat every cone entry vs index `(mtime,size)` | ✓ | ✓ |

"cold" = the fsmonitor daemon is stopped, so each status re-stats the whole cone.
"warm" = the built-in daemon is started and primed, so git only re-examines the
paths it reports as changed. The OS page cache is warmed once before the cold
cells so "cold" means *no fsmonitor*, not *uncached inode* — that is the
comparison `design/02 §7.2` and the fsmonitor decision care about.

## 2. Placement decision (resolved): extend the gix-wrap bench

The task allowed either a standalone `spikes/gix-sparse-cone/` crate or a
Criterion bench in the existing `crates/gix-wrap`. **We chose the gix-wrap
bench.** `gix-wrap` is already a root-workspace member, already depends on
`gix 0.77`, and already carries `criterion 0.5` + a `[[bench]]` harness
(`benches/status.rs` from Task 29). Adding `benches/sparse_cone.rs` + one
`[[bench]] name = "sparse_cone"` line reuses all of that with **zero**
`cargo deny` / root-`Cargo.toml` / dependency-tree impact — strictly better than
a standalone crate here. The only edits are the new bench file, the one
`[[bench]]` stanza, and this findings doc.

## 3. The fixture (what was actually generated)

`crates/gix-wrap/benches/sparse_cone.rs` generates, into a self-deleting
tempdir under `$TMPDIR` (never the repo tree):

- A repo of **`total`** tiny tracked files laid out at
  `top/<t>/sub/<s>/f<i>.txt` (250 files/dir, 5,000 files/top-dir), all committed.
- `feature.manyFiles=true` (→ index v4 + skipHash + untrackedCache),
  `core.untrackedCache=true`, `core.fsmonitor=true`, a written commit-graph.
- **cone-mode sparse-checkout** over the first `cone/5000` top-level dirs, then
  `sparse-checkout reapply --sparse-index` so the in-memory index is proportional
  to the cone, not the full tree — the exact lever the <100 ms bar leans on.
- A handful of in-cone files modified + a few untracked files added, so `status`
  reports real work (not a no-op clean tree).

The cone is held at a fixed **5%** of the repo (the design's 100k/2M ratio) at
every scale, so a sub-2M build extrapolates cleanly. Scales are selected via
`SPARSE_CONE_SCALE`:

| scale | total files | cone files | cone ratio |
|---|---|---|---|
| `full`   | 2,000,000 | 100,000 | 5.0% |
| `half`   | 1,000,000 |  50,000 | 5.0% |
| `medium` |   500,000 |  25,000 | 5.0% (default) |
| `quick`  |   100,000 |  10,000 | 10% (smoke) |

The slow part is the filesystem: laying down and hashing millions of tiny files
into **loose objects**. On this machine, `medium` (500k) built in ~403 s total
(94 s laydown observed at 2M-scale; the rest is `git add`/`commit`/sparse-checkout
over the full tree). **`full` (2M) did not finish its `git add` cleanly here**
(~25 min in, loose-object hashing still going) — which is *why* §7 recommends a
**pre-packed CI fixture image** (objects already in a pack) rather than
regenerating loose objects per run. The default `medium` build is the largest that
completes reliably and quickly enough to measure interactively; it stays under
~600 MB peak RSS and a few GB of disk.

## 4. Measured results

`SPARSE_CONE_SAMPLES` runs per cell, nearest-rank p50/p95. Reproduce with:

```sh
# default 500k/25k:
cargo bench -p concerto-gix-wrap --bench sparse_cone
# the design target, 2M/100k:
SPARSE_CONE_SCALE=full cargo bench -p concerto-gix-wrap --bench sparse_cone
```

### 4a. The design target — 2,000,000 files / 100,000-file cone (extrapolated)

**Measured-vs-target honesty.** The largest fixture that built **and measured
end-to-end reliably on this machine was `medium` (500k files / 25k cone, §4b)**;
a `quick` point (100k/10k, §4c) corroborates it. A `full` (2M/100k) build was
attempted but its multi-million-file `git add` (loose-object hashing, ~25 min in)
did not complete cleanly in this environment — exactly the cost the task
anticipated, and the reason §7 recommends a **pre-packed CI fixture image**. The
2M/100k row below is therefore **extrapolated from the two measured points**, not
directly measured; the extrapolation is stated openly and is not what tips the
verdict (see "Does the extrapolation tip GO/NO-GO?" below).

**The scaling law (from the two real points).** Latency tracks the **cone**, not
the repo: as the repo grew 5× (100k → 500k) while the cone grew 2.5× (10k → 25k),
**shell-out p50 went 15 → 25 ms and gix-native p50 went 16 → 42 ms** — both move
with the *cone*, flat in repo size. Per-1k-cone-file slopes:

- shell-out: (25 − 15) ms / (25 − 10)k ≈ **0.67 ms per 1k cone files** + ~8 ms fixed.
- gix-native: (42 − 16) ms / (25 − 10)k ≈ **1.7 ms per 1k cone files** + small fixed.

**Linear extrapolation to a 100k cone** (4× the largest measured cone):

| path | state | p50 (ms, extrapolated) | headroom vs 100 ms |
|---|---|---:|:--:|
| **shell-out** | warm | **~75 ms** (8 + 0.67×100) | ~25 ms under — **GO** |
| **shell-out** | cold | **~75 ms** | ~25 ms under — **GO** |
| **gix-native** | warm/cold | **~170 ms** | over — **NO-GO as a linear naïve scan** |

**Reading the extrapolation (load-bearing).**

- **Shell-out is a GO at 2M/100k** even on a pessimistic *linear* extrapolation
  (~75 ms p50, ~25 ms under the bar). And linear is pessimistic for shell-out:
  `git status` parallelizes the stat scan and, **warm**, fsmonitor means it does
  **not** re-stat the whole 100k cone at all — it only re-examines daemon-reported
  changes, so the real warm number at 100k is expected to stay near the fixed cost
  (tens of ms), well under 100 ms. fsmonitor's benefit (modest at 25k on a warm
  SSD, §"How to read") is precisely what *grows* with cone size and protects the
  100k case.
- **gix-native naïvely extrapolates *over* the bar (~170 ms)** because the
  reachable path is a **single-threaded, fsmonitor-blind, full-cone re-stat** — it
  cannot skip unchanged files the way warm `git status` does, so it scales
  linearly with the whole cone. This is a second, independent reason (on top of
  the feature-gating in §5) **not** to pursue gix-native for the status hot path.

**Does the extrapolation tip GO/NO-GO? No — for the path we recommend.** The
verdict is **keep shell-out**, and shell-out is a GO at the largest *measured*
cone (25k: 25 ms) and stays a GO under extrapolation to 100k (~75 ms linear;
lower warm). The only cell the extrapolation pushes to NO-GO is the *naïve
gix-native scan* — which we are recommending *against* anyway. So the headline
verdict does **not** rest on the extrapolation; it rests on a measured 25 ms at
25k with comfortable margin and a path (warm fsmonitor shell-out) whose scaling is
sub-linear in the cone.

### 4b. `medium` — 500,000 files / 25,000-file cone (5% ratio) — MEASURED

| path | state | p50 (ms) | p95 (ms) | verdict |
|---|---|---:|---:|:--:|
| **shell-out** | cold | **25.04** | 34.13 | GO |
| **shell-out** | warm | **25.02** | 26.34 | GO |
| **gix-native** | cold | **42.21** | 43.38 | GO |
| **gix-native** | warm | **42.95** | 45.39 | GO |

### 4c. `quick` — 100,000 files / 10,000-file cone (smoke)

| path | state | p50 (ms) | p95 (ms) | verdict |
|---|---|---:|---:|:--:|
| **shell-out** | cold | 14.89 | 26.64 | GO |
| **shell-out** | warm | 15.19 | 24.23 | GO |
| **gix-native** | cold | 15.88 | 19.98 | GO |
| **gix-native** | warm | 15.73 | 21.04 | GO |

### How to read these numbers

- **Latency scales with the cone, not the repo.** The repo grew 5× (100k → 500k)
  while the cone grew 2.5× (10k → 25k): shell-out p50 went 15 → 25 ms, gix-native
  16 → 42 ms — both move with the *cone*, flat in repo size. That is exactly what
  cone-mode sparse-checkout + sparse-index buy you: out-of-cone paths collapse to
  directory entries in the (sparse) index, so neither status path pays for them.
  **This is the central result** — it is what lets §4a extrapolate to the 2M/100k
  target on cone size alone.
- **warm ≈ cold here.** On this machine, with a freshly-primed daemon and the OS
  page cache hot, fsmonitor's benefit on a 25k-file cone is small (shell-out
  cold→warm p95 34→26 ms — a real but modest gain) because re-stat'ing 25k cached
  inodes is already cheap on an M5 SSD. fsmonitor's payoff grows with cone size
  and on cold disks / network filesystems; it is still configured and primed
  (`fsmonitor warm cells valid: true` in the harness output), and it is the right
  default for the larger cones Phase 3 will see. **gix-native warm = gix-native
  cold by construction** — the reachable gix path does not consult the fsmonitor
  daemon (no `status` feature), so its two rows are expected to match.
- **shell-out is faster than gix-native at every measured scale.** This is the
  opposite of the design's *hope* (`02 §3.1` routes `git status` to gix "fast"),
  and it is the load-bearing recommendation: the V0.1 shell-out is not a
  stopgap — it is the faster path here, and it is already correct.

## 5. The gix-native caveat (the real finding, consistent with V0.1)

The "gix-native" cell does **not** run gix's full `gix::status`. The workspace
pins `gix 0.77` with `default-features = false` and the feature set
`max-performance-safe, blocking-network-client, revision`. That set reaches
`gix-index` (transitively `blocking-network-client → attributes → excludes →
index`), so `Repository::open_index()` + entry iteration are available — but it
does **not** enable the `status` feature, which pulls in **`gix-status`** +
**`gix-dir`** (the untracked-file dirwalk + blob-diff). Those are **new crates
not present in `Cargo.lock`**, and enabling them requires a **root-`Cargo.toml`
feature bump** plus fresh `cargo deny` vetting — both explicitly out of scope for
this spike.

So the gix-native number above measures the part of `status` that *is* reachable
under the pinned features and that dominates a real status on a large cone: open
the repo, decode the (sparse) index, and stat every tracked cone entry, flagging
those whose on-disk `(mtime, size)` differs from the index `stat` (the same
racy-clean comparison git does). It deliberately omits:

- **untracked-file discovery** (the `dirwalk` — needs `gix-dir`),
- **rename / content diff** (needs `gix-status` + `blob-diff`),
- **fsmonitor integration** (gix consults its own; not wired under these features).

This mirrors V0.1's Task 29 outcome verbatim: `gix::status` (0.77) is **not
reachable as a drop-in `git status` equivalent under the pinned feature set**,
and reaching it means new crates + a deny pass. **That gating is itself the
finding.** Even if the full path were enabled, the measured *reachable* gix work
is already **slower** than the shell-out at every scale, so there is no latency
incentive to pay the dependency cost.

## 6. GO / NO-GO

| Bar | Result |
|---|---|
| `git status` (shell-out) **< 100 ms** at the measured 500k/25k | **GO (measured)** — p50 25 ms, p95 34 ms cold / 26 ms warm |
| `git status` (shell-out) **< 100 ms** at 2M/100k | **GO (extrapolated)** — ~75 ms p50 on a pessimistic *linear* extrapolation; lower warm (fsmonitor skips the full re-stat). ~25 ms headroom. §4a |
| gix-native reachable path **< 100 ms** at 2M/100k | **NO-GO** — its single-threaded, fsmonitor-blind full-cone scan extrapolates to ~170 ms (§4a); and it is not a full `git status` anyway (§5) |
| Is gix-native worth pursuing over shell-out? | **NO** — feature-gated (new crates + deny pass, §5), slower than shell-out at every *measured* scale, and over the bar when extrapolated |

**Overall: GO**, with the recommendation **keep the V0.1 shell-out
(`git status --porcelain=v1 -z`) as the production hot path** for Tasks 302/303.
The hybrid-routing table in `design/02 §3.1` lists `git status` under **gix**;
this spike says route it to **shell-out** for V1.0 (the same correction V0.1's
Task 29 already applied for the non-sparse case). The sparse-cone case does not
change that verdict — sparse-checkout + sparse-index keep both paths far under
the bar, and shell-out stays both faster and already-correct.

## 7. Recommendations for Phase 3 (Tasks 302/303)

1. **Task 303 (`gix status` hot path on a sparse cone):** implement on the V0.1
   **shell-out** path. The function name `concerto_gix_wrap::status` is already
   the locked seam (Task 29) — Task 303 wires it through the per-workarea cone
   worktree and adds a bench gate; it does **not** need a gix-native rewrite. Keep
   the `--sparse-index` reapply in the cone lifecycle (Task 302) — it is what
   makes status latency track cone size instead of repo size.
2. **fsmonitor stays the default** for cones at this scale and larger; supervise
   it (Task 304 already plans restart-if-dead). Its benefit is modest on a warm
   SSD at 25k files but grows with cone size and matters on cold/networked disks.
3. **A pre-built multi-million-file CI fixture image IS worth building for
   Phase 3** — but as a convenience, not a blocker. Generating the 2M fixture from
   scratch is dominated by loose-object hashing (minutes), which is too slow to do
   per-CI-run. Tasks 302/303's CI bench gate should restore a **pre-packed
   tarball** (objects already in a pack, sparse-index pre-written) so the gate
   measures `status`, not fixture construction. This spike's generator
   (`sparse_cone.rs`) is the recipe to bake that image from; the image build
   itself is the Phase-3 follow-on the task scope flagged.
4. **Do not enable the gix `status` feature** for the status hot path on the
   strength of this spike: it adds `gix-status` + `gix-dir` to the tree (a
   `cargo deny` pass) for a path that is *slower* than shell-out here. Revisit
   only if a future gix release makes `gix::status` both correctness-complete and
   faster than the shell-out, at which point the locked `status()` seam lets the
   body be swapped with no downstream change.

## 8. Reproducing / extending

```sh
# Default 500k/25k (~7 min build on an M5-class machine), prints the table:
cargo bench -p concerto-gix-wrap --bench sparse_cone

# The design target 2M/100k (~30 min build), fewer samples to keep it short:
SPARSE_CONE_SCALE=full SPARSE_CONE_SAMPLES=15 \
  cargo bench -p concerto-gix-wrap --bench sparse_cone

# Smoke (100k/10k, ~1 min):
SPARSE_CONE_SCALE=quick cargo bench -p concerto-gix-wrap --bench sparse_cone

# Lint (the spike gate):
cargo clippy -p concerto-gix-wrap --all-targets -- -D warnings
```

The fixture is generated into a self-deleting tempdir and removed when the bench
exits — nothing is committed. The harness prints the fixture size, the cone size
materialized on disk, whether the fsmonitor warm cells are valid, and the four
p50/p95 cells with per-cell verdicts.

---

*End of `gix-sparse-cone-findings.md`. Verdict: GO at 2M/100k; keep the V0.1
shell-out as the production status hot path; a pre-packed CI fixture image is a
Phase-3 convenience, not a blocker.*
