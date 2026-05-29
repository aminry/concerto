//! `smoke-client list-audit --data-dir <path> [--kind <s>]`
//!
//! Reads the JSONL audit log (Task 44) for today (UTC) under
//! `<data_dir>/audit/audit-YYYY-MM-DD.jsonl` and prints each line's
//! `kind` field. When `--kind` is set, only lines whose `kind`
//! exactly matches are printed (the smoke script greps either way,
//! but the typed filter saves a `grep` invocation downstream).
//!
//! This subcommand never opens a gRPC channel — it's a thin reader
//! on disk so the smoke gate can assert the JSONL writer is alive.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub async fn run(data_dir: &Path, kind: Option<&str>) -> Result<(), String> {
    let path = path_for_today(data_dir)?;
    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("list-audit: read {}: {e}", path.display()))?;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("list-audit: parse line {trimmed:?}: {e}"))?;
        let row_kind = value
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if let Some(filter) = kind {
            if row_kind != filter {
                continue;
            }
        }
        println!("{row_kind}");
    }
    Ok(())
}

/// Compute `<data_dir>/audit/audit-YYYY-MM-DD.jsonl` for today (UTC).
/// Mirrors `crates/core/src/audit/jsonl.rs::JsonlFileSubscriber::path_for`.
fn path_for_today(data_dir: &Path) -> Result<PathBuf, String> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("list-audit: clock before UNIX_EPOCH: {e}"))?;
    let (y, mo, d) = civil_from_unix(secs);
    Ok(data_dir
        .join("audit")
        .join(format!("audit-{y:04}-{mo:02}-{d:02}.jsonl")))
}

/// Convert a `Duration` since UNIX_EPOCH into a `(year, month, day)`
/// triple in UTC. Mirrors the in-tree civil-from-unix used by
/// `crates/core/src/audit/jsonl.rs`; kept local so the smoke client
/// has no extra deps.
fn civil_from_unix(since_epoch: Duration) -> (i32, u32, u32) {
    // Algorithm: Howard Hinnant, http://howardhinnant.github.io/date_algorithms.html.
    let days = (since_epoch.as_secs() / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = (y + if m <= 2 { 1 } else { 0 }) as i32;
    (y, m, d)
}
