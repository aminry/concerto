//! Audit log writer (Task 44).
//!
//! Per `design/09 §3.5`: every state-changing event flows through a
//! fan-out `AuditWriter` that drains a bounded channel and writes
//! typed events to a JSONL file at
//! `<data_dir>/audit/audit-<YYYY-MM-DD>.jsonl` (daily UTC rotation).
//!
//! ## V0.1 scope
//!
//! - [`event::AuditEvent`], [`event::AuditKind`], [`event::AuditActor`],
//!   [`event::EntityKind`], [`event::SubjectRef`] — the typed event
//!   surface. Frozen.
//! - [`writer::AuditWriter`] — the cheap-cloneable producer handle.
//!   `try_send`; drop-on-full per `design/10 §8`.
//! - [`writer::AuditWriterTask`] — the background drainer that fans out
//!   events to every registered subscriber.
//! - [`writer::AuditLogSubscriber`] trait — the fan-out hook. Frozen
//!   for V0.1; V1.0 adds syslog + HttpsForwarder impls.
//! - [`jsonl::JsonlFileSubscriber`] — the canonical on-disk writer.
//!   Daily rotation via an injectable clock.
//!
//! ## Wiring
//!
//! `main.rs` spawns the writer task at boot and hands a clone of the
//! [`writer::AuditWriter`] to every manager that wants to emit events.
//! Pre-existing `tracing::info!(audit.kind=...)` emissions remain in
//! place; V0.1 demonstrates one or two structured emissions
//! (workspace-create, permission-mode-change) and defers the rest
//! to a follow-on task — see the Task 44 handoff notes.

pub mod api;
pub mod event;
pub mod jsonl;
pub mod writer;

pub use event::{AuditActor, AuditEvent, AuditKind, EntityKind, SubjectRef};
pub use jsonl::{system_clock, ClockFn, JsonlFileSubscriber};
pub use writer::{AuditLogSubscriber, AuditWriter, AuditWriterTask, AUDIT_QUEUE_CAPACITY};
