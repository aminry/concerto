//! Sparse-checkout + cone + sparse-index lifecycle (Task 302, `design/02
//! §3.1`/`§3.2`/`§8`, `design/00 §6.3`).
//!
//! **Cone mode is mandatory and `--sparse-index` is always on.** The
//! non-cone path (`git sparse-checkout set --no-cone`) has subtle
//! correctness bugs (`design/00 §6.3`) and is never invoked from here —
//! every helper in this module either uses `--cone` explicitly or
//! reapplies the index with `--sparse-index`. A repo that arrives with
//! `core.sparseCheckoutCone=false` (a user cloned it manually) is
//! force-set to cone mode by the caller via [`force_cone_mode`] +
//! audit-logged (`design/02 §8`).
//!
//! All helpers shell out to `git` (never `gix`) because "sparse-cone
//! behavior is git's authoritative" (`design/02 §3.1`). They take a
//! `worktree: &Path` (the checked-out worktree directory, NOT the bare
//! object DB) and surface errors as [`concerto_error::Error::Git`].
//!
//! `--sparse-index` is the load-bearing detail: it collapses out-of-cone
//! paths to single directory entries in the in-memory index so the index
//! stays proportional to the cone, not the whole tree. Without it Task
//! 303's `< 100 ms status` bar fails. Every cone-changing helper
//! (`init`/`set`/`add`) carries `--sparse-index` and is followed by a
//! `reapply --sparse-index` so the on-disk index is rewritten in the
//! sparse format (`git sparse-checkout set` itself does not always rewrite
//! the index format on older gits).

use std::path::Path;

use concerto_error::{Error, Result};

use crate::cmd;

/// A cone path — a forward-slash, repo-root-relative directory path
/// (`design/02 §5.1`'s `Vec<ConePath>`).
///
/// FROZEN as `String` (Task 302). Cone paths are always git path syntax
/// (forward slashes) on every OS; git normalizes them internally, so
/// callers never convert separators. An empty cone set means "cone the
/// repo down to just the top-level files" (git's default cone with no
/// directories selected).
pub type ConePath = String;

/// `git sparse-checkout init --cone --sparse-index` at `worktree`.
///
/// Enables cone-mode sparse-checkout with the sparse index turned on. After
/// init the worktree materializes only the top-level files (no
/// subdirectories) until [`sparse_set`] / [`sparse_add`] selects cones.
/// Idempotent — git tolerates re-init.
pub async fn sparse_init_cone(worktree: &Path) -> Result<()> {
    cmd::run(
        &["sparse-checkout", "init", "--cone", "--sparse-index"],
        worktree,
    )
    .await
    .map(|_| ())
}

/// `git sparse-checkout set --sparse-index <paths…>` at `worktree`,
/// **replacing** the current cone with `cones`.
///
/// Each path is validated against the repo's `HEAD` tree *before* the set
/// is applied (see [`probe_cone_paths_exist`]) so a non-existent cone path
/// is rejected with a clean [`Error::Validation`] (mapped to
/// `INVALID_ARGUMENT` at the handler) and nothing is half-applied — `git
/// sparse-checkout set` itself only warns to stderr for a bad path and
/// silently produces an empty/partial materialization (`design/02 §8`).
///
/// Followed by [`sparse_reapply_index`] so the on-disk index is rewritten
/// in the sparse format regardless of the git version.
pub async fn sparse_set(worktree: &Path, cones: &[ConePath]) -> Result<()> {
    probe_cone_paths_exist(worktree, cones).await?;

    let mut args: Vec<&str> = vec!["sparse-checkout", "set", "--sparse-index"];
    // `--` then the paths so a leading-dash cone path can't be parsed as a
    // flag. Cone paths are repo-root-relative forward-slash strings.
    args.push("--");
    for c in cones {
        args.push(c.as_str());
    }
    cmd::run(&args, worktree).await?;
    sparse_reapply_index(worktree).await
}

/// `git sparse-checkout add <paths…>` at `worktree`, **adding** `cones` to
/// the existing cone set (does not replace).
///
/// Like [`sparse_set`], each new path is validated against `HEAD` first so
/// a bad path is a clean error, not a silent partial. Followed by
/// [`sparse_reapply_index`].
pub async fn sparse_add(worktree: &Path, cones: &[ConePath]) -> Result<()> {
    probe_cone_paths_exist(worktree, cones).await?;

    let mut args: Vec<&str> = vec!["sparse-checkout", "add"];
    args.push("--");
    for c in cones {
        args.push(c.as_str());
    }
    cmd::run(&args, worktree).await?;
    sparse_reapply_index(worktree).await
}

/// `git sparse-checkout reapply --sparse-index` at `worktree`.
///
/// Re-applies the current sparsity to the worktree AND rewrites the
/// on-disk index in the sparse format (the lever Task 303's `< 100 ms
/// status` bar leans on, `design/02 §7.2`). Call after every cone change.
pub async fn sparse_reapply_index(worktree: &Path) -> Result<()> {
    cmd::run(&["sparse-checkout", "reapply", "--sparse-index"], worktree)
        .await
        .map(|_| ())
}

/// `git sparse-checkout disable` at `worktree` — full materialization.
///
/// Turns sparse-checkout off entirely and checks out the whole tree. Used
/// when a user explicitly opts out of sparse for a (workarea, repo).
pub async fn sparse_disable(worktree: &Path) -> Result<()> {
    cmd::run(&["sparse-checkout", "disable"], worktree)
        .await
        .map(|_| ())
}

