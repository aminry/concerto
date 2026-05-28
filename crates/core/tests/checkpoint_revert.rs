//! Integration test for Task 34's per-repo checkpoints + revert.
//!
//! Three checks:
//!
//! 1. After a synthetic `TurnComplete`, the supervisor writes a row to
//!    `checkpoints` and points
//!    `refs/concerto/checkpoints/<workarea>/<repo>/<n>` at the worktree
//!    snapshot. The synthetic boundary uses
//!    [`AgentSupervisorHandle::synthesize_turn_complete`] — production
//!    sessions reach the same branch via a parser pack's
//!    `ParseEvent::TurnComplete`, but V0.1's terminal-mode echo pack
//!    never emits it, so the test drives the boundary directly.
//!
//! 2. Two successive synthetic turn-completes produce two
//!    monotonically-numbered refs.
//!
//! 3. Reverting to the first checkpoint hard-resets the worktree's
//!    HEAD to the first checkpoint's commit OID and soft-deletes
//!    chat_messages later than the checkpoint.
//!
//! The test spins up a tempdir-backed git repo with one initial commit
//! and one tracked file. Between checkpoints we modify the file so a
//! revert is detectable on disk.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use concerto_core::agent_supervisor::{
    AgentEvent, AgentKind, AgentSupervisorHandle, StartSessionRequest,
};
use concerto_persist::{
    chat_messages::{self, NewChatMessage},
    Persistence, PersistenceConfig, SessionId, WorkareaId,
};
use tempfile::TempDir;
use tokio::process::Command;

/// Build a `Persistence` over a tempdir DB.
async fn make_persistence() -> (TempDir, Arc<Persistence>, PathBuf) {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    let db_path = data_dir.join("concerto.db");
    let cfg = PersistenceConfig {
        db_path,
        max_readers: 2,
    };
    let p = Arc::new(Persistence::open(cfg).await.expect("open persistence"));
    (tmp, p, data_dir)
}

/// Run `git` in `cwd` and panic on non-zero exit.
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
        .output()
        .await
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Read `git rev-parse HEAD` for `repo_dir`.
async fn rev_parse_head(repo_dir: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_dir)
        .output()
        .await
        .expect("spawn rev-parse");
    assert!(out.status.success(), "rev-parse failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Probe whether a ref exists via `git rev-parse --verify --quiet`.
async fn ref_exists(repo_dir: &Path, ref_name: &str) -> bool {
    let out = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", ref_name])
        .current_dir(repo_dir)
        .output()
        .await
        .expect("spawn rev-parse --verify");
    out.status.success()
}

/// Seed projects/repositories/workspaces/workspace_repos/workareas/workarea_repos
/// for a workarea pointing at `worktree_path`. Returns the workarea id.
async fn seed_workarea(persistence: &Persistence, worktree_path: &Path) -> WorkareaId {
    let mut writer = persistence.writer().await;
    let now: i64 = 0;
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, ?, ?)")
        .bind("proj-1")
        .bind("test-project")
        .bind(now)
        .execute(&mut *writer)
        .await
        .expect("insert project");
    sqlx::query(
        "INSERT INTO repositories (id, project_id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, ?, 'full', 'main')",
    )
    .bind("repo-1")
    .bind("proj-1")
    .bind("repo-name")
    .bind("file:///tmp/fake")
    .bind(worktree_path.to_string_lossy().into_owned())
    .execute(&mut *writer)
    .await
    .expect("insert repository");
    sqlx::query(
        "INSERT INTO workspaces (id, project_id, name, slug, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind("ws-1")
    .bind("proj-1")
    .bind("ws-1")
    .bind("ws-1")
    .bind(now)
    .execute(&mut *writer)
    .await
    .expect("insert workspace");
    sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
        .bind("ws-1")
        .bind("repo-1")
        .execute(&mut *writer)
        .await
        .expect("insert workspace_repos");
    sqlx::query(
        "INSERT INTO workareas (id, workspace_id, composer_name, branch_name, worktree_root, status, created_at)
         VALUES (?, ?, ?, ?, ?, 'active', ?)",
    )
    .bind("wa-1")
    .bind("ws-1")
    .bind("alpha")
    .bind("concerto/alpha")
    .bind(worktree_path.to_string_lossy().into_owned())
    .bind(now)
    .execute(&mut *writer)
    .await
    .expect("insert workarea");
    sqlx::query(
        "INSERT INTO workarea_repos (workarea_id, repository_id, worktree_path)
         VALUES (?, ?, ?)",
    )
    .bind("wa-1")
    .bind("repo-1")
    .bind(worktree_path.to_string_lossy().into_owned())
    .execute(&mut *writer)
    .await
    .expect("insert workarea_repos");
    WorkareaId("wa-1".to_string())
}

/// Build a worktree at `path` with one initial commit on `main`.
async fn make_worktree(path: &Path) {
    tokio::fs::create_dir_all(path).await.unwrap();
    git(&["init", "-b", "main", "."], path).await;
    tokio::fs::write(path.join("file.txt"), "initial\n")
        .await
        .unwrap();
    git(&["add", "file.txt"], path).await;
    git(&["commit", "-m", "initial"], path).await;
}

/// Locate the `concerto-agent-host` binary via assert_cmd.
fn host_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("concerto-agent-host")
}

