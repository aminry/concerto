//! The fan-out audit writer + subscriber trait (Task 44; generalized in
//! Task 112).
//!
//! `AuditWriter` is a cheap-cloneable handle around an
//! `mpsc::Sender<AuditEvent>` (capacity 1000). Callers invoke
//! [`AuditWriter::append`]; on a full channel the event is dropped and a
//! `warn` is logged. The drop-on-overflow behaviour is mandated by
//! `design/10 §8`: we never let auditing back-pressure the producing
//! actor.
//!
//! [`AuditWriterTask`] is the singleton Tokio task that drains the
//! channel and fans out events to every registered subscriber. The
//! always-on [`crate::audit::jsonl::JsonlFileSubscriber`] is registered
//! first and is the durable floor — it is never reordered behind a
//! network subscriber. Network subscribers
//! ([`crate::audit::syslog::SyslogSubscriber`],
//! [`crate::audit::https::HttpsForwarderSubscriber`]) isolate their own
//! slow/failing I/O behind an internal bounded channel + background task
//! so a down endpoint can never stall the drain loop, the JSONL default,
//! or the producing actor. The task gates shutdown on every subscriber's
//! `flush` completing.

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

/// Trait implemented by every audit-log subscriber.
///
/// This is a published extension seam — one of the
/// `design/18 §3.7` trait surfaces — and its signature is FROZEN as of
/// Task 112. The V1.0 OSS impl set is:
///
/// - [`crate::audit::jsonl::JsonlFileSubscriber`] — the canonical
///   on-disk writer; always present (the durable floor).
/// - [`crate::audit::stdout::StdoutSubscriber`] — debug echo to stdout.
/// - [`crate::audit::syslog::SyslogSubscriber`] — RFC 5424 over UDP/TCP.
/// - [`crate::audit::https::HttpsForwarderSubscriber`] — POSTs NDJSON
///   events to a configured endpoint (poor-man's SIEM hook).
///
/// V2.0+ BSL impls live in their own crates and are RESERVED (not
/// implemented here) per `design/18 §3.7` so Task 707's trait-seam
/// completeness check can verify the names without a Core refork:
///
/// - `SiemForwarderSubscriber` — `crates/enterprise-siem` (BSL):
///   multi-tenant SIEM integration (Splunk HEC / Elastic / Datadog),
///   retry-with-replay buffer, field mapping, compliance attestations.
/// - `EncryptedAtRestSubscriber` — `crates/enterprise-encrypted-audit`
///   (BSL): AES-256-GCM at-rest writer with a keychain-derived key.
#[async_trait]
pub trait AuditLogSubscriber: Send + Sync {
    /// Stable identifier for this subscriber (e.g. `"jsonl"`, `"syslog"`).
    /// Used in diagnostics and the trait-seam registry.
    fn id(&self) -> &str;

    /// Called for every event, in arrival order. The always-on JSONL
    /// subscriber runs first and synchronously (the durable floor);
    /// network subscribers must NOT block here — they isolate their own
    /// slow/failing I/O behind an internal channel + background task and
    /// return promptly so a down endpoint never stalls the drain loop,
    /// the JSONL default, or the producing actor.
    async fn on_event(&self, event: &AuditEvent);

    /// Flush any buffered state. Called by the writer task on shutdown.
    async fn flush(&self);
}

/// The background task that drains the audit channel and fans out
/// events to every registered subscriber.
///
/// Subscribers are invoked in registration order; `boot::spawn_runtime`
/// registers the always-on [`crate::audit::jsonl::JsonlFileSubscriber`]
/// first so the durable on-disk write is never gated behind a network
/// subscriber.
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
                            sub.on_event(&event).await;
                        }
                    }
                    break;
                }
                maybe_event = self.rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            for sub in &self.subscribers {
                                sub.on_event(&event).await;
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
