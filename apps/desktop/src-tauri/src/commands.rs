//! Tauri command surface — the *only* IPC entry points the renderer
//! sees.
//!
//! Task 218 refactors the dispatch path off the hard-wired UDS channel onto the
//! transport-agnostic [`crate::transport::CoreClient`] trait, resolved from the
//! connected-Core registry ([`crate::cores_registry::CoresRegistry`]):
//!
//! - [`concerto_ping`] — smoke probe; round-trips a static string.
//! - [`concerto_rpc`] — single dispatch entry for unary RPCs, routed through the
//!   **active** Core's `CoreClient`. Method names follow `"<Service>.<Rpc>"`.
//! - [`concerto_subscribe`] / [`concerto_unsubscribe`] — server-stream bridge,
//!   also routed through the active `CoreClient`. Each frame is emitted on the
//!   Tauri event bus under `"concerto/<subject>"` (dots → slashes).
//! - [`list_paired_cores`] / [`get_active_core`] — the registry read-commands
//!   the renderer's `src/api/cores.ts` binding (Task 218) + Task 219's pairing
//!   UI consume. Mutating pairing commands are Task 219/207/209.
//!
//! The renderer is forbidden from speaking gRPC directly (see
//! `apps/desktop/src-tauri/capabilities/main.json` — no `http`, no `shell`, no
//! `fs`). All Core traffic flows through these commands.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;

use concerto_proto::v1::repositories_client::RepositoriesClient;
use concerto_proto::v1::CloneRequest;
use serde::Deserialize;
use serde_json::json;

use crate::core_client::{default_socket_path, get_or_connect, reset_channel, CoreClientError};
use crate::cores_registry::{CoresRegistry, PairedCore, TransportKind};
use crate::transport::{CoreClient, StreamSink, UdsCoreClient};

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

/// Resolve the **active** Core as a boxed [`CoreClient`].
///
/// Co-located path (`design/15 §3.10.2`): if the registry has an active UDS
/// Core, dial its socket; otherwise promote the default `~/.concerto/core.sock`
/// as the implicit "This machine" Core (step 2) and use that. The split-host
/// Iroh client is built by the connect/switch flow (Task 219/601) and is unit-
/// proven by the Tier-2 loopback test; the live command path here resolves the
/// co-located UDS Core, preserving the smoke happy-path.
fn resolve_active_client(registry: &CoresRegistry) -> Result<Box<dyn CoreClient>, CoreClientError> {
    if let Some(active) = registry.active() {
        match active.transport {
            TransportKind::Uds => {
                let path = active.uds_socket_path.ok_or_else(|| {
                    CoreClientError::Transport(
                        "active UDS Core has no socket path in registry".into(),
                    )
                })?;
                return Ok(Box::new(UdsCoreClient::new(path)));
            }
            TransportKind::Iroh => {
                // Live Iroh dial from the active registry row is the Task
                // 219/601 connect flow (it must build the client Endpoint +
                // resolve the cert from the keychain); it is unit-proven by the
                // Tier-2 loopback test. Until that flow lands, the command path
                // does not dial Iroh from here.
                return Err(CoreClientError::NotImplemented(
                    "split-host Iroh dispatch is wired by the connect flow (Task 219/601)".into(),
                ));
            }
        }
    }
    // No active Core recorded: promote the default co-located UDS socket
    // (`design/15 §3.10.2` step 2) and dial it.
    let socket_path = default_socket_path().ok_or_else(|| {
        CoreClientError::Transport("HOME not set — cannot resolve ~/.concerto/core.sock".into())
    })?;
    let _ = registry.promote_local_uds(socket_path.clone());
    Ok(Box::new(UdsCoreClient::new(socket_path)))
}

/// Renderer → shell smoke ping. Returns `"pong"`.
#[tauri::command]
pub async fn concerto_ping() -> Result<String, CoreClientError> {
    Ok("pong".to_string())
}

/// Renderer → shell gRPC dispatch through the active Core's `CoreClient`.
/// `method` is `"<Service>.<Rpc>"`; `payload` is the request body as JSON.
#[tauri::command]
pub async fn concerto_rpc(
    registry: State<'_, CoresRegistry>,
    method: String,
    payload: Value,
) -> Result<Value, CoreClientError> {
    let client = resolve_active_client(&registry)?;
    client.dispatch(&method, payload).await
}

