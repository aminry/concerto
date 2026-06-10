//! The `MaestroHandle` Core-side API surface (Task 401.5; design/08 §5.2,
//! PHASE4_PLANNING §4.2).
//!
//! This is the **stability contract** the Desktop UI (Task 415) and the rest
//! of the Rust spine build against, frozen here before any Maestro logic
//! exists. [`MaestroHandle`] is an **opaque** struct with five frozen async
//! signatures; each returns a typed `"unimplemented:"`-prefixed
//! [`concerto_error::Error`] (mirroring 313's `unimplemented_err` discipline —
//! NEVER `todo!()`/`unimplemented!()`, NEVER empty-success) until Task 414
//! supplies the real actor.
//!
//! 414 replaces the unit body of this struct with the actor channel and the
//! method bodies with real sends; the *signatures* below do not change.

use concerto_error::{Error, Result};
use concerto_persist::WorkareaId;
use concerto_proto::v1::{Digest, MaestroAttachment, MaestroVisibility};

/// Stable `"unimplemented:"`-prefixed error for the signature-frozen
/// [`MaestroHandle`] seams (mirrors `concerto_vcs::unimplemented_err`). Surfaces
/// as `Error::Internal` so callers + tests can recognize a frozen stub by the
/// prefix without a new error variant (313 precedent).
fn unimplemented_err(what: &str) -> Error {
    Error::Internal(format!("unimplemented: {what}"))
}

/// A minimal Core-side read-model of the Maestro's live state (design/08 §4.1
/// `maestro_state`). Frozen here as the `get_state` return shape; Task 414
/// fills it from Task 403's `maestro_state` singleton row. All instants are
/// `i64` unix-ms (PHASE4_PLANNING §2 — NOT `Instant`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaestroStateView {
    /// Whether the Maestro is enabled (vs disabled by the user or by
    /// `enterpriseDataPrivacy` policy — design/08 §3.10).
    pub enabled: bool,
    /// Input tokens spent today (the cumulative-across-backends daily budget,
    /// design/08 §3.9). Owned/wired by Task 403/412.
    pub daily_in_today: i64,
    /// Output tokens spent today.
    pub daily_out_today: i64,
    /// Unix-ms of the last generated digest, or `None` if none yet.
    pub last_digest_at_ms: Option<i64>,
}

/// The Core-side Maestro API (design/08 §5.2). **Opaque** struct: Task 414
/// replaces the unit body with the actor channel. The five signatures below
/// are FROZEN; until 414 each returns a typed `"unimplemented:"`-prefixed
/// `Err` (never `todo!()`, never empty-success).
#[derive(Debug, Clone)]
pub struct MaestroHandle {
    // Opaque. Task 414 stores the actor's `mpsc::Sender` here. Held as a
    // zero-sized placeholder so the type is constructible by 414's wiring (and
    // by tests) without exposing any field publicly.
    _opaque: (),
}

impl MaestroHandle {
    /// Send the user's chat input to the Maestro (design/08 §5.2).
    pub async fn send_to_maestro(
        &self,
        text: String,
        attachments: Vec<MaestroAttachment>,
    ) -> Result<()> {
        let _ = (text, attachments);
        Err(unimplemented_err(
            "MaestroHandle::send_to_maestro: not implemented until Task 414",
        ))
    }

    /// Return the current digest (design/08 §3.6 / §5.2).
    pub async fn get_digest(&self) -> Result<Digest> {
        Err(unimplemented_err(
            "MaestroHandle::get_digest: not implemented until Task 414",
        ))
    }

    /// Set the per-workarea Maestro visibility (design/08 §3.3 / §5.2).
    pub async fn set_workarea_visibility(
        &self,
        wa: WorkareaId,
        vis: MaestroVisibility,
    ) -> Result<()> {
        let _ = (wa, vis);
        Err(unimplemented_err(
            "MaestroHandle::set_workarea_visibility: not implemented until Task 414",
        ))
    }

    /// Enable or disable the Maestro (design/08 §5.2).
    pub async fn set_enabled(&self, on: bool) -> Result<()> {
        let _ = on;
        Err(unimplemented_err(
            "MaestroHandle::set_enabled: not implemented until Task 414",
        ))
    }

    /// Read the Maestro's live state view (design/08 §5.2).
    pub async fn get_state(&self) -> Result<MaestroStateView> {
        Err(unimplemented_err(
            "MaestroHandle::get_state: not implemented until Task 414",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct an opaque handle for the surface-freeze tests (414 builds the
    /// real one from the actor). Kept test-only so the frozen seam has no
    /// public constructor before 414.
    fn frozen_handle() -> MaestroHandle {
        MaestroHandle { _opaque: () }
    }

    fn assert_unimplemented(err: &Error) {
        match err {
            Error::Internal(m) => assert!(
                m.starts_with("unimplemented:"),
                "expected `unimplemented:`-prefixed error, got: {m}"
            ),
            other => panic!("expected Error::Internal(unimplemented: ..), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_to_maestro_is_typed_unimplemented() {
        let h = frozen_handle();
        let err = h
            .send_to_maestro(String::new(), Vec::new())
            .await
            .unwrap_err();
        assert_unimplemented(&err);
    }

    #[tokio::test]
    async fn get_digest_is_typed_unimplemented() {
        let h = frozen_handle();
        let err = h.get_digest().await.unwrap_err();
        assert_unimplemented(&err);
    }

    #[tokio::test]
    async fn set_workarea_visibility_is_typed_unimplemented() {
        let h = frozen_handle();
        let err = h
            .set_workarea_visibility(WorkareaId("wa-1".to_string()), MaestroVisibility::Full)
            .await
            .unwrap_err();
        assert_unimplemented(&err);
    }

    #[tokio::test]
    async fn set_enabled_is_typed_unimplemented() {
        let h = frozen_handle();
        let err = h.set_enabled(true).await.unwrap_err();
        assert_unimplemented(&err);
    }

    #[tokio::test]
    async fn get_state_is_typed_unimplemented() {
        let h = frozen_handle();
        let err = h.get_state().await.unwrap_err();
        assert_unimplemented(&err);
    }
}
