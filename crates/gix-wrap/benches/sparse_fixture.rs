//! Shared restore helper for the Task 303 pre-packed sparse-cone fixture.
//!
//! Both the bench gate (`benches/status_sparse_gate.rs`) and the
//! integration test (`tests/status_sparse.rs`) `#[path]`-include this
//! module so the restore contract lives in exactly one place. It is NOT a
//! library API (no `crates/*/src/api.rs` surface) — it is test/bench
//! scaffolding, included verbatim into each harness.
//!
//! ## The restore contract (FROZEN, per Task 303)
//!
//! The committed artifact `crates/gix-wrap/tests/fixtures/sparse-cone.tar`
//! (baked by `scripts/build-sparse-fixture.sh`, spike 104 §7 rec 3) is an
//! **uncompressed** tar of a real git repo whose:
//!   - objects all live in a single **pack** (no loose objects — the gate
//!     never pays loose-object hashing per run),
//!   - **index is a sparse index** (cone-mode, out-of-cone dirs collapsed
//!     to directory entries — the lever the `< 100 ms` bar leans on),
//!   - cone is `top/00000/`, with 3 in-cone tracked files modified + 2
//!     untracked files added, so `status` reports real work.
//!
//! [`restore`] untars it into a caller-provided directory and returns the
//! repo root. The tar is uncompressed precisely so the restore side needs
//! NO compression-crate dependency (`cargo deny` stays green); only the
//! pure-Rust `tar` reader (already a workspace pin from Task 111) is used.

#![allow(dead_code)] // each includer uses a subset of this module.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Path to the committed fixture tar, relative to the crate manifest dir.
pub const FIXTURE_TAR: &str = "tests/fixtures/sparse-cone.tar";

/// The FROZEN status latency bar (`design/00 §7.7`): `git status` on a
/// 2M-file repo with a sparse cone must stay **< 100 ms p50**. The gate
/// asserts this literal bar. At the committed fixture's small scale cold
/// status is a few ms, so the gate passes with enormous margin (spike 104:
/// 25 ms p50 at a 25k cone); the bar still FREEZES the regression ceiling.
/// The real 2M-file-monorepo number is the Phase-3 Tier-3 checklist line,
/// corroborated by spike 104's ~75 ms p50 linear extrapolation.
pub const P50_BUDGET: Duration = Duration::from_millis(100);

/// Per-cell sample count for the gate (override with `SPARSE_GATE_SAMPLES`).
/// 25 cold runs is plenty for a stable nearest-rank p50 on this fixture
/// without dominating wall-clock.
pub fn sample_count() -> usize {
    std::env::var("SPARSE_GATE_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25)
}

/// Restore the pre-packed sparse-cone fixture into `dest` and return the
/// restored repo root (`dest` itself — the archive root is the repo dir).
///
/// Uncompressed tar ⇒ pure `tar::Archive::unpack`, no compression crate.
/// `dest` should be a fresh empty directory (a `TempDir` in practice) so
/// the restore is hermetic and self-deleting.
pub fn restore(dest: &Path) -> io::Result<PathBuf> {
    let tar_path = fixture_path();
    let file = std::fs::File::open(&tar_path).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "open committed fixture {}: {e} — run scripts/build-sparse-fixture.sh to bake it",
                tar_path.display()
            ),
        )
    })?;
    let mut archive = tar::Archive::new(file);
    // Preserve mtimes so git's racy-clean stat comparison behaves as baked;
    // do not preserve permissions/ownership (portable across CI lanes).
    archive.set_preserve_mtime(true);
    archive.set_unpack_xattrs(false);
    archive.unpack(dest)?;
    Ok(dest.to_path_buf())
}

/// Absolute path to the committed fixture tar via `CARGO_MANIFEST_DIR`.
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_TAR)
}

/// Result of one gate measurement over the restored fixture.
pub struct GateMeasurement {
    pub p50: Duration,
    pub p95: Duration,
    pub samples: usize,
    /// Number of changed entries `status` reported (sanity: must be > 0 so
    /// we know we timed a real, non-clean status).
    pub changed_entries: usize,
    /// Whether the restored index was confirmed sparse before timing.
    pub sparse_index: bool,
}

