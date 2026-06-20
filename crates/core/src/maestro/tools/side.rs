//! The 2 Maestro **side-channel** tools (Task 407, `design/08 §5.1`), filling
//! the impls behind Task 401's FROZEN MCP schemas (`tools/mod.rs`):
//! `notify_user(text, severity)` and `propose_chip(chip)`.
//!
//! Both are [`super::ToolKind::SideChannel`]. Unlike the 11 read tools (which
//! wrap existing read APIs) and the 5 write tools (which mutate Core state),
//! the side-channels have **no live backend in Phase-4** — there is no
//! Notifications service (`design/14`, owned by Phase-5 Task 501/507) and the
//! Maestro chip slate is a brand-new in-process value type this file owns. So
//! both tools are backed by injected, in-memory handles ([`NotifyRecorder`] /
//! [`ChipSlate`]) the Maestro owns and 401's MCP server state holds — **no
//! global statics**, so 409 (slate append) / 414 (slate read) / 507 (sink swap)
//! can each hold their own `Arc`-clone of the **same** backing handle.
//!
//! ## `notify_user` — a typed stub against 14 (NOT `unimplemented!()`)
//!
//! `notify_user` records the notification **intent** ([`NotifyIntent`]) via a
//! swappable [`NotifySink`] and returns the FROZEN success output (`Ok`). It
//! performs **no real delivery** — there is no `NotificationHandle` until
//! Phase-5 Task 507 wires it. This is the README "`notify_user` (P4) stubs
//! against 14 and is wired live in P5" precedent (§6): the tool **succeeds** (so
//! the Maestro believes the notification was accepted and does not retry/loop),
//! while the actual push/inbox delivery is deferred. It is deliberately the
//! **inverse** of the usual 305/401 seam discipline (a typed
//! `unimplemented` *error*) — here a real `Ok` with the intent recorded. Task
//! 507 supplies a [`NotifySink`] backed by the live `NotificationHandle`; this
//! tool body and the FROZEN MCP schema are untouched. **Never `unimplemented!()`
//! / `todo!()`, never empty-silent-failure.**
//!
//! ## `propose_chip` — the Maestro-owned slate (D11, NOT the suggestion engine)
//!
//! `propose_chip` adds a [`MaestroChip`] to the Maestro-OWNED [`ChipSlate`]
//! (`Arc<Mutex<Vec<MaestroChip>>>`, **no TTL**). It deliberately does **NOT**
//! route through the V0.1 `SuggestionEngineHandle`
//! (`crates/core/src/suggestions/actor.rs`), whose chips evaporate after
//! `DEDUP_TTL` (60 s; `CHIP_RETENTION = DEDUP_TTL`) — a Maestro chip proposed
//! during a digest must still be there when the user reads the digest minutes
//! later (PHASE4_PLANNING D11). [`MaestroChip`] **mirrors** the field shape of
//! `suggestions::chip::Chip` (so 414's proto-mapping and 415's renderer reuse
//! the existing chip wire vocabulary) but is **not** that type and does **not**
//! import/extend `suggestions::*`. The slate is replaced wholesale
//! ([`ChipSlate::clear`] + re-`propose`) on the next digest/turn, **not** aged
//! out by time.
//!
//! ## What stays out of this file (Scope — out)
//!
//! Live notification delivery / `NotificationHandle` / push / inbox (Task 507),
//! the notification model (kinds, dedup, fan-out — Tasks 501–506), surfacing the
//! slate on the gRPC wire (Task 414), digest chip generation (Task 409, a pure
//! consumer of [`ChipSlate::propose`]), and the strict-mode confirmation-chip
//! classification of `propose_chip` (Task 402's `ToolClass`). 407 only records
//! the intent + produces the chip.

use std::sync::{Arc, Mutex};

use concerto_persist::WorkareaId;
use rmcp::ErrorData as McpError;
use serde_json::{json, Map, Value};

// ===========================================================================
// notify_user — typed stub against 14 (records intent, returns Ok).
// ===========================================================================

