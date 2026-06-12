//! Integration test for the Task 22 Agent Supervisor.
//!
//! Exercises the full echo round-trip end-to-end in-process:
//!
//! - Build a `Persistence` over a tempdir DB.
//! - Seed `projects`, `repositories`, `workspaces`, `workspace_repos`,
//!   `workareas`, and `workarea_repos` directly via sqlx. (The
//!   workarea-creation path itself is covered by Task 20's test — here
//!   we only need a valid workarea FK target.)
//! - Construct an `AgentSupervisorHandle` pointed at the real
//!   `concerto-agent-host` binary (resolved via
//!   `assert_cmd::cargo::cargo_bin`).
//! - Start an `agent_kind = Echo` session — the supervisor spawns
//!   `concerto-agent-host --agent-bin /bin/echo --agent-arg hello`.
//! - Drain the per-session broadcast and assert at least one
//!   `AgentEvent::Message` carrying `"hello"` and an `AgentEvent::Exited`
//!   show up.
//! - Assert the DB row's `status` transitions to `finished`.
//!
//! ## In-process rationale (Task 22 Drift)
//!
//! The shared test-harness (`concerto_test_harness::CoreUnderTest`)
//! spawns a real Core subprocess; the `AgentSupervisorHandle` lives
//! inside that subprocess and isn't reachable via the gRPC surface yet
//! (the `Sessions` service lands in Task 23). Running the test
//! in-process lets us assert on the handle's contract directly without
//! waiting on Task 23 or inventing a one-off harness accessor.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use concerto_core::agent_supervisor::{
    AgentEvent, AgentKind, AgentSupervisorHandle, StartSessionRequest,
};
use concerto_persist::{Persistence, PersistenceConfig, WorkareaId};
use tempfile::TempDir;

/// Process-wide guard serializing the tests that spawn a real
/// `concerto-agent-host`. Two agent-hosts coming up in the same millisecond
/// race on the supervisor's UDS handshake (the fallback socket name is keyed by
/// a short UUID-v7 prefix, which collides for near-simultaneous spawns) and one
/// Core connects to the other's host → `cookie mismatch`. This is a pre-existing
/// fragility in the test harness, latent until Task 6 added a second spawning
/// test to this binary; serializing the spawn-and-handshake window sidesteps it
/// without touching the production socket logic. Non-spawning tests
/// (`codex_kind`, `unknown_workarea`) don't take it.
fn spawn_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// RAII cleanup for a hand-rolled short tempdir (used by the Task-6 binding
/// test, which needs a `$TMPDIR`-shallow `data_dir` so the supervisor's socket
/// path stays under the macOS `sun_path` cap without the colliding fallback).
struct TmpDirGuard(PathBuf);
impl Drop for TmpDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a fresh `Persistence` over a tempdir-backed SQLite DB. Returns
/// the tempdir guard so callers can keep it alive for the test's
/// lifetime.
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

/// Seed the parents of `workareas` plus one workarea row so the
/// supervisor's workarea-existence check passes.
async fn seed_workarea(persistence: &Persistence) -> WorkareaId {
    let mut writer = persistence.writer().await;
    // SQLite errors when fed multiple statements separated by `;` via
    // the simple bind path, so issue each insert independently.
    let now: i64 = 0;
    sqlx::query(
        "INSERT INTO repositories (id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, 'full', 'main')",
    )
    .bind("repo-1")
    .bind("repo-name")
    .bind("file:///tmp/fake")
    .bind("/tmp/fake")
    .execute(&mut *writer)
    .await
    .expect("insert repository");
    sqlx::query("INSERT INTO workspaces (id, name, slug, created_at) VALUES (?, ?, ?, ?)")
        .bind("ws-1")
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
    .bind("/tmp/fake-worktree")
    .bind(now)
    .execute(&mut *writer)
    .await
    .expect("insert workarea");
    WorkareaId("wa-1".to_string())
}

