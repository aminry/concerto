//! Integration tests for the Task 112 audit subscriber fan-out.
//!
//! Covers the three `Scope — in` test requirements:
//!
//! 1. **Fan-out** — one event reaches *every* registered subscriber
//!    (JSONL default + a memory capture + a deliberately-failing one).
//! 2. **Rotation** — the `JsonlFileSubscriber` opens a new file at the
//!    daily boundary, and a size cap rolls to a numbered sibling.
//! 3. **Failing-subscriber isolation** — a subscriber whose `on_event`
//!    errors / blocks does NOT stop the always-on JSONL default from
//!    writing, nor block the foreground producer.

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use concerto_core::audit::{
    AuditActor, AuditEvent, AuditKind, AuditLogSubscriber, AuditWriterTask, ClockFn, EntityKind,
    HttpsForwarderSubscriber, JsonlFileSubscriber, RotationConfig, StdoutSubscriber,
    DEFAULT_RETENTION_DAYS,
};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

/// Captures every event in-memory; also counts `on_event` calls.
struct CountingSubscriber {
    id: &'static str,
    count: Arc<AtomicU32>,
}

#[async_trait]
impl AuditLogSubscriber for CountingSubscriber {
    fn id(&self) -> &str {
        self.id
    }
    async fn on_event(&self, _event: &AuditEvent) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
    async fn flush(&self) {}
}

/// A subscriber that "fails" on every event (here: panics would poison
/// the task, so it instead just blocks briefly then logs). It must never
/// stop the JSONL default. We model failure as a slow no-op that returns
/// without writing anything.
struct FailingSubscriber {
    seen: Arc<AtomicU32>,
}

#[async_trait]
impl AuditLogSubscriber for FailingSubscriber {
    fn id(&self) -> &str {
        "failing"
    }
    async fn on_event(&self, _event: &AuditEvent) {
        // Simulate a flaky forwarder: take a little time, then "fail"
        // (do nothing). The contract is that this neither panics nor
        // blocks the JSONL default that runs alongside it.
        self.seen.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    async fn flush(&self) {}
}

fn jsonl_files(dir: &std::path::Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("read audit dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect()
}

/// 1. Fan-out: an event reaches the JSONL default AND every other
///    registered subscriber.
#[tokio::test]
async fn event_reaches_every_registered_subscriber() {
    let tmp = TempDir::new().expect("tempdir");
    let audit_dir = tmp.path().to_path_buf();

    let jsonl: Arc<dyn AuditLogSubscriber> = Arc::new(JsonlFileSubscriber::new(audit_dir.clone()));
    let count_a = Arc::new(AtomicU32::new(0));
    let count_b = Arc::new(AtomicU32::new(0));
    let mem_a: Arc<dyn AuditLogSubscriber> = Arc::new(CountingSubscriber {
        id: "a",
        count: Arc::clone(&count_a),
    });
    let mem_b: Arc<dyn AuditLogSubscriber> = Arc::new(CountingSubscriber {
        id: "b",
        count: Arc::clone(&count_b),
    });
    // The stdout subscriber is registered too (smoke that it doesn't
    // panic in the fan-out).
    let stdout: Arc<dyn AuditLogSubscriber> = Arc::new(StdoutSubscriber::new());

    let shutdown = CancellationToken::new();
    let (writer, _drained, join) =
        AuditWriterTask::spawn(vec![jsonl, mem_a, mem_b, stdout], shutdown.clone());

    for i in 0..10 {
        writer.append(
            AuditEvent::new(AuditKind::WorkspaceCreated, AuditActor::System)
                .with_subject(EntityKind::Workspace, format!("ws-{i}")),
        );
    }

    shutdown.cancel();
    let _ = join.await;

    assert_eq!(
        count_a.load(Ordering::SeqCst),
        10,
        "subscriber A saw all 10"
    );
    assert_eq!(
        count_b.load(Ordering::SeqCst),
        10,
        "subscriber B saw all 10"
    );
    let files = jsonl_files(&audit_dir);
    assert_eq!(files.len(), 1, "JSONL default wrote a file");
    let lines = std::fs::read_to_string(&files[0]).unwrap();
    assert_eq!(lines.lines().count(), 10, "JSONL default wrote all 10");
}

/// 2a. Daily rotation produces a new file when the clock crosses the
///     UTC midnight boundary.
#[tokio::test]
async fn daily_rotation_opens_new_file_at_boundary() {
    let tmp = TempDir::new().expect("tempdir");
    let audit_dir = tmp.path().to_path_buf();

    // 2024-03-15 12:00:00 UTC.
    let clock_secs = Arc::new(AtomicI64::new(1_710_504_000));
    let clk = Arc::clone(&clock_secs);
    let clock: ClockFn = Arc::new(move || clk.load(Ordering::SeqCst));

    let sub: Arc<dyn AuditLogSubscriber> =
        Arc::new(JsonlFileSubscriber::with_clock(audit_dir.clone(), clock));

    sub.on_event(&AuditEvent::new(
        AuditKind::WorkspaceCreated,
        AuditActor::System,
    ))
    .await;

    // Advance 24h.
    clock_secs.fetch_add(86_400, Ordering::SeqCst);
    sub.on_event(&AuditEvent::new(
        AuditKind::WorkspaceArchived,
        AuditActor::System,
    ))
    .await;
    sub.flush().await;

    let mut names: Vec<String> = std::fs::read_dir(&audit_dir)
        .unwrap()
        .filter_map(|e| e.ok().and_then(|e| e.file_name().into_string().ok()))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "audit-2024-03-15.jsonl".to_string(),
            "audit-2024-03-16.jsonl".to_string()
        ],
        "rotation produced two daily files"
    );
}

