//! Per-RPC dispatch + subscribe logic over a resolved `tonic::transport::Channel`.
//!
//! Extracted from `commands.rs` so **both** transport impls
//! ([`crate::transport::UdsCoreClient`] and the feature-gated
//! `IrohCoreClient`) route the identical Tonic service calls through one place.
//! The transport-specific work (resolving the channel, presenting peer-UID vs
//! device-cert auth) happens in the impls; the method→service mapping lives
//! here.
//!
//! The dot→slash subject mapping for the Tauri event bus is locked in lockstep
//! with the renderer (`apps/desktop/src/api/client.ts`'s `eventNameForSubject`).

use std::sync::atomic::{AtomicU64, Ordering};

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
    AddRepoRequest, CreateProjectRequest, CreateSessionRequest, CreateWorkareaRequest,
    CreateWorkspaceRequest, GetDiffRequest, ListProjectsRequest, ListRepositoriesRequest,
    ListSchedulesRequest, ListSessionsRequest, ListSkillsRequest, ListWorkareasRequest,
    ListWorkspacesRequest, McpScopeRequest, PermissionMode, ResizeSessionRequest,
    SendMessageRequest, SessionId as ProtoSessionId, StopSessionRequest, SubscribeRequest,
    WorkareaId as ProtoWorkareaId, WorkspaceId as ProtoWorkspaceId,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::StreamExt;
use tonic::body::BoxBody;
use tonic::client::GrpcService;
use tonic::codegen::{Body, Bytes, StdError};

use crate::core_client::CoreClientError;
use crate::transport::{StreamSink, StreamSubscription};

/// Normalise a `PermissionMode` ordinal carried in a payload (`Unspecified`
/// ordinal → no override; unknown → dropped to `None`).
fn normalize_permission_mode(v: i32) -> Option<i32> {
    match PermissionMode::try_from(v) {
        Ok(PermissionMode::Unspecified) => None,
        Ok(_) => Some(v),
        Err(_) => None,
    }
}

#[derive(Debug, Deserialize)]
struct ListWorkspacesPayload {
    project_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateProjectPayload {
    name: String,
    #[serde(default)]
    icon: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdPayload {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ListSessionsPayload {
    workarea_id: String,
}

#[derive(Debug, Deserialize)]
struct CreateSessionPayload {
    workarea_id: String,
    agent_kind: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    permission_mode: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct SendMessagePayload {
    session_id: String,
    payload: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct StopSessionPayload {
    session_id: String,
    #[serde(default = "default_stop_reason")]
    reason: String,
}

fn default_stop_reason() -> String {
    "user_request".to_string()
}

#[derive(Debug, Deserialize)]
struct ResizeSessionPayload {
    session_id: String,
    rows: u32,
    cols: u32,
}

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

#[derive(Debug, Deserialize)]
struct CreateWorkareaPayload {
    workspace_id: String,
    #[serde(default)]
    permission_mode: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct ListWorkareasPayload {
    workspace_id: String,
    #[serde(default)]
    include_archived: bool,
}

#[derive(Debug, Deserialize)]
struct GetWorkareaRepoDiffPayload {
    workarea_id: String,
    repository_id: String,
}

#[derive(Debug, Deserialize)]
struct AddRepositoryPayload {
    project_id: String,
    name: String,
    url: String,
    #[serde(default)]
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct ListRepositoriesPayload {
    project_id: String,
}

#[derive(Debug, Deserialize)]
struct ListSchedulesPayload {
    workarea_id: String,
}

#[derive(Debug, Deserialize, Default)]
struct ListSkillsPayload {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    enabled_only: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
struct ListMcpServersPayload {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    repository_id: Option<String>,
}

/// Dispatch one unary `"<Service>.<Rpc>"` call over an already-resolved channel.
/// The transport impl owns resolving the channel + presenting auth (peer-UID for
/// UDS; device-cert metadata is baked into the Iroh channel); this maps the
/// method string onto the typed Tonic client call and serialises the response.
pub(crate) async fn dispatch_over_channel<T>(
    channel: T,
    method: &str,
    payload: Value,
) -> Result<Value, CoreClientError>
where
    T: GrpcService<BoxBody> + Send + 'static,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    T::Future: Send,
{
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
        "Projects.CreateProject" => {
            let req: CreateProjectPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for CreateProject: {e}"))
            })?;
            let mut client = ProjectsClient::new(channel);
            client
                .create_project(CreateProjectRequest {
                    name: req.name,
                    icon: req.icon,
                })
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
        "Sessions.ResizeSession" => {
            let req: ResizeSessionPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ResizeSession: {e}"))
            })?;
            let mut client = SessionsClient::new(channel);
            client
                .resize_session(ResizeSessionRequest {
                    session_id: req.session_id,
                    rows: req.rows,
                    cols: req.cols,
                })
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

    result
        .map_err(|status| CoreClientError::Rpc(format!("{}: {}", status.code(), status.message())))
}

/// Open `Streams.Subscribe(subject)` over the resolved channel and spawn a
/// forwarder that pushes each frame to `sink`. Returns the subscription id +
/// the forwarder `JoinHandle` for the registry to abort on unsubscribe.
pub(crate) async fn subscribe_over_channel<T>(
    channel: T,
    subject: &str,
    filter: Value,
    sink: StreamSink,
) -> Result<StreamSubscription, CoreClientError>
where
    T: GrpcService<BoxBody> + Send + 'static,
    T::Error: Into<StdError>,
    T::ResponseBody: Body<Data = Bytes> + Send + 'static,
    <T::ResponseBody as Body>::Error: Into<StdError> + Send,
    T::Future: Send,
{
    let filter_str = match filter {
        Value::Null => None,
        Value::String(s) => Some(s),
        other => Some(other.to_string()),
    };
    let stream = StreamsClient::new(channel)
        .subscribe(SubscribeRequest {
            subject: subject.to_string(),
            filter: filter_str,
            since_offset: None,
        })
        .await
        .map_err(|status| CoreClientError::Rpc(format!("{}: {}", status.code(), status.message())))?
        .into_inner();

    // Per-subscription id from a process-wide atomic counter (a millisecond
    // timestamp collides when React StrictMode double-mounts in the same
    // instant — see the original `commands.rs` note).
    static SUB_SEQ: AtomicU64 = AtomicU64::new(0);
    let id = format!("{subject}-{}", SUB_SEQ.fetch_add(1, Ordering::Relaxed));

    let id_for_task = id.clone();
    // The sink owns the bus-emit closure (and the dot→slash event-name mapping
    // built by the caller). Move it into the forwarder task.
    let join = tokio::spawn(async move {
        tokio::pin!(stream);
        while let Some(item) = stream.next().await {
            match item {
                Ok(event) => {
                    let frame = serde_json::to_value(&event).unwrap_or(Value::Null);
                    if !sink.emit(&frame) {
                        tracing::warn!(
                            subscription = %id_for_task,
                            "sink rejected event; dropping stream"
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

    Ok(StreamSubscription { id, join })
}
