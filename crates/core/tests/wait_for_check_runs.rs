//! Integration tests for the Task 318 Scheduler `wait_for_check_runs` primitive
//! (`design/05 §3.9`/§5.1) — exercised through the **public** `SchedulerHandle`
//! surface (the API Task 320's coordinated PR-set merge calls), against an
//! in-process [`CheckRunsSource`] stub + `tokio::time::pause`/auto-advance.
//!
//! No real network, no `wiremock`, no real wall-clock sleep — the poll/backoff/
//! timeout/webhook paths all resolve instantly + deterministically. These
//! complement the unit tests in `scheduler::wait_checks` (which drive the bare
//! poll loop) by proving the end-to-end handle path: source wiring via
//! `set_check_runs_source`, the `scheduler.no_vcs_source` error before wiring,
//! and the four-arg FROZEN signature.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use concerto_core::scheduler::{
    CheckRunSnapshot, CheckRunsSource, RequiredChecks, SchedulerHandle, WebhookWake,
};
use concerto_persist::{Persistence, PersistenceConfig, RepositoryId};
use concerto_vcs::checks::VcsEvent;
use tempfile::TempDir;
use tokio::sync::broadcast;

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

/// Build a scheduler handle, THEN pause the clock. Persistence must be opened
/// under real time — sqlx's pool acquisition uses the timer, and a paused clock
/// at open deadlocks it (`PoolTimedOut`). Once open, `pause()` freezes time so
/// the backoff/timeout sleeps auto-advance instantly.
async fn scheduler_paused() -> (TempDir, SchedulerHandle) {
    let (tmp, persistence, _dir) = make_persistence().await;
    let handle = SchedulerHandle::new(persistence, None);
    tokio::time::pause();
    (tmp, handle)
}

fn snap(name: &str, status: &str, conclusion: &str) -> CheckRunSnapshot {
    CheckRunSnapshot {
        name: name.into(),
        status: status.into(),
        conclusion: conclusion.into(),
    }
}

fn repo() -> RepositoryId {
    RepositoryId("owner/repo".into())
}

/// A scripted in-process check-runs source: each `check_runs` call returns the
/// next scripted entry, repeating the last once exhausted. `polls` counts every
/// poll; an optional broadcast wires the webhook fast-path.
struct ScriptedSource {
    script: Vec<Vec<CheckRunSnapshot>>,
    polls: AtomicUsize,
    wake_tx: Mutex<Option<broadcast::Sender<VcsEvent>>>,
}

impl ScriptedSource {
    fn new(script: Vec<Vec<CheckRunSnapshot>>) -> Arc<Self> {
        Arc::new(Self {
            script,
            polls: AtomicUsize::new(0),
            wake_tx: Mutex::new(None),
        })
    }

    fn with_webhook(self: Arc<Self>) -> broadcast::Sender<VcsEvent> {
        let (tx, _rx) = broadcast::channel(8);
        *self.wake_tx.lock().unwrap() = Some(tx.clone());
        tx
    }

    fn poll_count(&self) -> usize {
        self.polls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CheckRunsSource for ScriptedSource {
    async fn check_runs(
        &self,
        _repo: &RepositoryId,
        _sha: &str,
    ) -> concerto_error::Result<Vec<CheckRunSnapshot>> {
        let n = self.polls.fetch_add(1, Ordering::SeqCst);
        let idx = n.min(self.script.len().saturating_sub(1));
        Ok(self.script.get(idx).cloned().unwrap_or_default())
    }

    fn webhook_wake(&self, repo: &RepositoryId) -> Option<WebhookWake> {
        self.wake_tx
            .lock()
            .unwrap()
            .as_ref()
            .map(|tx| WebhookWake::new(tx.subscribe(), repo.0.clone()))
    }
}

#[tokio::test]
async fn wait_for_check_runs_resolves_passed_on_all_success() {
    let (_tmp, sched) = scheduler_paused().await;
    sched.set_check_runs_source(ScriptedSource::new(vec![
        vec![snap("build", "in_progress", "")],
        vec![snap("build", "completed", "success")],
    ]));
    let out = sched
        .wait_for_check_runs(
            repo(),
            "sha",
            Duration::from_secs(600),
            RequiredChecks::AllTerminal,
        )
        .await
        .unwrap();
    assert!(out.passed);
    assert!(!out.timed_out);
}

#[tokio::test]
async fn wait_for_check_runs_resolves_not_passed_on_failure() {
    let (_tmp, sched) = scheduler_paused().await;
    sched.set_check_runs_source(ScriptedSource::new(vec![vec![snap(
        "test",
        "completed",
        "failure",
    )]]));
    let out = sched
        .wait_for_check_runs(
            repo(),
            "sha",
            Duration::from_secs(600),
            RequiredChecks::AllTerminal,
        )
        .await
        .unwrap();
    assert!(!out.passed);
    assert!(!out.timed_out);
}

#[tokio::test]
async fn wait_for_check_runs_times_out_when_never_terminal() {
    let (_tmp, sched) = scheduler_paused().await;
    sched.set_check_runs_source(ScriptedSource::new(vec![vec![snap(
        "build",
        "in_progress",
        "",
    )]]));
    let out = sched
        .wait_for_check_runs(
            repo(),
            "sha",
            Duration::from_secs(5),
            RequiredChecks::AllTerminal,
        )
        .await
        .unwrap();
    assert!(out.timed_out);
    assert!(!out.passed);
}

#[tokio::test]
async fn wait_for_check_runs_named_subset_ignores_other_pending() {
    let (_tmp, sched) = scheduler_paused().await;
    sched.set_check_runs_source(ScriptedSource::new(vec![vec![
        snap("build", "completed", "success"),
        snap("lint", "in_progress", ""),
    ]]));
    let out = sched
        .wait_for_check_runs(
            repo(),
            "sha",
            Duration::from_secs(600),
            RequiredChecks::Named(vec!["build".into()]),
        )
        .await
        .unwrap();
    assert!(out.passed);
    assert!(!out.timed_out);
}

#[tokio::test]
async fn wait_for_check_runs_webhook_wake_short_circuits_sleep() {
    let (_tmp, sched) = scheduler_paused().await;
    let source = ScriptedSource::new(vec![
        vec![snap("build", "in_progress", "")],
        vec![snap("build", "completed", "success")],
    ]);
    let tx = source.clone().with_webhook();
    sched.set_check_runs_source(source.clone());

    let waiter = {
        let sched = sched.clone();
        tokio::spawn(async move {
            sched
                .wait_for_check_runs(
                    repo(),
                    "sha",
                    Duration::from_secs(600),
                    RequiredChecks::AllTerminal,
                )
                .await
        })
    };

    // Let the first poll run + the loop enter the backoff `select!`, then feed
    // an advisory wake that short-circuits the (1s) sleep → immediate re-poll.
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    tx.send(VcsEvent {
        workarea_id: "wa".into(),
        repository_id: "owner/repo".into(),
        frame: Vec::new(),
    })
    .unwrap();

    let out = waiter.await.unwrap().unwrap();
    assert!(out.passed);
    assert!(!out.timed_out);
    assert_eq!(
        source.poll_count(),
        2,
        "wake should trigger exactly one extra poll"
    );
}

#[tokio::test]
async fn wait_for_check_runs_errors_without_a_source() {
    let (_tmp, sched) = scheduler_paused().await;
    let err = sched
        .wait_for_check_runs(
            repo(),
            "sha",
            Duration::from_secs(1),
            RequiredChecks::AllTerminal,
        )
        .await
        .expect_err("no source wired → error");
    assert!(format!("{err}").contains("scheduler.no_vcs_source"));
}
