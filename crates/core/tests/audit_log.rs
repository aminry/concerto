//! Integration tests for the Task 44 audit log writer.
//!
//! Three rigs:
//!
//! 1. **Memory subscriber capture** — 100 events through an
//!    `Arc<Mutex<Vec<AuditEvent>>>`-backed subscriber; assert all
//!    captured + ordering preserved.
//! 2. **JSONL on-disk round-trip** — write events to a tempdir; read
//!    back every line; assert each is well-formed JSON with the
//!    frozen field set (`at`, `kind`, `actor`, `subject_ids`,
//!    `details`).
//! 3. **Daily rotation** — drive the `JsonlFileSubscriber` with an
//!    injected clock; cross a UTC day boundary; assert a second
//!    file appears with the new date in its name.

use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use concerto_core::audit::{
    AuditActor, AuditEvent, AuditKind, AuditLogSubscriber, AuditWriterTask, EntityKind,
    JsonlFileSubscriber,
};
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Stores every event in an `Arc<Mutex<Vec<AuditEvent>>>`. Used to
/// assert ordering + count without touching disk.
struct MemorySubscriber {
    events: Arc<Mutex<Vec<AuditEvent>>>,
}

impl MemorySubscriber {
    fn new() -> (Self, Arc<Mutex<Vec<AuditEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                events: Arc::clone(&events),
            },
            events,
        )
    }
}

#[async_trait]
impl AuditLogSubscriber for MemorySubscriber {
    fn id(&self) -> &str {
        "memory"
    }
    async fn on_event(&self, event: &AuditEvent) {
        self.events.lock().await.push(event.clone());
    }
    async fn flush(&self) {}
}

#[tokio::test]
async fn memory_subscriber_captures_100_events_in_order() {
    let (memory, captured) = MemorySubscriber::new();
    let shutdown = CancellationToken::new();
    let (writer, _drained, join) = AuditWriterTask::spawn(vec![Arc::new(memory)], shutdown.clone());

    for i in 0..100 {
        let evt = AuditEvent::new(AuditKind::WorkspaceCreated, AuditActor::System)
            .with_subject(EntityKind::Workspace, format!("ws-{i:03}"));
        writer.append(evt);
    }

    // Cancel shutdown to drain + flush, then await the join handle so
    // the test never races the writer task's final flush.
    shutdown.cancel();
    let _ = join.await;

    let captured = captured.lock().await;
    assert_eq!(captured.len(), 100, "all 100 events captured");
    for (i, e) in captured.iter().enumerate() {
        assert_eq!(e.kind, AuditKind::WorkspaceCreated);
        let id = &e.subject_ids[0].id;
        assert_eq!(id, &format!("ws-{i:03}"), "order preserved");
    }
}

#[tokio::test]
async fn jsonl_writes_one_complete_json_object_per_line() {
    let tmp = TempDir::new().expect("tempdir");
    let audit_dir = tmp.path().to_path_buf();

    let subscriber: Arc<dyn AuditLogSubscriber> =
        Arc::new(JsonlFileSubscriber::new(audit_dir.clone()));
    let shutdown = CancellationToken::new();
    let (writer, _drained, join) = AuditWriterTask::spawn(vec![subscriber], shutdown.clone());

    for i in 0..5 {
        let evt = AuditEvent::new(
            AuditKind::PermissionModeChanged,
            AuditActor::Device(format!("dev-{i}")),
        )
        .with_subject(EntityKind::Workspace, format!("ws-{i}"))
        .with_details(serde_json::json!({"from": "normal", "to": "auto"}));
        writer.append(evt);
    }

    shutdown.cancel();
    let _ = join.await;

    // Find the (single) JSONL file produced.
    let files: Vec<PathBuf> = std::fs::read_dir(&audit_dir)
        .expect("read audit dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    assert_eq!(files.len(), 1, "exactly one daily JSONL produced");
    let contents = std::fs::read_to_string(&files[0]).expect("read file");
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 5, "5 lines emitted");
    for line in &lines {
        let v: serde_json::Value = serde_json::from_str(line).expect("each line is valid JSON");
        assert!(v.get("at").is_some(), "at field present: {line}");
        assert_eq!(v["kind"], "permission_mode_changed");
        assert_eq!(v["actor"]["kind"], "device");
        assert_eq!(v["details"]["from"], "normal");
        let subjects = v["subject_ids"].as_array().expect("subject_ids array");
        assert_eq!(subjects.len(), 1);
        assert_eq!(subjects[0]["kind"], "workspace");
    }
}