/// Resolve the `concerto-agent-host` binary via `assert_cmd`. This is
/// the canonical "find a bin built by the workspace from within a test"
/// pattern — it doesn't depend on `$PATH` or the test's cwd.
fn host_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("concerto-agent-host")
}

/// Drain the broadcast receiver until either `Exited` is seen, the
/// receiver closes, or `budget` elapses. Returns every event observed
/// in order.
async fn drain_until_exit(
    mut rx: tokio::sync::broadcast::Receiver<AgentEvent>,
    budget: Duration,
) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ev)) => {
                let exit = matches!(ev, AgentEvent::Exited { .. });
                out.push(ev);
                if exit {
                    break;
                }
            }
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn echo_round_trip_streams_message_and_marks_finished() {
    let _spawn_guard = spawn_lock().lock().await;
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let workarea_id = seed_workarea(&persistence).await;
    let cwd = data_dir.clone();
    // We need the workarea's worktree_root path to actually exist as a
    // directory the host can cd into.
    tokio::fs::create_dir_all(&cwd).await.unwrap();

    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );

    let session_id = supervisor
        .start_session(StartSessionRequest {
            workarea_id: workarea_id.clone(),
            agent_kind: AgentKind::Echo,
            echo_text: Some("hello-from-echo".to_string()),
            cwd: cwd.clone(),
            permission_mode: None,
            resume_session_id: None,
            chat_id: None,
        })
        .await
        .expect("start_session");

    // Subscribe AFTER start_session returns. The `Started` event will
    // have already fired, but `Message` (from `StdoutBytes`) and
    // `Exited` arrive later as the read-pump task processes frames.
    let rx = supervisor
        .subscribe_events(&session_id)
        .await
        .expect("subscribe");
    let events = drain_until_exit(rx, Duration::from_secs(15)).await;

    let saw_message = events.iter().any(
        |e| matches!(e, AgentEvent::Message { content, .. } if content.contains("hello-from-echo")),
    );
    let saw_exit = events
        .iter()
        .any(|e| matches!(e, AgentEvent::Exited { .. }));
    assert!(
        saw_message,
        "expected Message event with echo payload; got {:?}",
        events
    );
    assert!(saw_exit, "expected Exited event; got {:?}", events);

    // Allow the read pump's "mark_ended" write to land.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let row = concerto_persist::sessions::get(persistence.readers(), &session_id)
        .await
        .expect("get session")
        .expect("session row exists");
    assert_eq!(row.status, "finished", "session should be marked finished");
    assert!(row.ended_at.is_some(), "ended_at should be populated");

    // Tear down: in-process supervisor doesn't need an explicit
    // shutdown; the read-pump task exits on its own once it sees
    // `AgentExited`.
    let _ = session_id;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_kind_returns_not_implemented() {
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let workarea_id = seed_workarea(&persistence).await;
    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );

    let err = supervisor
        .start_session(StartSessionRequest {
            workarea_id,
            agent_kind: AgentKind::Codex,
            echo_text: None,
            cwd: data_dir,
            permission_mode: None,
            resume_session_id: None,
            chat_id: None,
        })
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not_implemented"),
        "codex should error with NOT_IMPLEMENTED; got: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_workarea_errors_not_found() {
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );

    let err = supervisor
        .start_session(StartSessionRequest {
            workarea_id: WorkareaId("does-not-exist".into()),
            agent_kind: AgentKind::Echo,
            echo_text: Some("ignored".into()),
            cwd: data_dir,
            permission_mode: None,
            resume_session_id: None,
            chat_id: None,
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, concerto_error::Error::NotFound(_)),
        "expected NotFound; got {err:?}"
    );
}