/// `notify_user` severity — mirrors `design/14 §4`'s `low | medium | high`
/// severity column.
///
/// This is **NOT** a `concerto-notifications` type: no such type exists until
/// Phase-5 Task 501/507. It is a Maestro-local mirror so the FROZEN
/// `notify_user` MCP schema stays stable when 507 swaps in a live
/// `NotificationHandle`-backed [`NotifySink`]. Do not invent a 4th level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifySeverity {
    Low,
    Medium,
    High,
}

impl NotifySeverity {
    /// Wire string — `"low" | "medium" | "high"` (the `design/14 §4` column).
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            NotifySeverity::Low => "low",
            NotifySeverity::Medium => "medium",
            NotifySeverity::High => "high",
        }
    }

    /// Parse a wire severity string. An unknown / unexpected value maps to the
    /// documented default [`NotifySeverity::Medium`] and **never errors** — a
    /// side-channel notification must not fail the tool call over a typo'd
    /// severity (the Maestro should still believe its notification was
    /// accepted; 507 can re-classify on the live path).
    pub fn from_wire(s: &str) -> Self {
        match s {
            "low" => NotifySeverity::Low,
            "high" => NotifySeverity::High,
            // "medium" and any unknown value ⇒ Medium (documented default).
            _ => NotifySeverity::Medium,
        }
    }
}

/// One recorded `notify_user` intent. Task 507 dispatches these via the live
/// `NotificationHandle` (14); until then they are recorded + returned ok.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyIntent {
    /// The notification body text the Maestro asked to send.
    pub text: String,
    /// The severity (`low | medium | high`), defaulting to `Medium` on unknown.
    pub severity: NotifySeverity,
    /// Unix epoch milliseconds the intent was recorded.
    pub created_at_ms: i64,
}

/// Pluggable sink for `notify_user`. The Phase-4 default ([`NotifyRecorder`])
/// records intents in-process; Task 507 supplies a `NotificationHandle`-backed
/// sink with **no** change to the FROZEN MCP schema or [`notify_user`]'s body.
pub trait NotifySink: Send + Sync {
    /// Accept a recorded notification intent (record / dispatch / both).
    fn record(&self, intent: NotifyIntent);
}

/// The Phase-4 default [`NotifySink`]: an in-process recorder of [`NotifyIntent`]s.
///
/// Clone-cheap (`Arc`-backed) so the Maestro, the MCP server state, and the
/// Tier-1 tests can each hold a handle to the **same** recorder. Task 507's
/// handoff drains it via [`NotifyRecorder::snapshot`].
#[derive(Debug, Clone, Default)]
pub struct NotifyRecorder {
    inner: Arc<Mutex<Vec<NotifyIntent>>>,
}

impl NotifyRecorder {
    /// A fresh, empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot every recorded intent in record order (for the Tier-1 test +
    /// the Task 507 handoff). Does not clear the recorder.
    pub fn snapshot(&self) -> Vec<NotifyIntent> {
        self.inner
            .lock()
            .expect("notify recorder mutex poisoned")
            .clone()
    }
}

impl NotifySink for NotifyRecorder {
    fn record(&self, intent: NotifyIntent) {
        self.inner
            .lock()
            .expect("notify recorder mutex poisoned")
            .push(intent);
    }
}