/// Renderer → shell server-streaming bridge through the active Core's
/// `CoreClient`. Opens `Streams.Subscribe(subject)` and emits each frame to the
/// renderer as a Tauri event named `"concerto/<subject>"`. Returns a stable
/// subscription id the renderer hands to [`concerto_unsubscribe`].
#[tauri::command]
pub async fn concerto_subscribe(
    app: AppHandle,
    registry: State<'_, CoresRegistry>,
    subscriptions: State<'_, SubscriptionRegistry>,
    subject: String,
    filter: Option<String>,
) -> Result<String, CoreClientError> {
    let client = resolve_active_client(&registry)?;

    // Tauri 2 rejects event names containing '.', so the gRPC subject (e.g.
    // `session.io.<sid>`) maps to a slash-delimited Tauri event name
    // (`concerto/session/io/<sid>`). MUST stay in sync with the renderer in
    // `apps/desktop/src/api/client.ts` (`eventNameForSubject`).
    let event_name = format!("concerto/{}", subject.replace('.', "/"));
    let sink = StreamSink::new(move |frame: &Value| match app.emit(&event_name, frame) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "failed to emit subscription event; dropping stream");
            false
        }
    });

    let filter_value = filter.map(Value::String).unwrap_or(Value::Null);
    let sub = client.start_stream(&subject, filter_value, sink).await?;
    subscriptions.insert(sub.id.clone(), sub.join);
    Ok(sub.id)
}

/// Renderer → shell — drop a previously-opened subscription. Idempotent.
#[tauri::command]
pub async fn concerto_unsubscribe(
    subscriptions: State<'_, SubscriptionRegistry>,
    id: String,
) -> Result<(), CoreClientError> {
    if let Some(handle) = subscriptions.remove(&id) {
        handle.abort();
    }
    Ok(())
}

/// The renderer-facing view of a paired Core (`design/15 §3.10.1`). Mirrors the
/// cleartext [`PairedCore`] metadata the `src/api/cores.ts` binding (Task 218)
/// reads. `transport_kind` is the renderer's `UDS | IROH` string (lower-cased
/// by serde) the Connect-to-Core picker (Task 219/601) branches on; it agrees
/// with `ServerCapabilities.transport_kind` (Task 201). Secrets are never here.
#[derive(Debug, Clone, Serialize)]
pub struct PairedCoreView {
    pub core_id: String,
    pub display_name: String,
    pub transport_kind: TransportKind,
    pub iroh_endpoint_id: Option<String>,
    pub last_connected_at: Option<u64>,
    /// Whether this Core is the currently active one.
    pub is_active: bool,
}

fn to_view(core: &PairedCore, active_id: Option<&str>) -> PairedCoreView {
    PairedCoreView {
        core_id: core.core_id.clone(),
        display_name: core.display_name.clone(),
        transport_kind: core.transport,
        iroh_endpoint_id: core.iroh_endpoint_id.clone(),
        last_connected_at: core.last_connected_at,
        is_active: active_id == Some(core.core_id.as_str()),
    }
}

/// Renderer → shell — list every paired Core (cleartext metadata). The read
/// command `src/api/cores.ts` (Task 218) + the Connect-to-Core picker (Task
/// 219/601) build on. No secrets.
#[tauri::command]
pub async fn list_paired_cores(
    registry: State<'_, CoresRegistry>,
) -> Result<Vec<PairedCoreView>, CoreClientError> {
    let active = registry.active_core_id();
    let views = registry
        .list()
        .iter()
        .map(|c| to_view(c, active.as_deref()))
        .collect();
    Ok(views)
}

/// Renderer → shell — the active Core (or `null`). Carries its `transport_kind`
/// so the renderer can branch remote-mode affordances (Task 602) without
/// learning the transport mechanics.
#[tauri::command]
pub async fn get_active_core(
    registry: State<'_, CoresRegistry>,
) -> Result<Option<PairedCoreView>, CoreClientError> {
    let active = registry.active_core_id();
    Ok(registry.active().map(|c| to_view(&c, active.as_deref())))
}

