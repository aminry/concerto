//! Canonical on-disk audit subscriber (Task 44; rotation + retention in
//! Task 112).
//!
//! Writes one JSON object per line to
//! `<data_dir>/audit/audit-<YYYY-MM-DD>.jsonl`. This is the always-on
//! durable floor of the audit pipeline: the [`crate::audit::AuditWriter`]
//! fan-out registers it first and never reorders it behind a network
//! subscriber.
//!
//! ## Rotation
//!
//! Two rotation triggers keep the `<data_dir>/audit/` layout:
//!
//! - **Daily (UTC).** When the date observed on an event differs from the
//!   date of the currently-open file, the file is closed and a fresh
//!   `audit-<YYYY-MM-DD>.jsonl` is opened.
//! - **Size (optional).** When [`RotationConfig::max_bytes`] is set and the
//!   open file would exceed it, the current file is renamed to
//!   `audit-<YYYY-MM-DD>.<seq>.jsonl` (sequence climbing from 1) and a
//!   fresh primary file is opened. Off by default — set it via
//!   [`JsonlFileSubscriber::with_rotation`].
//!
//! ## Retention
//!
//! [`RotationConfig::retention_days`] (default
//! [`DEFAULT_RETENTION_DAYS`] = 90, per `design/12 §3.7`) bounds how long
//! rotated files live. On each daily roll, files whose embedded date is
//! older than the cutoff are deleted. Retention is policy-configurable via
//! managed settings (Phase 2, Task 211); this task ships the mechanism +
//! the documented 90-day default.
//!
//! ## Flushing
//!
//! Each `on_event` call writes the serialized line into the open file via
//! `write_all`, then `flush`es so the OS sees the bytes immediately; the
//! kernel still batches the actual disk I/O. `fsync` is deliberately NOT
//! called per event — the design's "100ms batched fsync" is approximated
//! by the [`crate::audit::AuditWriterTask`] draining one event at a time
//! at sub-millisecond cadence, plus a `sync_data` on `flush`. No event is
//! ever partially written because each line is a single `write_all` of a
//! complete `<json>\n` string.
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

/// Default audit-log retention, in days. Per `design/12 §3.7`: 90 days,
/// configurable via managed settings (full enforcement is Phase 2,
/// Task 211).
pub const DEFAULT_RETENTION_DAYS: u32 = 90;

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

/// Rotation + retention knobs for [`JsonlFileSubscriber`].
///
/// The defaults preserve the V0.1 behaviour exactly: daily UTC rotation,
/// no size cap, 90-day retention.
#[derive(Debug, Clone)]
pub struct RotationConfig {
    /// Optional size cap. When `Some(n)`, the current file is rolled to a
    /// numbered sibling before a write that would push it past `n` bytes.
    /// `None` (default) disables size-based rotation.
    pub max_bytes: Option<u64>,
    /// How many days of rotated files to keep. Files older than this are
    /// pruned on each daily roll. Defaults to [`DEFAULT_RETENTION_DAYS`].
    pub retention_days: u32,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            max_bytes: None,
            retention_days: DEFAULT_RETENTION_DAYS,
        }
    }
}

/// The canonical JSONL file subscriber.
///
/// Holds an injectable clock so rotation tests don't depend on the
/// real wall clock.
pub struct JsonlFileSubscriber {
    audit_dir: PathBuf,
    inner: Mutex<OpenFile>,
    clock: ClockFn,
    rotation: RotationConfig,
}

struct OpenFile {
    /// `(year, month, day)` of the currently-open file. `None` until the
    /// first event lands.
    date: Option<(i32, u32, u32)>,
    file: Option<File>,
    /// Bytes written to the currently-open primary file. Reset on every
    /// open (daily roll or size roll).
    bytes: u64,
}

impl JsonlFileSubscriber {
    /// Open (or create) the subscriber rooted at `audit_dir` with the
    /// default rotation policy (daily, 90-day retention, no size cap).
    ///
    /// The directory is created lazily on the first `on_event`; this
    /// constructor only validates the path argument.
    pub fn new(audit_dir: PathBuf) -> Self {
        Self::with_clock(audit_dir, system_clock())
    }

    /// Constructor with an injected clock. Used by tests that drive
    /// rotation without waiting for a real day boundary.
    pub fn with_clock(audit_dir: PathBuf, clock: ClockFn) -> Self {
        Self::with_rotation(audit_dir, clock, RotationConfig::default())
    }

    /// Full constructor: inject both the clock and the rotation policy.
    pub fn with_rotation(audit_dir: PathBuf, clock: ClockFn, rotation: RotationConfig) -> Self {
        Self {
            audit_dir,
            inner: Mutex::new(OpenFile {
                date: None,
                file: None,
                bytes: 0,
            }),
            clock,
            rotation,
        }
    }

    /// Compute the primary file path for a given `(year, month, day)`.
    fn path_for(&self, ymd: (i32, u32, u32)) -> PathBuf {
        let (y, mo, d) = ymd;
        self.audit_dir
            .join(format!("audit-{y:04}-{mo:02}-{d:02}.jsonl"))
    }

