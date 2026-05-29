//! Integration tests for Task 37 — cold resume from agent JSONL.
//!
//! Strategy
//! --------
//!
//! - Build a `Persistence` over a tempdir DB, seed a workarea.
//! - Construct a supervisor handle pointed at the real
//!   `concerto-agent-host`.
//! - Start an echo session, wait for the agent to exit (echo is the
//!   fast happy path — Claude isn't installed in CI). The supervisor
//!   marks the session `finished`.
//! - Manually flip the row to `crashed` and stamp an
//!   `external_session_id` (the V0.1 parser doesn't yet extract one;
//!   tests pre-seed it).
//! - Call `cold_resume_session(&handle, &sid)`; assert the row goes
//!   back to `running`, a new host process is spawned, and the in-
//!   memory entry is fresh (no stale `finished` flag).
//! - Repeat the path WITHOUT an `external_session_id` and assert the
//!   error tag `session.no_external_id`.
//!
//! The echo agent doesn't actually understand `--resume`, but the host
//! still forwards the flag to the wrapped CLI; the `/bin/sh -c "echo X;
//! sleep 1"` wrapper script ignores the unknown arg and prints the
//! marker as usual. The test verifies the *spawn-and-handshake* cycle
//! — the agent-CLI-level resume semantics are covered by the agent
//! CLI's own test suite (out of scope for Concerto).

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use concerto_core::agent_supervisor::{
    cold_resume_session, AgentEvent, AgentKind, AgentSupervisorHandle, StartSessionRequest,
};
use concerto_persist::{Persistence, PersistenceConfig, SessionId, WorkareaId};
use tempfile::TempDir;

async fn make_persistence() -> (TempDir, Arc<Persistence>, PathBuf) {
    let tmp = tempfile::Builder::new()
        .prefix("ccs-")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let data_dir = tmp.path().join("d");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    let db_path = data_dir.join("concerto.db");
    let cfg = PersistenceConfig {
        db_path,
        max_readers: 2,
    };
    let p = Arc::new(Persistence::open(cfg).await.expect("open persistence"));
    (tmp, p, data_dir)
}

async fn seed_workarea(persistence: &Persistence, worktree_root: &str) -> WorkareaId {
    let mut writer = persistence.writer().await;
    let now: i64 = 0;
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, ?, ?)")
        .bind("proj-1")
        .bind("test-project")
        .bind(now)
        .execute(&mut *writer)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO repositories (id, project_id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, ?, 'full', 'main')",
    )
    .bind("repo-1")
    .bind("proj-1")
    .bind("repo-name")
    .bind("file:///tmp/fake")
    .bind("/tmp/fake")
    .execute(&mut *writer)
    .await
    .unwrap();
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
    .unwrap();
    sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
        .bind("ws-1")
        .bind("repo-1")
        .execute(&mut *writer)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO workareas (id, workspace_id, composer_name, branch_name, worktree_root, status, created_at)
         VALUES (?, ?, ?, ?, ?, 'active', ?)",
    )
    .bind("wa-1")
    .bind("ws-1")
    .bind("alpha")
    .bind("concerto/alpha")
    .bind(worktree_root)
    .bind(now)
    .execute(&mut *writer)
    .await
    .unwrap();
    WorkareaId("wa-1".to_string())
}

fn host_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("concerto-agent-host")
}

