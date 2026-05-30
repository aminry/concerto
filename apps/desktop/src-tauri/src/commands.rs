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
use concerto_proto::v1::repositories_client::RepositoriesClient;
use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::schedules_client::SchedulesClient;
use concerto_proto::v1::sessions_client::SessionsClient;
use concerto_proto::v1::skills_client::SkillsClient;
use concerto_proto::v1::streams_client::StreamsClient;
use concerto_proto::v1::workareas_client::WorkareasClient;
use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{
    AddRepoRequest, CloneRequest, CreateSessionRequest, CreateWorkareaRequest,
    CreateWorkspaceRequest, GetDiffRequest, ListProjectsRequest, ListRepositoriesRequest,
    ListSchedulesRequest, ListSessionsRequest, ListSkillsRequest, ListWorkareasRequest,
    ListWorkspacesRequest, McpScopeRequest, PermissionMode, SendMessageRequest,
    SessionId as ProtoSessionId, StopSessionRequest, SubscribeRequest,
    WorkareaId as ProtoWorkareaId, WorkspaceId as ProtoWorkspaceId,
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
    // Retry transport failures. The Core daemon is a separate process that
    // may be mid-restart (new socket inode) when a call lands — or the
    // cached channel may have gone stale across a restart. `dispatch`
    // resets the channel on error, so a short wait + re-dial recovers
    // transparently instead of surfacing a spurious "Transport" error to
    // the user. Only Transport (dial) failures retry; real RPC errors
    // (NotFound, InvalidArgument, …) are returned immediately.
    let mut result = dispatch(socket_path.clone(), &method, payload.clone()).await;
    let mut attempts = 0;
    while attempts < 3 && matches!(result, Err(CoreClientError::Transport(_))) {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        result = dispatch(socket_path.clone(), &method, payload.clone()).await;
        attempts += 1;
    }
    result
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
    // Retry transport/dial failures, mirroring `concerto_rpc`. The Core is
    // a separate process; after it restarts the cached channel is stale,
    // and the FIRST subscribe lands on the dead connection. Without a retry
    // the renderer's subscription hook just logs and never re-runs (its
    // effect is keyed on the session id), leaving a permanently blank
    // terminal. Re-dial + retry so a Core restart is transparent.
    let mut attempts = 0;
    let stream = loop {
        let channel = match get_or_connect(&socket_path).await {
            Ok(ch) => ch,
            Err(e) => {
                reset_channel();
                if attempts >= 3 {
                    return Err(e);
                }
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                continue;
            }
        };
        match StreamsClient::new(channel)
            .subscribe(SubscribeRequest {
                subject: subject.clone(),
                filter: filter.clone(),
                since_offset: None,
            })
            .await
        {
            Ok(s) => break s.into_inner(),
            Err(status) => {
                reset_channel();
                if attempts >= 3 {
                    return Err(CoreClientError::Rpc(format!(
                        "{}: {}",
                        status.code(),
                        status.message()
                    )));
                }
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
        }
    };

    // Per-subscription id. A process-wide atomic counter guarantees
    // uniqueness — a millisecond timestamp collides when React StrictMode
    // double-mounts a hook in the same instant, which silently overwrote
    // (and leaked) the first forwarder task, double-rendering every byte.
    static SUB_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = format!(
        "{subject}-{}",
        SUB_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

/// Renderer → shell — opens the server-streaming
/// `Repositories.Clone(repository_id)` RPC and forwards every
/// `CloneProgress` frame to the renderer as a Tauri event named
/// `"concerto/clone-progress/<repository_id>"`. Returns once the
/// stream completes (either cleanly or with an error). Unlike the
/// generic [`concerto_subscribe`] bridge, this command does not
/// register a subscription id; the Tauri await contract is sufficient
/// because clone is a one-shot operation.
///
/// Why a dedicated command instead of routing through
/// `concerto_subscribe`? `Streams.Subscribe` carries `Event` frames
/// keyed by subject name, but `Repositories.Clone` is a typed
/// server-stream of `CloneProgress` — its own RPC, no pub/sub subject.
/// Mirroring it as a one-shot Tauri command keeps the wire shape
/// honest.
#[tauri::command]
pub async fn clone_repository(app: AppHandle, payload: Value) -> Result<Value, CoreClientError> {
    let req: CloneRepositoryPayload = serde_json::from_value(payload)
        .map_err(|e| CoreClientError::Rpc(format!("invalid payload for clone_repository: {e}")))?;
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
    let mut client = RepositoriesClient::new(channel);
    // UFCS: the generated `RepositoriesClient::clone` method shadows
    // `Clone::clone` under normal method resolution. Spelling it out
    // through the type path picks the inherent gRPC method we want.
    let stream = RepositoriesClient::<tonic::transport::Channel>::clone(
        &mut client,
        CloneRequest {
            repository_id: req.repository_id.clone(),
        },
    )
    .await
    .map_err(|s| {
        reset_channel();
        CoreClientError::Rpc(format!("{}: {}", s.code(), s.message()))
    })?
    .into_inner();

    let event_name = format!("concerto/clone-progress/{}", req.repository_id);
    tokio::pin!(stream);
    let mut last_done = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(progress) => {
                last_done = progress.done;
                if let Err(e) = app.emit(&event_name, &progress) {
                    tracing::warn!(
                        repository_id = %req.repository_id,
                        error = %e,
                        "failed to emit clone-progress event; dropping stream"
                    );
                    break;
                }
            }
            Err(status) => {
                reset_channel();
                return Err(CoreClientError::Rpc(format!(
                    "{}: {}",
                    status.code(),
                    status.message()
                )));
            }
        }
    }
    Ok(json!({ "done": last_done }))
}

/// Renderer → shell — probe `PATH` for an executable. Returns the
/// resolved absolute path or `null` when the binary is not on `PATH`.
/// V0.1 ships macOS-only desktop, so `which` is sufficient; Windows
/// would route through `where` in V1.0.
#[tauri::command]
pub async fn check_command(name: String) -> Result<Option<String>, CoreClientError> {
    if name.is_empty() {
        return Err(CoreClientError::Rpc("name is required".into()));
    }
    // Guard against shell-meta in the name — `which` doesn't interpret
    // them, but rejecting up front keeps the surface predictable.
    if name.contains(['/', '\\', '\n', '\0']) {
        return Err(CoreClientError::Rpc(
            "name must be a bare executable, not a path".into(),
        ));
    }
    let output = tokio::process::Command::new("which")
        .arg(&name)
        .output()
        .await
        .map_err(|e| CoreClientError::Transport(format!("which {name}: {e}")))?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            Ok(None)
        } else {
            Ok(Some(path))
        }
    } else {
        Ok(None)
    }
}

