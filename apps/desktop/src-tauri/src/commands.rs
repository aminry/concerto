//! Tauri command surface — the *only* IPC entry points the renderer
//! sees.
//!
//! Task 24 widens the V0.1 surface to cover the read-side of every
//! gRPC service that exists today plus a server-streaming bridge:
//!
//! - [`concerto_ping`] — smoke probe; round-trips a static string.
//! - [`concerto_rpc`] — single dispatch entry for unary RPCs. Method
//!   names follow `"<Service>.<Rpc>"`. Each new arm in [`dispatch`]
//!   reuses the persistent gRPC channel from [`crate::core_client`].
//! - [`concerto_subscribe`] — opens a `Streams.Subscribe(subject)`
//!   server-stream on the Rust side and emits each frame on the
//!   Tauri event bus under `"concerto/<subject>"`. Returns a
//!   `SubscriptionId` the renderer can hand back to
//!   [`concerto_unsubscribe`] to drop the stream.
//! - [`concerto_unsubscribe`] — aborts the spawned forwarder task
//!   and removes the id from the registry.
//!
//! The renderer is forbidden from speaking gRPC directly (see
//! `apps/desktop/src-tauri/capabilities/main.json` — no `http`, no
//! `shell`, no `fs` permissions). All Core traffic flows through
//! these commands.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use concerto_proto::v1::projects_client::ProjectsClient;
use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::sessions_client::SessionsClient;
use concerto_proto::v1::streams_client::StreamsClient;
use concerto_proto::v1::workareas_client::WorkareasClient;
use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{
    ListProjectsRequest, ListWorkspacesRequest, SubscribeRequest, WorkareaId as ProtoWorkareaId,
    WorkspaceId as ProtoWorkspaceId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;

use crate::core_client::{default_socket_path, get_or_connect, reset_channel, CoreClientError};

/// Process-wide registry mapping `SubscriptionId` → forwarder task
/// handle. The registry is wrapped in a sync `Mutex` because the only
/// operations are insert-on-subscribe and remove-on-unsubscribe — no
/// awaits while holding the guard.
#[derive(Default)]
pub struct SubscriptionRegistry {
    inner: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl SubscriptionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, id: String, handle: JoinHandle<()>) {
        let mut map = self.inner.lock().expect("subscription registry poisoned");
        map.insert(id, handle);
    }

    fn remove(&self, id: &str) -> Option<JoinHandle<()>> {
        let mut map = self.inner.lock().expect("subscription registry poisoned");
        map.remove(id)
    }
}

/// Renderer → shell smoke ping. Returns `"pong"`.
#[tauri::command]
pub async fn concerto_ping() -> Result<String, CoreClientError> {
    Ok("pong".to_string())
}

/// Renderer → shell gRPC dispatch. `method` is `"<Service>.<Rpc>"`;
/// `payload` is the request body as JSON. Returns the response body
/// as JSON.
#[tauri::command]
pub async fn concerto_rpc(method: String, payload: Value) -> Result<Value, CoreClientError> {
    let socket_path = default_socket_path().ok_or_else(|| {
        CoreClientError::Transport("HOME not set — cannot resolve ~/.concerto/core.sock".into())
    })?;
    dispatch(socket_path, &method, payload).await
}

/// Renderer → shell server-streaming bridge. Opens
/// `Streams.Subscribe(subject)` and emits each frame to the renderer
/// as a Tauri event named `"concerto/<subject>"`. Returns a stable
/// subscription id the renderer hands to [`concerto_unsubscribe`].
#[tauri::command]
pub async fn concerto_subscribe(
    app: AppHandle,
    registry: State<'_, SubscriptionRegistry>,
    subject: String,
    filter: Option<String>,
) -> Result<String, CoreClientError> {
    let socket_path = default_socket_path().ok_or_else(|| {
        CoreClientError::Transport("HOME not set — cannot resolve ~/.concerto/core.sock".into())
    })?;
    let channel = match get_or_connect(&socket_path).await {
        Ok(ch) => ch,
        Err(e) => {
            reset_channel();
            return Err(e);
        }
    };
    let mut client = StreamsClient::new(channel);
    let stream = client
        .subscribe(SubscribeRequest {
            subject: subject.clone(),
            filter,
            since_offset: None,
        })
        .await
        .map_err(|s| {
            reset_channel();
            CoreClientError::Rpc(format!("{}: {}", s.code(), s.message()))
        })?
        .into_inner();

    // Stable per-subscription id — uuid-shaped string is overkill;
    // a monotonic counter is enough for V0.1 since the renderer is
    // the only producer of ids. Use the subject + a millisecond
    // timestamp so duplicates are unlikely under hand testing.
    let id = format!(
        "{}-{}",
        subject,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default()
    );

    let event_name = format!("concerto/{subject}");
    let id_for_task = id.clone();
    let join = tokio::spawn(async move {
        tokio::pin!(stream);
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    // Tauri emit accepts any serde-serializable
                    // payload; the generated proto types derive
                    // serde so we can hand the `Event` straight to
                    // the bus. Renderer-side typing lives in
                    // `apps/desktop/src/api/`.
                    if let Err(e) = app.emit(&event_name, &event) {
                        tracing::warn!(
                            subscription = %id_for_task,
                            error = %e,
                            "failed to emit subscription event; dropping stream"
                        );
                        break;
                    }
                }
                Err(status) => {
                    tracing::warn!(
                        subscription = %id_for_task,
                        code = ?status.code(),
                        message = status.message(),
                        "stream item error; ending subscription"
                    );
                    break;
                }
            }
        }
    });

    registry.insert(id.clone(), join);
    Ok(id)
}

