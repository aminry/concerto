//! Integration test for the Task 23 `Sessions` and `Streams` gRPC
//! services.
//!
//! The test spawns a real `concerto-core` subprocess (via the Task 17
//! harness), seeds projects/repositories/workspaces/workspace_repos
//! directly in SQLite (the same fast-path the workarea integration test
//! uses), creates a workarea via `Workareas.CreateWorkarea`, then:
//!
//! 1. Calls `Sessions.CreateSession` with `agent_kind=echo`.
//! 2. Subscribes to `session.events.<sid>` AFTER the session id is
//!    known. The supervisor's broadcast channel does NOT replay past
//!    events to late subscribers, so the test does not assert on
//!    `AgentStarted` (it fires before subscribe and is therefore
//!    racy). Instead it asserts the `AgentMessage` carrying the echo
//!    output arrives — that's the load-bearing signal that the pipe
//!    is wired end-to-end.
//! 3. Verifies the `Session` row returned by `CreateSession` reflects
//!    the `running` state machine.
//! 4. Calls `Sessions.StopSession` and asserts the row transitions to
//!    `finished` in the DB.
//!
//! Unknown-subject coverage lives in a separate test that does not
//! need a session id.

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use concerto_proto::v1::{
    CreateSessionRequest, CreateWorkareaRequest, StopSessionRequest, SubscribeRequest,
};
use concerto_test_harness::CoreUnderTest;
use tempfile::TempDir;
use tokio::process::Command;

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
        "git {:?} failed: stderr={}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn make_bare_with_commit() -> (String, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
    let url = format!("file://{}", bare.path().display());
    git(&["remote", "add", "origin", url.as_str()], work.path()).await;
    git(&["push", "-u", "origin", "main"], work.path()).await;
    (url, bare, work)
}

struct Seeded {
    workspace_id: String,
    _bare: TempDir,
    _work: TempDir,
}

