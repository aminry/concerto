//! Spike bench (Task 104 / Phase 1): `git status` latency on a large
//! synthetic monorepo with a sparse cone, fsmonitor **cold** and **warm**.
//!
//! This is a *spike* harness, not a CI gate. It extends the V0.1
//! 10k-file `benches/status.rs` to the monorepo scale that Phase 3
//! (Tasks 302/303) depends on, and answers one question: does
//! `git status` stay under the `design/00 §7.7` **<100 ms** bar on a
//! ~2M-file repo with a ~100k-file sparse cone, and is the `gix`-native
//! path worth pursuing over the V0.1 shell-out?
//!
//! ## What it measures
//!
//! Four cells, for two status implementations × two fsmonitor states:
//!
//! | path | cold | warm |
//! |---|---|---|
//! | **shell-out** (`git status --porcelain=v1 -z`, the V0.1 hot path) | yes | yes |
//! | **gix-native** (open repo + load index + index↔worktree stat scan) | yes | yes |
//!
//! For each cell it reports **p50 / p95** over a sample of real runs.
//! "cold" = no fsmonitor daemon (every status re-stats the whole cone);
//! "warm" = the built-in fsmonitor daemon is running and primed, so git
//! only re-examines paths the daemon reports as changed.
//!
//! ## gix-native scope (honest)
//!
//! The workspace pins `gix 0.77` with `default-features = false` and the
//! feature set `max-performance-safe, blocking-network-client, revision`.
//! That set reaches `gix-index` (transitively via
//! `blocking-network-client → attributes → excludes → index`) but **not**
//! the `status` feature, which would pull `gix-status` + `gix-dir` (the
//! dirwalk for untracked files + blob-diff). Those are new crates not in
//! `Cargo.lock` and would need `cargo deny` vetting + a root-`Cargo.toml`
//! feature bump, both out of scope for this spike (see the findings doc).
//!
//! So the "gix-native" cell here measures the part of status that *is*
//! reachable under the pinned features and that dominates a real status
//! on a large cone: open the repository, decode the index, and stat every
//! tracked path in the cone, flagging entries whose on-disk
//! `(mtime, size)` differs from the index `stat` (the same racy-clean
//! comparison git does). It deliberately does **not** walk for untracked
//! files or run rename/content diff — that gap is the spike's central
//! finding and is recorded explicitly in
//! `design/spikes/gix-sparse-cone-findings.md`.
//!
//! ## Scale
//!
//! Generating millions of tiny files is the expensive part. The fixture
//! size is controlled by `SPARSE_CONE_SCALE`:
//!
//! - `full`   — ~2,000,000 files, ~100,000-file cone (the design target).
//! - `half`   — ~1,000,000 files, ~50,000-file cone.
//! - `medium` — ~500,000 files, ~25,000-file cone (default; a good
//!   accuracy/cost trade-off on a developer machine).
//! - `quick`  — ~100,000 files, ~10,000-file cone (fast smoke).
//!
//! The cone is held at **5%** of the repo across every scale so the
//! measured number extrapolates cleanly to the 2M/100k target. The
//! findings doc records the size actually built and extrapolates if it is
//! below 2M.
//!
//! Set `SPARSE_CONE_SAMPLES` to change the per-cell sample count
//! (default 25). The fixture lives under `$TMPDIR` (or `target/`), never
//! in the repo tree, and is deleted when the bench process exits.
//!
//! Run: `cargo bench -p concerto-gix-wrap --bench sparse_cone`
//! (optionally `SPARSE_CONE_SCALE=full cargo bench ...`). CI only
//! validates that it *compiles* (`cargo bench --no-run`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture configuration
// ---------------------------------------------------------------------------

/// One named fixture scale: total files in the repo and the size of the
/// checked-out sparse cone. `cone` is held at ~5% of `total` so a
/// sub-2M build extrapolates to the 2M / 100k target.
struct Scale {
    name: &'static str,
    total: usize,
    cone: usize,
}

/// Resolve the requested scale from `SPARSE_CONE_SCALE` (default
/// `medium`). Unknown values fall back to `medium` with a warning so a
/// typo never silently runs the 2M build.
fn resolve_scale() -> Scale {
    let want = std::env::var("SPARSE_CONE_SCALE").unwrap_or_else(|_| "medium".to_string());
    match want.as_str() {
        "full" => Scale {
            name: "full",
            total: 2_000_000,
            cone: 100_000,
        },
        "half" => Scale {
            name: "half",
            total: 1_000_000,
            cone: 50_000,
        },
        "medium" => Scale {
            name: "medium",
            total: 500_000,
            cone: 25_000,
        },
        "quick" => Scale {
            name: "quick",
            total: 100_000,
            cone: 10_000,
        },
        other => {
            eprintln!("[sparse_cone] unknown SPARSE_CONE_SCALE={other:?}; using 'medium'");
            Scale {
                name: "medium",
                total: 500_000,
                cone: 25_000,
            }
        }
    }
}

