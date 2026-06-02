//! Integration tests for Task 202 — `Streams.Subscribe` reconnect: the
//! per-subject ring buffer, `since_offset` replay, `GapDetected`, and the
//! unary `AckOffset` prune path — exercised over the LIVE UDS gRPC
//! surface (Tier 1).
//!
//! The buffer's unit-level invariants (count/byte eviction, floor math,
//! min-ack pruning arithmetic) are covered by the `#[cfg(test)]` module
//! in `crates/core/src/handlers/streams.rs`. These tests prove the wire
//! path end-to-end against a real `concerto-core` subprocess:
//!
//! - `workspace.events` is the driver subject: each `CreateWorkspace`
//!   emits exactly one event, so offsets are deterministic (0, 1, 2, …).
//! - Replay: a reconnect with `since_offset = k` yields exactly the
//!   events with `offset > k`.
//! - Two-subscriber agreement: two subscribers see identical offsets for
//!   the same event (the invariant the ring relies on).
//! - Gap: after `AckOffset` prunes the ring past the floor, a reconnect
//!   with a now-too-old `since_offset` gets a single `GapDetected` frame
//!   first, then continues live (continue-live-from-head — FROZEN).

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use concerto_proto::v1::event::Body;
use concerto_proto::v1::{AckOffsetRequest, CreateWorkspaceRequest, Event, SubscribeRequest};
use concerto_test_harness::CoreUnderTest;
use futures::StreamExt;
use tokio::process::Command;

const WORKSPACE_EVENTS: &str = "workspace.events";

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

