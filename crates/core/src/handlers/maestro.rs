//! gRPC `Maestro` service handler (Task 401.5 froze the surface; Task 414 fills
//! the live impl).
//!
//! The handler is the **thin gRPC adapter** over the in-process
//! [`MaestroHandle`] (design/08 §5.2): every RPC delegates to the handle; the
//! only logic here is request validation, `Status` mapping, and shaping the
//! unary reply. The routing/digest/visibility business logic lives behind the
//! handle (which stitches 408's `pre_parse`, 409's `generate_digest`, 413's
//! visibility toggle) — see [`crate::maestro::handle`].
//!
//! ## The inert seams (typed `Status`, never the macro — 305/313 discipline)
//!
//! - **Policy-disabled at boot** (`enterpriseDataPrivacy` + external model, D1):
//!   `boot.rs` never constructs the handle, so `self.handle` is `None` and every
//!   RPC returns `Status::failed_precondition("maestro.disabled_by_policy")` —
//!   NOT `unimplemented!()`, NOT empty-success (an empty-success `GetDigest`
//!   would let 415 build against a lie).
//! - **Budget-exhausted at run time** (412's tripwire flips the handle inert):
//!   the handle returns a typed `Error::Policy("maestro.budget_exhausted")`,
//!   which `error_to_status` maps to `FailedPrecondition` — the UI shows the
//!   last-good digest stale (R-7) rather than a 500. 412 wires the counter; the
//!   path is exercised by the test double until then.
//!
//! `#[cfg(unix)]`-gated because the Maestro sits over the `#[cfg(unix)]` agent
//! supervisor (mirrors `sessions`/`streams`/`suggestions`).

use concerto_proto::v1::maestro_server::Maestro as MaestroService;
use concerto_proto::v1::{
    Digest, GetDigestRequest, GetStateRequest, MaestroMessageRequest, MaestroState,
    MaestroVisibility, VisibilityRequest,
};
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::llm::{DEFAULT_DAILY_IN_CAP, DEFAULT_DAILY_OUT_CAP};
use crate::maestro::{InertReason, MaestroHandle};
use concerto_persist::WorkareaId;

/// The `Status` a policy-disabled (handle un-constructed at boot) RPC returns.
/// FROZEN string — 415 keys off the `maestro.disabled_by_policy` message.
const DISABLED_BY_POLICY: &str = "maestro.disabled_by_policy";

/// Implements the generated `Maestro` service trait. Holds an
/// `Option<MaestroHandle>`: `Some` when the boot gate is open (the live
/// service), `None` when the Maestro is disabled by policy (the inert seam).
#[derive(Clone)]
pub struct MaestroHandler {
    handle: Option<MaestroHandle>,
}

impl MaestroHandler {
    pub fn new(handle: Option<MaestroHandle>) -> Self {
        Self { handle }
    }

    /// The live handle, or the typed policy-disabled `Status` when the boot gate
    /// left it `None`. The inert reply is `failed_precondition`, NEVER
    /// `unimplemented` — the disabled Maestro is a real, documented state.
    ///
    /// `Status` is a large `Err` variant (the `tonic` type); boxing it here would
    /// fight the `?` ergonomics at every RPC, so we follow the handler-wide
    /// precedent (`sessions`/`streams`/`workareas`) and allow the lint.
    #[allow(clippy::result_large_err)]
    fn handle(&self) -> Result<&MaestroHandle, Status> {
        self.handle
            .as_ref()
            .ok_or_else(|| Status::failed_precondition(DISABLED_BY_POLICY))
    }
}

