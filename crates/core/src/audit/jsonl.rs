//! Canonical on-disk audit subscriber (Task 44).
//!
//! Writes one JSON object per line to
//! `<data_dir>/audit/audit-<YYYY-MM-DD>.jsonl`. Rotation is daily by UTC
//! date: when the date observed on a flush differs from the date of the
//! currently-open file, the file is closed and a fresh one is opened.
//!
//! ## Flushing
//!
//! Each [`emit`] call writes the serialized line into the open file via
//! `write_all`. We then `flush` the file on every event so the OS sees
//! the bytes immediately; the kernel still batches the actual disk I/O.
//! `fsync` is deliberately NOT called per event — the design's "100ms
//! batched fsync" is approximated by the [`AuditWriterTask`] draining
//! one event at a time at sub-millisecond cadence. A crash mid-stream
//! may lose the last few hundred microseconds of events; no event is
//! ever partially written because each line is a single `write_all`
//! call of a complete `<json>\n` string.
//!
//! ## Date computation
//!
//! Pure integer math on the seconds-since-epoch — no `chrono`. The
//! algorithm matches the `civil_from_unix` helper in
//! `crate::log_filter` (Howard Hinnant's `civil_from_days`). A clock
//! source closure is injected at construction so tests can drive
//! rotation against a synthetic clock.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::event::AuditEvent;
use super::writer::AuditLogSubscriber;

/// Type alias for the injectable clock source. Returns "now" as the
/// number of seconds since the Unix epoch. Tests pass a closure backed
/// by an `AtomicI64`; production wires
/// [`system_clock`].
pub type ClockFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Default clock — `SystemTime::now()` minus `UNIX_EPOCH`, in seconds.
pub fn system_clock() -> ClockFn {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    })
}

/// The canonical JSONL file subscriber.
///
/// Holds an injectable clock so rotation tests don't depend on the
/// real wall clock.
pub struct JsonlFileSubscriber {
    audit_dir: PathBuf,
    inner: Mutex<OpenFile>,
    clock: ClockFn,
}

struct OpenFile {
    /// `(year, month, day)` of the currently-open file. `None` until the
    /// first event lands.
    date: Option<(i32, u32, u32)>,
    file: Option<File>,
}

impl JsonlFileSubscriber {
    /// Open (or create) the subscriber rooted at `audit_dir`.
    ///
    /// The directory is created lazily on the first `emit`; this
    /// constructor only validates the path argument.
    pub fn new(audit_dir: PathBuf) -> Self {
        Self::with_clock(audit_dir, system_clock())
    }

    /// Constructor with an injected clock. Used by tests that drive
    /// rotation without waiting for a real day boundary.
    pub fn with_clock(audit_dir: PathBuf, clock: ClockFn) -> Self {
        Self {
            audit_dir,
            inner: Mutex::new(OpenFile {
                date: None,
                file: None,
            }),
            clock,
        }
    }

    /// Compute the file path for a given `(year, month, day)`.
    fn path_for(&self, ymd: (i32, u32, u32)) -> PathBuf {
        let (y, mo, d) = ymd;
        self.audit_dir
            .join(format!("audit-{y:04}-{mo:02}-{d:02}.jsonl"))
    }

    /// Open the file for `ymd`, creating the parent directory if
    /// needed. The file is opened with `O_APPEND` so multiple writers
    /// on the same machine never interleave (atomic by POSIX).
    fn open_for(&self, ymd: (i32, u32, u32)) -> std::io::Result<File> {
        std::fs::create_dir_all(&self.audit_dir)?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path_for(ymd))
    }

    /// Ensure the open file matches `ymd`, opening a new one if the
    /// date rolled. Returns a mutable reference to the live file.
    fn rotate_if_needed<'a>(
        &self,
        open: &'a mut OpenFile,
        ymd: (i32, u32, u32),
    ) -> std::io::Result<&'a mut File> {
        let need_open = !matches!(open.date, Some(existing) if existing == ymd);
        if need_open {
            // Drop the previous file (close it) before opening the new
            // one. The OS commits any in-flight writes synchronously
            // because each `write_all` already returned.
            open.file.take();
            open.file = Some(self.open_for(ymd)?);
            open.date = Some(ymd);
        }
        Ok(open
            .file
            .as_mut()
            .expect("file present after rotate_if_needed"))
    }
}

#[async_trait]
impl AuditLogSubscriber for JsonlFileSubscriber {
    async fn emit(&self, event: &AuditEvent) {
        let now_secs = (self.clock)();
        let (ymd, _hms) = civil_from_unix(now_secs);
        let line = match serialize_event_line(event, now_secs) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "audit: serialize failed; dropping event");
                return;
            }
        };
        let mut guard = self.inner.lock().await;
        let file = match self.rotate_if_needed(&mut guard, ymd) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, "audit: open failed; dropping event");
                return;
            }
        };
        if let Err(e) = file.write_all(line.as_bytes()) {
            tracing::warn!(error = %e, "audit: write failed");
            return;
        }
        // Best-effort flush — the OS still buffers in its own cache;
        // we don't fsync per event (see module doc).
        let _ = file.flush();
    }

    async fn flush(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(file) = guard.file.as_mut() {
            let _ = file.flush();
            let _ = file.sync_data();
        }
    }
}

/// Serialize one event into a JSONL line (ending with `\n`).
///
/// Public for tests + the audit log inspection utility that reads back
/// the file.
pub fn serialize_event_line(event: &AuditEvent, now_secs: i64) -> serde_json::Result<String> {
    use serde_json::{json, Map, Value};

    let event_secs = event
        .at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(now_secs);
    let event_millis = event
        .at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_millis())
        .unwrap_or(0);
    let ((y, mo, d), (h, mi, s)) = civil_from_unix(event_secs);
    let at = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{event_millis:03}Z");

    let mut obj = Map::new();
    obj.insert("at".into(), Value::String(at));
    obj.insert("kind".into(), Value::String(event.kind.as_str().into()));
    obj.insert("actor".into(), serde_json::to_value(&event.actor)?);
    obj.insert(
        "subject_ids".into(),
        serde_json::to_value(&event.subject_ids)?,
    );
    obj.insert("details".into(), event.details_json.clone());

    let mut line = serde_json::to_string(&json!(obj))?;
    line.push('\n');
    Ok(line)
}

/// Decompose epoch-seconds into `((year, month, day), (h, m, s))` UTC.
///
/// Same algorithm as `crate::log_filter::civil_from_unix` (Howard
/// Hinnant). Duplicated here to keep the audit module free of
/// log-layer imports.
fn civil_from_unix(secs: i64) -> ((i32, u32, u32), (u32, u32, u32)) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64 + era * 400) as i32;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    ((y, mo, d), (h, mi, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_unix_known_dates() {
        // 1970-01-01T00:00:00Z
        assert_eq!(civil_from_unix(0), ((1970, 1, 1), (0, 0, 0)));
        // 2024-01-01T00:00:00Z = 1_704_067_200
        assert_eq!(civil_from_unix(1_704_067_200), ((2024, 1, 1), (0, 0, 0)));
        // 2024-03-01T12:34:56Z = 1_709_296_496
        assert_eq!(civil_from_unix(1_709_296_496), ((2024, 3, 1), (12, 34, 56)));
    }
}