/// Spawn an echo session and return its id. Waits long enough for the
/// `AgentEvent::Exited` to fire so subsequent test steps don't race
/// the read-pump's mark_ended write — but the session ENTRY stays in
/// the supervisor's in-process map (see Task 22's design: late
/// subscribers can still read replay).
async fn spawn_echo_session(
    supervisor: &AgentSupervisorHandle,
    workarea_id: &WorkareaId,
    cwd: &Path,
) -> SessionId {
    let sid = supervisor
        .start_session(StartSessionRequest {
            workarea_id: workarea_id.clone(),
            agent_kind: AgentKind::Echo,
            echo_text: Some("hello".into()),
            cwd: cwd.to_path_buf(),
            permission_mode: None,
        })
        .await
        .expect("start_session");
    // Drain until Exited; the entry stays alive for late subscribers
    // (Task 22). We need the entry to still be in the supervisor's
    // map when we call synthesize_turn_complete — and stop_session
    // would evict it.
    if let Some(mut rx) = supervisor.subscribe_events(&sid).await {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(AgentEvent::Exited { .. })) => break,
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
    }
    sid
}

/// The two checkpoint scenarios live in a single test body because
/// they both spawn `concerto-agent-host` via the supervisor's
/// `start_session`, and the V0.1 socket path fallback truncates the
/// session UUID to 8 chars (Task 22 handoff `Drift from plan`). Two
/// parallel tests can collide on `$TMPDIR/ccs-<sid8>.sock` when the
/// UUIDv7 timestamp prefixes line up — Task 33's `tool_approval` test
/// has the same constraint and is gated with `#[ignore]` to avoid
/// the issue. Serializing the scenarios here keeps the checkpoint
/// suite enabled without requiring `--test-threads=1` on the workspace.
#[tokio::test(flavor = "multi_thread")]
async fn checkpoints_and_revert_end_to_end() {
    turn_complete_writes_checkpoint_row_and_ref().await;
    two_turns_create_two_monotonic_refs_and_revert_resets_branch().await;
}

async fn turn_complete_writes_checkpoint_row_and_ref() {
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let worktree = data_dir.join("worktree");
    make_worktree(&worktree).await;
    let workarea_id = seed_workarea(&persistence, &worktree).await;
    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );

    let session_id = spawn_echo_session(&supervisor, &workarea_id, &worktree).await;

    // Subscribe BEFORE the synthetic turn so the broadcast
    // CheckpointCreated event is observable.
    let mut rx = supervisor
        .subscribe_events(&session_id)
        .await
        .expect("subscribe");

    // Modify the worktree so the checkpoint commit captures something
    // beyond the initial state.
    tokio::fs::write(worktree.join("file.txt"), "after-turn-1\n")
        .await
        .unwrap();

    supervisor
        .synthesize_turn_complete(&session_id)
        .await
        .expect("synthesize_turn_complete");

    // Drain until we see CheckpointCreated.
    let mut checkpoint_id_opt: Option<String> = None;
    let mut git_ref_opt: Option<String> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while checkpoint_id_opt.is_none() {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(AgentEvent::CheckpointCreated {
                checkpoint_id,
                git_ref,
                ..
            })) => {
                checkpoint_id_opt = Some(checkpoint_id);
                git_ref_opt = Some(git_ref);
            }
            Ok(Ok(_)) => continue,
            _ => break,
        }
    }
    let checkpoint_id = checkpoint_id_opt.expect("expected CheckpointCreated event");
    let git_ref = git_ref_opt.expect("expected git_ref on event");

    // Assert the ref exists.
    assert!(
        ref_exists(&worktree, &git_ref).await,
        "ref {git_ref} should exist after checkpoint"
    );
    assert!(
        git_ref.starts_with("refs/concerto/checkpoints/wa-1/repo-1/"),
        "ref name should follow the locked scheme; got {git_ref}"
    );

    // Assert the DB row exists.
    let row = concerto_persist::checkpoints::get(persistence.readers(), &checkpoint_id)
        .await
        .expect("get checkpoint")
        .expect("checkpoint row");
    assert_eq!(row.workarea_id.0, "wa-1");
    assert_eq!(row.repository_id.0, "repo-1");
    assert_eq!(row.git_ref, git_ref);
}

