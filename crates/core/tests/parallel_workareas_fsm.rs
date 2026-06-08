//! Task 307: the workarea status FSM wired into the Workarea Manager.
//!
//! In-process tests against a `WorkareaManager` over a tempdir DB (no agent
//! host needed — the FSM funnel + session-event mapping are pure given a
//! seeded `workareas`/`sessions` table). Covers:
//!
//! - `transition_workarea` drives `active → running → awaiting → running →
//!   finished` and broadcasts `StatusChanged` on each step;
//! - an illegal transition returns a typed `FAILED_PRECONDITION`
//!   (`Error::Policy`) carrying the FSM wire code, never a panic;
//! - `apply_session_event` maps `AgentEvent`s onto the funnel and only
//!   reaches `finished` once no live session row remains (union-of-sessions);
//! - two parallel workareas on one workspace transition independently;
//! - `pause_workarea` / `resume_workarea` (hard pause → `paused` → `active`).
//!
//! The `partial`-on-multi-repo-create path is covered end-to-end (real
//! `git worktree add`) by `workarea_lifecycle.rs`'s
//! `partial_create_*` test against the subprocess harness.

#![cfg(unix)]

use std::sync::Arc;

use concerto_core::agent_supervisor::{AgentEvent, MessageRole};
use concerto_core::repo_manager::RepoManager;
use concerto_core::workspace_manager::fsm::{WorkareaEvent as Fsm, INVALID_TRANSITION_WIRE_CODE};
use concerto_core::workspace_manager::{WorkareaEvent, WorkareaManager};
use concerto_persist::{Persistence, PersistenceConfig, SessionId, WorkareaId};
use tempfile::TempDir;

async fn make_manager() -> (TempDir, Arc<Persistence>, WorkareaManager) {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    let db_path = data_dir.join("concerto.db");
    let persistence = Arc::new(
        Persistence::open(PersistenceConfig {
            db_path,
            max_readers: 2,
        })
        .await
        .expect("open"),
    );
    let repo_manager = RepoManager::new(Arc::clone(&persistence), tmp.path().join("repos"));
    let mgr = WorkareaManager::new(
        Arc::clone(&persistence),
        repo_manager,
        Arc::new(data_dir),
        Arc::new(tmp.path().join("config")),
    );
    (tmp, persistence, mgr)
}

/// Seed a project + repo + workspace once.
async fn seed_parents(persistence: &Persistence) {
    let mut w = persistence.writer().await;
    sqlx::query(
        "INSERT INTO repositories (id, name, url, local_path, clone_strategy, default_branch)
         VALUES ('r', 'r', 'file:///tmp/r', '/tmp/r', 'full', 'main')",
    )
    .execute(&mut *w)
    .await
    .expect("repo");
    sqlx::query("INSERT INTO workspaces (id, name, slug, created_at) VALUES ('ws', 'ws', 'ws', 0)")
        .execute(&mut *w)
        .await
        .expect("workspace");
}

/// Seed one `workareas` row (status `active`) with a distinct composer.
async fn seed_workarea(persistence: &Persistence, id: &str, composer: &str) -> WorkareaId {
    let mut w = persistence.writer().await;
    sqlx::query(
        "INSERT INTO workareas (id, workspace_id, composer_name, branch_name, worktree_root, status, created_at)
         VALUES (?, 'ws', ?, ?, ?, 'active', 0)",
    )
    .bind(id)
    .bind(composer)
    .bind(format!("concerto/{composer}"))
    .bind(format!("/tmp/wt/{id}"))
    .execute(&mut *w)
    .await
    .expect("workarea");
    WorkareaId(id.to_string())
}

/// Insert a live session row (`ended_at IS NULL`) for a workarea. Seeds
/// the required `chats` parent row on first use.
async fn seed_live_session(persistence: &Persistence, sid: &str, workarea: &str) {
    let mut w = persistence.writer().await;
    // chats(id) is a NOT NULL FK on sessions; create one per workarea
    // (id == workarea for simplicity). Use kind='maestro' so the chats
    // CHECK (`session_id IS NOT NULL OR kind = 'maestro'`) is satisfied
    // without the circular session↔chat reference — this row only needs
    // to exist as a valid `chat_id` FK target for the test session.
    // `INSERT OR IGNORE` so repeated sessions on one workarea reuse it.
    sqlx::query("INSERT OR IGNORE INTO chats (id, kind, created_at) VALUES (?, 'maestro', 0)")
        .bind(workarea)
        .execute(&mut *w)
        .await
        .expect("chat");
    sqlx::query(
        "INSERT INTO sessions (id, workarea_id, chat_id, agent_kind, status, started_at)
         VALUES (?, ?, ?, 'claude', 'running', 0)",
    )
    .bind(sid)
    .bind(workarea)
    .bind(workarea)
    .execute(&mut *w)
    .await
    .expect("session");
}

