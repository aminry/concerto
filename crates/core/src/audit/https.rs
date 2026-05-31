//! HTTPS audit forwarder (Task 112).
//!
//! POSTs each audit event as a newline-delimited JSON (NDJSON) body to a
//! configured endpoint — a poor-man's SIEM / log-collector hook usable
//! without any BSL module. Uses `reqwest` with the rustls TLS backend
//! (no OpenSSL / native-tls), so it builds on the Windows CI lane and
//! stays `cargo deny`-clean.
//!
//! ## Non-blocking isolation
//!
//! Exactly like the syslog forwarder: `on_event` only serializes and
//! `try_send`s onto an internal bounded channel; a worker task owns the
//! HTTP client and does the POSTs. A down endpoint can never block the
//! fan-out drain loop, the always-on JSONL subscriber, or the producing
//! actor — a full queue drops-and-logs, and a failed POST is logged while
//! the worker keeps running.

use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};

use super::event::AuditEvent;
use super::writer::AuditLogSubscriber;

/// Internal bound on the in-flight forward queue.
const FORWARD_QUEUE_CAPACITY: usize = 1024;

/// Per-request timeout. A hung endpoint must not pin the worker forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// HTTPS audit-event forwarder.
pub struct HttpsForwarderSubscriber {
    tx: mpsc::Sender<String>,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl HttpsForwarderSubscriber {
    /// Build a forwarder POSTing NDJSON lines to `endpoint`.
    ///
    /// Returns `None` if the rustls-backed HTTP client cannot be built
    /// (e.g. no system entropy) — callers treat that as "forwarding
    /// unavailable" and rely on the JSONL floor. The worker task is
    /// spawned immediately and connects lazily per request.
    pub fn new(endpoint: impl Into<String>) -> Option<Self> {
        let endpoint = endpoint.into();
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| {
                tracing::warn!(error = %e, "audit(https): client build failed; forwarding disabled");
                e
            })
            .ok()?;
        let (tx, rx) = mpsc::channel(FORWARD_QUEUE_CAPACITY);
        let worker = tokio::spawn(run_worker(client, endpoint, rx));
        Some(Self {
            tx,
            worker: Mutex::new(Some(worker)),
        })
    }
}

#[async_trait]
impl AuditLogSubscriber for HttpsForwarderSubscriber {
    fn id(&self) -> &str {
        "https"
    }

    async fn on_event(&self, event: &AuditEvent) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let line = match super::jsonl::serialize_event_line(event, now_secs) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "audit(https): serialize failed; dropping event");
                return;
            }
        };
        match self.tx.try_send(line) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    audit.kind = event.kind.as_str(),
                    "audit(https): forward queue full — dropping (JSONL retains it)"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Worker exited; JSONL still records it.
            }
        }
    }

    async fn flush(&self) {
        let mut guard = self.worker.lock().await;
        if let Some(handle) = guard.as_ref() {
            if handle.is_finished() {
                if let Some(h) = guard.take() {
                    let _ = h.await;
                }
            }
        }
    }
}

impl Drop for HttpsForwarderSubscriber {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.worker.try_lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}

/// Worker loop: owns the HTTP client and POSTs each queued NDJSON line.
/// Failures are logged and the loop continues so a recovered endpoint
/// resumes forwarding.
async fn run_worker(client: reqwest::Client, endpoint: String, mut rx: mpsc::Receiver<String>) {
    while let Some(line) = rx.recv().await {
        let resp = client
            .post(&endpoint)
            .header("content-type", "application/x-ndjson")
            .body(line)
            .send()
            .await;
        match resp {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => {
                tracing::warn!(
                    status = r.status().as_u16(),
                    "audit(https): endpoint returned non-2xx (JSONL retains it)"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "audit(https): POST failed (JSONL retains it)");
            }
        }
    }
}