/// 2b. Size rotation rolls the primary file to a numbered sibling once
///     the byte cap is exceeded — same `<data_dir>/audit/` layout.
#[tokio::test]
async fn size_rotation_rolls_to_numbered_sibling() {
    let tmp = TempDir::new().expect("tempdir");
    let audit_dir = tmp.path().to_path_buf();

    // Pin the clock so all writes land on one date — isolating the size
    // trigger from the daily one.
    let clock: ClockFn = Arc::new(|| 1_710_504_000); // 2024-03-15.
    let rotation = RotationConfig {
        max_bytes: Some(200),
        retention_days: DEFAULT_RETENTION_DAYS,
    };
    let sub: Arc<dyn AuditLogSubscriber> = Arc::new(JsonlFileSubscriber::with_rotation(
        audit_dir.clone(),
        clock,
        rotation,
    ));

    // Each event line is ~150 bytes; 200-byte cap → roll after the first.
    for i in 0..6 {
        sub.on_event(
            &AuditEvent::new(AuditKind::SecretAccessed, AuditActor::System)
                .with_subject(EntityKind::Secret, format!("secret-{i}")),
        )
        .await;
    }
    sub.flush().await;

    let files = jsonl_files(&audit_dir);
    assert!(
        files.len() >= 2,
        "size rotation produced at least one numbered sibling, got {} files: {files:?}",
        files.len()
    );
    // The primary file must still exist with the canonical name.
    assert!(
        audit_dir.join("audit-2024-03-15.jsonl").exists(),
        "primary file retains the canonical name"
    );
    // And at least one numbered sibling.
    assert!(
        audit_dir.join("audit-2024-03-15.1.jsonl").exists(),
        "first numbered sibling present"
    );
}

/// 2c. Retention prunes files older than the window on a daily roll.
#[tokio::test]
async fn retention_prunes_files_past_the_window() {
    let tmp = TempDir::new().expect("tempdir");
    let audit_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(&audit_dir).unwrap();

    // Plant an old file (2023-10-01, ~166 days back) that should be
    // pruned, and a recent one (2024-03-14) that should survive a 90-day
    // window anchored at 2024-03-15 (cutoff ≈ 2023-12-16).
    std::fs::write(audit_dir.join("audit-2023-10-01.jsonl"), b"{}\n").unwrap();
    std::fs::write(audit_dir.join("audit-2024-03-14.jsonl"), b"{}\n").unwrap();

    let clock: ClockFn = Arc::new(|| 1_710_504_000); // 2024-03-15.
    let rotation = RotationConfig {
        max_bytes: None,
        retention_days: 90,
    };
    let sub: Arc<dyn AuditLogSubscriber> = Arc::new(JsonlFileSubscriber::with_rotation(
        audit_dir.clone(),
        clock,
        rotation,
    ));

    // First event triggers the daily roll (date None -> 2024-03-15),
    // which runs the prune.
    sub.on_event(&AuditEvent::new(
        AuditKind::WorkspaceCreated,
        AuditActor::System,
    ))
    .await;
    sub.flush().await;

    assert!(
        !audit_dir.join("audit-2023-10-01.jsonl").exists(),
        "file past the 90-day window was pruned"
    );
    assert!(
        audit_dir.join("audit-2024-03-14.jsonl").exists(),
        "recent file within the window survived"
    );
    assert!(
        audit_dir.join("audit-2024-03-15.jsonl").exists(),
        "today's file exists"
    );
}