/// Mark a session ended (`ended_at` set) — the union-of-sessions probe.
async fn end_session(persistence: &Persistence, sid: &str) {
    let mut w = persistence.writer().await;
    sqlx::query("UPDATE sessions SET ended_at = 1, status = 'finished' WHERE id = ?")
        .bind(sid)
        .execute(&mut *w)
        .await
        .expect("end session");
}

async fn status_of(persistence: &Persistence, id: &WorkareaId) -> String {
    concerto_persist::workareas::get(persistence.readers(), id)
        .await
        .expect("get")
        .expect("row")
        .status
}

#[tokio::test(flavor = "multi_thread")]
async fn transition_workarea_drives_active_running_awaiting_running_finished() {
    let (_tmp, persistence, mgr) = make_manager().await;
    seed_parents(&persistence).await;
    let wa = seed_workarea(&persistence, "wa", "bach").await;

    let mut rx = mgr.subscribe();

    // active → running.
    let r = mgr
        .transition_workarea(&wa, Fsm::SessionStarted)
        .await
        .unwrap();
    assert_eq!(r.status, "running");
    assert_eq!(status_of(&persistence, &wa).await, "running");

    // running → awaiting.
    mgr.transition_workarea(&wa, Fsm::SessionAwaiting)
        .await
        .unwrap();
    assert_eq!(status_of(&persistence, &wa).await, "awaiting");

    // awaiting → running.
    mgr.transition_workarea(&wa, Fsm::SessionResumed)
        .await
        .unwrap();
    assert_eq!(status_of(&persistence, &wa).await, "running");

    // running → finished.
    mgr.transition_workarea(&wa, Fsm::SessionFinished)
        .await
        .unwrap();
    assert_eq!(status_of(&persistence, &wa).await, "finished");

    // Each transition broadcast a StatusChanged with the right to-status.
    let mut tos = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let WorkareaEvent::StatusChanged { to, .. } = ev {
            tos.push(to);
        }
    }
    assert_eq!(tos, vec!["running", "awaiting", "running", "finished"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn illegal_transition_returns_failed_precondition_not_panic() {
    let (_tmp, persistence, mgr) = make_manager().await;
    seed_parents(&persistence).await;
    let wa = seed_workarea(&persistence, "wa", "bach").await;

    // `active` has no `SessionAwaiting` edge → soft reject.
    let err = mgr
        .transition_workarea(&wa, Fsm::SessionAwaiting)
        .await
        .expect_err("must reject");
    match &err {
        concerto_error::Error::Policy(msg) => {
            assert!(
                msg.contains(INVALID_TRANSITION_WIRE_CODE),
                "Policy error must carry the FSM wire code; got {msg}"
            );
        }
        other => panic!("expected Error::Policy (FAILED_PRECONDITION), got {other:?}"),
    }
    // The Policy error maps to FAILED_PRECONDITION over gRPC.
    let status = concerto_core::error_map::error_to_status(err);
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    // Status unchanged after a rejected transition.
    assert_eq!(status_of(&persistence, &wa).await, "active");
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_session_event_only_finishes_when_no_live_session_remains() {
    let (_tmp, persistence, mgr) = make_manager().await;
    seed_parents(&persistence).await;
    let wa = seed_workarea(&persistence, "wa", "bach").await;

    // Two live sessions; move workarea to running.
    seed_live_session(&persistence, "s1", "wa").await;
    seed_live_session(&persistence, "s2", "wa").await;
    mgr.transition_workarea(&wa, Fsm::SessionStarted)
        .await
        .unwrap();
    assert_eq!(status_of(&persistence, &wa).await, "running");

    // s1 exits but s2 is still live → workarea must stay `running`.
    end_session(&persistence, "s1").await;
    mgr.apply_session_event(
        &wa,
        &AgentEvent::Exited {
            session_id: SessionId("s1".to_string()),
            exit_code: Some(0),
            signal: None,
        },
    )
    .await;
    assert_eq!(
        status_of(&persistence, &wa).await,
        "running",
        "must not finish while a sibling session is still live"
    );

    // s2 exits → now no live session → workarea finishes.
    end_session(&persistence, "s2").await;
    mgr.apply_session_event(
        &wa,
        &AgentEvent::Exited {
            session_id: SessionId("s2".to_string()),
            exit_code: Some(0),
            signal: None,
        },
    )
    .await;
    assert_eq!(status_of(&persistence, &wa).await, "finished");
}

#[tokio::test(flavor = "multi_thread")]
async fn apply_session_event_maps_started_awaiting_resumed_crashed() {
    let (_tmp, persistence, mgr) = make_manager().await;
    seed_parents(&persistence).await;
    let wa = seed_workarea(&persistence, "wa", "bach").await;
    let sid = SessionId("s".to_string());

    mgr.apply_session_event(
        &wa,
        &AgentEvent::Started {
            session_id: sid.clone(),
        },
    )
    .await;
    assert_eq!(status_of(&persistence, &wa).await, "running");

    mgr.apply_session_event(
        &wa,
        &AgentEvent::AwaitingApproval {
            session_id: sid.clone(),
            approval_id: "a".into(),
            tool: "t".into(),
            summary: "s".into(),
            payload_json: "{}".into(),
            urgent: false,
            destructive_label: None,
        },
    )
    .await;
    assert_eq!(status_of(&persistence, &wa).await, "awaiting");

    mgr.apply_session_event(
        &wa,
        &AgentEvent::ApprovalResolved {
            session_id: sid.clone(),
            approval_id: "a".into(),
            tool: "t".into(),
            decision: "approve".into(),
        },
    )
    .await;
    assert_eq!(status_of(&persistence, &wa).await, "running");

    mgr.apply_session_event(
        &wa,
        &AgentEvent::Crashed {
            session_id: sid.clone(),
        },
    )
    .await;
    assert_eq!(status_of(&persistence, &wa).await, "crashed");

    // A non-FSM event (Message) is a no-op.
    mgr.apply_session_event(
        &wa,
        &AgentEvent::Message {
            session_id: sid,
            role: MessageRole::Assistant,
            content: "hi".into(),
        },
    )
    .await;
    assert_eq!(status_of(&persistence, &wa).await, "crashed");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_parallel_workareas_transition_independently() {
    let (_tmp, persistence, mgr) = make_manager().await;
    seed_parents(&persistence).await;
    // Two workareas on the SAME workspace, distinct composers (parallel
    // attempts, e.g. `bach` + `mozart`).
    let bach = seed_workarea(&persistence, "wa-bach", "bach").await;
    let mozart = seed_workarea(&persistence, "wa-mozart", "mozart").await;

    // Drive bach → running, leave mozart untouched.
    mgr.transition_workarea(&bach, Fsm::SessionStarted)
        .await
        .unwrap();
    assert_eq!(status_of(&persistence, &bach).await, "running");
    assert_eq!(status_of(&persistence, &mozart).await, "active");

    // Drive mozart → running → awaiting; bach must stay running.
    mgr.transition_workarea(&mozart, Fsm::SessionStarted)
        .await
        .unwrap();
    mgr.transition_workarea(&mozart, Fsm::SessionAwaiting)
        .await
        .unwrap();
    assert_eq!(status_of(&persistence, &mozart).await, "awaiting");
    assert_eq!(status_of(&persistence, &bach).await, "running");
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_then_resume_round_trips_active() {
    let (_tmp, persistence, mgr) = make_manager().await;
    seed_parents(&persistence).await;
    let wa = seed_workarea(&persistence, "wa", "bach").await;

    // No agent supervisor attached → stop_live_sessions is a best-effort
    // no-op; the FSM transition is what we assert.
    let paused = mgr.pause_workarea(&wa).await.unwrap();
    assert_eq!(paused.status, "paused");

    let resumed = mgr.resume_workarea(&wa).await.unwrap();
    assert_eq!(resumed.status, "active");
}