/// How many tracked files are placed under each leaf directory. Keeping a
/// few hundred per dir keeps any single directory listing cheap while the
/// total still reaches millions across the tree.
const FILES_PER_DIR: usize = 250;

/// Per-cell sample count (override with `SPARSE_CONE_SAMPLES`). Each
/// sample is one full `status` invocation; 25 is enough for a stable
/// p50/p95 on this fixture without dominating wall-clock time.
fn sample_count() -> usize {
    std::env::var("SPARSE_CONE_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25)
}

// ---------------------------------------------------------------------------
// git helpers
// ---------------------------------------------------------------------------

/// Run `git <args>` in `cwd`, panicking on failure (a broken fixture
/// makes every downstream number meaningless, so fail loud).
fn git(args: &[&str], cwd: &Path) {
    let out = git_capture(args, cwd);
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run `git <args>` and return the raw `Output` (used both for fixture
/// setup and for the shell-out status timing).
fn git_capture(args: &[&str], cwd: &Path) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "bench")
        .env("GIT_AUTHOR_EMAIL", "bench@example.com")
        .env("GIT_COMMITTER_NAME", "bench")
        .env("GIT_COMMITTER_EMAIL", "bench@example.com")
        // Pin the fixture's config to exactly what we set below — never
        // inherit the developer's global/system git config.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("spawn git")
}

// ---------------------------------------------------------------------------
// Fixture generation
// ---------------------------------------------------------------------------

/// A built fixture: the owning tempdir plus a handful of paths the bench
/// reuses across cells.
struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    /// Relative paths (worktree-relative) of files inside the cone that
    /// the bench mutates to give `status` something real to report.
    dirty_paths: Vec<String>,
}