async fn two_turns_create_two_monotonic_refs_and_revert_resets_branch() {
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let worktree = data_dir.join("worktree");
    make_worktree(&worktree).await;
    let workarea_id = seed_workarea(&persistence, &worktree).await;
    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );

    let session_id = spawn_echo_session(&supervisor, &workarea_id, &worktree).await;

    // Two turns: each modifies file.txt and synthesizes a turn-complete.
    let mut checkpoint_ids: Vec<String> = Vec::new();
    let mut git_refs: Vec<String> = Vec::new();
    let mut commit_oids: Vec<String> = Vec::new();
    for i in 0..2 {
        let payload = format!("after-turn-{}\n", i + 1);
        tokio::fs::write(worktree.join("file.txt"), payload.as_bytes())
            .await
            .unwrap();
        let mut rx = supervisor
            .subscribe_events(&session_id)
            .await
            .expect("subscribe");
        supervisor
            .synthesize_turn_complete(&session_id)
            .await
            .expect("synthesize_turn_complete");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                panic!("never saw CheckpointCreated for turn {i}");
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(AgentEvent::CheckpointCreated {
                    checkpoint_id,
                    git_ref,
                    ..
                })) => {
                    let oid = Command::new("git")
                        .args(["rev-parse", &git_ref])
                        .current_dir(&worktree)
                        .output()
                        .await
                        .expect("rev-parse ref");
                    let oid = String::from_utf8_lossy(&oid.stdout).trim().to_string();
                    commit_oids.push(oid);
                    git_refs.push(git_ref);
                    checkpoint_ids.push(checkpoint_id);
                    break;
                }
                Ok(Ok(_)) => continue,
                _ => panic!("read pump closed during turn {i}"),
            }
        }
    }

    // Refs should be monotonic 1 and 2.
    assert!(
        git_refs[0].ends_with("/1"),
        "first ref tail; got {}",
        git_refs[0]
    );
    assert!(
        git_refs[1].ends_with("/2"),
        "second ref tail; got {}",
        git_refs[1]
    );

    // Add a chat_message after the FIRST checkpoint so the revert's
    // soft-delete has a row to flip. Use the session's chat_id.
    let session = concerto_persist::sessions::get(persistence.readers(), &session_id)
        .await
        .unwrap()
        .expect("session row");
    let cp1 = concerto_persist::checkpoints::get(persistence.readers(), &checkpoint_ids[0])
        .await
        .unwrap()
        .expect("cp1 row");
    let after_msg_id = uuid::Uuid::now_v7().to_string();
    {
        let mut writer = persistence.writer().await;
        chat_messages::insert(
            &mut writer,
            NewChatMessage {
                id: after_msg_id.clone(),
                chat_id: session.chat_id.clone(),
                role: "user".to_string(),
                content_json: "{}".to_string(),
                // Strictly later than the checkpoint's created_at.
                created_at: cp1.created_at + 100,
                parent_id: None,
                superseded_by: None,
            },
        )
        .await
        .unwrap();
    }

    // Make the worktree dirty so the revert has tracked work to undo.
    tokio::fs::write(worktree.join("file.txt"), "post-revert-dirty\n")
        .await
        .unwrap();

    // Revert to checkpoint 1.
    supervisor
        .revert_to_checkpoint(&checkpoint_ids[0], &session_id)
        .await
        .expect("revert_to_checkpoint");

    // HEAD should match checkpoint 1's commit OID.
    let head_after = rev_parse_head(&worktree).await;
    assert_eq!(
        head_after, commit_oids[0],
        "branch HEAD should match checkpoint 1 OID after revert; HEAD={head_after}, cp1={}",
        commit_oids[0]
    );

    // The "after-turn-1" content from checkpoint 1 should be on disk;
    // the post-revert dirty content + scratch.txt should be gone.
    let after = tokio::fs::read_to_string(worktree.join("file.txt"))
        .await
        .unwrap();
    assert_eq!(
        after.trim(),
        "after-turn-1",
        "file.txt should hold checkpoint 1's content after revert"
    );

    // The chat_message inserted after the checkpoint should be
    // superseded by the checkpoint's chat_message_id.
    let after_row: (Option<String>,) =
        sqlx::query_as("SELECT superseded_by FROM chat_messages WHERE id = ?")
            .bind(&after_msg_id)
            .fetch_one(persistence.readers())
            .await
            .unwrap();
    assert_eq!(
        after_row.0.as_deref(),
        Some(cp1.chat_message_id.as_str()),
        "chat message after checkpoint 1 should be superseded by the checkpoint's chat_message_id"
    );
}

// Last line is left blank to match the workspace style.