/// The Phase-5 LIVE [`NotifySink`] (Task 507b-ii): bridges the Maestro's
/// `notify_user` intent to a real notification via the sub-system 14
/// [`NotificationHandle`].
///
/// This swaps the Phase-4 [`NotifyRecorder`] stub for live delivery with **no**
/// change to the FROZEN `notify_user` MCP schema or [`notify_user`]'s body — the
/// MCP server simply holds a `LiveNotifySink` instead of a `NotifyRecorder`
/// (both are `dyn NotifySink`). The [`NotifySink::record`] trait method is
/// **sync** (the tool body records synchronously and returns the frozen `Ok`),
/// but [`NotificationHandle::notify`] is **async**; we bridge by mapping the
/// intent to a [`NotifyRequest`] and `tokio::spawn`-ing the `notify(..)` call.
/// The tool therefore returns the frozen success immediately (the Maestro
/// believes the notification was accepted and does not retry), and the row lands
/// asynchronously — the same fire-and-forget posture the typed stub had, now
/// backed by real persistence + fan-out.
///
/// Intent → request mapping (PHASE5 Task 507b-ii):
/// - `kind` = [`NotificationKind::AgentCompletedWithMessage`] (the Maestro is
///   surfacing a message it composed; not a crash/approval/PR event).
/// - `subject_kind` = [`SubjectKind::Session`], `subject_id` = the Maestro
///   session id when known (`subject_id` field), else `"maestro"`.
/// - `title` = `"Concerto"`, `body` = the intent text.
/// - `severity` = the intent severity mapped onto the sub-system 14 [`Severity`].
///
/// Clone-cheap: holds an `Arc`-backed [`NotificationHandle`] (itself `Clone`) and
/// a cheap `Arc<str>` subject id, so the MCP server clones it per accepted
/// connection at no real cost.
#[derive(Clone)]
pub struct LiveNotifySink {
    handle: crate::notifications::handle::NotificationHandle,
    /// The notification subject id: the live Maestro session id when known, else
    /// the `"maestro"` sentinel (Task 507b-ii). An `Arc<str>` so cloning the sink
    /// per connection is cheap.
    subject_id: Arc<str>,
}

impl LiveNotifySink {
    /// Build a live sink over `handle`, using `subject_id` for the notification
    /// subject (the Maestro session id when known). `None` falls back to the
    /// `"maestro"` sentinel.
    pub fn new(
        handle: crate::notifications::handle::NotificationHandle,
        subject_id: Option<String>,
    ) -> Self {
        Self {
            handle,
            subject_id: Arc::from(subject_id.unwrap_or_else(|| "maestro".to_string()).as_str()),
        }
    }

    /// Map a Maestro `notify_user` severity onto the sub-system 14 [`Severity`].
    fn map_severity(sev: NotifySeverity) -> crate::notifications::model::Severity {
        use crate::notifications::model::Severity;
        match sev {
            NotifySeverity::Low => Severity::Low,
            NotifySeverity::Medium => Severity::Medium,
            NotifySeverity::High => Severity::High,
        }
    }

    /// Build the [`NotifyRequest`] an intent maps to (extracted so the Tier-1
    /// test can assert the mapping without driving the async handle).
    fn request_for(&self, intent: &NotifyIntent) -> crate::notifications::model::NotifyRequest {
        use crate::notifications::model::{NotificationKind, NotifyRequest, SubjectKind};
        NotifyRequest {
            kind: NotificationKind::AgentCompletedWithMessage,
            subject_kind: SubjectKind::Session,
            subject_id: self.subject_id.to_string(),
            workspace_id: None,
            workarea_id: None,
            session_id: None,
            title: "Concerto".to_string(),
            body: intent.text.clone(),
            chips: Vec::new(),
            approval: None,
            severity: Some(Self::map_severity(intent.severity)),
        }
    }
}

impl NotifySink for LiveNotifySink {
    fn record(&self, intent: NotifyIntent) {
        let handle = self.handle.clone();
        let req = self.request_for(&intent);
        // Bridge sync `record()` → async `notify()`: spawn and let the row land
        // out-of-band. The tool already returned the frozen `Ok`, so a delivery
        // failure must not panic the in-process MCP server — log and move on.
        tokio::spawn(async move {
            if let Err(e) = handle.notify(req).await {
                tracing::warn!(
                    target: "concerto::maestro",
                    error = %e,
                    "notify_user live delivery failed"
                );
            }
        });
    }
}

/// `notify_user(text, severity) → {}` — record the notification intent and
/// return the FROZEN success output (`Ok`).
///
/// **This is a typed stub against 14 (Task 507 wires the live delivery).** It
/// records a [`NotifyIntent`] via the injected [`NotifySink`] and returns the
/// empty success object 401 froze. It performs **no real delivery**: the README
/// "`notify_user` (P4) stubs against 14 and is wired live in P5" precedent. It
/// is **NOT** `unimplemented!()` / `todo!()` and **NOT** an empty-silent-failure
/// — the Maestro must believe the notification was accepted so it does not
/// retry. 507 swaps the sink; this body + the MCP schema do not change.
pub fn notify_user(sink: &dyn NotifySink, text: String, severity: &str, now_ms: i64) -> Value {
    let intent = NotifyIntent {
        text,
        severity: NotifySeverity::from_wire(severity),
        created_at_ms: now_ms,
    };
    sink.record(intent);
    // The FROZEN `notify_user` output schema is the empty object `{}`; a real
    // success (`Ok`), never a typed-unimplemented error.
    json!({})
}

