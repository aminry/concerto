//! gRPC `Maestro` service handler (Task 401.5 — wire-contract freeze).
//!
//! The surface is FROZEN here; the **impl is deferred to Task 414**. Every RPC
//! returns a typed `Status::unimplemented` (the `UpsertProjectMcp`/305 seam
//! discipline — NEVER `todo!()`/`unimplemented!()`, NEVER empty-success: an
//! empty-success `GetDigest` would let Task 415 build against a lie). 414 wires
//! the real [`MaestroHandle`] into [`MaestroHandler::handle`] and replaces the
//! bodies; the registration at BOTH front-door sites (`add_core_services` +
//! `connect_bridge::build_and_serve`, D8) already exists.
//!
//! `#[cfg(unix)]`-gated because the Maestro sits over the `#[cfg(unix)]` agent
//! supervisor (mirrors `sessions`/`streams`/`suggestions`).

use concerto_proto::v1::maestro_server::Maestro as MaestroService;
use concerto_proto::v1::{Digest, GetDigestRequest, MaestroMessageRequest, VisibilityRequest};
use tonic::{Request, Response, Status};

use crate::maestro::MaestroHandle;

/// Implements the generated `Maestro` service trait.
///
/// Holds an `Option<MaestroHandle>` so Task 414 can thread a live handle
/// through `boot.rs`/`CoreServiceSet`/`BridgeServices`; until then it is always
/// `None` and every RPC returns `Status::unimplemented`.
#[derive(Clone)]
pub struct MaestroHandler {
    /// The live Maestro handle (Task 414). `None` in 401.5 — the surface is
    /// frozen but unwired, so every RPC returns `UNIMPLEMENTED`.
    #[allow(dead_code)]
    handle: Option<MaestroHandle>,
}

impl MaestroHandler {
    pub fn new(handle: Option<MaestroHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait::async_trait]
impl MaestroService for MaestroHandler {
    async fn send_to_maestro(
        &self,
        _request: Request<MaestroMessageRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented(
            "maestro.send_to_maestro: not implemented until Task 414",
        ))
    }

    async fn get_digest(
        &self,
        _request: Request<GetDigestRequest>,
    ) -> Result<Response<Digest>, Status> {
        Err(Status::unimplemented(
            "maestro.get_digest: not implemented until Task 414",
        ))
    }

    async fn set_workarea_visibility(
        &self,
        _request: Request<VisibilityRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented(
            "maestro.set_workarea_visibility: not implemented until Task 414",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_to_maestro_returns_unimplemented() {
        let h = MaestroHandler::new(None);
        let err = h
            .send_to_maestro(Request::new(MaestroMessageRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }

    #[tokio::test]
    async fn get_digest_returns_unimplemented() {
        let h = MaestroHandler::new(None);
        let err = h
            .get_digest(Request::new(GetDigestRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }

    #[tokio::test]
    async fn set_workarea_visibility_returns_unimplemented() {
        let h = MaestroHandler::new(None);
        let err = h
            .set_workarea_visibility(Request::new(VisibilityRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }
}
