//! Per-(workarea, repo) checkpoint creation + revert (Task 34).
//!
//! Two entrypoints:
//!
//! - [`create_checkpoint_for_workarea`] is invoked from the read-pump
//!   task whenever the parser pack emits `ParseEvent::TurnComplete`.
//!   Walks every repo attached to the workarea, snapshots the worktree
//!   into a tree+commit via `gix-wrap`, points a namespaced ref at it,
//!   and persists a `checkpoints` row. Returns one record per ref so the
//!   supervisor can emit one `AgentEvent::CheckpointCreated` per repo.
//!
//! - [`revert_workarea_to_checkpoint`] (called by
//!   `AgentSupervisorHandle::revert_to_checkpoint`) hard-resets every
//!   repo in the checkpoint's sibling set, then soft-deletes any
//!   `chat_messages` later than the checkpoint by overwriting their
//!   `superseded_by` to the checkpoint's `chat_message_id`.
//!
//! Both paths shell out to `git` via `gix-wrap` because the commit /
//! reset semantics need to match the porcelain layer exactly — `gix`'s
//! tree-builder + reference APIs would re-implement work `git
//! commit-tree` / `git reset --hard` already do correctly. The
//! per-turn cost is one fork-exec per repo, which is invisible next to
//! the agent's own latency.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use concerto_error::{Error, Result};
use concerto_persist::{
    chat_messages, checkpoints, sessions, workareas, Persistence, RepositoryId, SessionId,
    WorkareaId,
};
use sqlx::Connection;

/// Frozen ref-name scheme for V0.1.
const REF_PREFIX: &str = "refs/concerto/checkpoints";

/// One record per ref a single [`create_checkpoint_for_workarea`] call
/// wrote. Mirrors the persisted `checkpoints` row's identity-relevant
/// columns so callers (the read pump) can emit one
/// `AgentEvent::CheckpointCreated` per ref without re-reading the DB.
#[derive(Debug, Clone)]
pub struct CheckpointRecord {
    pub checkpoint_id: String,
    pub repository_id: RepositoryId,
    pub git_ref: String,
}

/// Create a checkpoint for every repo attached to `workarea_id`.
///
/// `chat_message_id` ties sibling rows together so revert can reverse
/// the whole turn atomically. `session_id` is included in the commit
/// message for debugging only — checkpoint refs are invisible to git
/// porcelain by default (no `refs/heads/*` or `refs/tags/*`).
///
/// Errors propagate as `Error::Git` (commit / ref-update failure) or
/// `Error::Sqlx` (persistence write failure). The caller treats this as
/// best-effort: a failure inside the read pump's spawned task is
/// logged at WARN and the read pump keeps reading.
pub async fn create_checkpoint_for_workarea(
    persistence: &Persistence,
    workarea_id: &WorkareaId,
    chat_message_id: &str,
    session_id: &SessionId,
) -> Result<Vec<CheckpointRecord>> {
    let repos = workareas::list_workarea_repos(persistence.readers(), workarea_id).await?;
    if repos.is_empty() {
        // Defensive: a workarea with no repos can't checkpoint. V0.1
        // single-repo invariant means this is exceedingly rare; surface
        // as Ok([]) so callers don't error a perfectly valid no-op.
        return Ok(Vec::new());
    }

    let now_ms = now_unix_ms();
    let mut records = Vec::with_capacity(repos.len());

    for (repository_id, worktree_path) in repos {
        let worktree = Path::new(&worktree_path);
        // Compute next monotonic n for this (workarea, repo).
        let max_n =
            checkpoints::max_n_for(persistence.readers(), workarea_id, &repository_id).await?;
        let n = max_n + 1;
        let git_ref = format!("{REF_PREFIX}/{}/{}/{}", workarea_id.0, repository_id.0, n);

        // Snapshot the worktree state.
        let message = format!("concerto checkpoint {n} for {session_id}");
        let commit_oid = concerto_gix_wrap::commit_index(worktree, &message)
            .await
            .map_err(|e| Error::Git(format!("checkpoint commit_index: {e}")))?;
        concerto_gix_wrap::update_ref(worktree, &git_ref, &commit_oid)
            .await
            .map_err(|e| Error::Git(format!("checkpoint update_ref: {e}")))?;

        // Persist the row.
        let checkpoint_id = uuid::Uuid::now_v7().to_string();
        {
            let mut writer = persistence.writer().await;
            let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            checkpoints::insert(
                &mut tx,
                checkpoints::NewCheckpoint {
                    id: checkpoint_id.clone(),
                    workarea_id: workarea_id.clone(),
                    repository_id: repository_id.clone(),
                    chat_message_id: chat_message_id.to_string(),
                    git_ref: git_ref.clone(),
                    created_at: now_ms,
                    diff_stats_json: None,
                },
            )
            .await?;
            tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        }

        records.push(CheckpointRecord {
            checkpoint_id,
            repository_id,
            git_ref,
        });
    }

    Ok(records)
}

