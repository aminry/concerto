//! Public re-exports for the audit module (Task 44; Task 112).
//!
//! Callers should import from `concerto_core::audit::*` directly via
//! the parent `mod.rs`; this submodule exists to gather the public
//! surface in one place for `regen-interfaces.sh` discovery.

pub use super::event::{AuditActor, AuditEvent, AuditKind, EntityKind, SubjectRef};
pub use super::https::HttpsForwarderSubscriber;
pub use super::jsonl::{
    system_clock, ClockFn, JsonlFileSubscriber, RotationConfig, DEFAULT_RETENTION_DAYS,
};
pub use super::stdout::StdoutSubscriber;
pub use super::syslog::{SyslogSubscriber, SyslogTransport};
pub use super::writer::{AuditLogSubscriber, AuditWriter, AuditWriterTask, AUDIT_QUEUE_CAPACITY};
