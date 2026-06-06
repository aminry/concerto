//! Task 303 — the `git status` sparse-cone **bench GATE**.
//!
//! Unlike the Task 104 spike harness (`benches/sparse_cone.rs`, which
//! regenerates a multi-million-file fixture and only prints numbers), this
//! is a **gate**: it restores a committed, **pre-packed** sparse-cone
//! fixture (spike 104 §7 rec 3 — objects in a pack, sparse index
//! pre-written, baked by `scripts/build-sparse-fixture.sh`), measures
//! **cold** `git status` p50 over the restored fixture, and **exits
//! non-zero if p50 ≥ 100 ms** (the `design/00 §7.7` bar). On regression CI
//! observes a failure.
//!
//! ## Why cold, why pre-packed (FROZEN choices)
//!
//! - **Cold** (fsmonitor stopped): a deterministic, pessimistic floor. The
//!   spike's cold shell-out was 25 ms at a 25k cone / ~75 ms extrapolated
//!   at 100k — both under the bar. Warm (production) is faster; the gate
//!   proves the floor, not the production number.
//! - **Pre-packed**: the gate must time `status`, not fixture construction.
//!   Generating loose objects per run is the minutes-long cost spike 104
//!   flagged; the committed tar's objects are already in a pack.
//! - **Committed-small scale**: the full 2M-file image is impractical to
//!   commit (and slow to build); the committed fixture is hundreds of files
//!   at the largest scale that fits the repo + every OS CI lane (Task 113).
//!   It passes the 100 ms bar with enormous margin. The real 2M-file number
//!   stays the Phase-3 **Tier-3 checklist** line, corroborated by spike
//!   104's linear extrapolation (~75 ms p50). The shared restore + measure
//!   contract lives in `benches/sparse_fixture.rs`.
//!
//! A test-shaped twin of this gate (`gate_p50_under_budget` in
//! `tests/status_sparse.rs`) runs the SAME measurement under the standard
//! `cargo test --workspace`, so the budget is enforced even when CI does
//! not invoke `cargo bench`. This file is the explicit
//! `cargo bench -p concerto-gix-wrap --bench status_sparse_gate` entry that
//! exits non-zero over budget.
//!
//! Run: `cargo bench -p concerto-gix-wrap --bench status_sparse_gate`

use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::TempDir;

#[path = "sparse_fixture.rs"]
mod sparse_fixture;

fn bench_status_sparse_gate(c: &mut Criterion) {
    let dest = TempDir::new().expect("tempdir for fixture restore");

    let Some(m) = sparse_fixture::measure_gate(dest.path()) else {
        // Ancient git without sparse-checkout — skip cleanly (the CI matrix
        // ships a modern git; a stray runner must not fail the gate build).
        eprintln!("[status_sparse_gate] SKIP: git lacks sparse-checkout");
        return;
    };

    eprintln!("\n========== {} ==========", sparse_fixture::summary(&m));

    // Feed the measured cold-status closure to Criterion too, so
    // `cargo bench` stores a tracked result (regression trend), then enforce
    // the hard gate below.
    let root = dest.path().to_path_buf();
    let mut group = c.benchmark_group("status_sparse_gate");
    group.sample_size(10);
    group.bench_function("cold_status_porcelain_v1", |b| {
        b.iter(|| {
            // Re-run the same cold shell-out `concerto_gix_wrap::status`
            // uses; the heavy lifting is the git subprocess.
            let out = std::process::Command::new("git")
                .args(["status", "--porcelain=v1", "-z"])
                .current_dir(&root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .expect("git status");
            std::hint::black_box(out.stdout.len());
        });
    });
    group.finish();

    // ---- the GATE ----
    if !m.passes() {
        eprintln!(
            "[status_sparse_gate] FAIL: p50 {:.2}ms >= {}ms budget (design/00 §7.7)",
            m.p50.as_secs_f64() * 1000.0,
            sparse_fixture::P50_BUDGET.as_millis(),
        );
        // Non-zero exit so `cargo bench --bench status_sparse_gate` fails CI
        // on a status-latency regression (Criterion does not threshold).
        std::process::exit(1);
    }
    eprintln!(
        "[status_sparse_gate] PASS: p50 {:.2}ms < {}ms budget",
        m.p50.as_secs_f64() * 1000.0,
        sparse_fixture::P50_BUDGET.as_millis(),
    );
}

criterion_group!(benches, bench_status_sparse_gate);
criterion_main!(benches);
