//! Security subsystem (Task 32 onward, design/12).
//!
//! Owns the per-session [`permission::PermissionMode`] resolver, the
//! managed-policy ([`managed::ManagedPolicy`]) reader, and entry-ceremony
//! enforcement for elevated modes. Task 32 ships the inheritance walk +
//! the `managed.json` cap; downstream tasks (33 tool-approval intercept,
//! 41/42/43 filesystem allow/deny + destructive-command intercept) plug
//! enforcement into the runtime.
//!
//! The acknowledgement strings frozen by this task:
//!
//! - `"I understand"` — required to set a workarea/session to
//!   [`permission::PermissionMode::Yolo`].
//! - `"I understand the risks"` — required to set
//!   `workareas.bypass_destructive_guard = true`.
//!
//! Both literals are checked by [`permission::ack_for_yolo`] /
//! [`permission::ack_for_bypass_destructive_guard`].

pub mod destructive;
pub mod identity;
pub mod managed;
pub mod pairing;
pub mod path_policy;
pub mod permission;
pub mod tool_classes;

pub use destructive::{is_destructive, DestructiveMatch};
pub use managed::{load_managed_policy, ManagedPolicy, ManagedPolicySource};
pub use path_policy::{classify as classify_path, AllowList, DenyList, PathDecision};
pub use permission::{
    ack_for_bypass_destructive_guard, ack_for_yolo, parse_permission_mode, resolve_effective_mode,
    Decision, EffectiveMode, ModeSource, PermissionMode, PermissionResolver, ToolClass,
    ACK_BYPASS_DESTRUCTIVE_GUARD, ACK_YOLO, POLICY_BYPASS_BLOCKED, POLICY_LOCKED_GENERIC,
    POLICY_YOLO_BLOCKED,
};
