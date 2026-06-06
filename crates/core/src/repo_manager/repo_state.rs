//! Repo-local `concerto-state.json` read-modify-write (Task 301).
//!
//! `design/02 §4` defines a small durable, repo-scoped JSON file that does
//! NOT live in SQLite — it travels with the repo directory if copied. The
//! file sits at `<repo.local_path>/.git/concerto-state.json` and carries:
//!
//! ```json
//! {
//!     "last_fetch_at": 1716800000,
//!     "last_maintenance_at": 1716700000,
//!     "prefetch_cursor": "<commit-sha>",
//!     "size_bytes": 42000000000,
//!     "object_count": 18000000
//! }
//! ```
//!
//! Task 301 writes `size_bytes` / `object_count` after a successful clone.
//! Every write is a **read-modify-write** so a future field (Task 304's
//! `prefetch_cursor`) added by another code path is never clobbered:
//! unknown keys are preserved verbatim via `serde_json::Value` (the
//! `#[serde(flatten)]` `extra` map below).
//!
//! Errors are surfaced as [`Error::Internal`] — the caller treats a
//! state-file write failure as non-fatal (best-effort durable telemetry,
//! not correctness state).

use std::path::Path;

use concerto_error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Filename under the repo's `.git/` directory (`design/02 §4`).
const STATE_FILE: &str = "concerto-state.json";

/// Typed view of `concerto-state.json`. Known fields are explicit; any
/// other keys round-trip through `extra` so a read-modify-write by one
/// task never drops a field written by another.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RepoState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_fetch_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_maintenance_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefetch_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_count: Option<u64>,
    /// Any keys this binary doesn't model — preserved on rewrite.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Absolute path to a repo's state file (`<local_path>/.git/concerto-state.json`).
fn state_path(repo_local_path: &Path) -> std::path::PathBuf {
    repo_local_path.join(".git").join(STATE_FILE)
}

/// Read the current state, or [`RepoState::default`] when the file is
/// absent. A present-but-corrupt file is an [`Error::Internal`] so a
/// caller doesn't silently overwrite a manually-edited file.
async fn read(repo_local_path: &Path) -> Result<RepoState> {
    let path = state_path(repo_local_path);
    match tokio::fs::read(&path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| Error::Internal(format!("parse {}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(RepoState::default()),
        Err(e) => Err(Error::Internal(format!("read {}: {e}", path.display()))),
    }
}

/// Pretty-print and write the state file (creating `.git/` if needed).
async fn write(repo_local_path: &Path, state: &RepoState) -> Result<()> {
    let path = state_path(repo_local_path);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::Internal(format!("create_dir_all({}): {e}", parent.display())))?;
    }
    let json = serde_json::to_vec_pretty(state)
        .map_err(|e| Error::Internal(format!("serialize concerto-state.json: {e}")))?;
    tokio::fs::write(&path, json)
        .await
        .map_err(|e| Error::Internal(format!("write {}: {e}", path.display())))?;
    Ok(())
}

/// Read-modify-write `size_bytes` + `object_count` into the repo's state
/// file (Task 301). Existing / unknown fields are preserved.
pub(crate) async fn record_size(
    repo_local_path: &Path,
    size_bytes: u64,
    object_count: u64,
) -> Result<()> {
    let mut state = read(repo_local_path).await?;
    state.size_bytes = Some(size_bytes);
    state.object_count = Some(object_count);
    write(repo_local_path, &state).await
}

/// Read-modify-write the `prefetch_cursor` (the last prewarmed commit SHA)
/// into the repo's state file (Task 304). Existing fields — Task 301's
/// `size_bytes`/`object_count` and any unknown keys — are preserved.
pub(crate) async fn record_prefetch_cursor(repo_local_path: &Path, commit: &str) -> Result<()> {
    let mut state = read(repo_local_path).await?;
    state.prefetch_cursor = Some(commit.to_string());
    write(repo_local_path, &state).await
}

/// Read the current `prefetch_cursor`, or `None` when unset / the file is
/// absent (Task 304). Used by the HEAD-update trigger to skip a prewarm
/// when the cursor already matches the new HEAD.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) async fn read_prefetch_cursor(repo_local_path: &Path) -> Result<Option<String>> {
    Ok(read(repo_local_path).await?.prefetch_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn record_prefetch_cursor_preserves_size_fields() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        // Seed Task 301's size fields first.
        record_size(repo, 5000, 12).await.unwrap();
        // Then record a prefetch cursor (Task 304's read-modify-write).
        record_prefetch_cursor(repo, "deadbeef").await.unwrap();

        let state = read(repo).await.unwrap();
        assert_eq!(state.prefetch_cursor.as_deref(), Some("deadbeef"));
        // 301's fields must survive the 304 write.
        assert_eq!(state.size_bytes, Some(5000));
        assert_eq!(state.object_count, Some(12));

        // Round-trip the reader.
        assert_eq!(
            read_prefetch_cursor(repo).await.unwrap().as_deref(),
            Some("deadbeef")
        );
    }

    #[tokio::test]
    async fn record_size_after_cursor_preserves_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        record_prefetch_cursor(repo, "abc").await.unwrap();
        // 301's writer must not clobber 304's cursor.
        record_size(repo, 1, 2).await.unwrap();
        let state = read(repo).await.unwrap();
        assert_eq!(state.prefetch_cursor.as_deref(), Some("abc"));
        assert_eq!(state.size_bytes, Some(1));
    }

    #[tokio::test]
    async fn record_size_preserves_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        tokio::fs::create_dir_all(repo.join(".git")).await.unwrap();
        // Seed a file with a known + an unknown (future-task) field.
        tokio::fs::write(
            repo.join(".git").join(STATE_FILE),
            br#"{"last_fetch_at": 7, "prefetch_cursor": "abc123"}"#,
        )
        .await
        .unwrap();

        record_size(repo, 42, 9).await.unwrap();

        let state = read(repo).await.unwrap();
        assert_eq!(state.size_bytes, Some(42));
        assert_eq!(state.object_count, Some(9));
        // Read-modify-write must not clobber sibling fields.
        assert_eq!(state.last_fetch_at, Some(7));
        assert_eq!(state.prefetch_cursor.as_deref(), Some("abc123"));
    }

    #[tokio::test]
    async fn record_size_creates_file_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        record_size(repo, 100, 5).await.unwrap();
        let state = read(repo).await.unwrap();
        assert_eq!(state.size_bytes, Some(100));
        assert_eq!(state.object_count, Some(5));
    }
}