impl GateMeasurement {
    /// True iff the measured p50 is strictly under the FROZEN budget.
    pub fn passes(&self) -> bool {
        self.p50 < P50_BUDGET
    }
}

/// True if this git has the `sparse-checkout` subcommand (>= 2.27).
fn have_sparse_checkout() -> bool {
    match Command::new("git").args(["sparse-checkout", "-h"]).output() {
        Ok(o) => {
            let s = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            s.contains("sparse-checkout")
        }
        Err(_) => false,
    }
}

/// True iff `git ls-files --sparse` in `root` shows a collapsed directory
/// entry (trailing slash) — the signature of a genuine sparse index.
fn index_is_sparse(root: &Path) -> bool {
    let out = Command::new("git")
        .args(["ls-files", "--sparse"])
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.trim_end().ends_with('/')),
        Err(_) => false,
    }
}

/// Stop any fsmonitor daemon for `root` so the gate runs **cold** — the
/// deterministic, pessimistic floor (spike 104: warm is faster and is what
/// production uses; the gate proves the floor). Best-effort; the committed
/// fixture ships with `core.fsmonitor` unset, so there is usually nothing
/// to stop.
fn ensure_cold(root: &Path) {
    let _ = Command::new("git")
        .args(["fsmonitor--daemon", "stop"])
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output();
}

/// One cold `git status --porcelain=v1 -z` — byte-for-byte the command
/// `concerto_gix_wrap::status` shells out to. Returns the number of
/// NUL-delimited records so the optimizer can't elide the call and the
/// caller can confirm a non-empty (real) status.
fn cold_status(root: &Path) -> usize {
    let out = Command::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("spawn git status");
    assert!(out.status.success(), "git status failed during gate");
    out.stdout
        .split(|b| *b == 0)
        .filter(|c| !c.is_empty())
        .count()
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

/// Restore the pre-packed fixture into `dest`, confirm the index is sparse,
/// then measure **cold** `git status` p50/p95 over `sample_count()` runs.
///
/// Returns `None` only when this git lacks `sparse-checkout` (an ancient
/// runner) — every other failure (missing tar, non-sparse index) panics,
/// because they mean the gate would otherwise measure the wrong thing.
pub fn measure_gate(dest: &Path) -> Option<GateMeasurement> {
    if !have_sparse_checkout() {
        return None;
    }
    let root = restore(dest).expect("restore committed sparse-cone fixture");

    // A fixture that lost its sparse index would silently make the gate
    // time full-repo status — refuse to measure against the wrong thing.
    let sparse_index = index_is_sparse(&root);
    assert!(
        sparse_index,
        "restored fixture index is NOT sparse — the gate would measure \
         full-repo status; re-bake via scripts/build-sparse-fixture.sh"
    );

    ensure_cold(&root);

    // Warm the OS page cache once so "cold" means *no fsmonitor*, not
    // *uncached inode* (the comparison design/02 §7.2 cares about).
    let changed_entries = cold_status(&root);
    assert!(
        changed_entries > 0,
        "baked fixture reports a clean tree; the gate must time a real status"
    );

    let samples = sample_count();
    let mut durs: Vec<Duration> = Vec::with_capacity(samples);
    for _ in 0..samples {
        let t = Instant::now();
        let _ = cold_status(&root);
        durs.push(t.elapsed());
    }
    durs.sort_unstable();

    Some(GateMeasurement {
        p50: pct(&durs, 50.0),
        p95: pct(&durs, 95.0),
        samples,
        changed_entries,
        sparse_index,
    })
}

/// Render a one-line summary (used by both the bench harness and the test).
pub fn summary(m: &GateMeasurement) -> String {
    format!(
        "sparse-cone status gate (cold, pre-packed fixture): \
         p50={:.2}ms p95={:.2}ms over {} samples, {} changed entries, \
         sparse_index={}; budget={}ms",
        m.p50.as_secs_f64() * 1000.0,
        m.p95.as_secs_f64() * 1000.0,
        m.samples,
        m.changed_entries,
        m.sparse_index,
        P50_BUDGET.as_millis(),
    )
}
