//! Secrets-redacting JSON file layer for `tracing`.
//!
//! Per design/00 §7.4: known-secret field names must never reach disk.
//! This module provides [`SecretsFilter`] — a `tracing_subscriber::Layer`
//! impl that serializes every `Event` as a single-line JSON object with
//! values for blocklisted field names replaced by `"<redacted>"`.
//!
//! The blocklist is intentionally tiny and additive (see
//! [`REDACTED_FIELDS`]). Adding a name is a one-line change; removing one
//! is forbidden by the task contract.
//!
//! ## Why a custom layer instead of `fmt::layer().json()`
//!
//! `tracing-subscriber`'s built-in JSON formatter records values via a
//! private `Visit` that goes straight to a `serde_json::Serializer`,
//! with no hook to intercept individual field values. Implementing a
//! custom `FormatFields` would replicate roughly the same amount of code
//! as the layer below, so we keep the redaction logic and the JSON
//! serialization in one place.

use std::io::Write;
use std::sync::Mutex;

use serde_json::{json, Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

/// Field names whose values are replaced with `"<redacted>"` before
/// being written to the log file. Locked by Task 16's public-interface
/// contract: additions are a one-line change; removals require a
/// revision task.
pub const REDACTED_FIELDS: &[&str] = &[
    "token",
    "password",
    "secret",
    "pat",
    "api_key",
    "pairing_key",
    "private_key",
];

/// Sentinel value substituted for redacted field values.
pub const REDACTED_VALUE: &str = "<redacted>";

/// Output style for [`SecretsFilter`].
#[derive(Copy, Clone, Debug)]
pub enum OutputStyle {
    /// Single-line JSON object per event. Used for the on-disk file
    /// layer; designed for `jq` queries and future OTLP ingestion.
    Json,
    /// Compact human format: `<ts>  <LEVEL> <target>: <message> k=v ...`.
    /// Mirrors `tracing_subscriber::fmt::compact()` but routes through
    /// the same redaction path as the JSON output so the console layer
    /// never leaks blocklisted field values either.
    CompactHuman {
        /// Whether to colourize the level with ANSI escapes.
        ansi: bool,
    },
}

/// A `tracing_subscriber::Layer` that writes each event to `writer`,
/// scrubbing values for any field whose name appears in
/// [`REDACTED_FIELDS`].
///
/// `writer` is any `Write` implementation; in production it is either
/// the `NonBlocking` half of `tracing_appender::non_blocking` (file
/// layer) or `std::io::stderr` (console layer).
pub struct SecretsFilter<W: Write + Send + 'static> {
    writer: Mutex<W>,
    style: OutputStyle,
}

impl<W: Write + Send + 'static> SecretsFilter<W> {
    /// Build a new filter that writes JSON to `writer`.
    pub fn json(writer: W) -> Self {
        Self {
            writer: Mutex::new(writer),
            style: OutputStyle::Json,
        }
    }

    /// Build a new filter that writes the compact human format to
    /// `writer`. Used for the stderr console layer.
    pub fn compact_human(writer: W, ansi: bool) -> Self {
        Self {
            writer: Mutex::new(writer),
            style: OutputStyle::CompactHuman { ansi },
        }
    }
}

impl<S, W> Layer<S> for SecretsFilter<W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: Write + Send + 'static,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let meta = event.metadata();
        let timestamp = chrono_like_timestamp();

        // Collect event fields once; both styles need them.
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);
        let event_fields: Vec<(String, Value)> = visitor
            .fields
            .into_iter()
            .map(|(k, v)| {
                let redacted = maybe_redact(&k, v);
                (k, redacted)
            })
            .collect();

        let line = match self.style {
            OutputStyle::Json => {
                let mut obj = Map::new();
                obj.insert("timestamp".into(), Value::String(timestamp));
                obj.insert("level".into(), Value::String(meta.level().to_string()));
                obj.insert("target".into(), Value::String(meta.target().to_string()));

                // Collect span context (outermost → innermost) so log
                // readers can correlate by workspace_id / session_id.
                if let Some(scope) = ctx.event_scope(event) {
                    let mut spans = Vec::new();
                    for span in scope.from_root() {
                        let mut span_obj = Map::new();
                        span_obj.insert("name".into(), Value::String(span.name().to_string()));
                        if let Some(fields) = span.extensions().get::<RecordedFields>() {
                            for (k, v) in &fields.0 {
                                span_obj.insert(k.clone(), maybe_redact(k, v.clone()));
                            }
                        }
                        spans.push(Value::Object(span_obj));
                    }
                    if !spans.is_empty() {
                        obj.insert("spans".into(), Value::Array(spans));
                    }
                }

                for (k, v) in event_fields {
                    obj.insert(k, v);
                }
                let mut s = match serde_json::to_string(&Value::Object(obj)) {
                    Ok(s) => s,
                    Err(_) => return,
                };
                s.push('\n');
                s
            }
            OutputStyle::CompactHuman { ansi } => {
                let level = meta.level();
                let level_str = if ansi {
                    ansi_level(level)
                } else {
                    level.to_string()
                };
                let mut s = String::new();
                s.push_str(&timestamp);
                s.push(' ');
                s.push_str(&level_str);
                s.push(' ');
                s.push_str(meta.target());
                s.push_str(": ");

                // The standard `message` field carries the format
                // string output. Pull it out and emit unquoted.
                let mut message: Option<String> = None;
                let mut other: Vec<(String, Value)> = Vec::new();
                for (k, v) in event_fields {
                    if k == "message" {
                        if let Value::String(m) = &v {
                            message = Some(m.clone());
                            continue;
                        }
                    }
                    other.push((k, v));
                }
                if let Some(msg) = message {
                    s.push_str(&msg);
                }
                for (k, v) in other {
                    s.push(' ');
                    s.push_str(&k);
                    s.push('=');
                    match v {
                        Value::String(t) => s.push_str(&t),
                        other => s.push_str(&other.to_string()),
                    }
                }
                s.push('\n');
                s
            }
        };

        if let Ok(mut w) = self.writer.lock() {
            // Best-effort: dropped writes in this layer must not panic
            // the program (e.g. if the non_blocking worker is gone).
            let _ = w.write_all(line.as_bytes());
        }
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            let mut ext = span.extensions_mut();
            // Multiple `SecretsFilter` instances (one for the file
            // layer, one for the console layer) may receive the same
            // `on_new_span` callback for the same span. The first one
            // wins; subsequent calls are no-ops. `replace` would also
            // work, but the recorded fields are identical for both
            // callers — re-recording is just wasted work.
            if ext.get_mut::<RecordedFields>().is_none() {
                let mut visitor = JsonVisitor::default();
                attrs.record(&mut visitor);
                ext.insert(RecordedFields(visitor.fields));
            }
        }
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        ctx: Context<'_, S>,
    ) {
        if let Some(span) = ctx.span(id) {
            let mut ext = span.extensions_mut();
            let mut visitor = JsonVisitor::default();
            values.record(&mut visitor);
            // Idempotent across multiple layers: only the first
            // `on_record` for any given (span, field) actually
            // extends the bucket. Subsequent calls would add the
            // same fields a second time. Track via a small bitset
            // would be over-engineered; the field-name comparison
            // below de-duplicates instead.
            let bucket = ext.get_mut::<RecordedFields>();
            match bucket {
                Some(existing) => {
                    for (k, v) in visitor.fields {
                        if !existing.0.iter().any(|(ek, _)| ek == &k) {
                            existing.0.push((k, v));
                        }
                    }
                }
                None => {
                    ext.insert(RecordedFields(visitor.fields));
                }
            }
        }
    }
}