/// Normalise a `PermissionMode` ordinal carried in a payload. Any
/// numeric value that doesn't match a known variant is dropped to
/// `None` so the Core's `permission_mode is required` validation
/// surfaces cleanly. The `Unspecified` ordinal is treated as "no
/// override".
fn normalize_permission_mode(v: i32) -> Option<i32> {
    match PermissionMode::try_from(v) {
        Ok(PermissionMode::Unspecified) => None,
        Ok(_) => Some(v),
        Err(_) => None,
    }
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
    workarea_id: String,
}

/// Payload wrapper for `Sessions.CreateSession`. Mirrors the proto's
/// `CreateSessionRequest`; `permission_mode` carries the proto enum
/// ordinal as an integer.
#[derive(Debug, Deserialize)]
struct CreateSessionPayload {
    workarea_id: String,
    agent_kind: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    permission_mode: Option<i32>,
}

/// Payload wrapper for `Sessions.SendMessage`. `payload` is the raw
/// bytes that get forwarded to the agent's stdin. Renderer-side code
/// serializes as a JSON array of u8 — serde's default for `Vec<u8>`.
#[derive(Debug, Deserialize)]
struct SendMessagePayload {
    session_id: String,
    payload: Vec<u8>,
}

/// Payload wrapper for `Sessions.StopSession`.
#[derive(Debug, Deserialize)]
struct StopSessionPayload {
    session_id: String,
    #[serde(default = "default_stop_reason")]
    reason: String,
}

