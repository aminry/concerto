//! The fan-out audit writer + subscriber trait (Task 44).
//!
//! `AuditWriter` is a cheap-cloneable handle around an
//! `mpsc::Sender<AuditEvent>` (capacity 1000). Callers invoke
//! [`AuditWriter::append`]; on a full channel the event is dropped and a
//! `warn` is logged. The drop-on-overflow behaviour is mandated by
//! `design/10 §8`: we never let auditing back-pressure the producing
//! actor.
//!
//! [`AuditWriterTask`] is the singleton Tokio task that drains the
//! channel and fans out events to every registered subscriber in
//! parallel via `futures::future::join_all`. The task gates shutdown on
//! every subscriber's `flush` completing.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

use super::event::AuditEvent;

/// Bounded channel capacity. Sized per `design/10 §8`: even a
/// burst-of-1000 events at ~1KB each is ~1MB of memory — well below the
/// process's RSS budget.
pub const AUDIT_QUEUE_CAPACITY: usize = 1000;

/// Cheap-cloneable handle to the audit writer.
///
/// Holding an `AuditWriter` is the only way to emit audit events.
/// Clone freely; the underlying `mpsc::Sender` is itself cheap to clone.
///
/// On a closed channel (the writer task has exited) `append` is a no-op
/// — by then the runtime is shutting down and audit events are no
/// longer being persisted regardless.
#[derive(Clone)]
pub struct AuditWriter {
    tx: mpsc::Sender<AuditEvent>,
}

impl AuditWriter {
    /// Build a writer from a sender. Normally callers construct a writer
    /// via [`AuditWriterTask::spawn`]; this constructor exists for the
    /// integration tests' minimal in-process writers.
    pub fn new(tx: mpsc::Sender<AuditEvent>) -> Self {
        Self { tx }
    }

    /// Build a writer that drops every event on the floor. Used by
    /// test rigs that don't want to instantiate a full subscriber.
    pub fn noop() -> Self {
        // `mpsc::channel` returns a sender + receiver pair. Drop the
        // receiver here — every `try_send` then returns `Err(_::Closed)`,
        // which `append` swallows.
        let (tx, _rx) = mpsc::channel(1);
        Self { tx }
    }

    /// Non-blocking append. Drops the event + logs a `warn` on a full
    /// channel; silently no-ops on a closed channel.
    ///
    /// Returns immediately — the actual write to disk happens on the
    /// background [`AuditWriterTask`] (or whatever subscriber set the
    /// caller wired up).
    pub fn append(&self, event: AuditEvent) {
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(dropped)) => {
                tracing::warn!(
                    audit.kind = dropped.kind.as_str(),
                    "audit channel full — dropping event"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Runtime is shutting down; nothing to do.
            }
        }
    }
}

/// Trait implemented by every audit-log subscriber. Per `design/09 §3.5`
/// the V0.1 subscriber set is just [`crate::audit::jsonl::JsonlFileSubscriber`]
/// (canonical on-disk writer) and the in-memory test subscriber. Syslog
/// + HttpsForwarder ship in V1.0.
#[async_trait]
pub trait AuditLogSubscriber: Send + Sync {
    /// Called for every event, in arrival order. Implementations must
    /// not block longer than ~10ms — the writer task fans out to every
    /// subscriber in sequence.
    async fn emit(&self, event: &AuditEvent);

    /// Flush any buffered state. Called by the writer task on shutdown.
    async fn flush(&self);
}

/// The background task that drains the audit channel and fans out
/// events to every registered subscriber.
pub struct AuditWriterTask {
    rx: mpsc::Receiver<AuditEvent>,
    subscribers: Vec<Arc<dyn AuditLogSubscriber>>,
    shutdown: CancellationToken,
}

impl AuditWriterTask {
    /// Build the task + writer pair.
    ///
    /// Returns:
    /// - the cheap-cloneable [`AuditWriter`] handle producers hold,
    /// - the [`Notify`] producers can wait on for "writer fully drained",
    /// - the task itself, which the caller spawns onto a Tokio runtime.
    pub fn new(
        subscribers: Vec<Arc<dyn AuditLogSubscriber>>,
        shutdown: CancellationToken,
    ) -> (AuditWriter, Arc<Notify>, Self) {
        let (tx, rx) = mpsc::channel(AUDIT_QUEUE_CAPACITY);
        let drained = Arc::new(Notify::new());
        let task = AuditWriterTask {
            rx,
            subscribers,
            shutdown,
        };
        (AuditWriter::new(tx), drained, task)
    }

    /// Spawn the task on the current Tokio runtime. Returns the writer
    /// handle producers hold + a `JoinHandle` the runtime can await on
    /// shutdown.
    ///
    /// The `drained` notify is fired when the task exits (clean or
    /// shutdown). Callers gating shutdown on the writer can await it.
    pub fn spawn(
        subscribers: Vec<Arc<dyn AuditLogSubscriber>>,
        shutdown: CancellationToken,
    ) -> (AuditWriter, Arc<Notify>, tokio::task::JoinHandle<()>) {
        let (writer, drained, task) = Self::new(subscribers, shutdown);
        let drained_for_task = Arc::clone(&drained);
        let handle = tokio::spawn(async move {
            task.run().await;
            drained_for_task.notify_waiters();
        });
        (writer, drained, handle)
    }

    /// Run loop. Exits on:
    /// - `shutdown` cancellation (drains any remaining queued events
    ///   first, then flushes every subscriber), or
    /// - the channel being closed (all producers dropped).
    async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.cancelled() => {
                    // Drain any events that producers managed to queue
                    // before they observed the shutdown.
                    while let Ok(event) = self.rx.try_recv() {
                        for sub in &self.subscribers {
                            sub.emit(&event).await;
                        }
                    }
                    break;
                }
                maybe_event = self.rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            for sub in &self.subscribers {
                                sub.emit(&event).await;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        for sub in &self.subscribers {
            sub.flush().await;
        }
    }
}