/// Seed a project + one repository directly in SQLite (mirrors the
/// `sessions_grpc` seed) so `CreateWorkspace` has a valid project + repo
/// to reference. Returns `(project_id, repo_id)`.
async fn seed_project_repo(core: &CoreUnderTest, slug: &str) -> (String, String) {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tempfile::TempDir;

    // A bare repo with one commit so the URL resolves; we never clone it
    // (workspace creation does not need an on-disk worktree).
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
    let bare_url = format!("file://{}", bare.path().display());
    git(&["remote", "add", "origin", bare_url.as_str()], work.path()).await;
    git(&["push", "-u", "origin", "main"], work.path()).await;

    let project_id = format!("proj-{slug}");
    let repo_id = format!("repo-{slug}");
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
    .bind(format!("name-{slug}"))
    .bind(&bare_url)
    .bind(local_path.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .expect("insert repository");
    pool.close().await;

    // Keep the temp dirs alive for the whole test by leaking them: the
    // process is short-lived and the OS reclaims on exit. (We never read
    // the worktree again.)
    std::mem::forget(bare);
    std::mem::forget(work);

    (project_id, repo_id)
}

/// Read the next `Event` from a subscribe stream within `budget`, or
/// `None` on timeout / end-of-stream / error.
async fn next_event<S>(stream: &mut S, budget: Duration) -> Option<Event>
where
    S: futures::Stream<Item = Result<Event, tonic::Status>> + Unpin,
{
    match tokio::time::timeout(budget, stream.next()).await {
        Ok(Some(Ok(ev))) => Some(ev),
        _ => None,
    }
}

/// True iff the event is a `workspace.events` body (filters out any
/// stray frames; there should be none on this subject).
fn is_workspace(ev: &Event) -> bool {
    matches!(ev.body, Some(Body::Workspace(_)))
}

#[tokio::test(flavor = "multi_thread")]
async fn since_offset_replays_exactly_the_gap() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let (project_id, repo_id) = seed_project_repo(&core, "replay").await;

    let mut ws = core.workspaces_client().await.expect("workspaces client");

    // Create three workspaces → offsets 0, 1, 2 on workspace.events.
    // (The first Subscribe is what spawns the subject pump; create
    // workspaces AFTER an initial subscribe so the pump is live and
    // captures every event.)
    let mut sub0 = {
        let mut sc = core.streams_client().await.expect("streams client");
        sc.subscribe(SubscribeRequest {
            subject: WORKSPACE_EVENTS.to_string(),
            filter: None,
            since_offset: None,
        })
        .await
        .expect("Subscribe(None)")
        .into_inner()
    };

    for i in 0..3 {
        ws.create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: format!("ws-{i}"),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: None,
        })
        .await
        .expect("CreateWorkspace");
    }

    // Drain the three live events on the original subscriber to learn
    // their offsets (0, 1, 2). The pump assigns publish-time offsets.
    let mut offsets = Vec::new();
    while offsets.len() < 3 {
        let ev = next_event(&mut sub0, Duration::from_secs(5))
            .await
            .expect("live workspace event");
        if is_workspace(&ev) {
            offsets.push(ev.offset);
        }
    }
    assert_eq!(offsets, vec![0, 1, 2], "publish-time offsets are monotonic");

    // Reconnect with since_offset = 0 → must replay exactly offsets 1, 2.
    let mut resub = {
        let mut sc = core.streams_client().await.expect("streams client 2");
        sc.subscribe(SubscribeRequest {
            subject: WORKSPACE_EVENTS.to_string(),
            filter: None,
            since_offset: Some(0),
        })
        .await
        .expect("Subscribe(since=0)")
        .into_inner()
    };

    let first = next_event(&mut resub, Duration::from_secs(5))
        .await
        .expect("replay event 1");
    assert!(is_workspace(&first));
    assert_eq!(first.offset, 1, "replay starts at offset > since");

    let second = next_event(&mut resub, Duration::from_secs(5))
        .await
        .expect("replay event 2");
    assert_eq!(second.offset, 2);

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_subscribers_agree_on_offsets() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let (project_id, repo_id) = seed_project_repo(&core, "agree").await;

    let mut ws = core.workspaces_client().await.expect("workspaces client");

    // Two concurrent subscribers, both live-only.
    let mut a = {
        let mut sc = core.streams_client().await.expect("streams client a");
        sc.subscribe(SubscribeRequest {
            subject: WORKSPACE_EVENTS.to_string(),
            filter: None,
            since_offset: None,
        })
        .await
        .expect("Subscribe a")
        .into_inner()
    };
    let mut b = {
        let mut sc = core.streams_client().await.expect("streams client b");
        sc.subscribe(SubscribeRequest {
            subject: WORKSPACE_EVENTS.to_string(),
            filter: None,
            since_offset: None,
        })
        .await
        .expect("Subscribe b")
        .into_inner()
    };

    for i in 0..3 {
        ws.create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: format!("agree-{i}"),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: None,
        })
        .await
        .expect("CreateWorkspace");
    }

    let mut a_offsets = Vec::new();
    let mut b_offsets = Vec::new();
    while a_offsets.len() < 3 {
        let ev = next_event(&mut a, Duration::from_secs(5))
            .await
            .expect("a ev");
        if is_workspace(&ev) {
            a_offsets.push(ev.offset);
        }
    }
    while b_offsets.len() < 3 {
        let ev = next_event(&mut b, Duration::from_secs(5))
            .await
            .expect("b ev");
        if is_workspace(&ev) {
            b_offsets.push(ev.offset);
        }
    }
    // Both subscribers see the SAME offset numbering — the publish-time
    // assignment invariant (V0.1's per-consumer increment would have made
    // these disagree).
    assert_eq!(a_offsets, vec![0, 1, 2]);
    assert_eq!(b_offsets, vec![0, 1, 2]);

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn ack_prunes_then_old_since_offset_yields_gap_detected() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let (project_id, repo_id) = seed_project_repo(&core, "gap").await;

    let mut ws = core.workspaces_client().await.expect("workspaces client");
    let mut streams = core.streams_client().await.expect("streams client");

    // One attached subscriber spawns the pump and holds the ring.
    let mut sub = streams
        .subscribe(SubscribeRequest {
            subject: WORKSPACE_EVENTS.to_string(),
            filter: None,
            since_offset: None,
        })
        .await
        .expect("Subscribe")
        .into_inner();

    for i in 0..3 {
        ws.create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: format!("gap-{i}"),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: None,
        })
        .await
        .expect("CreateWorkspace");
    }

    // Drain offsets 0,1,2 so the subscriber is caught up.
    let mut seen = 0;
    while seen < 3 {
        let ev = next_event(&mut sub, Duration::from_secs(5))
            .await
            .expect("live event");
        if is_workspace(&ev) {
            seen += 1;
        }
    }

    // Ack up to offset 2. With this lone subscriber attached, min-ack = 2
    // → the ring prunes everything <= 2, advancing the floor past the old
    // events.
    streams
        .ack_offset(AckOffsetRequest {
            subject: WORKSPACE_EVENTS.to_string(),
            offset: 2,
        })
        .await
        .expect("AckOffset");

    // Reconnect from offset 0 — now older than the pruned floor → a
    // single GapDetected frame must arrive FIRST.
    let mut resub = streams
        .subscribe(SubscribeRequest {
            subject: WORKSPACE_EVENTS.to_string(),
            filter: None,
            since_offset: Some(0),
        })
        .await
        .expect("Subscribe(since=0 after prune)")
        .into_inner();

    let gap = next_event(&mut resub, Duration::from_secs(5))
        .await
        .expect("gap frame");
    match gap.body {
        Some(Body::GapDetected(g)) => {
            assert_eq!(g.subject, WORKSPACE_EVENTS);
            assert!(g.buffer_floor >= 3, "floor advanced past acked offsets");
        }
        other => panic!("expected GapDetected first, got {other:?}"),
    }

    // Continue-live-from-head: a NEW workspace event published after the
    // gap is delivered live on the same stream (the stream did not
    // terminate).
    ws.create_workspace(CreateWorkspaceRequest {
        project_id: project_id.clone(),
        name: "gap-live".to_string(),
        repository_ids: vec![repo_id.clone()],
        permission_mode: None,
        description: None,
    })
    .await
    .expect("CreateWorkspace live");

    let live = next_event(&mut resub, Duration::from_secs(5))
        .await
        .expect("live event after gap");
    assert!(
        is_workspace(&live),
        "stream continues live after GapDetected"
    );
    assert!(
        live.offset >= 3,
        "live offset is at/after the gap floor, got {}",
        live.offset
    );

    core.shutdown().await.expect("shutdown");
}
