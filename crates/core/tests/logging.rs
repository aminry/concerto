//! Integration tests for Task 16's logging discipline.
//!
//! Coverage:
//!  - The file appender writes JSON lines into the configured log
//!    directory under the expected `core.YYYY-MM-DD.log` schema.
//!  - The `SecretsFilter` replaces blocklisted field values with
//!    `"<redacted>"` before the line hits disk.
//!  - Non-blocklisted fields pass through unchanged.
//!  - Span fields produced by the `*_span!` macros land in the JSON
//!    output under the `spans` array.
//!
//! The tests share a process-global subscriber, so they must run
//! sequentially. We serialise with a single `Mutex`; the runtime cost
//! is negligible because the tests are I/O bound.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

use concerto_core::workspace_span;

/// Tests share a single process-global default dispatcher. Use this
/// mutex to make sure only one test at a time installs one.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Convenience: install logging into a tempdir, emit `f()`, drop the
/// guard so the non_blocking worker flushes, then return the path of
/// the freshly-written log file.
fn run_with_logging<F: FnOnce()>(f: F) -> (TempDir, PathBuf) {
    let _g = TEST_LOCK.lock().expect("test mutex poisoned");
    let tmp = TempDir::new().expect("tempdir");
    let log_dir = tmp.path().to_path_buf();
    {
        let _guard =
            concerto_core::logging::init_with_log_dir(&log_dir).expect("init_with_log_dir");
        f();
        // Allow the non_blocking worker a moment to drain before we
        // drop the guard.
        std::thread::sleep(Duration::from_millis(50));
    }
    // After the guard drop, the worker has been joined. Find the
    // produced log file.
    let mut entries = fs::read_dir(&log_dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().is_some_and(|n| {
                let s = n.to_string_lossy();
                s.starts_with("core.") && s.ends_with(".log")
            })
        })
        .collect::<Vec<_>>();
    entries.sort();
    let log_path = entries
        .pop()
        .expect("at least one core.YYYY-MM-DD.log file");
    (tmp, log_path)
}

fn read_to_string(path: &PathBuf) -> String {
    let mut s = String::new();
    fs::File::open(path)
        .expect("open log")
        .read_to_string(&mut s)
        .expect("read log");
    s
}

#[test]
fn redacts_blocklisted_event_field() {
    let (_tmp, log_path) = run_with_logging(|| {
        tracing::info!(token = "xyz-secret", "test redaction");
    });
    let body = read_to_string(&log_path);
    assert!(
        body.contains("\"token\":\"<redacted>\""),
        "expected token redaction, got: {body}"
    );
    assert!(
        !body.contains("xyz-secret"),
        "raw secret value leaked: {body}"
    );
}

#[test]
fn passes_through_non_secret_fields() {
    let (_tmp, log_path) = run_with_logging(|| {
        tracing::info!(workspace_id = "ws-abc", "test passthrough");
    });
    let body = read_to_string(&log_path);
    assert!(
        body.contains("\"workspace_id\":\"ws-abc\""),
        "expected workspace_id passthrough, got: {body}"
    );
}

#[test]
fn file_layer_emits_json_lines() {
    let (_tmp, log_path) = run_with_logging(|| {
        tracing::info!("hello");
    });
    let body = read_to_string(&log_path);
    let line = body
        .lines()
        .find(|l| l.contains("\"message\":\"hello\""))
        .unwrap_or_else(|| panic!("no event found in:\n{body}"));
    let parsed: Value = serde_json::from_str(line).expect("each line is valid JSON");
    assert_eq!(parsed["level"], "INFO");
    assert!(parsed["timestamp"].is_string());
    assert!(parsed["target"].is_string());
}

#[test]
fn rotation_file_naming_schema() {
    let (_tmp, log_path) = run_with_logging(|| {
        tracing::info!("naming check");
    });
    let name = log_path.file_name().unwrap().to_string_lossy().to_string();
    // core.YYYY-MM-DD.log — verify the literal prefix, the date width,
    // and the .log suffix.
    assert!(name.starts_with("core."), "{name}");
    assert!(name.ends_with(".log"), "{name}");
    let middle = &name["core.".len()..name.len() - ".log".len()];
    assert_eq!(middle.len(), 10, "date should be 10 chars: {name}");
    assert_eq!(&middle[4..5], "-", "{name}");
    assert_eq!(&middle[7..8], "-", "{name}");
}

#[test]
fn span_fields_appear_in_json() {
    let (_tmp, log_path) = run_with_logging(|| {
        let span = workspace_span!("ws-42");
        let _e = span.enter();
        tracing::info!("inside workspace span");
    });
    let body = read_to_string(&log_path);
    let line = body
        .lines()
        .find(|l| l.contains("inside workspace span"))
        .unwrap_or_else(|| panic!("event missing from log:\n{body}"));
    let parsed: Value = serde_json::from_str(line).expect("valid JSON");
    let spans = parsed["spans"]
        .as_array()
        .unwrap_or_else(|| panic!("missing spans array in {parsed}"));
    let workspace_span = spans
        .iter()
        .find(|s| s["name"] == "workspace")
        .unwrap_or_else(|| panic!("missing workspace span in {parsed}"));
    assert_eq!(workspace_span["workspace_id"], "ws-42");
}
