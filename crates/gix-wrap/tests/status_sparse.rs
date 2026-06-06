//! Task 303 — `concerto_gix_wrap::status` on a sparse-cone worktree.
//!
//! Proves the FROZEN V0.1 shell-out `status()` seam (Task 29), when run
//! against a per-(workarea, repo) **sparse cone** worktree (the one Task
//! 302 materializes), reports only **in-cone** changes — the wiring this
//! task locks. Spike 104 (`design/spikes/gix-sparse-cone-findings.md`)
//! returned GO for the shell-out path; this test is the correctness side
//! of that wiring (the `< 100 ms` latency side is the bench gate,
//! `benches/status_sparse_gate.rs`).
//!
//! Two cases:
//!
//! 1. `status_on_fresh_cone_reports_only_in_cone` — build a cone-mode +
//!    `--sparse-index` repo with two top-level dirs, cone down to one,
//!    dirty an in-cone tracked file + drop an in-cone untracked file, and
//!    assert `status()` reports exactly those — never an out-of-cone path
//!    (the out-of-cone dir is collapsed to a sparse-index directory entry
//!    and is not on disk, so status cannot pay for it).
//!
//! 2. `status_on_restored_prepacked_fixture` — restore the committed
//!    pre-packed fixture (`tests/fixtures/sparse-cone.tar`, baked by
//!    `scripts/build-sparse-fixture.sh`), assert its index is a sparse
//!    index, and assert `status()` over it returns the baked-in dirty set
//!    (the same fixture the bench gate times — proving the restore
//!    contract the gate depends on).

use std::path::Path;

use concerto_gix_wrap::{status, StatusState};
use tempfile::TempDir;
use tokio::process::Command;

/// Run `git <args>` in `cwd`, asserting success (a broken fixture makes
/// every assertion meaningless, so fail loud). Pins the config to exactly
/// what the test sets — never inherits the developer's global/system git.
async fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `git <args>` capturing stdout (trimmed). Used for the sparse-index probe.
async fn git_stdout(args: &[&str], cwd: &Path) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .expect("spawn git");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// True iff `git ls-files --sparse` shows at least one collapsed directory
/// entry (a trailing-slash path) — the signature of a genuine sparse index.
/// A fixture that lost its sparse index would silently make `status`
/// measure full-repo work; every sparse-cone assertion checks this first.
async fn index_is_sparse(worktree: &Path) -> bool {
    git_stdout(&["ls-files", "--sparse"], worktree)
        .await
        .lines()
        .any(|l| l.trim_end().ends_with('/'))
}