fn default_stop_reason() -> String {
    "user_request".to_string()
}

/// Payload wrapper for `Workspaces.CreateWorkspace`. Mirrors
/// `concerto.v1.CreateWorkspaceRequest`; `permission_mode` carries the
/// proto enum ordinal as an integer (matches `PermissionMode`).
#[derive(Debug, Deserialize)]
struct CreateWorkspacePayload {
    name: String,
    project_id: String,
    repository_ids: Vec<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    permission_mode: Option<i32>,
}

/// Payload wrapper for `Workareas.CreateWorkarea`. V0.1 takes no extra
/// inputs beyond the parent workspace; `permission_mode` defaults to
/// whatever the workspace inherits.
#[derive(Debug, Deserialize)]
struct CreateWorkareaPayload {
    workspace_id: String,
    #[serde(default)]
    permission_mode: Option<i32>,
}

/// Payload wrapper for `Workareas.ListWorkareas`.
#[derive(Debug, Deserialize)]
struct ListWorkareasPayload {
    workspace_id: String,
    #[serde(default)]
    include_archived: bool,
}

/// Payload wrapper for `Workareas.GetWorkareaRepoDiff` (Task 29 RPC,
/// Task 47 renderer surface). Identifies the `(workarea, repository)`
/// pair; the Core resolves the matching per-repo worktree and returns
/// a structured `DiffPayload`.
#[derive(Debug, Deserialize)]
struct GetWorkareaRepoDiffPayload {
    workarea_id: String,
    repository_id: String,
}

/// Payload wrapper for `Repositories.AddRepository`.
#[derive(Debug, Deserialize)]
struct AddRepositoryPayload {
    project_id: String,
    name: String,
    url: String,
    #[serde(default)]
    default_branch: String,
}

/// Payload wrapper for `Repositories.ListByProject`.
#[derive(Debug, Deserialize)]
struct ListRepositoriesPayload {
    project_id: String,
}

/// Payload wrapper for the [`clone_repository`] streaming command.
#[derive(Debug, Deserialize)]
struct CloneRepositoryPayload {
    repository_id: String,
}

/// Payload wrapper for `Schedules.ListSchedules`. Task 46 wires the
/// right-rail Scheduler tab; the request mirrors the proto's
/// `ListSchedulesRequest` shape.
#[derive(Debug, Deserialize)]
struct ListSchedulesPayload {
    workarea_id: String,
}

/// Payload wrapper for `Skills.ListSkills`. All three filter fields are
/// optional — the right-rail Skills tab passes `project_id` only.
#[derive(Debug, Deserialize, Default)]
struct ListSkillsPayload {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    enabled_only: Option<bool>,
}

