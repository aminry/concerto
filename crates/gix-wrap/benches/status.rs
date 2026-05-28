//! Criterion benchmark for `concerto_gix_wrap::status` (Task 29).
//!
//! Builds a synthetic repo with 10k tiny tracked files (and a handful of
//! modifications + untracked files mixed in) and measures the latency of
//! `status()` against the 100 ms p50 budget locked by `design/00 §7.7`.
//!
//! Run via `cargo bench -p concerto-gix-wrap`. CI only validates that
//! the bench *compiles* (`cargo bench --no-run`) — actual measurements
//! are run manually on developer machines per Task 29's pre-decision 8.
//!
//! ## Fixture
//!
//! - 10,000 files at `files/<i>.txt` with one line of content each.
//! - One initial commit captures all of them.
//! - 5 files are then mutated (`status` should report them as modified).
//! - 5 untracked files are added.
//!
//! The benchmark is intentionally synchronous from criterion's
//! perspective: it builds a small tokio runtime per-iteration-group and
//! drives `status()` through it.

use std::path::Path;
use std::process::Command;

use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::TempDir;

/// Shell out to `git` for the fixture setup. Panic on failure — the
/// benchmark is meaningless if the fixture cannot be built.
fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "bench")
        .env("GIT_AUTHOR_EMAIL", "bench@example.com")
        .env("GIT_COMMITTER_NAME", "bench")
        .env("GIT_COMMITTER_EMAIL", "bench@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Build the 10k-file repo and return the owning tempdir.
///
/// The tempdir is returned so callers can keep it alive across the bench
/// iterations; dropping it deletes the entire fixture from disk.
fn build_fixture() -> TempDir {
    const N: usize = 10_000;
    let dir = TempDir::new().expect("tempdir");
    git(&["init", "-q", "-b", "main", "."], dir.path());

    // Lay down the files in shards so any single directory listing
    // stays under typical FS limits.
    let files_dir = dir.path().join("files");
    std::fs::create_dir_all(&files_dir).expect("files dir");
    for i in 0..N {
        let shard = files_dir.join(format!("{:03}", i / 100));
        std::fs::create_dir_all(&shard).expect("shard");
        let f = shard.join(format!("{i}.txt"));
        std::fs::write(&f, format!("file {i}\n")).expect("write file");
    }
    git(&["add", "-A"], dir.path());
    git(&["commit", "-q", "-m", "seed"], dir.path());

    // Add 5 modifications + 5 untracked entries so `status` has to
    // report something — pure clean-worktree timings are trivial.
    for i in 0..5 {
        let shard = files_dir.join(format!("{:03}", i / 100));
        let f = shard.join(format!("{i}.txt"));
        std::fs::write(&f, format!("file {i} (modified)\n")).expect("modify");
    }
    for i in 0..5 {
        let f = dir.path().join(format!("untracked-{i}.txt"));
        std::fs::write(&f, "untracked\n").expect("untracked");
    }

    dir
}

fn bench_status(c: &mut Criterion) {
    let fixture = build_fixture();
    let worktree = fixture.path().to_path_buf();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt");

    let mut group = c.benchmark_group("gix_wrap::status");
    // Reduce sample count: each iteration shells out to `git status`,
    // and 10k files is not the cheapest fixture. 30 samples is enough
    // for the locked p50 budget.
    group.sample_size(30);

    group.bench_function("10k_files", |b| {
        b.iter(|| {
            let _ =
                rt.block_on(async { concerto_gix_wrap::status(&worktree).await.expect("status") });
        });
    });
    group.finish();
}

criterion_group!(benches, bench_status);
criterion_main!(benches);
