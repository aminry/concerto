//! Push delivery seam (Task 503; design/14 §3.2/§3.6).
//!
//! The [`PushBackend`] trait is the enterprise-extension seam (`18 §3.7`,
//! modeled on `MaestroProvider`). V1.0 ships [`expo::ExpoPushBackend`] (BYO Expo
//! creds, design V3); [`mock::MockPushBackend`] is the Tier-2 CI double; a
//! `DirectApnsFcmBackend` is a frozen V1.5 swap (not built — the trait is the
//! only seam V1.5 needs).
//!
//! **The wakeup payload is ID-only** ([`WakeupBody`] = `{notification_id, kind,
//! source}` and NOTHING else): Apple/Google/Expo see no workspace/agent/tool
//! content (design/14 §3.2, locked `00 §7.2`). The phone wakes, opens its E2EE
//! Iroh tunnel, and pulls the body via `Notifications.GetNotification` (Task
//! 507). Task 506's property test enforces the no-PII invariant.

use async_trait::async_trait;
use concerto_error::Result;
use serde::{Deserialize, Serialize};

pub mod expo;
pub mod mock;

pub use expo::ExpoPushBackend;
pub use mock::MockPushBackend;

/// The fixed `source` tag in every wakeup (design/14 §3.2).
pub const WAKEUP_SOURCE: &str = "concerto-relay";

/// Push platform a device registered under (the `devices.push_platform` set,
/// widened to add `expo` in migration 0018). V1.0 default is `Expo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushPlatform {
    Apns,
    Fcm,
    Expo,
}

impl PushPlatform {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Apns => "apns",
            Self::Fcm => "fcm",
            Self::Expo => "expo",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "apns" => Self::Apns,
            "fcm" => Self::Fcm,
            "expo" => Self::Expo,
            _ => return None,
        })
    }
}

/// One device to wake: its id + push token + platform (from the `devices` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    pub device_id: String,
    pub token: String,
    pub platform: PushPlatform,
}

/// The **entire** wakeup payload — ID-only, no content (design/14 §3.2). Carried
/// opaquely inside `concerto_transport::WakeupPayload` / the Expo `data` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeupBody {
    pub notification_id: String,
    /// The snake_case notification kind (so the client can pre-style the
    /// notification before the E2EE fetch completes).
    pub kind: String,
    /// Always [`WAKEUP_SOURCE`].
    pub source: String,
}

impl WakeupBody {
    pub fn new(notification_id: impl Into<String>, kind: impl Into<String>) -> Self {
        Self {
            notification_id: notification_id.into(),
            kind: kind.into(),
            source: WAKEUP_SOURCE.to_string(),
        }
    }

    /// The opaque ID-only bytes a [`concerto_transport::api::WakeupPayload`]
    /// carries over the push-hint channel (Task 507/516 drive `send_wakeup_hint`).
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// Per-target delivery outcome. `ok=false` with `detail="DeviceNotRegistered"`
/// signals the caller (504) to null the device's `push_token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    pub ok: bool,
    pub detail: Option<String>,
}

impl DeliveryReport {
    pub fn ok() -> Self {
        Self {
            ok: true,
            detail: None,
        }
    }
    pub fn failed(detail: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: Some(detail.into()),
        }
    }
    /// True iff the failure is a rotated/invalid token the OS dropped — the
    /// caller should clear `devices.push_token` (design/14 §8).
    pub fn is_device_not_registered(&self) -> bool {
        self.detail.as_deref() == Some("DeviceNotRegistered")
    }
}

/// The swappable push-delivery backend (design/14 §3.6). One per Core.
#[async_trait]
pub trait PushBackend: Send + Sync + 'static {
    /// Send a single ID-only wakeup to one device. `Err` ⇒ transport/backend
    /// failure (caller retries); `Ok(DeliveryReport{ok:false,..})` ⇒ the backend
    /// accepted the request but the device rejected it (e.g. stale token).
    async fn send_wakeup(&self, target: &PushTarget, body: &WakeupBody) -> Result<DeliveryReport>;

    /// Register/refresh a device with the backend. No-op for Expo (tokens are
    /// per-install; the Core's `devices` table is the registry).
    async fn register_device(
        &self,
        device_id: &str,
        token: &str,
        platform: PushPlatform,
    ) -> Result<()>;

    /// Tell the backend to stop delivering to a device. No-op for Expo.
    async fn revoke_device(&self, device_id: &str) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakeup_body_is_id_only() {
        let b = WakeupBody::new("01HXYZ", "tool_approval_needed");
        let v: serde_json::Value = serde_json::from_slice(&b.to_bytes()).unwrap();
        let obj = v.as_object().unwrap();
        // EXACTLY three keys, no content fields ever (the privacy invariant;
        // 506 proves this exhaustively).
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        assert_eq!(keys, vec!["kind", "notification_id", "source"]);
        assert_eq!(obj["source"], "concerto-relay");
        assert_eq!(obj["notification_id"], "01HXYZ");
        assert_eq!(obj["kind"], "tool_approval_needed");
    }

    #[test]
    fn platform_db_roundtrips() {
        for p in [PushPlatform::Apns, PushPlatform::Fcm, PushPlatform::Expo] {
            assert_eq!(PushPlatform::from_db(p.as_db()), Some(p));
        }
        assert_eq!(PushPlatform::from_db("nope"), None);
    }

    #[test]
    fn delivery_report_device_not_registered() {
        assert!(DeliveryReport::failed("DeviceNotRegistered").is_device_not_registered());
        assert!(!DeliveryReport::ok().is_device_not_registered());
    }
}