/// True if this git has the `sparse-checkout` subcommand (>= 2.27). The
/// CI matrix (Task 113) ships it, but skip cleanly on an ancient git so a
/// stray runner does not fail the suite.
async fn have_sparse_checkout() -> bool {
    let out = Command::new("git")
        .args(["sparse-checkout", "-h"])
        .output()
        .await;
    match out {
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

#[tokio::test(flavor = "multi_thread")]
async fn status_on_fresh_cone_reports_only_in_cone() {
    if !have_sparse_checkout().await {
        eprintln!("SKIP: git lacks sparse-checkout");
        return;
    }

    let dir = TempDir::new().unwrap();
    let root = dir.path();

    git(&["init", "-q", "-b", "main", "."], root).await;
    git(&["config", "core.sparseCheckoutCone", "true"], root).await;

    // Two top-level dirs, each with a tracked file. `in_cone/` will be the
    // cone; `out_of_cone/` will be collapsed out of the worktree.
    for top in ["in_cone", "out_of_cone"] {
        let sub = root.join(top);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("tracked.txt"), b"original\n").unwrap();
    }
    git(&["add", "-A"], root).await;
    git(&["commit", "-q", "-m", "seed"], root).await;

    // Cone down to in_cone/ only, with the sparse index on.
    git(
        &["sparse-checkout", "init", "--cone", "--sparse-index"],
        root,
    )
    .await;
    git(
        &["sparse-checkout", "set", "--sparse-index", "--", "in_cone"],
        root,
    )
    .await;
    git(&["sparse-checkout", "reapply", "--sparse-index"], root).await;

    // The lever the latency bar leans on: the index must be sparse, and the
    // out-of-cone file must be GONE from the worktree.
    assert!(
        index_is_sparse(root).await,
        "expected a sparse index (collapsed out_of_cone dir entry)"
    );
    assert!(
        !root.join("out_of_cone/tracked.txt").exists(),
        "out_of_cone/ should be collapsed out of the sparse worktree"
    );
    assert!(
        root.join("in_cone/tracked.txt").exists(),
        "in_cone/ should be materialized"
    );

    // Dirty an in-cone tracked file + add an in-cone untracked file.
    std::fs::write(root.join("in_cone/tracked.txt"), b"changed\n").unwrap();
    std::fs::write(root.join("in_cone/untracked.txt"), b"new\n").unwrap();

    let report = status(root).await.expect("status on sparse cone");

    // Exactly the two in-cone changes; nothing out-of-cone.
    let paths: Vec<String> = report
        .files
        .iter()
        .map(|e| e.path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        paths.iter().all(|p| p.starts_with("in_cone/")),
        "status must report only in-cone paths; got {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "in_cone/tracked.txt"),
        "expected the modified in-cone file; got {paths:?}"
    );
    assert!(
        paths.iter().any(|p| p == "in_cone/untracked.txt"),
        "expected the untracked in-cone file; got {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.starts_with("out_of_cone")),
        "status leaked an out-of-cone path: {paths:?}"
    );

    // The modified tracked file is Modified; the new file Untracked.
    let modified = report
        .files
        .iter()
        .find(|e| e.path.to_string_lossy().replace('\\', "/") == "in_cone/tracked.txt")
        .unwrap();
    assert_eq!(modified.state, StatusState::Modified);
    let untracked = report
        .files
        .iter()
        .find(|e| e.path.to_string_lossy().replace('\\', "/") == "in_cone/untracked.txt")
        .unwrap();
    assert_eq!(untracked.state, StatusState::Untracked);
}

#[tokio::test(flavor = "multi_thread")]
async fn status_on_restored_prepacked_fixture() {
    if !have_sparse_checkout().await {
        eprintln!("SKIP: git lacks sparse-checkout");
        return;
    }

    let dest = TempDir::new().unwrap();
    let root = match concerto_sparse_fixture::restore(dest.path()) {
        Ok(p) => p,
        Err(e) => {
            // The fixture ships committed; a missing/corrupt tar is a real
            // failure, not a skip — surface it.
            panic!("restore pre-packed sparse-cone fixture: {e}");
        }
    };

    // The restore contract: the index IS a sparse index (collapsed dirs).
    assert!(
        index_is_sparse(&root).await,
        "restored fixture must carry a sparse index"
    );

    // The baked dirty set: 3 in-cone modifications + 2 untracked files.
    let report = status(&root).await.expect("status on restored fixture");
    let paths: Vec<String> = report
        .files
        .iter()
        .map(|e| e.path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        paths.iter().all(|p| p.starts_with("top/00000/")),
        "every baked change is in the cone (top/00000/); got {paths:?}"
    );
    let modified = report
        .files
        .iter()
        .filter(|e| e.state == StatusState::Modified)
        .count();
    let untracked = report
        .files
        .iter()
        .filter(|e| e.state == StatusState::Untracked)
        .count();
    assert_eq!(
        modified, 3,
        "expected 3 baked in-cone modifications; got {paths:?}"
    );
    assert_eq!(
        untracked, 2,
        "expected 2 baked untracked files; got {paths:?}"
    );
}

/// The `< 100 ms p50` GATE, test-shaped so the standard
/// `cargo test --workspace` enforces it (the task's preferred form — the
/// `cargo bench --bench status_sparse_gate` twin is the explicit
/// process-exit gate). Restores the pre-packed fixture, confirms its index
/// is sparse, measures **cold** `git status` p50 over the restored cone,
/// and asserts it is under the FROZEN `design/00 §7.7` budget. At the
/// committed fixture's small scale this passes with enormous margin (spike
/// 104: 25 ms p50 at a 25k cone); the bar FREEZES the regression ceiling.
/// The real 2M-file-monorepo number is the Phase-3 Tier-3 checklist line.
#[test]
fn gate_p50_under_budget() {
    let dest = TempDir::new().unwrap();
    let Some(m) = concerto_sparse_fixture::measure_gate(dest.path()) else {
        eprintln!("SKIP: git lacks sparse-checkout");
        return;
    };
    eprintln!("{}", concerto_sparse_fixture::summary(&m));
    assert!(
        m.sparse_index,
        "the restored fixture must carry a sparse index"
    );
    assert!(
        m.changed_entries > 0,
        "the gate must time a real (non-clean) status"
    );
    assert!(
        m.passes(),
        "status p50 {:.2}ms regressed past the {}ms budget (design/00 §7.7)",
        m.p50.as_secs_f64() * 1000.0,
        concerto_sparse_fixture::P50_BUDGET.as_millis(),
    );
}

/// Shared restore + gate-measurement helper, used by both this test and the
/// bench gate (`benches/status_sparse_gate.rs`). Included via `#[path]` so
/// the restore contract lives in exactly one place and is never a library
/// API (`crates/*/src/api.rs`) — keeping `docs/interfaces/` unchanged.
#[path = "../benches/sparse_fixture.rs"]
mod concerto_sparse_fixture;
