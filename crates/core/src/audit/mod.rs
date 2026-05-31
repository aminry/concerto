//! Audit log pipeline (Task 44; generalized into a subscriber fan-out in
//! Task 112).
//!
//! Per `design/09 §3.5`: every state-changing event flows through a
//! fan-out [`writer::AuditWriter`] that drains a bounded channel and
//! dispatches each event to a chain of [`writer::AuditLogSubscriber`]
//! implementations. The always-on [`jsonl::JsonlFileSubscriber`] writes
//! typed events to a JSONL file at
//! `<data_dir>/audit/audit-<YYYY-MM-DD>.jsonl` (daily UTC rotation, with
//! optional size rotation + 90-day retention); the other subscribers are
//! opt-in network forwarders.
//!
//! ## Subscriber set
//!
//! - [`event::AuditEvent`], [`event::AuditKind`], [`event::AuditActor`],
//!   [`event::EntityKind`], [`event::SubjectRef`] — the typed event
//!   surface. Frozen.
//! - [`writer::AuditWriter`] — the cheap-cloneable producer handle.
//!   `try_send`; drop-on-full per `design/10 §8`.
//! - [`writer::AuditWriterTask`] — the background drainer that fans out
//!   events to every registered subscriber (JSONL first — the durable
//!   floor — then opt-in forwarders).
//! - [`writer::AuditLogSubscriber`] trait — the fan-out hook. FROZEN
//!   `design/18 §3.7` extension seam (`id` / `on_event` / `flush`).
//! - V1.0 OSS impls (all MIT, in this crate):
//!   - [`jsonl::JsonlFileSubscriber`] — canonical on-disk writer; always
//!     present.
//!   - [`stdout::StdoutSubscriber`] — debug echo to stdout.
//!   - [`syslog::SyslogSubscriber`] — RFC 5424 over UDP/TCP.
//!   - [`https::HttpsForwarderSubscriber`] — NDJSON POST to an endpoint.
//!
//! ### Reserved V2.0+ BSL impls (NOT in this MIT crate)
//!
//! Per `design/18 §3.7` the following enterprise subscribers ship in
//! their own BSL crates and are reserved here by name only so Task 707's
//! trait-seam completeness check passes without a Core refork:
//!
//! - `SiemForwarderSubscriber` — `crates/enterprise-siem` (BSL):
//!   multi-tenant SIEM integration (Splunk HEC / Elastic / Datadog),
//!   retry-with-replay buffer, field mapping, compliance attestations.
//! - `EncryptedAtRestSubscriber` — `crates/enterprise-encrypted-audit`
//!   (BSL): AES-256-GCM at-rest writer with a keychain-derived key.
//!
//! ## Wiring
//!
//! `boot.rs` spawns the writer task at boot with the JSONL subscriber
//! registered first and hands a clone of the [`writer::AuditWriter`] to
//! every manager that emits events. Opt-in forwarders are appended to the
//! subscriber vec when configured (see the Task 112 handoff notes for the
//! current config seam).

pub mod api;
pub mod event;
pub mod https;
pub mod jsonl;
pub mod stdout;
pub mod syslog;
pub mod writer;

pub use event::{AuditActor, AuditEvent, AuditKind, EntityKind, SubjectRef};
pub use https::HttpsForwarderSubscriber;
pub use jsonl::{
    system_clock, ClockFn, JsonlFileSubscriber, RotationConfig, DEFAULT_RETENTION_DAYS,
};
pub use stdout::StdoutSubscriber;
pub use syslog::{SyslogSubscriber, SyslogTransport};
pub use writer::{AuditLogSubscriber, AuditWriter, AuditWriterTask, AUDIT_QUEUE_CAPACITY};