#[tokio::test]
async fn daily_rotation_opens_new_file_when_clock_crosses_midnight() {
    let tmp = TempDir::new().expect("tempdir");
    let audit_dir = tmp.path().to_path_buf();

    // 2024-03-15 12:00:00 UTC = 1_710_504_000
    let clock_secs = Arc::new(AtomicI64::new(1_710_504_000));
    let clock_clone = Arc::clone(&clock_secs);
    let clock: concerto_core::audit::ClockFn = Arc::new(move || clock_clone.load(Ordering::SeqCst));

    let subscriber: Arc<dyn AuditLogSubscriber> =
        Arc::new(JsonlFileSubscriber::with_clock(audit_dir.clone(), clock));

    // Day 1: one event.
    subscriber
        .on_event(&AuditEvent::new(
            AuditKind::WorkspaceCreated,
            AuditActor::System,
        ))
        .await;

    // Advance the clock by 24h → 2024-03-16 12:00:00 UTC.
    clock_secs.fetch_add(86_400, Ordering::SeqCst);
    subscriber
        .on_event(&AuditEvent::new(
            AuditKind::WorkspaceArchived,
            AuditActor::System,
        ))
        .await;
    subscriber.flush().await;

    let mut filenames: Vec<String> = std::fs::read_dir(&audit_dir)
        .expect("read audit dir")
        .filter_map(|e| {
            e.ok()
                .and_then(|e| e.file_name().to_str().map(|s| s.to_string()))
        })
        .collect();
    filenames.sort();
    assert_eq!(
        filenames,
        vec![
            "audit-2024-03-15.jsonl".to_string(),
            "audit-2024-03-16.jsonl".to_string(),
        ],
        "rotation produced two files"
    );
    // Day-1 file has one line, day-2 file has one line — and the line
    // on each contains the right `kind`.
    let day1 = std::fs::read_to_string(audit_dir.join("audit-2024-03-15.jsonl")).unwrap();
    let day2 = std::fs::read_to_string(audit_dir.join("audit-2024-03-16.jsonl")).unwrap();
    assert!(day1.contains("workspace_created"));
    assert!(day2.contains("workspace_archived"));
    assert_eq!(day1.lines().count(), 1);
    assert_eq!(day2.lines().count(), 1);
}

#[tokio::test]
async fn drop_on_full_does_not_block_producers() {
    // Build a writer with a noop subscriber that sleeps to keep the
    // queue full. Then enqueue more than the channel capacity and
    // assert `append` returned promptly without panicking.
    struct SlowSubscriber;
    #[async_trait]
    impl AuditLogSubscriber for SlowSubscriber {
        fn id(&self) -> &str {
            "slow"
        }
        async fn on_event(&self, _event: &AuditEvent) {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        async fn flush(&self) {}
    }

    let shutdown = CancellationToken::new();
    let (writer, _drained, join) =
        AuditWriterTask::spawn(vec![Arc::new(SlowSubscriber)], shutdown.clone());

    let started = std::time::Instant::now();
    // 1000 is the channel cap; push 1500 to force ~500 drops.
    for _ in 0..1500 {
        writer.append(AuditEvent::new(
            AuditKind::SecretAccessed,
            AuditActor::System,
        ));
    }
    let elapsed = started.elapsed();
    // The whole burst should land in well under 100ms even on a slow
    // CI box; the producer never awaits the slow subscriber.
    assert!(
        elapsed < std::time::Duration::from_millis(200),
        "producer blocked: {elapsed:?}"
    );

    shutdown.cancel();
    let _ = join.await;
}