/// 3. A failing/slow subscriber does NOT block the JSONL default or the
///    foreground producer.
#[tokio::test]
async fn failing_subscriber_does_not_block_jsonl_default() {
    let tmp = TempDir::new().expect("tempdir");
    let audit_dir = tmp.path().to_path_buf();

    let jsonl: Arc<dyn AuditLogSubscriber> = Arc::new(JsonlFileSubscriber::new(audit_dir.clone()));
    let seen = Arc::new(AtomicU32::new(0));
    let failing: Arc<dyn AuditLogSubscriber> = Arc::new(FailingSubscriber {
        seen: Arc::clone(&seen),
    });

    // JSONL is registered FIRST — the durable floor is never reordered
    // behind the flaky subscriber.
    let shutdown = CancellationToken::new();
    let (writer, _drained, join) = AuditWriterTask::spawn(vec![jsonl, failing], shutdown.clone());

    let started = Instant::now();
    for i in 0..50 {
        writer.append(
            AuditEvent::new(AuditKind::SessionStarted, AuditActor::System)
                .with_subject(EntityKind::Session, format!("s-{i}")),
        );
    }
    // The foreground producer never awaits the slow subscriber.
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "producer blocked on the slow subscriber: {:?}",
        started.elapsed()
    );

    shutdown.cancel();
    let _ = join.await;

    // Despite the flaky subscriber, every event made it to disk.
    let files = jsonl_files(&audit_dir);
    assert_eq!(files.len(), 1, "JSONL default still wrote its file");
    let lines = std::fs::read_to_string(&files[0]).unwrap();
    assert_eq!(
        lines.lines().count(),
        50,
        "JSONL default wrote every event despite the failing subscriber"
    );
}

/// 3b. The HTTPS forwarder pointed at a dead URL must fail gracefully and
///     never block the JSONL default or the foreground.
#[tokio::test]
async fn https_forwarder_to_dead_url_does_not_block_jsonl() {
    let tmp = TempDir::new().expect("tempdir");
    let audit_dir = tmp.path().to_path_buf();

    let jsonl: Arc<dyn AuditLogSubscriber> = Arc::new(JsonlFileSubscriber::new(audit_dir.clone()));
    // Port 1 on localhost: connection refused immediately.
    let https = HttpsForwarderSubscriber::new("https://127.0.0.1:1/ingest").expect("client builds");
    let https: Arc<dyn AuditLogSubscriber> = Arc::new(https);

    let shutdown = CancellationToken::new();
    let (writer, _drained, join) = AuditWriterTask::spawn(vec![jsonl, https], shutdown.clone());

    let started = Instant::now();
    for i in 0..20 {
        writer.append(
            AuditEvent::new(AuditKind::ToolApprovalDecided, AuditActor::System)
                .with_subject(EntityKind::ToolApproval, format!("ta-{i}")),
        );
    }
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "producer blocked on the dead HTTPS endpoint: {:?}",
        started.elapsed()
    );

    shutdown.cancel();
    let _ = join.await;

    let files = jsonl_files(&audit_dir);
    assert_eq!(
        files.len(),
        1,
        "JSONL default wrote despite the dead endpoint"
    );
    let lines = std::fs::read_to_string(&files[0]).unwrap();
    assert_eq!(
        lines.lines().count(),
        20,
        "every event landed on disk despite the dead HTTPS endpoint"
    );
}
