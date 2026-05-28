//! Tauri command surface — the *only* IPC entry points the renderer
//! sees.
//!
//! V0.1 ships two commands:
//!
//! - [`concerto_ping`] — a smoke probe that round-trips a static
//!   string. Lets the renderer prove the IPC bridge works before
//!   blaming a gRPC bug.
//! - [`concerto_rpc`] — the single dispatch entry point for every
//!   gRPC call. Method names follow `"<Service>.<Rpc>"`. Only
//!   `"Runtime.GetServerCapabilities"` is wired today; future tasks
//!   add cases by extending the `match` in [`dispatch`].
//!
//! The renderer is forbidden from speaking gRPC directly (see
//! `apps/desktop/src-tauri/capabilities/main.json` — no `http`, no
//! `shell`, no `fs` permissions). All Core traffic flows through
//! `concerto_rpc`.

use std::path::PathBuf;

use concerto_proto::v1::runtime_client::RuntimeClient;
use serde_json::{json, Value};

use crate::core_client::{connect_uds, default_socket_path, CoreClientError};

/// Renderer → shell smoke ping. Returns `"pong"`.
#[tauri::command]
pub async fn concerto_ping() -> Result<String, CoreClientError> {
    Ok("pong".to_string())
}

/// Renderer → shell gRPC dispatch. `method` is `"<Service>.<Rpc>"`;
/// `payload` is the request body as JSON. Returns the response body
/// as JSON.
///
/// V0.1 only wires `"Runtime.GetServerCapabilities"`. Adding a new
/// method means extending [`dispatch`] — a single point of growth.
#[tauri::command]
pub async fn concerto_rpc(method: String, payload: Value) -> Result<Value, CoreClientError> {
    let socket_path = default_socket_path().ok_or_else(|| {
        CoreClientError::Transport("HOME not set — cannot resolve ~/.concerto/core.sock".into())
    })?;
    dispatch(socket_path, &method, payload).await
}

/// Method-dispatch core, factored out of the Tauri command so it can
/// be unit-tested against a tempdir socket without standing up the
/// Tauri runtime.
pub(crate) async fn dispatch(
    socket_path: PathBuf,
    method: &str,
    _payload: Value,
) -> Result<Value, CoreClientError> {
    match method {
        "Runtime.GetServerCapabilities" => {
            let channel = connect_uds(&socket_path).await?;
            let mut client = RuntimeClient::new(channel);
            let resp = client
                .get_server_capabilities(())
                .await
                .map_err(|s| CoreClientError::Rpc(format!("{}: {}", s.code(), s.message())))?;
            let caps = resp.into_inner();
            // Hand-roll the JSON shape to keep the wire surface stable
            // even if `prost`'s default serde mapping shifts. Field
            // names mirror the proto.
            Ok(json!({
                "server_version": caps.server_version,
                "schema_version": caps.schema_version,
                "optional_services": caps.optional_services,
                "limits": caps.limits.map(|l| json!({
                    "max_concurrent_streams": l.max_concurrent_streams,
                    "max_payload_bytes": l.max_payload_bytes,
                })),
                "transport_kind": caps.transport_kind,
                "core_host_os": caps.core_host_os,
                "core_hostname": caps.core_hostname,
            }))
        }
        other => Err(CoreClientError::NotImplemented(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_method_returns_not_implemented() {
        let sock = std::path::PathBuf::from("/tmp/concerto-nonexistent.sock");
        let err = dispatch(sock, "Bogus.Method", json!({}))
            .await
            .expect_err("should fail");
        match err {
            CoreClientError::NotImplemented(m) => assert_eq!(m, "Bogus.Method"),
            other => panic!("expected NotImplemented, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn known_method_with_missing_socket_returns_transport_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("nope.sock");
        let err = dispatch(sock, "Runtime.GetServerCapabilities", json!({}))
            .await
            .expect_err("should fail without a running Core");
        match err {
            CoreClientError::Transport(_) => {}
            other => panic!("expected Transport, got {other:?}"),
        }
    }
}