/// Wait for an event matching `pred` on `rx` or until the budget elapses.
async fn wait_for_event<F>(
    rx: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
    pred: F,
    budget: Duration,
) -> bool
where
    F: Fn(&AgentEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ev)) => {
                if pred(&ev) {
                    return true;
                }
            }
            _ => return false,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_resume_respawns_host_and_returns_running() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_test_writer()
        .try_init();

    let (_tmp, persistence, data_dir) = make_persistence().await;
    let cwd = data_dir.clone();
    tokio::fs::create_dir_all(&cwd).await.unwrap();
    let workarea_id = seed_workarea(&persistence, cwd.to_string_lossy().as_ref()).await;
    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );

    // Phase 1: spawn an echo session and wait for it to finish so we
    // have a real `sessions` row to cold-resume against.
    let original_sid = supervisor
        .start_session(StartSessionRequest {
            workarea_id: workarea_id.clone(),
            agent_kind: AgentKind::Echo,
            echo_text: Some("MARKER-PHASE1".to_string()),
            cwd: cwd.clone(),
            permission_mode: None,
            resume_session_id: None,
        })
        .await
        .expect("start_session");

    {
        let mut rx = supervisor
            .subscribe_events(&original_sid)
            .await
            .expect("subscribe");
        let saw_exit = wait_for_event(
            &mut rx,
            |ev| matches!(ev, AgentEvent::Exited { .. }),
            Duration::from_secs(10),
        )
        .await;
        assert!(saw_exit, "expected echo session to emit Exited");
    }

    // Phase 2: simulate the cold case. The agent has exited and Task
    // 36's `mark_ended` has flipped the row to `finished`. Manually
    // pretend the host crashed (status='crashed', external_session_id
    // populated, ended_at cleared) so cold_resume has a row to act on.
    let token = "ext-session-abc-123";
    {
        let mut w = persistence.writer().await;
        sqlx::query(
            "UPDATE sessions
             SET status = 'crashed',
                 external_session_id = ?,
                 ended_at = NULL
             WHERE id = ?",
        )
        .bind(token)
        .bind(&original_sid.0)
        .execute(&mut *w)
        .await
        .expect("mutate row");
    }
    // The in-memory entry from phase 1 still lives in the supervisor
    // map (`finished=true`, replay buffer drained). cold_resume_existing
    // evicts it as part of the re-spawn — the test verifies the
    // resulting entry is fresh.

    // Phase 3: cold resume.
    let resumed_sid = cold_resume_session(&supervisor, &original_sid)
        .await
        .expect("cold_resume_session");
    assert_eq!(
        resumed_sid, original_sid,
        "cold resume must reuse the original session id"
    );

    // Row should be `running` with `ended_at = NULL`.
    let row = concerto_persist::sessions::get(persistence.readers(), &original_sid)
        .await
        .unwrap()
        .expect("session row after cold resume");
    assert_eq!(row.status, "running", "row status after cold resume");
    assert!(
        row.ended_at.is_none(),
        "ended_at should be cleared after cold resume"
    );
    assert_eq!(
        row.external_session_id.as_deref(),
        Some(token),
        "external_session_id must be preserved"
    );

    // The supervisor map must hold a fresh entry. A new subscribe
    // should succeed and the post-resume `Started` event must be on
    // the replay buffer.
    let (replay, _rx) = supervisor
        .subscribe_events_with_replay(&original_sid)
        .await
        .expect("subscribe via supervisor after cold resume");
    let saw_started = replay
        .iter()
        .any(|ev| matches!(ev, AgentEvent::Started { .. }));
    assert!(
        saw_started,
        "expected Started event on replay buffer of resumed session"
    );

    // Teardown: stop the fresh session so the test process exits clean.
    let _ = supervisor.stop_session(&original_sid, None).await;
    // Give the read pump's mark_ended write room to land before drop.
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_resume_errors_when_external_session_id_missing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_test_writer()
        .try_init();

    let (_tmp, persistence, data_dir) = make_persistence().await;
    let cwd = data_dir.clone();
    let workarea_id = seed_workarea(&persistence, cwd.to_string_lossy().as_ref()).await;
    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );

    let sid = supervisor
        .start_session(StartSessionRequest {
            workarea_id: workarea_id.clone(),
            agent_kind: AgentKind::Echo,
            echo_text: Some("MARKER-NO-EXT".to_string()),
            cwd: cwd.clone(),
            permission_mode: None,
            resume_session_id: None,
        })
        .await
        .expect("start_session");

    // Wait for exit.
    {
        let mut rx = supervisor.subscribe_events(&sid).await.expect("subscribe");
        let _ = wait_for_event(
            &mut rx,
            |ev| matches!(ev, AgentEvent::Exited { .. }),
            Duration::from_secs(10),
        )
        .await;
    }
    {
        let mut w = persistence.writer().await;
        sqlx::query(
            "UPDATE sessions SET status='crashed', external_session_id=NULL, ended_at=NULL WHERE id=?",
        )
        .bind(&sid.0)
        .execute(&mut *w)
        .await
        .expect("mutate row");
    }

    let err = cold_resume_session(&supervisor, &sid)
        .await
        .expect_err("must error without external_session_id");
    let msg = format!("{err}");
    assert!(
        msg.contains("session.no_external_id"),
        "expected wire code session.no_external_id; got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cold_resume_unknown_session_returns_not_found() {
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let _ = seed_workarea(&persistence, data_dir.to_string_lossy().as_ref()).await;
    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );
    let err = cold_resume_session(&supervisor, &SessionId("does-not-exist".into()))
        .await
        .expect_err("must error");
    assert!(
        matches!(err, concerto_error::Error::NotFound(_)),
        "expected NotFound; got {err:?}"
    );
}
