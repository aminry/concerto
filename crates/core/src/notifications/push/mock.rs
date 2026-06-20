//! `MockPushBackend` — the Tier-2 CI double for [`PushBackend`] (design/14 §10).
//!
//! Records every `send_wakeup` so fan-out tests (504) can assert which devices
//! were woken, first-to-approve-wins, and the no-second-wakeup-on-dedup
//! invariant — and can program the per-call outcome to exercise the retry /
//! stale-token / Expo-down failure paths without any network.

use std::sync::Mutex;

use async_trait::async_trait;
use concerto_error::{Error, Result};

use super::{DeliveryReport, PushBackend, PushPlatform, PushTarget, WakeupBody};

/// One recorded `send_wakeup` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockSend {
    pub target: PushTarget,
    pub body: WakeupBody,
}

/// What the mock returns from `send_wakeup`.
#[derive(Debug, Clone)]
pub enum MockOutcome {
    /// Return this report (`DeliveryReport::ok()` by default).
    Report(DeliveryReport),
    /// Simulate a transport/backend error (caller retries).
    TransportError,
}

impl Default for MockOutcome {
    fn default() -> Self {
        MockOutcome::Report(DeliveryReport::ok())
    }
}

#[derive(Default)]
pub struct MockPushBackend {
    sends: Mutex<Vec<MockSend>>,
    outcome: Mutex<MockOutcome>,
}

impl MockPushBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Program the outcome returned by subsequent `send_wakeup` calls.
    pub fn set_outcome(&self, outcome: MockOutcome) {
        *self.outcome.lock().unwrap() = outcome;
    }

    /// Every recorded `send_wakeup` (in call order).
    pub fn sends(&self) -> Vec<MockSend> {
        self.sends.lock().unwrap().clone()
    }

    /// Count of recorded `send_wakeup` calls.
    pub fn send_count(&self) -> usize {
        self.sends.lock().unwrap().len()
    }
}

#[async_trait]
impl PushBackend for MockPushBackend {
    async fn send_wakeup(&self, target: &PushTarget, body: &WakeupBody) -> Result<DeliveryReport> {
        self.sends.lock().unwrap().push(MockSend {
            target: target.clone(),
            body: body.clone(),
        });
        match self.outcome.lock().unwrap().clone() {
            MockOutcome::Report(r) => Ok(r),
            MockOutcome::TransportError => {
                Err(Error::Validation("push.mock.transport_error".into()))
            }
        }
    }

    async fn register_device(
        &self,
        _id: &str,
        _token: &str,
        _platform: PushPlatform,
    ) -> Result<()> {
        Ok(())
    }

    async fn revoke_device(&self, _device_id: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> PushTarget {
        PushTarget {
            device_id: "dev-1".into(),
            token: "tok".into(),
            platform: PushPlatform::Expo,
        }
    }

    #[tokio::test]
    async fn records_sends_and_returns_ok_by_default() {
        let mock = MockPushBackend::new();
        let body = WakeupBody::new("n-1", "tool_approval_needed");
        let r = mock.send_wakeup(&target(), &body).await.unwrap();
        assert!(r.ok);
        assert_eq!(mock.send_count(), 1);
        assert_eq!(mock.sends()[0].body, body);
    }

    #[tokio::test]
    async fn programmed_transport_error() {
        let mock = MockPushBackend::new();
        mock.set_outcome(MockOutcome::TransportError);
        let err = mock
            .send_wakeup(&target(), &WakeupBody::new("n", "agent_crashed"))
            .await;
        assert!(err.is_err());
        // The call is still recorded (so a retry test can count attempts).
        assert_eq!(mock.send_count(), 1);
    }

    #[tokio::test]
    async fn programmed_stale_token_report() {
        let mock = MockPushBackend::new();
        mock.set_outcome(MockOutcome::Report(DeliveryReport::failed(
            "DeviceNotRegistered",
        )));
        let r = mock
            .send_wakeup(&target(), &WakeupBody::new("n", "agent_crashed"))
            .await
            .unwrap();
        assert!(r.is_device_not_registered());
    }
}