/// Renderer → shell — set the active Core by id. The UI-only active-Core
/// selection lives in Zustand; this persists the server-canonical pointer so
/// the choice survives a relaunch. (The full disconnect/reconnect switch UX is
/// Task 601; this is the registry-write seam it calls.)
#[tauri::command]
pub async fn set_active_core(
    registry: State<'_, CoresRegistry>,
    core_id: String,
) -> Result<(), CoreClientError> {
    registry.set_active(&core_id)
}

/// Renderer → shell — opens the server-streaming
/// `Repositories.Clone(repository_id)` RPC and forwards every `CloneProgress`
/// frame to the renderer as a Tauri event named
/// `"concerto/clone-progress/<repository_id>"`. Returns once the stream
/// completes. Clone is a one-shot typed server-stream (its own RPC, no pub/sub
/// subject), so it keeps a dedicated UDS path rather than routing through the
/// generic `CoreClient::start_stream` subject bridge.
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
    // `Clone::clone` under normal method resolution.
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

/// Renderer → shell — probe `PATH` for an executable. Returns the resolved
/// absolute path or `null` when the binary is not on `PATH`.
#[tauri::command]
pub async fn check_command(name: String) -> Result<Option<String>, CoreClientError> {
    if name.is_empty() {
        return Err(CoreClientError::Rpc("name is required".into()));
    }
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

/// Payload wrapper for the [`clone_repository`] streaming command.
#[derive(Debug, Deserialize)]
struct CloneRepositoryPayload {
    repository_id: String,
}

/// Convenience for the Tauri builder: register the [`SubscriptionRegistry`] as
/// managed state.
pub fn manage_subscriptions<R: tauri::Runtime, M: Manager<R>>(manager: &M) {
    manager.manage(SubscriptionRegistry::new());
}

/// Convenience for the Tauri builder: open + register the connected-Core
/// registry as managed state, rooted at `config_dir`. A failure to open the
/// registry is non-fatal — an empty in-memory registry is used so the app still
/// boots (the co-located path promotes the default socket on first dispatch).
pub fn manage_cores_registry<R: tauri::Runtime, M: Manager<R>>(
    manager: &M,
    config_dir: std::path::PathBuf,
) {
    let registry = CoresRegistry::open(config_dir.clone()).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to open cores.json; starting with an empty registry");
        CoresRegistry::open(config_dir).unwrap_or_else(|_| {
            // Fall back to a throwaway temp path so state managers always have a
            // registry. The default-socket promotion still works.
            CoresRegistry::open(std::env::temp_dir()).expect("temp dir cores registry should open")
        })
    });
    manager.manage(registry);
}

#[cfg(test)]
mod tests {
    use crate::cores_registry::CoresRegistry;
    use crate::transport::{CoreClient, UdsCoreClient};
    use serde_json::json;

    #[tokio::test]
    async fn uds_client_unknown_method_or_transport() {
        // A UDS client pointed at a missing socket surfaces Transport (dial
        // fails) or NotImplemented (cell already populated by another test).
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("nope.sock");
        let client = UdsCoreClient::new(sock);
        let err = client
            .dispatch("Bogus.Method", json!({}))
            .await
            .expect_err("should fail");
        match err {
            crate::core_client::CoreClientError::NotImplemented(_)
            | crate::core_client::CoreClientError::Transport(_) => {}
            other => panic!("expected NotImplemented or Transport, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn uds_known_method_missing_socket_is_transport() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sock = tmp.path().join("nope.sock");
        let client = UdsCoreClient::new(sock);
        let err = client
            .dispatch("Runtime.GetServerCapabilities", json!({}))
            .await
            .expect_err("should fail without a running Core");
        assert!(matches!(
            err,
            crate::core_client::CoreClientError::Transport(_)
        ));
    }

    #[test]
    fn registry_open_is_reusable_for_state() {
        // The state-manager path opens a registry rooted at a config dir.
        let tmp = tempfile::TempDir::new().unwrap();
        let reg = CoresRegistry::open(tmp.path().to_path_buf()).unwrap();
        assert!(reg.list().is_empty());
    }
}