// ===========================================================================
// propose_chip — the Maestro-owned current slate (D11).
// ===========================================================================

/// A Maestro-proposed chip on the current slate.
///
/// **MIRRORS** `suggestions::chip::Chip`'s field shape (title / priority /
/// created_at / action) so 414's proto-mapping (to the `concerto.v1.Chip`-shaped
/// message) and 415's renderer reuse the existing chip wire vocabulary — but it
/// is deliberately **NOT** that type and does **NOT** route through the
/// suggestion engine (PHASE4_PLANNING D11). Differences from `Chip`:
/// - `action` is a **free-form** wire-token string (mirrors V0.1
///   `Chip.action`'s on-the-wire convention), not the closed `ChipAction` enum
///   — Maestro chips name actions the rule catalog never enumerated.
/// - `workarea_id` is **`Option`** — a Maestro chip may be workspace-scoped or
///   unscoped (e.g. the digest's "Compare TokenStore.ts").
/// - there is **no `rule_id`** — these are Maestro-proposed, not rule-emitted.
///
/// The suggestion engine's chips evaporate after `DEDUP_TTL` (60 s); the Maestro
/// slate persists in-process across that window (D11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaestroChip {
    /// Short human-readable label rendered on the chip.
    pub title: String,
    /// Priority — higher wins. Mirrors `Chip.priority` (V0.1 uses `1..=100`).
    pub priority: i32,
    /// Free-form wire-token action string (mirrors the V0.1 `Chip.action` wire
    /// convention; not the closed `ChipAction` enum).
    pub action: String,
    /// The workarea this chip is scoped to, if any (Maestro chips may be
    /// workspace-scoped or unscoped).
    pub workarea_id: Option<WorkareaId>,
    /// Unix epoch milliseconds the chip was proposed.
    pub created_at_ms: i64,
}

/// The Maestro-OWNED current chip slate (PHASE4_PLANNING D11).
///
/// A clone-cheap (`Arc<Mutex<…>>`) handle to the Maestro's current chip set:
/// held by the Maestro, surfaced on the wire by Task 414
/// ([`ChipSlate::current`]), appended-to by Task 409's digest
/// ([`ChipSlate::propose`]). **No time-based eviction** — the slate is replaced
/// wholesale ([`ChipSlate::clear`] + re-`propose`) on the next digest/turn, in
/// deliberate contrast to the suggestion engine's `CHIP_RETENTION = DEDUP_TTL`
/// (60 s) buffer. There is intentionally **no TTL / age field** here.
#[derive(Debug, Clone, Default)]
pub struct ChipSlate {
    inner: Arc<Mutex<Vec<MaestroChip>>>,
}

impl ChipSlate {
    /// A fresh, empty slate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a chip to the slate. Accumulates in proposal order; never evicts.
    pub fn propose(&self, chip: MaestroChip) {
        self.inner
            .lock()
            .expect("chip slate mutex poisoned")
            .push(chip);
    }

    /// Snapshot the current slate in proposal order.
    pub fn current(&self) -> Vec<MaestroChip> {
        self.inner
            .lock()
            .expect("chip slate mutex poisoned")
            .clone()
    }

    /// Clear the slate (a slate **refresh** — the next digest/turn replaces it).
    /// This is the only way chips leave the slate; it is **NOT** a TTL.
    pub fn clear(&self) {
        self.inner
            .lock()
            .expect("chip slate mutex poisoned")
            .clear();
    }
}