/// Span-attached storage for recorded fields. Built up by
/// [`SecretsFilter::on_new_span`] / [`SecretsFilter::on_record`], read
/// back by [`SecretsFilter::on_event`] when assembling the JSON line.
struct RecordedFields(Vec<(String, Value)>);

/// A `Visit` impl that converts every field into a `serde_json::Value`.
#[derive(Default)]
struct JsonVisitor {
    fields: Vec<(String, Value)>,
}

impl Visit for JsonVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .push((field.name().to_string(), Value::String(value.to_string())));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .push((field.name().to_string(), Value::Bool(value)));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.push((field.name().to_string(), json!(value)));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.push((field.name().to_string(), json!(value)));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        // serde_json supports i128 via arbitrary_precision; fall back to string.
        self.fields
            .push((field.name().to_string(), Value::String(value.to_string())));
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        self.fields
            .push((field.name().to_string(), Value::String(value.to_string())));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields.push((field.name().to_string(), json!(value)));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.fields
            .push((field.name().to_string(), Value::String(value.to_string())));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields.push((
            field.name().to_string(),
            Value::String(format!("{value:?}")),
        ));
    }
}

/// ANSI-colourized rendering of a level, used by the compact human
/// style when stderr is a TTY. Matches the colour scheme that
/// `tracing-subscriber`'s built-in compact formatter uses.
fn ansi_level(level: &tracing::Level) -> String {
    let (code, name) = match *level {
        tracing::Level::ERROR => ("31", "ERROR"),
        tracing::Level::WARN => ("33", " WARN"),
        tracing::Level::INFO => ("32", " INFO"),
        tracing::Level::DEBUG => ("34", "DEBUG"),
        tracing::Level::TRACE => ("35", "TRACE"),
    };
    format!("\x1b[{code}m{name}\x1b[0m")
}

/// Replace `value` with the redacted sentinel if `name` is in the
/// blocklist. Preserves the original otherwise.
fn maybe_redact(name: &str, value: Value) -> Value {
    if REDACTED_FIELDS.contains(&name) {
        Value::String(REDACTED_VALUE.to_string())
    } else {
        value
    }
}

/// Emit an ISO-8601-ish UTC timestamp without pulling in `chrono`.
/// Format: `YYYY-MM-DDThh:mm:ss.sssZ`. Good enough for log correlation;
/// not a clock primitive.
fn chrono_like_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let millis = dur.subsec_millis();
    // Convert epoch seconds to civil date via the proleptic Gregorian
    // calendar. Algorithm: Howard Hinnant's days_from_civil inverse
    // (`civil_from_days`). Simple, branch-light, and exact for the
    // full i64 range — overkill for log timestamps but cheap.
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

fn civil_from_unix(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;
    // civil_from_days: see http://howardhinnant.github.io/date_algorithms.html
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
    (y, mo, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_blocklisted_names() {
        for name in REDACTED_FIELDS {
            let v = maybe_redact(name, Value::String("xyz".into()));
            assert_eq!(v, Value::String(REDACTED_VALUE.into()), "{name}");
        }
    }

    #[test]
    fn passes_through_other_names() {
        let v = maybe_redact("workspace_id", Value::String("abc".into()));
        assert_eq!(v, Value::String("abc".into()));
    }

    #[test]
    fn timestamp_shape() {
        let ts = chrono_like_timestamp();
        // YYYY-MM-DDThh:mm:ss.sssZ — 24 chars exactly.
        assert_eq!(ts.len(), 24, "{ts}");
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
    }
}