/// Hard-reset every repo in the checkpoint's sibling set and
/// soft-delete chat messages after the checkpoint.
///
/// Caller (the [`crate::agent_supervisor::AgentSupervisorHandle`])
/// stops live sessions on the workarea BEFORE calling this — a hard
/// reset under a running agent corrupts the agent's expectation of the
/// worktree.
///
/// `chat_id` is the conversation thread to scope the soft-delete to;
/// it's looked up by the supervisor from `session_id` and passed in
/// here. The checkpoint's `chat_message_id` is the new tip — every
/// message later than the checkpoint gets `superseded_by` overwritten
/// to point at the checkpoint's message.
///
/// Returns the number of repos reset.
pub async fn revert_workarea_to_checkpoint(
    persistence: &Persistence,
    checkpoint_id: &str,
    chat_id: &str,
) -> Result<usize> {
    let pool = persistence.readers();
    let head = checkpoints::get(pool, checkpoint_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("checkpoint {checkpoint_id} not found")))?;

    let siblings = checkpoints::get_with_siblings(pool, &head.chat_message_id).await?;
    // Defensive: `get_with_siblings` returns the head's row too. If
    // upstream changes ever cause an empty result, fall back to
    // resetting just the head's repo.
    let to_reset = if siblings.is_empty() {
        vec![head.clone()]
    } else {
        siblings
    };

    // Resolve each repo's worktree path so we know where to `git reset`.
    let repo_paths = workareas::list_workarea_repos(pool, &head.workarea_id).await?;
    let mut n_reset = 0usize;
    for cp in &to_reset {
        let worktree = repo_paths
            .iter()
            .find_map(|(rid, path)| {
                if rid.0 == cp.repository_id.0 {
                    Some(path.clone())
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "no workarea_repos row for ({}, {}) — cannot resolve worktree path",
                    cp.workarea_id, cp.repository_id
                ))
            })?;
        concerto_gix_wrap::hard_reset(Path::new(&worktree), &cp.git_ref)
            .await
            .map_err(|e| Error::Git(format!("revert hard_reset: {e}")))?;
        n_reset += 1;
    }

    // Soft-delete every chat message later than the checkpoint.
    {
        let mut writer = persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        chat_messages::soft_delete_after(&mut tx, chat_id, head.created_at, &head.chat_message_id)
            .await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
    }

    Ok(n_reset)
}

/// Look up the `chat_id` for `session_id`. Returns `None` when the
/// session row is missing (a logic error — the supervisor only calls
/// this for sessions it just created).
pub async fn chat_id_for_session(
    persistence: &Persistence,
    session_id: &SessionId,
) -> Result<Option<String>> {
    let row = sessions::get(persistence.readers(), session_id).await?;
    Ok(row.map(|s| s.chat_id))
}

/// Insert a placeholder `chat_messages` row that the V0.1 read-pump can
/// reference as the `chat_message_id` for a turn-complete-triggered
/// checkpoint.
///
/// V0.1 doesn't yet parse the agent's per-turn message into structured
/// chat-history rows (the V1.0 structured parser is the authoritative
/// path — `tasks/33` notes this as deferred). We need *some* row to
/// satisfy the `checkpoints.chat_message_id NOT NULL REFERENCES
/// chat_messages(id)` FK, so the supervisor writes a synthetic
/// assistant-role row at turn-complete time with a marker
/// `content_json` payload. The schema is loose enough that this stays
/// invisible to other readers; V1.0's real parser will write the
/// equivalent row with real content.
///
/// Returns the new `chat_messages.id`. Best-effort — a failure here
/// surfaces as `Error::Sqlx` so the read-pump can log + skip the
/// checkpoint.
pub async fn insert_turn_message(persistence: &Persistence, chat_id: &str) -> Result<String> {
    let id = uuid::Uuid::now_v7().to_string();
    let now_ms = now_unix_ms();
    let mut writer = persistence.writer().await;
    let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
    chat_messages::insert(
        &mut tx,
        chat_messages::NewChatMessage {
            id: id.clone(),
            chat_id: chat_id.to_string(),
            role: "assistant".to_string(),
            content_json: r#"{"v0_1_turn_marker":true}"#.to_string(),
            created_at: now_ms,
            parent_id: None,
            superseded_by: None,
            metadata: None,
        },
    )
    .await?;
    tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