/// `propose_chip(chip) → {}` — add a [`MaestroChip`] to the Maestro-owned
/// [`ChipSlate`] and return the FROZEN success output (`Ok`).
///
/// The frozen input is `{ chip: object }`; the chip object's fields map onto
/// [`MaestroChip`] (`title`, `priority`, `action`, optional `workarea_id`).
/// Missing `title` is the one required field (a chip with no label is useless);
/// `priority` defaults to `0` and `action` to the empty string if absent, so a
/// minimal `{ "title": "…" }` chip is accepted. This writes the Maestro-owned
/// slate, **NOT** the V0.1 suggestion-engine buffer (D11).
pub fn propose_chip(slate: &ChipSlate, chip: &Value, now_ms: i64) -> Result<Value, McpError> {
    let title = chip
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            McpError::invalid_params("propose_chip: chip.title is required".to_string(), None)
        })?;
    let priority = chip
        .get("priority")
        .and_then(|v| v.as_i64())
        .map(|p| p as i32)
        .unwrap_or(0);
    let action = chip
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let workarea_id = chip
        .get("workarea_id")
        .and_then(|v| v.as_str())
        .map(|s| WorkareaId(s.to_string()));

    slate.propose(MaestroChip {
        title,
        priority,
        action,
        workarea_id,
        created_at_ms: now_ms,
    });
    // The FROZEN `propose_chip` output schema is the empty object `{}`.
    Ok(json!({}))
}

// ===========================================================================
// Argument-deserializing entry point (the frozen 401 arg sets).
// ===========================================================================

/// Dispatch a side-channel tool by its frozen name, deserializing `args` per
/// 401's frozen input schema and returning the frozen output JSON.
///
/// This is the seam the live MCP server (`super::super::mcp`, once 402/414 wire
/// the Maestro's backing handles into `MaestroMcpServer`) calls in place of
/// 401's typed-unimplemented arm for the 2 side-channel tools. The `sink` and
/// `slate` are the Maestro's own injected handles (no global statics); `now_ms`
/// is the caller's clock (the supervisor's wall clock in prod; a fixed value in
/// tests).
///
/// `notify_user` always succeeds (the typed P5 stub — it records the intent);
/// `propose_chip` returns a typed `invalid_params` error only when the chip
/// object lacks a `title` (the frozen input requires a `chip` object, but a chip
/// with no label is meaningless). An unknown name is a typed `invalid_params`.
pub fn dispatch_side(
    name: &str,
    args: Option<Map<String, Value>>,
    sink: &dyn NotifySink,
    slate: &ChipSlate,
    now_ms: i64,
) -> Result<Value, McpError> {
    match name {
        "notify_user" => {
            let text = req_str(&args, "text")?;
            // `severity` is a frozen-required string; an unknown value defaults
            // to Medium rather than erroring (see `NotifySeverity::from_wire`),
            // but a wholly-absent `severity` arg is still a malformed call.
            let severity = req_str(&args, "severity")?;
            Ok(notify_user(sink, text, &severity, now_ms))
        }
        "propose_chip" => {
            let chip = args.as_ref().and_then(|m| m.get("chip")).ok_or_else(|| {
                McpError::invalid_params("missing required arg: chip".to_string(), None)
            })?;
            propose_chip(slate, chip, now_ms)
        }
        other => Err(McpError::invalid_params(
            format!("not a maestro side-channel tool: {other}"),
            None,
        )),
    }
}

