//! Archive + restore lifecycle (Task 31, design/03 §3.7).
//!
//! Sits next to [`crate::workspace_manager::actor`] and
//! [`crate::workspace_manager::workarea`]. The four locked entry points
//! ([`archive_workarea`], [`restore_workarea`], [`archive_workspace`],
//! [`restore_workspace`]) are methods on [`super::WorkareaManager`] and
//! [`super::WorkspaceManager`] respectively; this module owns the helper
//! shared between them (`stop_sessions_for_workarea`,
//! `remove_worktrees_for_workarea`, etc.) plus the [`ArchiveOpts`] knob.
//!
//! ## Locked surface (Task 31)
//!
//! - `archive_workarea(id, ArchiveOpts { remove_worktree })` — stops every
//!   live session via the Agent Supervisor, optionally tears down the
//!   worktree, sets `archived_at` + `status = 'archived'` in one tx, emits
//!   `WorkareaEvent::Archived`.
//! - `restore_workarea(id)` — re-creates the worktree via `gix-wrap` if
//!   the directory is gone, clears `archived_at`, resets `permission_mode`
//!   to `NULL` (security stance per §3.7), sets `status = 'active'`, emits
//!   `WorkareaEvent::Restored`.
//! - `archive_workspace(id)` — cascades to every non-archived workarea in
//!   one writer transaction, sets `workspaces.archived_at`, emits
//!   `WorkspaceEvent::Archived`.
//! - `restore_workspace(id)` — clears `workspaces.archived_at` only;
//!   workareas remain individually archived.
//!
//! ## Crash adoption
//!
//! [`adopt_crashed_workareas`] runs once at Core boot from
//! `WorkareaManager::new` (`design/03 §6.5`): scan non-archived workareas,
//! probe `worktree_root` existence, transition missing ones to
//! `'crashed'`. The user — not Concerto — decides whether to restart or
//! archive a crashed workarea.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use concerto_error::{Error, Result};
use concerto_persist::{Persistence, WorkareaId, WorkspaceId};
use sqlx::Connection;
use tokio::process::Command;

/// Options for [`super::WorkareaManager::archive_workarea`].
///
/// Default is the design's R-5 stance: keep the worktree on disk so a
/// later restore is instant (no re-clone, no fresh `git worktree add`).
/// Callers set `remove_worktree = true` when they actually want the disk
/// reclaimed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchiveOpts {
    /// When `true`, `git worktree remove --force` is run for every repo
    /// and the workarea root directory is deleted.
    pub remove_worktree: bool,
}

/// Probe every non-archived, non-crashed workarea; transition rows whose
/// `worktree_root` directory is gone from disk to `status = 'crashed'`.
///
/// Called once at the end of [`super::WorkareaManager::new`]
/// (`design/03 §6.5`). Returns the number of workareas adopted. Best-effort:
/// errors enumerating one workarea log a warning and the sweep continues.
pub async fn adopt_crashed_workareas(persistence: &Arc<Persistence>) -> Result<usize> {
    let candidates =
        concerto_persist::workareas::list_all_non_archived(persistence.readers()).await?;
    let mut adopted = 0usize;
    for (id, worktree_root) in candidates {
        // The worktree_root directory is the locked workarea-root layout
        // (`<data_dir>/workspaces/<slug>/<composer>/`). A missing root is
        // the only signal Core has at boot that the workarea disappeared
        // out from under it.
        let path = PathBuf::from(&worktree_root);
        let exists = match tokio::fs::metadata(&path).await {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                tracing::warn!(
                    workarea = %id,
                    path = %path.display(),
                    error = %e,
                    "stat failed during crash-adoption sweep; skipping"
                );
                continue;
            }
        };
        if exists {
            continue;
        }
        let mut writer = persistence.writer().await;
        if let Err(e) =
            concerto_persist::workareas::update_status(&mut writer, &id, "crashed").await
        {
            tracing::warn!(
                workarea = %id,
                error = %e,
                "failed to mark workarea crashed during boot sweep"
            );
            continue;
        }
        adopted += 1;
        tracing::info!(workarea = %id, path = %path.display(), "adopted crashed workarea");
    }
    Ok(adopted)
}