/// Task 6 (Maestro live-integration), Sub-change A: `StartSessionRequest.chat_id
/// = Some(id)` binds the new session to a pre-existing chat instead of minting a
/// fresh `kind='session'` row. We pre-insert a `kind='maestro'` chat (the
/// singleton the Maestro spawn binds to), start an Echo session bound to it, and
/// assert: (1) the session's `chat_id` is the passed id, (2) NO new chat row was
/// created (still exactly one chat), and (3) joining `sessions` → `chats WHERE
/// kind='maestro'` resolves the session — i.e. Seam 4b is closed (a bound
/// Maestro session is discoverable by `MaestroHandle::maestro_session_id`).
/// Driven through the real `concerto-agent-host` + Echo, the existing fake-agent
/// harness, since `AgentKind::Maestro` would require the real `claude` CLI.
#[tokio::test(flavor = "multi_thread")]
async fn chat_id_binds_session_to_existing_chat() {
    let _spawn_guard = spawn_lock().lock().await;
    let (_tmp, persistence, _deep_data_dir) = make_persistence().await;
    let workarea_id = seed_workarea(&persistence).await;

    // A SHORT data_dir (directly under $TMPDIR, keyed by this session's
    // workarea) so the supervisor's canonical socket path
    // `<data_dir>/runtime/agents/<full-uuid>.sock` stays under the ~100-char
    // macOS `sun_path` cap and the full-UUID keying avoids the short-prefix
    // socket-name collision the deep-tempdir fallback (`ccs-<uuid8>.sock`)
    // would otherwise share with the sibling Echo test in this binary.
    let short = std::env::temp_dir().join(format!("c6-{}", std::process::id()));
    tokio::fs::create_dir_all(&short).await.unwrap();
    let _short_guard = TmpDirGuard(short.clone());
    let data_dir = short;
    let cwd = data_dir.clone();

    // Pre-existing singleton maestro chat (session_id NULL — the CHECK allows it).
    {
        let mut w = persistence.writer().await;
        sqlx::query(
            "INSERT INTO chats (id, session_id, kind, created_at) \
             VALUES ('maestro-chat', NULL, 'maestro', 0)",
        )
        .execute(&mut *w)
        .await
        .expect("insert maestro chat");
    }

    let supervisor = AgentSupervisorHandle::new(
        Arc::clone(&persistence),
        Arc::new(data_dir.clone()),
        Arc::new(data_dir.clone()),
        host_bin(),
    );

    let session_id = supervisor
        .start_session(StartSessionRequest {
            workarea_id,
            agent_kind: AgentKind::Echo,
            echo_text: Some("bound".to_string()),
            cwd,
            permission_mode: None,
            resume_session_id: None,
            chat_id: Some("maestro-chat".to_string()),
        })
        .await
        .expect("start_session bound to maestro chat");

    // (1) The session links to the passed chat.
    let chat_id: String = sqlx::query_scalar("SELECT chat_id FROM sessions WHERE id = ?")
        .bind(&session_id.0)
        .fetch_one(persistence.readers())
        .await
        .expect("chat_id");
    assert_eq!(
        chat_id, "maestro-chat",
        "session binds to the pre-existing chat"
    );

    // (2) No fresh kind='session' chat was created.
    let n_chats: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chats")
        .fetch_one(persistence.readers())
        .await
        .expect("count chats");
    assert_eq!(n_chats, 1, "no new chat row created when chat_id is Some");

    // (3) Seam 4b: the bound session is resolvable via the maestro-chat join.
    let resolved: String = sqlx::query_scalar(
        "SELECT s.id FROM sessions s JOIN chats c ON c.id = s.chat_id WHERE c.kind = 'maestro'",
    )
    .fetch_one(persistence.readers())
    .await
    .expect("resolve via maestro chat");
    assert_eq!(
        resolved, session_id.0,
        "maestro-chat join resolves the bound session"
    );

    // Tear down the spawned host so no orphaned `concerto-agent-host` lingers
    // into a sibling test's handshake window (the cookie-mismatch fragility).
    let _ = supervisor.stop_session(&session_id, None).await;
}

// Last line is left blank to match the workspace style.