    /// Compute the numbered sibling path for a size-rolled file:
    /// `audit-<YYYY-MM-DD>.<seq>.jsonl`.
    fn numbered_path_for(&self, ymd: (i32, u32, u32), seq: u32) -> PathBuf {
        let (y, mo, d) = ymd;
        self.audit_dir
            .join(format!("audit-{y:04}-{mo:02}-{d:02}.{seq}.jsonl"))
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
    /// date rolled. Runs retention pruning on a fresh daily roll.
    /// Returns a mutable reference to the live file.
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
            let file = self.open_for(ymd)?;
            open.bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
            open.file = Some(file);
            open.date = Some(ymd);
            // A new day rolled — prune anything past the retention window.
            self.prune_expired(ymd);
        }
        Ok(open
            .file
            .as_mut()
            .expect("file present after rotate_if_needed"))
    }

    /// Roll the primary file to the next numbered sibling when a write of
    /// `incoming` bytes would exceed `max_bytes`. No-op when size rotation
    /// is disabled or the write fits.
    fn roll_by_size_if_needed(
        &self,
        open: &mut OpenFile,
        ymd: (i32, u32, u32),
        incoming: u64,
    ) -> std::io::Result<()> {
        let Some(max) = self.rotation.max_bytes else {
            return Ok(());
        };
        // Never roll an empty file — a single line larger than the cap
        // still has to land somewhere.
        if open.bytes == 0 || open.bytes + incoming <= max {
            return Ok(());
        }
        // Close the primary file, move it aside to the next free numbered
        // slot, then reopen a fresh empty primary.
        open.file.take();
        let mut seq = 1;
        while self.numbered_path_for(ymd, seq).exists() {
            seq += 1;
        }
        std::fs::rename(self.path_for(ymd), self.numbered_path_for(ymd, seq))?;
        let file = self.open_for(ymd)?;
        open.bytes = 0;
        open.file = Some(file);
        Ok(())
    }

    /// Delete rotated files whose embedded date is older than the
    /// retention cutoff (`today - retention_days`). Best-effort: I/O
    /// errors are logged and swallowed so pruning never blocks writes.
    fn prune_expired(&self, today: (i32, u32, u32)) {
        let cutoff = days_from_civil(today) - i64::from(self.rotation.retention_days);
        let dir = match std::fs::read_dir(&self.audit_dir) {
            Ok(d) => d,
            Err(_) => return,
        };
        for entry in dir.flatten() {
            let name = entry.file_name();
            let name = match name.to_str() {
                Some(s) => s,
                None => continue,
            };
            if let Some(ymd) = parse_audit_date(name) {
                if days_from_civil(ymd) < cutoff {
                    if let Err(e) = std::fs::remove_file(entry.path()) {
                        tracing::warn!(error = %e, file = %name, "audit: retention prune failed");
                    }
                }
            }
        }
    }
}

#[async_trait]
impl AuditLogSubscriber for JsonlFileSubscriber {
    fn id(&self) -> &str {
        "jsonl"
    }

    async fn on_event(&self, event: &AuditEvent) {
        let now_secs = (self.clock)();
        let (ymd, _hms) = civil_from_unix(now_secs);
        let line = match serialize_event_line(event, now_secs) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "audit: serialize failed; dropping event");
                return;
            }
        };
        let incoming = line.len() as u64;
        let mut guard = self.inner.lock().await;
        // Daily roll first (also primes `bytes` from the on-disk length).
        if let Err(e) = self.rotate_if_needed(&mut guard, ymd) {
            tracing::warn!(error = %e, "audit: open failed; dropping event");
            return;
        }
        // Then a size roll if this write would overflow the cap.
        if let Err(e) = self.roll_by_size_if_needed(&mut guard, ymd, incoming) {
            tracing::warn!(error = %e, "audit: size rotation failed; dropping event");
            return;
        }
        let file = guard
            .file
            .as_mut()
            .expect("file present after rotation checks");
        if let Err(e) = file.write_all(line.as_bytes()) {
            tracing::warn!(error = %e, "audit: write failed");
            return;
        }
        // Best-effort flush — the OS still buffers in its own cache;
        // we don't fsync per event (see module doc).
        let _ = file.flush();
        guard.bytes += incoming;
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
    let (y, mo, d) = civil_from_days(days);
    ((y, mo, d), (h, mi, s))
}

/// `days`-since-epoch → `(year, month, day)` (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
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
    (y, mo, d)
}

/// `(year, month, day)` → days-since-epoch (Howard Hinnant's
/// `days_from_civil`). The inverse of [`civil_from_days`]; used by
/// retention to compare file dates against the cutoff.
fn days_from_civil(ymd: (i32, u32, u32)) -> i64 {
    let (y, mo, d) = ymd;
    let y = if mo <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 }.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse `audit-<YYYY>-<MM>-<DD>[.<seq>].jsonl` → `(year, month, day)`.
/// Returns `None` for any file that doesn't match the audit naming, so
/// retention never touches unrelated files.
fn parse_audit_date(name: &str) -> Option<(i32, u32, u32)> {
    let rest = name.strip_prefix("audit-")?;
    if !rest.ends_with(".jsonl") {
        return None;
    }
    // The date is the first 10 chars: `YYYY-MM-DD`.
    let date = rest.get(0..10)?;
    let mut parts = date.split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let mo: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, mo, d))
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

    #[test]
    fn days_from_civil_round_trips() {
        for secs in [0_i64, 1_704_067_200, 1_709_296_496, 1_710_504_000] {
            let (ymd, _) = civil_from_unix(secs);
            let days = secs.div_euclid(86_400);
            assert_eq!(days_from_civil(ymd), days, "round-trip for {secs}");
        }
    }

    #[test]
    fn parse_audit_date_matches_primary_and_numbered() {
        assert_eq!(
            parse_audit_date("audit-2024-03-15.jsonl"),
            Some((2024, 3, 15))
        );
        assert_eq!(
            parse_audit_date("audit-2024-03-15.2.jsonl"),
            Some((2024, 3, 15))
        );
        assert_eq!(parse_audit_date("audit-2024-03-15.jsonl.bak"), None);
        assert_eq!(parse_audit_date("concerto.db"), None);
        assert_eq!(parse_audit_date("audit-not-a-date.jsonl"), None);
    }
}
