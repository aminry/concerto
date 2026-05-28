//! Standard span macros for Concerto log lines.
//!
//! Per design/00 §7.4 and Task 16's public-interface contract, every
//! public function that takes an ID parameter wraps its body in the
//! corresponding span. These macros are the only sanctioned way to
//! create those spans so that field names stay consistent across the
//! codebase and JSON log queries don't break.
//!
//! Usage:
//!
//! ```ignore
//! use concerto_core::workspace_span;
//!
//! fn do_thing(workspace_id: &str) {
//!     let _g = workspace_span!(workspace_id).entered();
//!     // ... work happens inside the span ...
//! }
//! ```
//!
//! The macros expand to `tracing::info_span!` calls with a fixed
//! field name (`workspace_id`, `workarea_id`, `session_id`,
//! `device_id`). The `%` formatter is used so anything `Display` can
//! be passed without an extra allocation.

/// Create an `info`-level span for a workspace-scoped operation.
///
/// Field name: `workspace_id`.
#[macro_export]
macro_rules! workspace_span {
    ($workspace_id:expr) => {
        ::tracing::info_span!("workspace", workspace_id = %$workspace_id)
    };
}

/// Create an `info`-level span for a workarea-scoped operation.
///
/// Field name: `workarea_id`.
#[macro_export]
macro_rules! workarea_span {
    ($workarea_id:expr) => {
        ::tracing::info_span!("workarea", workarea_id = %$workarea_id)
    };
}

/// Create an `info`-level span for an agent-session-scoped operation.
///
/// Field name: `session_id`.
#[macro_export]
macro_rules! session_span {
    ($session_id:expr) => {
        ::tracing::info_span!("session", session_id = %$session_id)
    };
}

/// Create an `info`-level span for a device-scoped operation
/// (placeholder in V0.1; populated by the device-cert subsystem later).
///
/// Field name: `device_id`.
#[macro_export]
macro_rules! device_span {
    ($device_id:expr) => {
        ::tracing::info_span!("device", device_id = %$device_id)
    };
}