#[async_trait::async_trait]
impl MaestroService for MaestroHandler {
    #[tracing::instrument(skip_all, name = "Maestro::SendToMaestro")]
    async fn send_to_maestro(
        &self,
        request: Request<MaestroMessageRequest>,
    ) -> Result<Response<()>, Status> {
        let handle = self.handle()?;
        let req = request.into_inner();
        // The handle runs 408's `pre_parse` → routes / handles slash / forwards
        // freeform, emitting the matching `maestro.events`. The dispatch outcome
        // rides `maestro.events`, not this unary reply (which is `Empty`).
        handle
            .send_to_maestro(req.text, req.attachments)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Maestro::GetDigest")]
    async fn get_digest(
        &self,
        _request: Request<GetDigestRequest>,
    ) -> Result<Response<Digest>, Status> {
        let handle = self.handle()?;
        // 409's digest path (force-refresh-stale-60s summaries then compose,
        // `<5s p50`), mapped onto the proto `Digest` 401.5 froze + chips; the
        // handle emits `maestro.digest_generated`.
        let digest = handle.get_digest().await.map_err(error_to_status)?;
        Ok(Response::new(digest))
    }

    #[tracing::instrument(skip_all, name = "Maestro::SetWorkareaVisibility")]
    async fn set_workarea_visibility(
        &self,
        request: Request<VisibilityRequest>,
    ) -> Result<Response<()>, Status> {
        let handle = self.handle()?;
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let visibility =
            MaestroVisibility::try_from(req.visibility).unwrap_or(MaestroVisibility::Unspecified);
        // 413's `exclude_from_maestro` toggle behind the handle; typed
        // `error_to_status` on failure (Unspecified ⇒ validation error).
        handle
            .set_workarea_visibility(WorkareaId(req.workarea_id), visibility)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Maestro::GetState")]
    async fn get_state(
        &self,
        _request: Request<GetStateRequest>,
    ) -> Result<Response<MaestroState>, Status> {
        let handle = self.handle()?;
        // Read projection (mirrors `get_digest`): assemble the wire `MaestroState`
        // from the handle's read-model (enabled/counts/last_digest) + 412's caps
        // + the inert reason + the live Maestro session id (Task 417 subscribes
        // to its `session.events.<sid>` for write-tool approval chips). No
        // business logic; the handle owns every source.
        let view = handle.get_state().await.map_err(error_to_status)?;
        // `inert_reason()` ⇒ the `inert` flag + the FROZEN reason string
        // (`"" | "budget_exhausted" | "disabled_by_policy"`), consistent with
        // what 412 enforces and 414 emits.
        let (inert, inert_reason) = match handle.inert_reason().await {
            None => (false, String::new()),
            Some(InertReason::BudgetExhausted { .. }) => (true, "budget_exhausted".to_string()),
            Some(InertReason::DisabledByPolicy { .. }) => (true, "disabled_by_policy".to_string()),
        };
        let state = MaestroState {
            enabled: view.enabled,
            daily_in_today: view.daily_in_today,
            daily_out_today: view.daily_out_today,
            in_cap: DEFAULT_DAILY_IN_CAP as i64,
            out_cap: DEFAULT_DAILY_OUT_CAP as i64,
            last_digest_at_ms: view.last_digest_at_ms.unwrap_or(0),
            inert,
            inert_reason,
            // Empty string when there is no live Maestro session (417 keys off
            // this to decide whether to subscribe to the session's events).
            maestro_session_id: handle.maestro_session_id_str().await,
        };
        Ok(Response::new(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A policy-disabled handler (no handle constructed at boot) returns
    /// `failed_precondition("maestro.disabled_by_policy")` for every RPC — NOT
    /// `unimplemented`, NOT empty-success.
    #[tokio::test]
    async fn policy_disabled_get_digest_is_failed_precondition_not_unimplemented() {
        let h = MaestroHandler::new(None);
        let err = h
            .get_digest(Request::new(GetDigestRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert_eq!(err.message(), DISABLED_BY_POLICY);
    }

    #[tokio::test]
    async fn policy_disabled_send_to_maestro_is_failed_precondition() {
        let h = MaestroHandler::new(None);
        let err = h
            .send_to_maestro(Request::new(MaestroMessageRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert_eq!(err.message(), DISABLED_BY_POLICY);
    }

    #[tokio::test]
    async fn policy_disabled_set_visibility_is_failed_precondition() {
        let h = MaestroHandler::new(None);
        let err = h
            .set_workarea_visibility(Request::new(VisibilityRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert_eq!(err.message(), DISABLED_BY_POLICY);
    }

    #[tokio::test]
    async fn policy_disabled_get_state_is_failed_precondition() {
        let h = MaestroHandler::new(None);
        let err = h
            .get_state(Request::new(GetStateRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert_eq!(err.message(), DISABLED_BY_POLICY);
    }
}