async fn seed(core: &CoreUnderTest, slug: &str) -> Seeded {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let (bare_url, bare, work) = make_bare_with_commit().await;

    let project_id = format!("proj-{slug}");
    let workspace_id = format!("ws-{slug}");
    let repo_id = format!("repo-{slug}");
    let repo_name = format!("name-{slug}");
    let local_path = core.data_dir.join("repos").join(&repo_id);

    let opts = SqliteConnectOptions::new()
        .filename(&core.db_path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open db write pool");
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, 'test', 0)")
        .bind(&project_id)
        .execute(&pool)
        .await
        .expect("insert project");
    sqlx::query(
        "INSERT INTO repositories (id, project_id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, ?, 'full', 'main')",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&repo_name)
    .bind(&bare_url)
    .bind(local_path.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .expect("insert repository");
    sqlx::query(
        "INSERT INTO workspaces (id, project_id, name, slug, created_at) VALUES (?, ?, 'test', ?, 0)",
    )
    .bind(&workspace_id)
    .bind(&project_id)
    .bind(slug)
    .execute(&pool)
    .await
    .expect("insert workspace");
    sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
        .bind(&workspace_id)
        .bind(&repo_id)
        .execute(&pool)
        .await
        .expect("insert workspace_repos");
    pool.close().await;

    // Clone the bare repo so workarea creation finds the on-disk repo.
    tokio::fs::create_dir_all(local_path.parent().unwrap())
        .await
        .unwrap();
    let out = Command::new("git")
        .args(["clone", bare_url.as_str(), &local_path.to_string_lossy()])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("git clone");
    assert!(
        out.status.success(),
        "seed clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    Seeded {
        workspace_id,
        _bare: bare,
        _work: work,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn echo_session_streams_message_and_marks_finished() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "alpha").await;

    // Create the workarea via the gRPC service so the worktree root
    // actually exists and the agent host has a real cwd.
    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    let mut sessions_client = core.sessions_client().await.expect("sessions client");
    let session = sessions_client
        .create_session(CreateSessionRequest {
            workarea_id: wa.id.clone(),
            agent_kind: "echo".to_string(),
            model: None,
            permission_mode: None,
        })
        .await
        .expect("CreateSession")
        .into_inner();
    assert!(!session.id.is_empty());
    assert_eq!(session.workarea_id, wa.id);
    assert_eq!(session.agent_kind, "claude"); // echo writes "claude" to DB per Task 22 drift.
    assert_eq!(session.status, "running");

    // Subscribe AFTER CreateSession returns. The supervisor's broadcast
    // channel does not replay past events to late subscribers, so
    // `AgentStarted` may have already fired and is therefore not a
    // reliable assertion target. The `AgentMessage` carrying the echo
    // stdout arrives once the read-pump task processes the host's
    // StdoutBytes frame — that's the signal we assert on.
    let mut streams_client = core.streams_client().await.expect("streams client");
    let subject = format!("session.events.{}", session.id);
    let mut subscribe_stream = streams_client
        .subscribe(SubscribeRequest {
            subject: subject.clone(),
            filter: None,
            since_offset: None,
        })
        .await
        .expect("Subscribe")
        .into_inner();

    let saw_message =
        drain_for_message(&mut subscribe_stream, "hello", Duration::from_secs(10)).await;
    assert!(
        saw_message,
        "expected an AgentMessage event carrying the echo payload"
    );

    // Stop the session and verify the DB row transitions to finished.
    let _ = sessions_client
        .stop_session(StopSessionRequest {
            session_id: session.id.clone(),
            reason: "user_request".to_string(),
        })
        .await;

    // Give the DB write a moment to land.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let pool = core.db().await.expect("db");
    let (status_db,): (String,) = sqlx::query_as("SELECT status FROM sessions WHERE id = ?")
        .bind(&session.id)
        .fetch_one(&pool)
        .await
        .expect("sessions row");
    assert_eq!(status_db, "finished");

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_subject_returns_invalid_argument() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let mut streams_client = core.streams_client().await.expect("streams client");
    let result = streams_client
        .subscribe(SubscribeRequest {
            subject: "nope.bogus".to_string(),
            filter: None,
            since_offset: None,
        })
        .await;
    let err = match result {
        Ok(resp) => {
            // Subscribe returns the stream eagerly; the validation
            // error must surface as the RPC's outer Result, not as a
            // stream item. If we get a Response here, the handler is
            // wrong.
            let _ = resp;
            panic!("expected Subscribe to error on unknown subject")
        }
        Err(e) => e,
    };
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(
        err.message().contains("streams.unknown_subject"),
        "error message should carry the wire code: {}",
        err.message()
    );
    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_agent_kind_returns_invalid_argument() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "delta").await;
    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    let mut sessions_client = core.sessions_client().await.expect("sessions client");
    let err = sessions_client
        .create_session(CreateSessionRequest {
            workarea_id: wa.id,
            agent_kind: "codex".to_string(),
            model: None,
            permission_mode: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert!(
        err.message().contains("agent.unsupported"),
        "error message should carry the wire code: {}",
        err.message()
    );
    core.shutdown().await.expect("shutdown");
}

/// Drain a server-streaming subscribe response until we see an
/// `AgentMessage` whose `content` contains `needle`, the stream closes,
/// or `budget` elapses. Returns whether the message was seen.
async fn drain_for_message<S>(stream: &mut S, needle: &str, budget: Duration) -> bool
where
    S: futures::Stream<Item = Result<concerto_proto::v1::Event, tonic::Status>> + Unpin,
{
    use concerto_proto::v1::event::Body;
    use concerto_proto::v1::session_event::Kind;
    use futures::StreamExt;
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let next = tokio::time::timeout(remaining, stream.next()).await;
        let Ok(Some(Ok(ev))) = next else {
            // Timeout, end-of-stream, or status error — bail; the
            // caller's assertion treats this as failure.
            return false;
        };
        let Some(Body::Session(session_ev)) = ev.body else {
            continue;
        };
        let Some(Kind::Message(msg)) = session_ev.kind else {
            continue;
        };
        let s = String::from_utf8_lossy(&msg.content);
        if s.contains(needle) {
            return true;
        }
    }
}
