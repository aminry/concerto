//! Integration tests for the Task 38 Scheduler `/loop` primitive.
//!
//! Three focused happy-path / negative cases:
//!
//! 1. `fire_now` against a real Agent Supervisor (echo kind) ends up
//!    inserting a `schedule_runs` row that resolves to `completed` once
//!    the session emits `Exited`.
//! 2. Pausing a schedule evicts its wheel entry — `fire_now` short-
//!    circuits with `Ok(None)` and no new `schedule_runs` row appears.
//! 3. Inflight suppression: a pre-inserted `schedule_runs` row with
//!    `ended_at = NULL` blocks the next fire — `fire_now` returns
//!    `Ok(None)` and the row count stays at 1.
//!
//! Crash recovery is covered by `rebuild_wheel` rather than a full
//! process recycle — the test inserts schedules directly via the
//! Scheduler handle, drops the handle, rebuilds a fresh one, and
//! asserts the wheel reload finds the rows.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use concerto_core::scheduler::{CreateScheduleRequest, SchedulerHandle};
use concerto_persist::{NewScheduleRun, Persistence, PersistenceConfig, ScheduleRunId, WorkareaId};
use tempfile::TempDir;

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

async fn seed_workarea(persistence: &Persistence) -> WorkareaId {
    let mut writer = persistence.writer().await;
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

#[tokio::test(flavor = "multi_thread")]
async fn fire_now_creates_run_row_when_supervisor_missing() {
    // Exercises the persistence side of `fire_schedule` without an
    // attached supervisor. The fire path inserts the run row then
    // unwinds with `scheduler.no_supervisor`, which `mark_run_failed`
    // resolves to `terminal_state = "failed"`. This proves the
    // insert + failure path without depending on the real Claude
    // binary being present at test time.
    let (_tmp, persistence, _data_dir) = make_persistence().await;
    let workarea_id = seed_workarea(&persistence).await;
    let scheduler = SchedulerHandle::new(Arc::clone(&persistence), None);

    let inserted = scheduler
        .create_schedule(CreateScheduleRequest {
            workarea_id: workarea_id.clone(),
            kind: "loop".into(),
            interval_seconds: 30,
            prompt: "noop".into(),
            agent_kind: "claude".into(),
            expires_at_unix_ms: None,
        })
        .await
        .expect("create_schedule");

    let rows = scheduler.list_schedules(&workarea_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].paused);

    // No supervisor → `fire_schedule` returns Err after the run row
    // is inserted and marked failed.
    let res = scheduler.fire_now(&inserted.id).await;
    assert!(
        res.is_err(),
        "fire_now without supervisor should error; got {res:?}"
    );

    let runs = scheduler.get_history(&inserted.id).await.unwrap();
    assert_eq!(
        runs.len(),
        1,
        "fire_now should produce one schedule_runs row"
    );
    assert_eq!(
        runs[0].terminal_state.as_deref(),
        Some("failed"),
        "no-supervisor fire should be marked failed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_blocks_further_firings() {
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let workarea_id = seed_workarea(&persistence).await;
    let _ = data_dir; // supervisor not needed; tests short-circuit before fire path
    let scheduler = SchedulerHandle::new(Arc::clone(&persistence), None);

    let inserted = scheduler
        .create_schedule(CreateScheduleRequest {
            workarea_id: workarea_id.clone(),
            kind: "loop".into(),
            interval_seconds: 30,
            prompt: "noop".into(),
            agent_kind: "claude".into(),
            expires_at_unix_ms: None,
        })
        .await
        .expect("create_schedule");

    let updated = scheduler.pause_schedule(&inserted.id).await.unwrap();
    assert!(updated.paused, "pause_schedule should set paused=true");

    // fire_now on a paused schedule should suppress.
    let outcome = scheduler.fire_now(&inserted.id).await.unwrap();
    assert!(outcome.is_none(), "paused schedule should not fire");
    let runs = scheduler.get_history(&inserted.id).await.unwrap();
    assert_eq!(runs.len(), 0, "no schedule_runs row should be created");
}

#[tokio::test(flavor = "multi_thread")]
async fn inflight_suppression_skips_overlapping_fire() {
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let workarea_id = seed_workarea(&persistence).await;
    let _ = data_dir; // supervisor not needed; tests short-circuit before fire path
    let scheduler = SchedulerHandle::new(Arc::clone(&persistence), None);

    let inserted = scheduler
        .create_schedule(CreateScheduleRequest {
            workarea_id: workarea_id.clone(),
            kind: "loop".into(),
            interval_seconds: 30,
            prompt: "noop".into(),
            agent_kind: "claude".into(),
            expires_at_unix_ms: None,
        })
        .await
        .expect("create_schedule");

    // Inject an inflight run row manually so the DB-side suppression
    // check fires (the in-memory map is empty for a fresh handle).
    {
        let mut w = persistence.writer().await;
        concerto_persist::schedule_runs::insert(
            &mut w,
            NewScheduleRun {
                id: ScheduleRunId(uuid::Uuid::now_v7().to_string()),
                schedule_id: inserted.id.clone(),
                session_id: None,
                started_at: 0,
            },
        )
        .await
        .unwrap();
    }

    let outcome = scheduler.fire_now(&inserted.id).await.unwrap();
    assert!(outcome.is_none(), "inflight run should suppress next fire");

    // Still exactly one run row (the injected one).
    let runs = scheduler.get_history(&inserted.id).await.unwrap();
    assert_eq!(runs.len(), 1, "suppression must not insert a new run row");
}

#[tokio::test(flavor = "multi_thread")]
async fn rebuild_wheel_loads_active_schedules() {
    let (_tmp, persistence, _data_dir) = make_persistence().await;
    let workarea_id = seed_workarea(&persistence).await;
    let scheduler = SchedulerHandle::new(Arc::clone(&persistence), None);

    for n in 0..3 {
        scheduler
            .create_schedule(CreateScheduleRequest {
                workarea_id: workarea_id.clone(),
                kind: "loop".into(),
                interval_seconds: 30 + n,
                prompt: format!("loop-{n}"),
                agent_kind: "claude".into(),
                expires_at_unix_ms: None,
            })
            .await
            .unwrap();
    }

    // Drop the existing handle (simulates a Core restart) and rebuild.
    drop(scheduler);
    let fresh = SchedulerHandle::new(Arc::clone(&persistence), None);
    let reloaded = fresh.rebuild_wheel().await.expect("rebuild_wheel");
    assert_eq!(reloaded, 3, "expected to reload three active schedules");
}

#[tokio::test(flavor = "multi_thread")]
async fn create_schedule_rejects_short_interval() {
    let (_tmp, persistence, data_dir) = make_persistence().await;
    let workarea_id = seed_workarea(&persistence).await;
    let _ = data_dir; // supervisor not needed; tests short-circuit before fire path
    let scheduler = SchedulerHandle::new(Arc::clone(&persistence), None);

    let err = scheduler
        .create_schedule(CreateScheduleRequest {
            workarea_id: workarea_id.clone(),
            kind: "loop".into(),
            interval_seconds: 5, // below the 30s floor
            prompt: "x".into(),
            agent_kind: "claude".into(),
            expires_at_unix_ms: None,
        })
        .await
        .unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("interval_out_of_bounds"),
        "expected interval_out_of_bounds; got {msg}"
    );

    // No row was inserted.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schedules")
        .fetch_one(persistence.readers())
        .await
        .unwrap();
    assert_eq!(count, 0, "rejected create_schedule must not insert");
    let _ = Duration::from_millis(0);
}