/// Extract a required string argument from the validated tool-call args.
fn req_str(args: &Option<Map<String, Value>>, key: &str) -> Result<String, McpError> {
    args.as_ref()
        .and_then(|m| m.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| McpError::invalid_params(format!("missing required arg: {key}"), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- notify_user: records intent + returns Ok (typed stub) ------------

    #[test]
    fn notify_user_records_intent_and_returns_frozen_success() {
        let recorder = NotifyRecorder::new();
        let out = notify_user(&recorder, "build broke".to_string(), "high", 1_700);
        // The FROZEN success output is the empty object `{}` — a real Ok, NOT a
        // typed-unimplemented error and NOT a panic.
        assert!(out.is_object());
        assert!(out.as_object().unwrap().is_empty());

        let recorded = recorder.snapshot();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0],
            NotifyIntent {
                text: "build broke".to_string(),
                severity: NotifySeverity::High,
                created_at_ms: 1_700,
            }
        );
    }

    #[test]
    fn notify_user_via_dispatch_records_and_returns_ok() {
        let recorder = NotifyRecorder::new();
        let slate = ChipSlate::new();
        let mut args = Map::new();
        args.insert("text".into(), json!("build broke"));
        args.insert("severity".into(), json!("high"));

        let out = dispatch_side("notify_user", Some(args), &recorder, &slate, 42)
            .expect("notify_user succeeds (typed P5 stub)");
        assert!(out.as_object().unwrap().is_empty());

        let recorded = recorder.snapshot();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].text, "build broke");
        assert_eq!(recorded[0].severity, NotifySeverity::High);
    }

    // ---- notify_user: severity round-trip + unknown ⇒ Medium --------------

    #[test]
    fn severity_round_trips_and_unknown_defaults_to_medium() {
        for (wire, sev) in [
            ("low", NotifySeverity::Low),
            ("medium", NotifySeverity::Medium),
            ("high", NotifySeverity::High),
        ] {
            assert_eq!(NotifySeverity::from_wire(wire), sev);
            assert_eq!(sev.as_wire_str(), wire);
        }
        // Unknown / typo'd severity ⇒ Medium, never an error.
        assert_eq!(NotifySeverity::from_wire("URGENT"), NotifySeverity::Medium);
        assert_eq!(NotifySeverity::from_wire(""), NotifySeverity::Medium);
        assert_eq!(
            NotifySeverity::from_wire("critical"),
            NotifySeverity::Medium
        );
    }

    #[test]
    fn notify_user_unknown_severity_records_medium_without_erroring() {
        let recorder = NotifyRecorder::new();
        let out = notify_user(&recorder, "fyi".to_string(), "whatever", 1);
        assert!(out.as_object().unwrap().is_empty());
        assert_eq!(recorder.snapshot()[0].severity, NotifySeverity::Medium);
    }

    // ---- propose_chip: adds to the Maestro-owned slate --------------------

    #[test]
    fn propose_chip_adds_to_slate() {
        let slate = ChipSlate::new();
        let chip = json!({
            "title": "Compare TokenStore.ts",
            "priority": 50,
            "action": "open_diff",
            "workarea_id": "wa-a",
        });
        let out = propose_chip(&slate, &chip, 1_234).expect("ok");
        assert!(out.as_object().unwrap().is_empty());

        let current = slate.current();
        assert_eq!(current.len(), 1);
        assert_eq!(
            current[0],
            MaestroChip {
                title: "Compare TokenStore.ts".to_string(),
                priority: 50,
                action: "open_diff".to_string(),
                workarea_id: Some(WorkareaId("wa-a".to_string())),
                created_at_ms: 1_234,
            }
        );
    }

    #[test]
    fn propose_chip_via_dispatch_with_minimal_and_unscoped_chip() {
        let recorder = NotifyRecorder::new();
        let slate = ChipSlate::new();
        let mut args = Map::new();
        // A minimal, workspace-unscoped chip: only `title` is required.
        args.insert("chip".into(), json!({ "title": "Review the diff" }));

        let out = dispatch_side("propose_chip", Some(args), &recorder, &slate, 9)
            .expect("propose_chip succeeds");
        assert!(out.as_object().unwrap().is_empty());

        let current = slate.current();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].title, "Review the diff");
        assert_eq!(current[0].priority, 0); // default
        assert_eq!(current[0].action, ""); // default
        assert_eq!(current[0].workarea_id, None); // unscoped
    }

    #[test]
    fn propose_chip_without_title_is_typed_error() {
        let slate = ChipSlate::new();
        let err = propose_chip(&slate, &json!({ "priority": 1 }), 0).expect_err("no title");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(slate.current().is_empty());
    }

    // ---- slate survives the 60 s window (no TTL eviction) -----------------

    /// The V0.1 suggestion engine's chip retention window (`DEDUP_TTL`,
    /// `suggestions/actor.rs:59` / `CHIP_RETENTION = DEDUP_TTL`, line 63) — the
    /// behaviour the Maestro slate deliberately does NOT have. Referenced here
    /// only to make the contrast explicit in the test.
    const SUGGESTION_ENGINE_DEDUP_TTL_MS: i64 = 60_000;

    #[test]
    fn slate_survives_the_dedup_ttl_window_no_time_eviction() {
        let slate = ChipSlate::new();
        // Propose a chip "at" t=0.
        slate.propose(MaestroChip {
            title: "Compare TokenStore.ts".to_string(),
            priority: 10,
            action: "open_diff".to_string(),
            workarea_id: None,
            created_at_ms: 0,
        });

        // Simulate reading the slate WELL past the suggestion engine's 60 s
        // DEDUP_TTL window — the slate has no time-based eviction, so the chip
        // is still present. (The slate carries no clock; `current()` takes no
        // `now`, proving there is nothing to age out against.)
        let much_later = SUGGESTION_ENGINE_DEDUP_TTL_MS * 10;
        let _ = much_later; // no API consumes a clock — the point of the test
        let current = slate.current();
        assert_eq!(
            current.len(),
            1,
            "the Maestro slate must outlive the 60 s DEDUP_TTL window (D11)"
        );
        assert_eq!(current[0].title, "Compare TokenStore.ts");

        // The only way chips leave is an explicit refresh (`clear`), never time.
        slate.clear();
        assert!(slate.current().is_empty());
    }

    // ---- multiple proposals accumulate in order until clear ---------------

    #[test]
    fn proposals_accumulate_in_order_until_clear() {
        let slate = ChipSlate::new();
        for (i, title) in ["first", "second", "third"].into_iter().enumerate() {
            slate.propose(MaestroChip {
                title: title.to_string(),
                priority: i as i32,
                action: "noop".to_string(),
                workarea_id: None,
                created_at_ms: i as i64,
            });
        }
        let current = slate.current();
        let titles: Vec<&str> = current.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["first", "second", "third"]);

        slate.clear();
        assert!(slate.current().is_empty());

        // A fresh slate after clear accumulates anew.
        slate.propose(MaestroChip {
            title: "fourth".to_string(),
            priority: 0,
            action: "noop".to_string(),
            workarea_id: None,
            created_at_ms: 100,
        });
        assert_eq!(slate.current().len(), 1);
        assert_eq!(slate.current()[0].title, "fourth");
    }

    // ---- dispatch routing -------------------------------------------------

    #[test]
    fn dispatch_side_rejects_non_side_channel_tool() {
        let recorder = NotifyRecorder::new();
        let slate = ChipSlate::new();
        let err = dispatch_side("list_workspaces", None, &recorder, &slate, 1)
            .expect_err("not a side-channel tool");
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);

        // notify_user with a missing `text` arg → typed invalid_params.
        let err2 =
            dispatch_side("notify_user", None, &recorder, &slate, 1).expect_err("missing text");
        assert_eq!(err2.code, rmcp::model::ErrorCode::INVALID_PARAMS);

        // propose_chip with a missing `chip` arg → typed invalid_params.
        let err3 =
            dispatch_side("propose_chip", None, &recorder, &slate, 1).expect_err("missing chip");
        assert_eq!(err3.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }

    // ---- frozen-schema round-trip -----------------------------------------

    #[test]
    fn outputs_validate_against_frozen_401_schemas() {
        // Both side-channel tools' FROZEN 401 output schemas are the empty
        // object `{}` (no `required` keys); assert each tool returns an object.
        let recorder = NotifyRecorder::new();
        let slate = ChipSlate::new();
        let descriptors = crate::maestro::tools::all_tools();
        for name in ["notify_user", "propose_chip"] {
            let d = descriptors.iter().find(|d| d.name == name).unwrap();
            assert!(d.output_schema.is_object());
            assert!(
                d.output_schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .is_none(),
                "{name} frozen output schema has no required keys"
            );
        }
        let n = notify_user(&recorder, "x".into(), "low", 1);
        assert!(n.is_object());
        let c = propose_chip(&slate, &json!({ "title": "t" }), 1).unwrap();
        assert!(c.is_object());
    }
}
