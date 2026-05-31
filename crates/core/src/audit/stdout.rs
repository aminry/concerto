//! Stdout audit subscriber (Task 112).
//!
//! Echoes each event as a single JSONL line to stdout. Opt-in, intended
//! for local debugging and container log capture. Reuses the canonical
//! [`super::jsonl::serialize_event_line`] renderer so the on-stdout shape
//! is byte-identical to the on-disk shape.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use super::event::AuditEvent;
use super::writer::AuditLogSubscriber;

/// Writes each event as one JSONL line to stdout.
///
/// Stdout is line-buffered (or block-buffered when piped); we `flush`
/// after each line so events are visible promptly in `docker logs` /
/// `journalctl` style capture.
#[derive(Debug, Default)]
pub struct StdoutSubscriber;

impl StdoutSubscriber {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AuditLogSubscriber for StdoutSubscriber {
    fn id(&self) -> &str {
        "stdout"
    }

    async fn on_event(&self, event: &AuditEvent) {
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let line = match super::jsonl::serialize_event_line(event, now_secs) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "audit(stdout): serialize failed; dropping event");
                return;
            }
        };
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        // Best-effort: a closed/broken stdout must never panic the audit
        // pipeline.
        let _ = lock.write_all(line.as_bytes());
        let _ = lock.flush();
    }

    async fn flush(&self) {
        let _ = std::io::stdout().flush();
    }
}
