//! Implementation of the `concerto.v1.Runtime` gRPC service (Task 13).
//!
//! Locked surface — `RuntimeHandler`:
//! - `GetServerCapabilities` returns static + environment-derived data
//!   (server version, schema version, resource limits, transport kind,
//!   host OS, hostname). The `transport_kind` reflects the **live**
//!   connection: it reads the [`crate::conn_transport::ConnTransport`]
//!   extension each listener tags onto its requests, defaulting to
//!   [`TransportKind::Uds`] when absent (Task 201).
//! - `GetStatus` returns the per-process started-at timestamp and an
//!   uptime in seconds.
//!
//! The handler holds:
//! - `started_at` — `Arc<SystemTime>` cloned from [`crate::runtime::Runtime`].
//! - `supervisor_view` — cheap snapshot handle for the future RPC fields
//!   that surface live actor state. V0.1 does not yet return actor
//!   listings, but the wiring is in place so Task 13+ can extend
//!   `RuntimeStatus` without changing the handler's construction.
//!
//! All error paths flow through [`crate::error_map::error_to_status`].

use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use concerto_proto::v1::runtime_server::Runtime as RuntimeService;
use concerto_proto::v1::{ResourceLimits, RuntimeStatus, ServerCapabilities, TransportKind};
use prost_types::Timestamp;
use tonic::{Request, Response, Status};

use crate::conn_transport::ConnTransport;
use crate::supervisor::SupervisorView;

/// Resource limits advertised by `GetServerCapabilities`.
///
/// Frozen for V0.1; tuning is a V1.0 task. The numbers track the local
/// UDS happy-path: 256 concurrent streams comfortably accommodates a
/// dozen agents each with two open streams; 16 MiB matches the SQLite
/// page-cache rough order of magnitude and is the largest single
/// payload the design admits before paging is required.
const MAX_CONCURRENT_STREAMS: u32 = 256;
const MAX_PAYLOAD_BYTES: u64 = 16 * 1024 * 1024;

/// Schema version string returned over the wire. Bumps when the proto
/// package changes from `concerto.v1` to something else; renaming this
/// string is a breaking change for every client.
const SCHEMA_VERSION: &str = "concerto.v1";

/// Implements the generated `Runtime` trait from `concerto-proto`.
///
/// Constructed by [`crate::api_server::ApiServerActor`] with handles
/// cloned from [`crate::runtime::Runtime`]; never built directly by
/// callers.
#[derive(Clone)]
pub struct RuntimeHandler {
    started_at: Arc<SystemTime>,
    /// Cloneable snapshot view of the supervisor. V0.1 does not return
    /// the actor list over the wire, but the handler holds the handle
    /// so subsequent tasks can extend `RuntimeStatus` without touching
    /// the construction path. `#[allow(dead_code)]` mutes the warning
    /// until a consumer lands.
    #[allow(dead_code)]
    supervisor_view: SupervisorView,
}

impl RuntimeHandler {
    /// Build a new handler. `started_at` is the wall-clock instant
    /// recorded by [`crate::runtime::Runtime::start`].
    pub fn new(started_at: Arc<SystemTime>, supervisor_view: SupervisorView) -> Self {
        Self {
            started_at,
            supervisor_view,
        }
    }

    fn server_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn core_hostname() -> String {
        match hostname::get() {
            Ok(h) => h.to_string_lossy().into_owned(),
            Err(e) => {
                tracing::warn!(error = %e, "hostname::get() failed; defaulting to <unknown>");
                "<unknown>".to_string()
            }
        }
    }
}

#[async_trait]
impl RuntimeService for RuntimeHandler {
    #[tracing::instrument(skip_all, name = "Runtime::GetServerCapabilities")]
    async fn get_server_capabilities(
        &self,
        request: Request<()>,
    ) -> Result<Response<ServerCapabilities>, Status> {
        // Report the transport this request physically arrived on. Each
        // listener tags its connections with a `ConnTransport` extension
        // (UDS now, Iroh in 212, WSS bridge in 204 — see
        // `crate::conn_transport`). The handler never infers transport
        // from socket internals; it only reads the tag, defaulting to
        // `Uds` when absent (direct in-process construction in tests, or
        // any not-yet-tagged path).
        let transport_kind = request
            .extensions()
            .get::<ConnTransport>()
            .map(|t| t.kind())
            .unwrap_or(TransportKind::Uds);

        let caps = ServerCapabilities {
            server_version: Self::server_version().to_string(),
            schema_version: SCHEMA_VERSION.to_string(),
            optional_services: Vec::new(),
            limits: Some(ResourceLimits {
                max_concurrent_streams: MAX_CONCURRENT_STREAMS,
                max_payload_bytes: MAX_PAYLOAD_BYTES,
            }),
            transport_kind: transport_kind as i32,
            core_host_os: std::env::consts::OS.to_string(),
            core_hostname: Self::core_hostname(),
        };
        Ok(Response::new(caps))
    }