/// True iff `core.sparseCheckoutCone` is set to `true` at `worktree`.
///
/// Reads the git config key. A repo with sparse-checkout enabled in
/// non-cone mode (or one where the key was never set) returns `false` —
/// the caller force-sets it via [`force_cone_mode`] + audit (`design/02
/// §8`). A repo that never enabled sparse-checkout at all also returns
/// `false`; the caller only force-sets when sparse is (or is about to be)
/// active, so a plain full clone is untouched.
pub async fn is_cone_mode(worktree: &Path) -> Result<bool> {
    // `git config --get --bool` exits non-zero when the key is unset; map
    // that (and only that) to `false`. Any other failure propagates.
    match cmd::run(
        &["config", "--get", "--bool", "core.sparseCheckoutCone"],
        worktree,
    )
    .await
    {
        Ok(out) => Ok(out.stdout.trim() == "true"),
        // Unset key (`git config --get` exits 1) folds into Error::Git via
        // the shell-out helper; treat the absence as "not cone mode".
        Err(Error::Git(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Force `core.sparseCheckoutCone=true` at `worktree` (`design/02 §8`, the
/// "Non-cone-mode sparse config (pre-existing repo)" failure mode).
///
/// The caller emits the corresponding audit event ("forced non-cone sparse
/// config to cone mode") — this helper only flips the config key so the
/// gix-wrap layer stays free of the Core's audit surface.
pub async fn force_cone_mode(worktree: &Path) -> Result<()> {
    cmd::run(&["config", "core.sparseCheckoutCone", "true"], worktree)
        .await
        .map(|_| ())
}

/// List the active cone paths reported by `git sparse-checkout list` at
/// `worktree`, one per line, trimmed.
///
/// Returns an empty `Vec` when sparse-checkout is disabled (git prints a
/// hint to stderr and exits non-zero in that case; we fold that into an
/// empty list rather than an error so callers/tests can probe freely).
/// Used by the smoke check + tests to assert the cone was applied.
pub async fn sparse_list(worktree: &Path) -> Result<Vec<ConePath>> {
    match cmd::run(&["sparse-checkout", "list"], worktree).await {
        Ok(out) => Ok(out
            .stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()),
        Err(Error::Git(_)) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Validate that every path in `cones` names a directory present in the
/// repo's `HEAD` tree at `repo_dir`, WITHOUT touching the sparse config
/// (design/02 §3.2/§8). A bad path returns [`Error::Validation`] (mapped to
/// `INVALID_ARGUMENT` at the handler); an empty `cones` slice is OK.
///
/// This is the up-front, non-mutating validation the Core's
/// `set_repo_cone_defaults` runs against a repo clone before persisting the
/// repo-level default + propagating to workareas — so a truly invalid path
/// is rejected before anything is written, mirroring [`sparse_set`]'s
/// pre-apply probe (which the repo clone is not a sparse worktree for).
pub async fn validate_cone_paths(repo_dir: &Path, cones: &[ConePath]) -> Result<()> {
    probe_cone_paths_exist(repo_dir, cones).await
}

/// Reject any cone path in `cones` that does not name a directory present
/// in the repo's `HEAD` tree (`design/02 §8`).
///
/// `git sparse-checkout set <bad/path>` does NOT error — it only warns to
/// stderr and produces an empty/partial materialization. We detect a
/// missing path *before* applying via a `git ls-tree -d HEAD <path>` probe
/// (cone paths are directories) so the set is never half-applied and the
/// caller can map the error to `INVALID_ARGUMENT`.
///
/// An empty `cones` slice is allowed (it cones the repo down to top-level
/// files only). A path that exists as a blob but not a tree is rejected —
/// cone mode only accepts directory prefixes.
async fn probe_cone_paths_exist(worktree: &Path, cones: &[ConePath]) -> Result<()> {
    for cone in cones {
        // Normalize: strip a leading `/` so `ls-tree` sees a repo-relative
        // path, AND strip a trailing `/` — cone paths legitimately carry a
        // trailing slash (git's cone syntax + callers pass `a/`), but
        // `git ls-tree -d HEAD a/` matches NOTHING (it only matches `a`),
        // which would otherwise wrongly reject a valid directory cone. An
        // empty/`.` cone path is the repo root and always exists, so skip
        // the probe for it.
        let probe = cone.trim_start_matches('/').trim_end_matches('/');
        if probe.is_empty() || probe == "." {
            continue;
        }
        // `ls-tree -d HEAD <path>` prints the tree entry for the directory
        // when it exists and nothing when it does not. We treat an empty
        // stdout as "path absent". `-d` restricts to tree (directory)
        // entries so a file at that path is correctly rejected.
        let out = cmd::run(&["ls-tree", "-d", "--name-only", "HEAD", probe], worktree).await?;
        if out.stdout.trim().is_empty() {
            // A bad cone path is a caller-facing input error, NOT a git
            // failure — surface it as `Error::Validation` so the gRPC
            // handler maps it to `INVALID_ARGUMENT` (`design/02 §8`).
            // `git sparse-checkout set` itself would only warn to stderr
            // and silently produce a partial materialization, so we reject
            // here BEFORE applying — nothing is half-applied.
            return Err(Error::Validation(format!(
                "sparse-cone path {cone:?} does not exist as a directory in HEAD; \
                 cone paths must name directories present in the repository tree"
            )));
        }
    }
    Ok(())
}