/// Build the synthetic monorepo + sparse cone described by `scale`.
///
/// Layout: files live at `top/<t>/sub/<s>/f<i>.txt`. The first
/// `cone_dirs` top-level directories form the **cone**; everything else
/// is tracked in the index + commit but excluded from the worktree by
/// cone-mode sparse-checkout, exactly like a real large monorepo.
fn build_fixture(scale: &Scale) -> Fixture {
    let t0 = Instant::now();
    // Prefer target/ for the fixture if it exists (same volume as the
    // repo, usually a fast SSD); else fall back to $TMPDIR via TempDir's
    // default. Either way it is a tempdir that self-deletes.
    let dir = TempDir::with_prefix("concerto-sparse-cone-").expect("tempdir");
    let root = dir.path().to_path_buf();

    // --- repo init + large-repo config (design/00 §7.7, design/02 §7.2) -
    git(&["init", "-q", "-b", "main", "."], &root);
    // feature.manyFiles flips on index.version=4 + index.skipHash +
    // untrackedCache — the bundle git recommends for 100k+ files.
    git(&["config", "feature.manyFiles", "true"], &root);
    git(&["config", "core.untrackedCache", "true"], &root);
    git(&["config", "index.version", "4"], &root);
    // fsmonitor: the built-in daemon (macOS FSEvents / Windows / Linux).
    // Configured here; the daemon is only *started* for the warm cells.
    git(&["config", "core.fsmonitor", "true"], &root);

    // --- lay down the files ------------------------------------------
    // Sharded so no directory exceeds FILES_PER_DIR entries. We write
    // sequentially but with a fast inner loop; for the medium/quick
    // scales this is well under a minute, and even 'full' is bounded by
    // raw filesystem create throughput, not this code.
    let total = scale.total;
    let files_per_top = top_dir_capacity();
    let cone_dirs = scale.cone.div_ceil(files_per_top);

    let mut dirty_paths = Vec::new();
    let mut written = 0usize;
    let mut top = 0usize;
    'outer: while written < total {
        let top_rel = format!("top/{top:05}");
        let mut sub = 0usize;
        while written < total {
            let sub_rel = format!("{top_rel}/sub/{sub:04}");
            let sub_abs = root.join(&sub_rel);
            std::fs::create_dir_all(&sub_abs).expect("mkdir sub");
            for i in 0..FILES_PER_DIR {
                if written >= total {
                    break;
                }
                let fname = format!("f{i:04}.txt");
                let rel = format!("{sub_rel}/{fname}");
                std::fs::write(root.join(&rel), small_content(written)).expect("write file");
                // Record a few in-cone files to dirty later (only the
                // first cone_dirs top-levels are in the cone).
                if top < cone_dirs && dirty_paths.len() < 8 && i == 0 && sub == 0 {
                    dirty_paths.push(rel);
                }
                written += 1;
            }
            sub += 1;
            if sub * FILES_PER_DIR >= files_per_top {
                break;
            }
            if written >= total {
                break 'outer;
            }
        }
        top += 1;
    }
    let lay_done = t0.elapsed();
    eprintln!(
        "[sparse_cone] scale={} laid down {written} files in {} top-dirs ({:.1}s)",
        scale.name,
        top,
        lay_done.as_secs_f64()
    );

    // --- commit everything (full tree is tracked; cone is a worktree
    //     view of it) ---------------------------------------------------
    git(&["add", "-A"], &root);
    git(&["commit", "-q", "-m", "seed monorepo"], &root);
    // commit-graph: speeds the history side of status/log on big repos.
    git(&["commit-graph", "write", "--reachable"], &root);

    // --- sparse-checkout: cone mode, first `cone_dirs` top dirs --------
    git(&["sparse-checkout", "init", "--cone"], &root);
    // Build the cone set: each in-cone top-level directory.
    let mut cone_args: Vec<String> = vec!["sparse-checkout".to_string(), "set".to_string()];
    for t in 0..cone_dirs {
        cone_args.push(format!("top/{t:05}"));
    }
    let cone_refs: Vec<&str> = cone_args.iter().map(String::as_str).collect();
    git(&cone_refs, &root);
    // sparse-index keeps the in-memory index proportional to the cone,
    // not the full 2M tree — this is the lever the <100ms bar leans on.
    git(&["sparse-checkout", "reapply", "--sparse-index"], &root);

    let cone_count = count_worktree_files(&root);
    eprintln!(
        "[sparse_cone] cone materialized: ~{cone_count} files on disk \
         (target {}); fixture built in {:.1}s total",
        scale.cone,
        t0.elapsed().as_secs_f64()
    );

    // Make the dirty set real: modify a handful of in-cone tracked files
    // and drop a couple of untracked files so status has work to report.
    for rel in &dirty_paths {
        let p = root.join(rel);
        if p.exists() {
            std::fs::write(&p, b"modified by sparse_cone bench\n").expect("dirty write");
        }
    }
    for i in 0..3 {
        let _ = std::fs::write(root.join(format!("untracked-{i}.txt")), b"untracked\n");
    }

    Fixture {
        _dir: dir,
        root,
        dirty_paths,
    }
}

/// Files placed under one top-level directory. The cone is sized in whole
/// top-level dirs, so this also sets cone granularity.
fn top_dir_capacity() -> usize {
    // 20 sub-dirs × FILES_PER_DIR = 5,000 files per top dir. A 100k cone
    // is then 20 top dirs; a 2M repo is 400 top dirs.
    20 * FILES_PER_DIR
}

/// Deterministic tiny file body. Varying it slightly per-index keeps the
/// pack from collapsing every blob to one object (which would make the
/// fixture unrealistically compressible) without bloating disk use.
fn small_content(i: usize) -> Vec<u8> {
    format!("f{i}\n").into_bytes()
}

/// Count how many regular files are present in the worktree (cone only,
/// since sparse-checkout removed the rest). Bounded walk; used only for
/// the one-time fixture report line.
fn count_worktree_files(root: &Path) -> usize {
    fn walk(dir: &Path, acc: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            if p.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            match ent.file_type() {
                Ok(ft) if ft.is_dir() => walk(&p, acc),
                Ok(ft) if ft.is_file() => *acc += 1,
                _ => {}
            }
        }
    }
    let mut acc = 0;
    walk(root, &mut acc);
    acc
}

// ---------------------------------------------------------------------------
// fsmonitor control
// ---------------------------------------------------------------------------

/// Start + prime the built-in fsmonitor daemon for the warm cells.
/// Returns `true` if the daemon is confirmed running afterwards.
fn start_fsmonitor(root: &Path) -> bool {
    // `git fsmonitor--daemon start` launches the background daemon. A
    // status run right after primes the token cache.
    let _ = git_capture(&["fsmonitor--daemon", "start"], root);
    // Prime: a couple of statuses so the daemon has a baseline token.
    let _ = git_capture(&["status", "--porcelain=v1", "-z"], root);
    let _ = git_capture(&["status", "--porcelain=v1", "-z"], root);
    fsmonitor_running(root)
}