/// Renderer → shell — drop a previously-opened subscription.
/// Idempotent: dropping an unknown id is a no-op (the JoinHandle
/// has already been removed and aborted).
#[tauri::command]
pub async fn concerto_unsubscribe(
    registry: State<'_, SubscriptionRegistry>,
    id: String,
) -> Result<(), CoreClientError> {
    if let Some(handle) = registry.remove(&id) {
        handle.abort();
    }
    Ok(())
}

/// Optional payload wrapper for `Workspaces.ListWorkspaces`.
#[derive(Debug, Deserialize)]
struct ListWorkspacesPayload {
    project_id: String,
}

/// Payload wrapper for any RPC whose request is `{"id": "..."}`.
#[derive(Debug, Deserialize)]
struct IdPayload {
    id: String,
}

/// Payload wrapper for `Sessions.ListSessions`.
#[derive(Debug, Deserialize)]
struct ListSessionsPayload {
    #[allow(dead_code)]
    workarea_id: String,
}

/// Method-dispatch core, factored out of the Tauri command so it can
/// be unit-tested against a tempdir socket without standing up the
/// Tauri runtime.
pub(crate) async fn dispatch(
    socket_path: PathBuf,
    method: &str,
    payload: Value,
) -> Result<Value, CoreClientError> {
    let channel = match get_or_connect(&socket_path).await {
        Ok(ch) => ch,
        Err(e) => {
            reset_channel();
            return Err(e);
        }
    };

    // Macro pattern: every RPC arm follows the same shape (build the
    // typed client, await the call, serialize the response). On any
    // failure we reset the cached channel so the next call dials
    // fresh.
    let result = match method {
        "Runtime.GetServerCapabilities" => {
            let mut client = RuntimeClient::new(channel);
            let resp = client.get_server_capabilities(()).await;
            resp.map(|r| {
                let caps = r.into_inner();
                json!({
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
                })
            })
        }
        "Projects.ListProjects" => {
            let mut client = ProjectsClient::new(channel);
            client
                .list_projects(ListProjectsRequest {})
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Workspaces.ListWorkspaces" => {
            let req: ListWorkspacesPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListWorkspaces: {e}"))
            })?;
            let mut client = WorkspacesClient::new(channel);
            client
                .list_workspaces(ListWorkspacesRequest {
                    project_id: req.project_id,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Workspaces.GetWorkspace" => {
            let req: IdPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for GetWorkspace: {e}"))
            })?;
            let mut client = WorkspacesClient::new(channel);
            client
                .get_workspace(ProtoWorkspaceId { value: req.id })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Workareas.GetWorkarea" => {
            let req: IdPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for GetWorkarea: {e}"))
            })?;
            let mut client = WorkareasClient::new(channel);
            client
                .get_workarea(ProtoWorkareaId { value: req.id })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Sessions.ListSessions" => {
            // V0.1 stub per the task spec: the sidebar doesn't render
            // sessions yet (Task 26 wires the terminal). Returning an
            // empty list keeps the renderer's React Query happy and
            // the Tauri command surface honest.
            let _: ListSessionsPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListSessions: {e}"))
            })?;
            // Use a manual Ok branch with the Sessions client unused
            // — we deliberately do NOT call the Core; the stub is
            // local-only. The `SessionsClient` import remains to keep
            // the dispatcher shape uniform for the Task 26 follow-on.
            let _ = SessionsClient::<tonic::transport::Channel>::new;
            Ok(json!({ "sessions": [] }))
        }
        other => return Err(CoreClientError::NotImplemented(other.to_string())),
    };

    match result {
        Ok(value) => Ok(value),
        Err(status) => {
            reset_channel();
            Err(CoreClientError::Rpc(format!(
                "{}: {}",
                status.code(),
                status.message()
            )))
        }
    }
}

/// Convenience for the Tauri builder: register the
/// [`SubscriptionRegistry`] as managed state.
pub fn manage_subscriptions<R: tauri::Runtime, M: Manager<R>>(manager: &M) {
    manager.manage(SubscriptionRegistry::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_method_returns_not_implemented() {
        // Use a tempdir so connect fails fast (no real socket).
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("nope.sock");
        let err = dispatch(sock, "Bogus.Method", json!({}))
            .await
            .expect_err("should fail");
        // We connect lazily; an unknown method may surface as
        // Transport (cell empty + dial fails) OR NotImplemented (cell
        // already populated by a prior test). Accept either to keep
        // the test independent of run order.
        match err {
            CoreClientError::NotImplemented(_) | CoreClientError::Transport(_) => {}
            other => panic!("expected NotImplemented or Transport, got {other:?}"),
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
