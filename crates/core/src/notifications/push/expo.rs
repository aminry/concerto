//! `ExpoPushBackend` — the V1.0 [`PushBackend`] (design/14 §3.6).
//!
//! Posts ID-only wakeups to Expo's push API (`https://exp.host/--/api/v2/push/send`)
//! using the device's Expo push token. Self-host = BYO Expo creds (design V3):
//! the optional access token comes from `SecretKind::PushExpoApiKey` (keychain),
//! supplied at construction by boot (Task 507). Expo wraps APNs/FCM; it sees
//! only the wakeup id + kind (design/14 §3.2), never content.
//!
//! The request body is built by [`build_message`] (unit-tested without network);
//! `send_wakeup` POSTs it and maps Expo's `{data:{status,details}}` envelope to a
//! [`DeliveryReport`]. Retry/backoff across the fan-out is Task 504's job; this
//! backend does one attempt and reports the outcome.

use async_trait::async_trait;
use concerto_error::{Error, Result};
use serde_json::{json, Value};

use super::{DeliveryReport, PushBackend, PushTarget, WakeupBody};

/// Expo's push endpoint. Overridable in tests / for a self-hosted proxy.
const EXPO_PUSH_URL: &str = "https://exp.host/--/api/v2/push/send";

/// The V1.0 push backend. Cheap to clone (`reqwest::Client` is an `Arc`).
#[derive(Clone)]
pub struct ExpoPushBackend {
    client: reqwest::Client,
    /// Optional Expo access token (`SecretKind::PushExpoApiKey`) for the
    /// `Authorization: Bearer` header when the Expo project enforces it.
    access_token: Option<String>,
    endpoint: String,
}

impl ExpoPushBackend {
    pub fn new(access_token: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            access_token,
            endpoint: EXPO_PUSH_URL.to_string(),
        }
    }

    /// Test/self-host hook: point at a stand-in endpoint.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }
}

/// Build the Expo push message for an ID-only wakeup. `data` carries the
/// [`WakeupBody`] verbatim; `_contentAvailable` + high priority make it a silent
/// background wakeup (design/14 §3.2). Pure — unit-tested without the network.
pub fn build_message(target: &PushTarget, body: &WakeupBody) -> Value {
    json!({
        "to": target.token,
        "data": body,
        "priority": "high",
        "_contentAvailable": true,
    })
}

/// Parse Expo's `{ "data": { "status": "ok"|"error", "details": {"error": ...} } }`
/// envelope into a [`DeliveryReport`]. Unknown shapes ⇒ a generic failure.
fn parse_response(v: &Value) -> DeliveryReport {
    let data = &v["data"];
    match data["status"].as_str() {
        Some("ok") => DeliveryReport::ok(),
        Some("error") => {
            // Prefer the machine-readable `details.error` (e.g.
            // "DeviceNotRegistered") so 504 can null the token.
            let detail = data["details"]["error"]
                .as_str()
                .or_else(|| data["message"].as_str())
                .unwrap_or("expo_error");
            DeliveryReport::failed(detail)
        }
        _ => DeliveryReport::failed("expo_unexpected_response"),
    }
}

#[async_trait]
impl PushBackend for ExpoPushBackend {
    async fn send_wakeup(&self, target: &PushTarget, body: &WakeupBody) -> Result<DeliveryReport> {
        let msg = build_message(target, body);
        let mut req = self
            .client
            .post(&self.endpoint)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .json(&msg);
        if let Some(tok) = &self.access_token {
            req = req.bearer_auth(tok);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Validation(format!("push.expo.transport: {e}")))?;
        if !resp.status().is_success() {
            let code = resp.status();
            return Ok(DeliveryReport::failed(format!(
                "expo_http_{}",
                code.as_u16()
            )));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| Error::Validation(format!("push.expo.decode: {e}")))?;
        Ok(parse_response(&body))
    }

    async fn register_device(
        &self,
        _id: &str,
        _token: &str,
        _platform: super::PushPlatform,
    ) -> Result<()> {
        // Expo tokens are per-install; the Core's `devices` table is the
        // registry. Nothing to register server-side.
        Ok(())
    }

    async fn revoke_device(&self, _device_id: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::push::PushPlatform;

    fn target() -> PushTarget {
        PushTarget {
            device_id: "dev-1".into(),
            token: "ExponentPushToken[abc]".into(),
            platform: PushPlatform::Expo,
        }
    }

    #[test]
    fn build_message_carries_id_only_data() {
        let body = WakeupBody::new("01H", "agent_crashed");
        let msg = build_message(&target(), &body);
        assert_eq!(msg["to"], "ExponentPushToken[abc]");
        assert_eq!(msg["_contentAvailable"], true);
        assert_eq!(msg["priority"], "high");
        // `data` is exactly the ID-only body.
        assert_eq!(msg["data"]["notification_id"], "01H");
        assert_eq!(msg["data"]["kind"], "agent_crashed");
        assert_eq!(msg["data"]["source"], "concerto-relay");
        assert!(msg["data"].get("title").is_none());
        assert!(msg["data"].get("body").is_none());
    }

    #[test]
    fn parse_ok_and_error_envelopes() {
        let ok = serde_json::json!({"data": {"status": "ok", "id": "receipt-1"}});
        assert!(parse_response(&ok).ok);

        let err = serde_json::json!({
            "data": {"status": "error", "message": "...", "details": {"error": "DeviceNotRegistered"}}
        });
        let r = parse_response(&err);
        assert!(!r.ok);
        assert!(r.is_device_not_registered());

        let weird = serde_json::json!({"unexpected": true});
        assert!(!parse_response(&weird).ok);
    }
}