/// Payload wrapper for `Sessions.ListMcpServers`. The right-rail MCP
/// tab passes `repository_id` when scoping to project; both fields are
/// optional per the proto.
#[derive(Debug, Deserialize, Default)]
struct ListMcpServersPayload {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    repository_id: Option<String>,
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
        "Workareas.ListWorkareas" => {
            let req: ListWorkareasPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListWorkareas: {e}"))
            })?;
            let mut client = WorkareasClient::new(channel);
            client
                .list_workareas(ListWorkareasRequest {
                    workspace_id: req.workspace_id,
                    include_archived: req.include_archived,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Workareas.CreateWorkarea" => {
            let req: CreateWorkareaPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for CreateWorkarea: {e}"))
            })?;
            let mut client = WorkareasClient::new(channel);
            client
                .create_workarea(CreateWorkareaRequest {
                    workspace_id: req.workspace_id,
                    permission_mode: req.permission_mode.and_then(normalize_permission_mode),
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Workareas.GetWorkareaRepoDiff" => {
            let req: GetWorkareaRepoDiffPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for GetWorkareaRepoDiff: {e}"))
            })?;
            let mut client = WorkareasClient::new(channel);
            client
                .get_workarea_repo_diff(GetDiffRequest {
                    workarea_id: req.workarea_id,
                    repository_id: req.repository_id,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Workspaces.CreateWorkspace" => {
            let req: CreateWorkspacePayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for CreateWorkspace: {e}"))
            })?;
            let mut client = WorkspacesClient::new(channel);
            client
                .create_workspace(CreateWorkspaceRequest {
                    project_id: req.project_id,
                    name: req.name,
                    repository_ids: req.repository_ids,
                    permission_mode: req.permission_mode.and_then(normalize_permission_mode),
                    description: req.description,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Repositories.AddRepository" => {
            let req: AddRepositoryPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for AddRepository: {e}"))
            })?;
            let mut client = RepositoriesClient::new(channel);
            client
                .add_repository(AddRepoRequest {
                    project_id: req.project_id,
                    name: req.name,
                    url: req.url,
                    default_branch: req.default_branch,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Repositories.ListByProject" => {
            let req: ListRepositoriesPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListByProject: {e}"))
            })?;
            let mut client = RepositoriesClient::new(channel);
            client
                .list_by_project(ListRepositoriesRequest {
                    project_id: req.project_id,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Sessions.ListSessions" => {
            let req: ListSessionsPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListSessions: {e}"))
            })?;
            let mut client = SessionsClient::new(channel);
            client
                .list_sessions(ListSessionsRequest {
                    workarea_id: req.workarea_id,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Sessions.CreateSession" => {
            let req: CreateSessionPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for CreateSession: {e}"))
            })?;
            let mut client = SessionsClient::new(channel);
            client
                .create_session(CreateSessionRequest {
                    workarea_id: req.workarea_id,
                    agent_kind: req.agent_kind,
                    model: req.model,
                    permission_mode: req.permission_mode.and_then(normalize_permission_mode),
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Sessions.GetSession" => {
            let req: IdPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for GetSession: {e}"))
            })?;
            let mut client = SessionsClient::new(channel);
            client
                .get_session(ProtoSessionId { value: req.id })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Sessions.SendMessage" => {
            let req: SendMessagePayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for SendMessage: {e}"))
            })?;
            let mut client = SessionsClient::new(channel);
            client
                .send_message(SendMessageRequest {
                    session_id: req.session_id,
                    payload: req.payload,
                })
                .await
                .map(|_| Value::Null)
        }
        "Sessions.StopSession" => {
            let req: StopSessionPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for StopSession: {e}"))
            })?;
            let mut client = SessionsClient::new(channel);
            client
                .stop_session(StopSessionRequest {
                    session_id: req.session_id,
                    reason: req.reason,
                })
                .await
                .map(|_| Value::Null)
        }
        "Sessions.DeleteSession" => {
            let req: IdPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for DeleteSession: {e}"))
            })?;
            let mut client = SessionsClient::new(channel);
            client
                .delete_session(ProtoSessionId { value: req.id })
                .await
                .map(|_| Value::Null)
        }
        "Schedules.ListSchedules" => {
            let req: ListSchedulesPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListSchedules: {e}"))
            })?;
            let mut client = SchedulesClient::new(channel);
            client
                .list_schedules(ListSchedulesRequest {
                    workarea_id: req.workarea_id,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Skills.ListSkills" => {
            let req: ListSkillsPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListSkills: {e}"))
            })?;
            let mut client = SkillsClient::new(channel);
            client
                .list_skills(ListSkillsRequest {
                    scope: req.scope,
                    project_id: req.project_id,
                    enabled_only: req.enabled_only,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Sessions.ListMcpServers" => {
            let req: ListMcpServersPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListMcpServers: {e}"))
            })?;
            let mut client = SessionsClient::new(channel);
            client
                .list_mcp_servers(McpScopeRequest {
                    scope: req.scope,
                    repository_id: req.repository_id,
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
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