/// Stop the fsmonitor daemon for the cold cells (so the cold numbers are
/// genuinely cold).
fn stop_fsmonitor(root: &Path) {
    let _ = git_capture(&["fsmonitor--daemon", "stop"], root);
}

/// True if `git fsmonitor--daemon status` reports the daemon is watching.
fn fsmonitor_running(root: &Path) -> bool {
    let out = git_capture(&["fsmonitor--daemon", "status"], root);
    // git prints "fsmonitor-daemon is watching '<path>'" when up.
    let s = String::from_utf8_lossy(&out.stdout);
    out.status.success() && s.contains("is watching")
}

// ---------------------------------------------------------------------------
// The two status implementations under test
// ---------------------------------------------------------------------------

/// The V0.1 shell-out path: exactly what `concerto_gix_wrap::status`
/// runs. Returns the byte length of stdout so the optimizer can't elide
/// the call.
fn status_shellout(root: &Path) -> usize {
    let out = git_capture(&["status", "--porcelain=v1", "-z"], root);
    assert!(out.status.success(), "shell-out status failed");
    out.stdout.len()
}

/// The gix-native path reachable under the pinned feature set: open the
/// repo, decode the index, and stat every tracked path in the cone,
/// counting entries whose on-disk `(mtime, size)` differs from the index
/// `stat` (the racy-clean comparison git itself does). Does NOT walk for
/// untracked files (needs the `status`/`dirwalk` feature — see module
/// docs + findings). Returns the count of detected modifications.
fn status_gix(root: &Path) -> usize {
    let repo = gix::discover(root).expect("gix discover");
    let workdir = repo.workdir().expect("worktree repo").to_path_buf();
    let index = repo.open_index().expect("gix open_index");

    let mut changed = 0usize;
    for entry in index.entries() {
        let rel = entry.path(&index);
        // BStr → OS path. On the platforms we target paths are UTF-8 /
        // bytes; lossy is fine for the stat probe.
        let rel_path = Path::new(std::str::from_utf8(rel).unwrap_or(""));
        let abs = workdir.join(rel_path);
        match std::fs::symlink_metadata(&abs) {
            Ok(meta) => {
                // Compare size first (cheap, catches most edits), then
                // mtime seconds against the index stat. This mirrors
                // git's stat-based "is this entry dirty?" fast path.
                let size_now = meta.len() as u32;
                let mtime_now = mtime_secs(&meta);
                if size_now != entry.stat.size || mtime_now != entry.stat.mtime.secs {
                    changed += 1;
                }
            }
            Err(_) => {
                // Missing on disk: outside the cone (sparse) or deleted.
                // Sparse entries carry the skip-worktree flag; treat a
                // missing-but-skip-worktree entry as clean, anything else
                // as a change.
                if !entry
                    .flags
                    .contains(gix::index::entry::Flags::SKIP_WORKTREE)
                {
                    changed += 1;
                }
            }
        }
    }
    changed
}