    #[tracing::instrument(skip_all, name = "Runtime::GetStatus")]
    async fn get_status(&self, _request: Request<()>) -> Result<Response<RuntimeStatus>, Status> {
        let started_at: SystemTime = *self.started_at;
        let now = SystemTime::now();
        let uptime_seconds = now
            .duration_since(started_at)
            .map(|d| d.as_secs())
            // System clock went backward between boot and this call.
            // Report zero uptime rather than fail the RPC — a non-fatal
            // diagnostic surface.
            .unwrap_or(0);

        let status = RuntimeStatus {
            version: Self::server_version().to_string(),
            started_at: Some(system_time_to_prost(started_at)),
            uptime_seconds,
        };
        Ok(Response::new(status))
    }
}

/// Convert a `std::time::SystemTime` into a `prost_types::Timestamp`.
///
/// Clamps pre-epoch times to zero; in practice the runtime's
/// `started_at` is always post-epoch, but the conversion is total so
/// the handler can never panic on a bad clock.
fn system_time_to_prost(t: SystemTime) -> Timestamp {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => Timestamp {
            seconds: d.as_secs() as i64,
            nanos: d.subsec_nanos() as i32,
        },
        Err(_) => Timestamp {
            seconds: 0,
            nanos: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capabilities_advertise_uds_transport() {
        let h = RuntimeHandler::new(Arc::new(SystemTime::now()), SupervisorView::default());
        let caps = h
            .get_server_capabilities(Request::new(()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(caps.transport_kind, TransportKind::Uds as i32);
        assert_eq!(caps.schema_version, SCHEMA_VERSION);
        assert_eq!(caps.server_version, env!("CARGO_PKG_VERSION"));
        let limits = caps.limits.expect("limits");
        assert_eq!(limits.max_concurrent_streams, MAX_CONCURRENT_STREAMS);
        assert_eq!(limits.max_payload_bytes, MAX_PAYLOAD_BYTES);
        assert_eq!(caps.core_host_os, std::env::consts::OS);
        assert!(!caps.core_hostname.is_empty());
    }

    #[tokio::test]
    async fn capabilities_report_injected_iroh_transport() {
        // Proves the per-connection tagging seam end-to-end without a
        // live Iroh listener: a request carrying an injected
        // `ConnTransport(Iroh)` extension makes the handler report IROH.
        // Task 212's Iroh listener tags this same extension; Task 204's
        // WSS bridge tags WSS_BRIDGE — neither touches this handler.
        let h = RuntimeHandler::new(Arc::new(SystemTime::now()), SupervisorView::default());
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(ConnTransport(TransportKind::Iroh));
        let caps = h
            .get_server_capabilities(request)
            .await
            .unwrap()
            .into_inner();
        assert_eq!(caps.transport_kind, TransportKind::Iroh as i32);
    }

    #[tokio::test]
    async fn capabilities_default_to_uds_when_untagged() {
        // Back-compat: a request with no `ConnTransport` (direct
        // in-process construction) defaults to UDS.
        let h = RuntimeHandler::new(Arc::new(SystemTime::now()), SupervisorView::default());
        let caps = h
            .get_server_capabilities(Request::new(()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(caps.transport_kind, TransportKind::Uds as i32);
    }

    #[tokio::test]
    async fn status_reports_nonzero_uptime_after_sleep() {
        let started = SystemTime::now() - std::time::Duration::from_secs(3);
        let h = RuntimeHandler::new(Arc::new(started), SupervisorView::default());
        let st = h.get_status(Request::new(())).await.unwrap().into_inner();
        assert!(
            st.uptime_seconds >= 3,
            "uptime={} should be >=3",
            st.uptime_seconds
        );
        assert_eq!(st.version, env!("CARGO_PKG_VERSION"));
        assert!(st.started_at.is_some());
    }
}