/// Shell out to `git worktree remove --force <dest>` from a repo's `.git`
/// directory. Best-effort — the disk-side `remove_dir_all` that follows
/// is the real cleanup, so a stale git bookkeeping entry is swallowed.
pub(crate) async fn remove_worktree_via_git(repo_dir: &Path, dest: &Path) -> Result<()> {
    let out = Command::new("git")
        .args(["worktree", "remove", "--force", &dest.to_string_lossy()])
        .current_dir(repo_dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(Error::Io)?;
    if !out.status.success() {
        tracing::debug!(
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git worktree remove failed during archive; ignoring"
        );
    }
    Ok(())
}

/// Resolve the `worktree_path` + `repository_id` pairs for a workarea.
/// The archive cascade needs both to run `git worktree remove` against
/// the repository's `local_path` and to remove the worktree on disk.
pub(crate) async fn list_workarea_repos(
    persistence: &Arc<Persistence>,
    workarea_id: &WorkareaId,
) -> Result<Vec<(PathBuf, PathBuf)>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT wr.worktree_path, r.local_path
         FROM workarea_repos wr
         JOIN repositories r ON r.id = wr.repository_id
         WHERE wr.workarea_id = ?",
    )
    .bind(&workarea_id.0)
    .fetch_all(persistence.readers())
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                PathBuf::from(r.get::<String, _>("worktree_path")),
                PathBuf::from(r.get::<String, _>("local_path")),
            )
        })
        .collect())
}

/// Tear down every repo's worktree for a workarea + the workarea root
/// directory itself. Called from the archive path when
/// `ArchiveOpts.remove_worktree` is `true`.
pub(crate) async fn remove_worktrees_and_root(
    persistence: &Arc<Persistence>,
    workarea_id: &WorkareaId,
    worktree_root: &Path,
) -> Result<()> {
    let repos = list_workarea_repos(persistence, workarea_id).await?;
    for (worktree_path, repo_local) in repos {
        // git's bookkeeping: tell the source repo to forget the worktree
        // before we yank the directory.
        if worktree_path.exists() {
            remove_worktree_via_git(&repo_local, &worktree_path).await?;
        }
    }
    // Finally, blow away the whole workarea root (`.context/` and any
    // straggler bookkeeping). `remove_dir_all` is idempotent w.r.t.
    // already-gone paths.
    match tokio::fs::remove_dir_all(worktree_root).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::Io(e)),
    }
}

/// Re-run `git worktree add` for each repo of a previously-archived
/// workarea whose worktree was removed. Called from the restore path
/// before the DB row is updated.
pub(crate) async fn recreate_worktrees(
    persistence: &Arc<Persistence>,
    workarea_id: &WorkareaId,
    worktree_root: &Path,
    branch_name: &str,
) -> Result<()> {
    let repos = list_workarea_repos(persistence, workarea_id).await?;
    tokio::fs::create_dir_all(worktree_root).await?;
    for (worktree_path, repo_local) in repos {
        if worktree_path.exists() {
            // Already on disk (the user opted to keep the worktree at
            // archive time). Nothing to do.
            continue;
        }
        concerto_gix_wrap::worktree_add(&repo_local, branch_name, &worktree_path).await?;
    }
    Ok(())
}

/// Helper: scope the four UPDATEs of `archive_workspace` (workspace row +
/// every workarea's archive UPDATE) in one writer transaction so a
/// failure midway rolls back everything.
///
/// `now_ms` is the same timestamp stamped on the workspace and every
/// workarea — by design the cascade is atomic from the DB's perspective.
pub(crate) async fn archive_workspace_tx(
    persistence: &Arc<Persistence>,
    workspace_id: &WorkspaceId,
    workareas_to_archive: &[WorkareaId],
    now_ms: i64,
) -> Result<()> {
    let mut writer = persistence.writer().await;
    let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
    for wa_id in workareas_to_archive {
        concerto_persist::workareas::archive(&mut tx, wa_id, now_ms).await?;
    }
    concerto_persist::workspaces::archive(&mut tx, workspace_id, now_ms).await?;
    tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}