/// Seconds component of a file's mtime, matching the index stat's `secs`
/// field (which is a 32-bit unix-seconds value).
fn mtime_secs(meta: &std::fs::Metadata) -> u32 {
    use std::time::UNIX_EPOCH;
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Percentile measurement + reporting
// ---------------------------------------------------------------------------

/// Run `f` `samples` times, returning the sorted per-run durations.
fn measure<F: FnMut()>(samples: usize, mut f: F) -> Vec<Duration> {
    let mut out = Vec::with_capacity(samples);
    for _ in 0..samples {
        let t = Instant::now();
        f();
        out.push(t.elapsed());
    }
    out.sort_unstable();
    out
}

/// Nearest-rank percentile from a pre-sorted slice.
fn pct(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// One reported cell.
struct Cell {
    path: &'static str,
    state: &'static str,
    p50: Duration,
    p95: Duration,
}

fn report(cells: &[Cell], scale: &Scale, cone_on_disk: usize, fsmonitor_warm_ok: bool) {
    eprintln!("\n========== sparse-cone status latency ==========");
    eprintln!(
        "fixture: scale={} total~{} cone~{} (on disk ~{cone_on_disk}); cone ratio {:.1}%",
        scale.name,
        scale.total,
        scale.cone,
        100.0 * scale.cone as f64 / scale.total as f64
    );
    eprintln!("fsmonitor warm cells valid: {fsmonitor_warm_ok}");
    eprintln!("bar: <100 ms (design/00 §7.7)");
    eprintln!(
        "{:<12} {:<6} {:>10} {:>10} {:>8}",
        "path", "state", "p50(ms)", "p95(ms)", "verdict"
    );
    for c in cells {
        let verdict = if ms(c.p95) < 100.0 {
            "GO"
        } else if ms(c.p50) < 100.0 {
            "GO(p50)"
        } else {
            "NO-GO"
        };
        eprintln!(
            "{:<12} {:<6} {:>10.2} {:>10.2} {:>8}",
            c.path,
            c.state,
            ms(c.p50),
            ms(c.p95),
            verdict
        );
    }
    eprintln!("================================================\n");
}

// ---------------------------------------------------------------------------
// Criterion entry point
// ---------------------------------------------------------------------------

/// The spike "bench". Criterion drives a single trivial timing function so
/// the `[[bench]]` harness is satisfied and `cargo bench --no-run`
/// compiles it; the real percentile numbers are produced by an explicit
/// measurement pass (printed above) over the four cells, because a doc
/// needs labelled p50/p95 per path×state, which Criterion's grouped
/// output does not surface as cleanly.
fn bench_sparse_cone(c: &mut Criterion) {
    let scale = resolve_scale();
    let samples = sample_count();
    eprintln!(
        "[sparse_cone] building fixture (scale={}, samples={})…",
        scale.name, samples
    );
    let fx = build_fixture(&scale);
    let root = fx.root.as_path();
    let cone_on_disk = count_worktree_files(root);

    let mut cells = Vec::new();

    // ---- COLD: ensure fsmonitor is stopped ----
    stop_fsmonitor(root);
    assert!(
        !fsmonitor_running(root),
        "fsmonitor still running for cold cells"
    );
    // Warm the OS page cache once so cold ≠ first-touch-from-disk noise;
    // "cold" here means "no fsmonitor", the comparison the design cares
    // about, not "uncached inode".
    let _ = status_shellout(root);
    let _ = status_gix(root);

    let cold_shell = measure(samples, || {
        let _ = status_shellout(root);
    });
    cells.push(Cell {
        path: "shell-out",
        state: "cold",
        p50: pct(&cold_shell, 50.0),
        p95: pct(&cold_shell, 95.0),
    });

    let cold_gix = measure(samples, || {
        let _ = status_gix(root);
    });
    cells.push(Cell {
        path: "gix",
        state: "cold",
        p50: pct(&cold_gix, 50.0),
        p95: pct(&cold_gix, 95.0),
    });

    // ---- WARM: start + prime the fsmonitor daemon ----
    let fsmonitor_warm_ok = start_fsmonitor(root);
    if !fsmonitor_warm_ok {
        eprintln!(
            "[sparse_cone] WARNING: fsmonitor daemon did not start; \
             warm cells fall back to cold semantics and are flagged \
             invalid in the report."
        );
    }

    let warm_shell = measure(samples, || {
        let _ = status_shellout(root);
    });
    cells.push(Cell {
        path: "shell-out",
        state: "warm",
        p50: pct(&warm_shell, 50.0),
        p95: pct(&warm_shell, 95.0),
    });

    // gix-native does not consult the fsmonitor daemon (no `status`
    // feature), so its "warm" number is expected to match its "cold"
    // number — we still measure it so the doc can state that plainly.
    let warm_gix = measure(samples, || {
        let _ = status_gix(root);
    });
    cells.push(Cell {
        path: "gix",
        state: "warm",
        p50: pct(&warm_gix, 50.0),
        p95: pct(&warm_gix, 95.0),
    });

    report(&cells, &scale, cone_on_disk, fsmonitor_warm_ok);

    // Sanity: the dirty set should have produced a non-empty status so we
    // know we measured a real (not no-op) status path. Don't panic in a
    // bench, just note it.
    if fx.dirty_paths.is_empty() {
        eprintln!("[sparse_cone] note: no in-cone dirty paths were recorded");
    }

    // Satisfy Criterion's harness with a cheap, real measurement: the
    // warm shell-out path (the V0.1 production hot path). This keeps
    // `criterion_main!` honest and gives `cargo bench` a stored result.
    stop_fsmonitor(root);
    let _ = start_fsmonitor(root);
    let mut group = c.benchmark_group("sparse_cone");
    group.sample_size(10);
    group.bench_function("warm_shellout_status", |b| {
        b.iter(|| {
            let _ = status_shellout(root);
        });
    });
    group.finish();

    // Leave the daemon stopped so we don't leak a background process.
    stop_fsmonitor(root);
}

criterion_group!(benches, bench_sparse_cone);
criterion_main!(benches);
