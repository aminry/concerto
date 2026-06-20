//! `concerto-maestro-mcp` — the in-process `rmcp` stdio MCP server (Task 401,
//! design/08 §3.2 / PHASE4_PLANNING §4.1) and the net-new Core↔CLI MCP-stdio
//! transport.
//!
//! This is the **first MCP _server_ in the codebase**. It is greenfield: there
//! is no `concerto-mcp` referent to copy, and `agent_supervisor/mcp.rs` is
//! read-only config *discovery* (it parses `~/.claude/mcp.json` to render a
//! list), **not** a server — do not conflate them.
//!
//! ## What it is
//!
//! A [`rmcp::ServerHandler`] ([`MaestroMcpServer`]) that exposes the FROZEN
//! 18-tool Maestro registry ([`super::tools`]) over the MCP wire protocol. When
//! the Maestro CLI spawns (Task 402), the supervisor points it at this server's
//! stdio endpoint via the CLI's own `--mcp-config` + `--strict-mcp-config`, so
//! ONLY the 18 Maestro tools are visible (no filesystem, no shell, no other MCP
//! servers). 401 owns the **endpoint shape + the stdio framing**; 402 owns the
//! dial flags.
//!
//! ## The in-process stdio framing (400's pin)
//!
//! The MCP channel is an `rmcp` `transport-io` (`AsyncRead` + `AsyncWrite`)
//! duplex — newline-delimited JSON-RPC, the MCP stdio framing. It is a **local
//! pipe pair to the agent host**, distinct from the agent-host's own PTY +
//! CBOR-over-UDS terminal stream (400's reconciliation: the agent-host is NOT an
//! MCP transport). [`serve_maestro_mcp`] consumes the Core-side half of that
//! pipe pair (any `AsyncRead + AsyncWrite`), runs the server on it, and returns
//! a [`McpServerHandle`] the caller (402's spawn) keeps alive for the session.
//!
//! ## The typed-unimplemented contract (the 305 seam discipline)
//!
//! Every tool is **registered** with its frozen schema (so the CLI sees all 18)
//! but its dispatch returns a **typed** `rmcp` error until 405/406/407 fill it —
//! never `todo!()`/`unimplemented!()` (a panic crashes the in-process server),
//! never empty-success. See [`super::tools::dispatch`].
//!
//! ## Platform gating
//!
//! The server sits over the `#[cfg(unix)]` agent supervisor, so the whole
//! `maestro` module is `#[cfg(unix)]` (lib.rs). A non-unix build never reaches
//! this code; no stub is needed at a call site because nothing outside the
//! `cfg(unix)` module references it (the Windows lane simply omits `maestro`).

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer, RunningService};
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::IntoTransport;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

use concerto_persist::Persistence;

use super::summary::SummaryCache;
use super::tools::side::{ChipSlate, LiveNotifySink};
use super::tools::{self, ToolKind};

/// The MCP server name the Maestro CLI dials. FROZEN (design/08 §3.2): the CLI's
/// `--mcp-config` references this server by name and `--strict-mcp-config`
/// restricts the visible tool set to exactly this server's 18 tools.
pub const SERVER_NAME: &str = "concerto-maestro-mcp";

/// The in-process Maestro MCP server. Holds (cheap-clone) handles into the Core
/// subsystems the live tool impls (405/406/407) will call; in Task 401 it holds
/// none and calls none — every tool returns a typed unimplemented error.
///
/// Later tasks add `Arc`-cloned subsystem handles (03/05/07/13/14) as fields
/// here and thread them into the `tools::dispatch` arms they fill.
#[derive(Clone, Default)]
pub struct MaestroMcpServer {
    // The read-tool Core handles. `None` keeps the server handle-less (the
    // Task 401 registration/handshake tests build it via `new()`/`default()`
    // and only exercise registration + the typed-unimplemented seam); `Some`
    // (built via [`with_read_handles`]) routes the 11 read tools to the live
    // `tools::read::dispatch_read`. Write/side-channel tools keep the frozen
    // typed-unimplemented seam regardless unless side handles are wired.
    handles: Option<ReadHandles>,
    // The side-channel handles (Task 507b-ii). `None` keeps `notify_user` /
    // `propose_chip` on the frozen typed-unimplemented seam; `Some` (built via
    // [`with_side_handles`]) routes the side-channel tools to the live
    // `tools::side::dispatch_side` — `notify_user` lands a real notification via
    // the [`LiveNotifySink`] and `propose_chip` appends to the [`ChipSlate`].
    side: Option<SideHandles>,
}

/// The cheap-clone Core handles the 11 read tools query: the persistence the
/// reads run against and the Task 404 `WorkareaSummary` cache (which also owns
/// the injected [`super::summary::Clock`] this server sources `now_ms` from).
#[derive(Clone)]
struct ReadHandles {
    persist: Arc<Persistence>,
    cache: Arc<Mutex<SummaryCache>>,
}

/// The cheap-clone side-channel handles (Task 507b-ii): the live `notify_user`
/// sink (backed by sub-system 14's `NotificationHandle`) and the Maestro-owned
/// chip slate `propose_chip` appends to. Both are `Arc`-backed so cloning the
/// server per accepted connection is cheap.
#[derive(Clone)]
struct SideHandles {
    sink: LiveNotifySink,
    slate: ChipSlate,
}

impl MaestroMcpServer {
    /// Construct the server with no wired subsystem handles (Task 401). The
    /// registration/handshake tests use this; every tool returns the typed
    /// unimplemented error. Live read-tool routing needs [`with_read_handles`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct the server with the read-tool Core handles wired in (Milestone
    /// 1): the 11 `ToolKind::ReadOnly` tools route to the live
    /// [`tools::read::dispatch_read`] against `persist` + the 404 summary
    /// `cache`. Write/side-channel tools still return the frozen
    /// typed-unimplemented error (Milestone 2). The handles are cheap `Arc`
    /// clones, so cloning the server per accepted connection is cheap.
    pub fn with_read_handles(persist: Arc<Persistence>, cache: Arc<Mutex<SummaryCache>>) -> Self {
        Self {
            handles: Some(ReadHandles { persist, cache }),
            side: None,
        }
    }

    /// Wire the side-channel handles (Task 507b-ii) onto the server: the live
    /// `notify_user` [`LiveNotifySink`] (backed by sub-system 14's
    /// `NotificationHandle`) and the Maestro-owned [`ChipSlate`] `propose_chip`
    /// appends to. Until this is called the side-channel tools keep the frozen
    /// typed-unimplemented seam; once wired, `call_tool` routes them through the
    /// live [`tools::side::dispatch_side`]. Chainable after [`with_read_handles`]
    /// so boot can wire all live handles on one server. The handles are cheap
    /// `Arc` clones, so the per-connection server clone stays cheap.
    pub fn with_side_handles(mut self, sink: LiveNotifySink, slate: ChipSlate) -> Self {
        self.side = Some(SideHandles { sink, slate });
        self
    }

    /// The full set of registered tools, as `rmcp` [`Tool`]s, built from the
    /// FROZEN registry. Exposed so the Tier-1 harness can assert registration
    /// without a connected peer.
    pub fn registered_tools(&self) -> Vec<Tool> {
        tools::all_tools()
            .iter()
            .map(|d| d.to_rmcp_tool())
            .collect()
    }

    /// Synchronous dispatch shared by the [`ServerHandler::call_tool`] path and
    /// the in-process Tier-1 harness: routes a tool call to the frozen registry
    /// dispatch (typed-unimplemented in 401).
    pub fn dispatch_tool(&self, params: CallToolRequestParams) -> Result<CallToolResult, McpError> {
        tools::dispatch(&params.name, params.arguments)
    }
}

impl ServerHandler for MaestroMcpServer {
    fn get_info(&self) -> ServerInfo {
        let server_info = Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION"))
            .with_title("Concerto Maestro")
            .with_description("In-process MCP server exposing the 18 Maestro orchestration tools.");
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(server_info)
            .with_instructions(
                "Concerto Maestro tools: read workspace/workarea/session state, route prompts, \
                 and create workspaces/workareas. Write tools require user confirmation.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.registered_tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        match tools::class_of(&request.name) {
            // The 11 read tools route to the live, Core-handle-bearing
            // `read::dispatch_read` (Milestone 1). If the server was built
            // handle-less (`new()`/`default()`), the read handles are not wired
            // and we return a typed internal error rather than panicking.
            Some(ToolKind::ReadOnly) => {
                let h = self.handles.as_ref().ok_or_else(|| {
                    McpError::internal_error("maestro read handles not wired", None)
                })?;
                // We hold the cache guard across the whole read-tool dispatch. Only
                // `get_workarea_summary` actually reads the cache; the other read tools use the
                // concurrent Persistence read pool. For the single-user desktop Maestro (one
                // session, sequential tool calls) this coarse serialization is an accepted
                // tradeoff — narrowing it would mean changing dispatch_read's `&SummaryCache`
                // signature, which is intentionally frozen here.
                //
                // Source `now_ms` from the cache's injected clock (the
                // synthetic-clock seam, summary.rs §10) so the whole maestro
                // read path shares one clock — `SystemClock` in prod, a
                // `ManualClock` in tests.
                let cache = h.cache.lock().await;
                let now_ms = cache.now_ms();
                let value = tools::read::dispatch_read(
                    &request.name,
                    request.arguments,
                    &h.persist,
                    &cache,
                    now_ms,
                )
                .await?;
                Ok(value_to_call_result(value))
            }
            // Side-channel tools route to the live `side::dispatch_side` when
            // the side handles are wired (Task 507b-ii): `notify_user` lands a
            // real notification via the `LiveNotifySink` and `propose_chip`
            // appends to the `ChipSlate`. A side server built without the side
            // handles keeps the frozen typed-unimplemented seam.
            Some(ToolKind::SideChannel) => match self.side.as_ref() {
                Some(side) => {
                    // Source `now_ms` from the read cache's injected clock when
                    // wired (the synthetic-clock seam, shared with the read path)
                    // so prod/tests use one clock; fall back to wall-clock for a
                    // side-only server.
                    let now_ms = match self.handles.as_ref() {
                        Some(h) => h.cache.lock().await.now_ms(),
                        None => {
                            use crate::maestro::summary::Clock as _;
                            crate::maestro::summary::SystemClock.now_ms()
                        }
                    };
                    let value = tools::side::dispatch_side(
                        &request.name,
                        request.arguments,
                        &side.sink,
                        &side.slate,
                        now_ms,
                    )?;
                    Ok(value_to_call_result(value))
                }
                None => self.dispatch_tool(request),
            },
            // Write tools keep the frozen typed-unimplemented seam (Milestone 2
            // fills them); unknown names map to invalid_params via the same
            // `dispatch_tool` path.
            Some(ToolKind::Write) | None => self.dispatch_tool(request),
        }
    }
}

/// Wrap a read tool's frozen output JSON into a successful [`CallToolResult`].
///
/// Uses `CallToolResult::structured`, which places the JSON both as the
/// machine-readable `structured_content` (matched against the tool's frozen
/// output schema) and as a text `Content` rendering for agents that read the
/// unstructured content. `is_error` is `Some(false)` — never a typed error.
fn value_to_call_result(value: serde_json::Value) -> CallToolResult {
    CallToolResult::structured(value)
}

/// A live `concerto-maestro-mcp` server bound to one transport. Keep it alive
/// for the duration of the Maestro CLI session; dropping it tears the server
/// down. 402's spawn holds this for the session's lifetime.
pub struct McpServerHandle {
    running: RunningService<RoleServer, MaestroMcpServer>,
}

impl McpServerHandle {
    /// Cancel the server (clean shutdown). 402 calls this when the Maestro
    /// session ends.
    pub fn cancel_token(&self) -> rmcp::service::RunningServiceCancellationToken {
        self.running.cancellation_token()
    }

    /// Wait for the server to quit (peer closed, cancelled, or errored).
    pub async fn waiting(self) -> Result<rmcp::service::QuitReason, tokio::task::JoinError> {
        self.running.waiting().await
    }
}

/// Build and start the in-process `concerto-maestro-mcp` stdio MCP server over
/// the given transport (the Core-side half of the pipe pair to the agent host).
///
/// `transport` is any `rmcp` transport — in practice an `(AsyncRead, AsyncWrite)`
/// duplex pipe pair (the in-process stdio framing 400 pins). The returned
/// [`McpServerHandle`] owns the running service; 402's spawn keeps it alive for
/// the Maestro session and points the CLI's `--mcp-config` at the other half.
///
/// In Task 401 every tool the server serves returns a typed unimplemented MCP
/// error (405/406/407 fill the bodies behind the unchanged schemas).
pub async fn serve_maestro_mcp<T, E, A>(transport: T) -> Result<McpServerHandle, McpError>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let running = MaestroMcpServer::new()
        .serve(transport)
        .await
        .map_err(|e| McpError::internal_error(format!("maestro mcp serve failed: {e}"), None))?;
    Ok(McpServerHandle { running })
}

/// Bind `socket` (mode `0600`) and serve the Maestro MCP over every accepted
/// connection, each on its own task with a fresh clone of `template` (the
/// `Arc` handles make the clone cheap).
///
/// This is the Core end of the CLI bridge (Task 1): the bridge dials this UDS
/// and speaks newline-delimited JSON-RPC (the MCP stdio framing) over it; each
/// accepted connection is one MCP session. The loop runs until the task is
/// aborted — boot (the spawn site) holds the `JoinHandle` and drops/aborts it
/// on shutdown. Transient `accept` errors (e.g. ECONNABORTED, EMFILE/ENFILE on
/// fd exhaustion) are logged and skipped with a short backoff; they do not
/// terminate the listener.
///
/// A stale socket left by a crashed prior run is removed before bind; the `0600`
/// mode keeps the session owner-only (the bridge runs as the same user).
pub async fn serve_maestro_mcp_listener(
    socket: PathBuf,
    template: MaestroMcpServer,
) -> std::io::Result<()> {
    // Clear a stale socket from a prior run so `bind` does not fail with
    // EADDRINUSE on a leftover path.
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;

    loop {
        let (conn, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(target: "concerto::maestro", error = %e, "maestro mcp accept failed; continuing");
                // Avoid a hot spin on persistent fd-exhaustion style errors.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };
        let server = template.clone();
        tokio::spawn(async move {
            let (r, w) = conn.into_split();
            match server
                .serve(AsyncRwTransport::<RoleServer, _, _>::new(r, w))
                .await
            {
                Ok(running) => {
                    // Drive this MCP session to completion (peer closed,
                    // cancelled, or errored) before the task ends.
                    let _ = running.waiting().await;
                }
                Err(e) => tracing::warn!(
                    target: "concerto::maestro",
                    error = %e,
                    "maestro mcp serve failed for accepted connection"
                ),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::CallToolRequestParams;
    use rmcp::transport::async_rw::AsyncRwTransport;
    use rmcp::ServiceExt;

    #[test]
    fn server_registers_all_frozen_tools() {
        let server = MaestroMcpServer::new();
        let tools = server.registered_tools();
        // 11 read + 5 write + 2 side-channel = 18 (the design doc's "16"
        // headline is an arithmetic slip; the enumerated §5.1 set is 18).
        assert_eq!(tools.len(), 18, "the server registers exactly the §5.1 set");

        // Names match the registry (which the tools-module test pins to §5.1).
        let names: std::collections::BTreeSet<&str> =
            tools.iter().map(|t| t.name.as_ref()).collect();
        for d in tools::all_tools() {
            assert!(names.contains(d.name), "{} must be registered", d.name);
        }
    }

    #[test]
    fn server_advertises_tools_capability_and_its_name() {
        let info = MaestroMcpServer::new().get_info();
        assert!(
            info.capabilities.tools.is_some(),
            "tools capability must be enabled"
        );
        assert_eq!(info.server_info.name, SERVER_NAME);
    }

    #[test]
    fn server_dispatch_returns_typed_unimplemented_not_panic() {
        let server = MaestroMcpServer::new();
        for d in tools::all_tools() {
            let params = CallToolRequestParams::new(d.name);
            let err = server
                .dispatch_tool(params)
                .expect_err(&format!("{} must reject with a typed error", d.name));
            assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
            assert!(err.message.contains("wired in Task 40"));
        }
    }

    /// End-to-end over a real in-process duplex pipe pair: a client peer dials
    /// the server, lists tools (gets all 18), and calls one (gets the typed
    /// unimplemented error over the wire — not a panic, not an empty success).
    /// This exercises the actual stdio framing 402 will dial.
    ///
    /// The MCP `initialize` handshake is symmetric — `serve_maestro_mcp(..).await`
    /// (server) does not return until the client has connected, so the two sides
    /// MUST be driven concurrently (the server is spawned, not awaited inline,
    /// before the client connects). A timeout guard turns any future framing
    /// regression into a fast failure rather than a hang.
    #[tokio::test]
    async fn end_to_end_duplex_lists_all_tools_and_typed_errors_on_call() {
        use std::time::Duration;

        let (server_io, client_io) = tokio::io::duplex(8192);
        let (sr, sw) = tokio::io::split(server_io);
        let (cr, cw) = tokio::io::split(client_io);

        // Server side: spawn the real serve_maestro_mcp so the initialize
        // handshake can complete against the client we start next.
        let server_task = tokio::spawn(async move {
            serve_maestro_mcp(AsyncRwTransport::<RoleServer, _, _>::new(sr, sw)).await
        });

        let body = async {
            // Client side: a default ClientHandler peer over the other half.
            let client =
                ().serve(AsyncRwTransport::<rmcp::RoleClient, _, _>::new(cr, cw))
                    .await
                    .expect("client connects");

            let listed = client.list_all_tools().await.expect("list tools");
            assert_eq!(listed.len(), 18, "client sees all 18 frozen tools");

            // `serve_maestro_mcp` builds a HANDLE-LESS server, so a WRITE tool
            // keeps the frozen typed-unimplemented seam (Milestone 2 fills it).
            // (Read tools now route to the live `dispatch_read` when handles are
            // wired — see `call_tool_list_workspaces_returns_live_data`; over a
            // handle-less server they return the "read handles not wired" guard,
            // which is still a typed error, never a panic or empty success.)
            let result = client
                .call_tool(CallToolRequestParams::new("create_workspace"))
                .await;
            let err = result.expect_err("call returns a typed error, not Ok");
            // The error propagates as an rmcp ServiceError carrying the McpError.
            let msg = format!("{err}");
            assert!(
                msg.contains("wired in Task 40"),
                "typed unimplemented message must cross the wire, got: {msg}"
            );

            client.cancel().await.ok();
        };

        tokio::time::timeout(Duration::from_secs(20), body)
            .await
            .expect("end-to-end MCP round-trip must finish well under the guard");

        // The server started (handshake completed) and is now torn down with the
        // client; drop its handle.
        let server_handle = tokio::time::timeout(Duration::from_secs(5), server_task)
            .await
            .expect("server task joins")
            .expect("server task did not panic")
            .expect("server started");
        server_handle.cancel_token().cancel();
    }

    // -- Milestone 1: live read-tool routing -------------------------------

    /// A fresh on-disk `Persistence` seeded with one workspace, mirroring the
    /// `tools/read.rs` test fixture (`Persistence::open` + `workspaces::insert`).
    /// The tempdir is returned so the caller keeps the DB alive for the test.
    async fn fresh_persist_with_workspace(
        id: &str,
        name: &str,
    ) -> (tempfile::TempDir, Persistence) {
        use concerto_persist::{NewWorkspace, PersistenceConfig, WorkspaceId};

        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persistence::open(PersistenceConfig {
            db_path: dir.path().join("test.db"),
            max_readers: 2,
        })
        .await
        .expect("open persistence");

        let mut w = persist.writer().await;
        concerto_persist::workspaces::insert(
            &mut w,
            NewWorkspace {
                id: WorkspaceId(id.into()),
                name: name.into(),
                slug: id.into(),
                icon: None,
                description: None,
                permission_mode: None,
                created_at: 1,
            },
        )
        .await
        .expect("insert workspace");
        drop(w);

        (dir, persist)
    }

    fn system_clock_cache() -> Arc<Mutex<SummaryCache>> {
        Arc::new(Mutex::new(SummaryCache::with_system_clock()))
    }

    /// A handle-bearing server routes `list_workspaces` to the live
    /// `dispatch_read` over the in-memory transport pair: the result is a
    /// SUCCESS (`is_error != Some(true)`) carrying the seeded workspace id —
    /// NOT the frozen typed-unimplemented error.
    #[tokio::test]
    async fn call_tool_list_workspaces_returns_live_data() {
        use std::time::Duration;

        let (_dir, persist) = fresh_persist_with_workspace("ws-real", "Real").await;
        let server = MaestroMcpServer::with_read_handles(Arc::new(persist), system_clock_cache());

        let (server_io, client_io) = tokio::io::duplex(8192);
        let (sr, sw) = tokio::io::split(server_io);
        let (cr, cw) = tokio::io::split(client_io);

        let server_task = tokio::spawn(async move {
            server
                .serve(AsyncRwTransport::<RoleServer, _, _>::new(sr, sw))
                .await
                .expect("server connects")
                .waiting()
                .await
                .ok();
        });

        let body = async {
            let client =
                ().serve(AsyncRwTransport::<rmcp::RoleClient, _, _>::new(cr, cw))
                    .await
                    .expect("client connects");

            let result = client
                .call_tool(CallToolRequestParams::new("list_workspaces"))
                .await
                .expect("list_workspaces returns Ok, not a typed error");

            assert_ne!(
                result.is_error,
                Some(true),
                "live read tool must be a success, not an error"
            );
            // The seeded workspace id crosses the wire in both the structured
            // content and the text rendering.
            let serialized = serde_json::to_string(&result).expect("serialize result");
            assert!(
                serialized.contains("ws-real"),
                "result must mention the seeded workspace id, got: {serialized}"
            );

            client.cancel().await.ok();
        };

        tokio::time::timeout(Duration::from_secs(20), body)
            .await
            .expect("round-trip finishes under the guard");
        server_task.abort();
    }

    /// The routing decision table: read tools classify `ReadOnly` (→ live
    /// `dispatch_read`), write/side tools keep their classes (→ frozen seam),
    /// and an unknown name is `None` (→ `invalid_params` via `dispatch_tool`).
    #[test]
    fn class_of_drives_read_vs_frozen_routing() {
        assert_eq!(tools::class_of("list_workspaces"), Some(ToolKind::ReadOnly));
        assert_eq!(
            tools::class_of("cross_workarea_search"),
            Some(ToolKind::ReadOnly)
        );
        assert_eq!(tools::class_of("create_workspace"), Some(ToolKind::Write));
        assert_eq!(tools::class_of("notify_user"), Some(ToolKind::SideChannel));
        assert_eq!(tools::class_of("not_a_real_tool"), None);
    }

    // -- Task 507b-ii: live side-channel routing (notify_user) -------------

    /// A side-channel server built WITHOUT side handles keeps the frozen
    /// typed-unimplemented seam for `notify_user` (the 407 stub error), never a
    /// panic or empty success.
    #[tokio::test]
    async fn notify_user_without_side_handles_stays_typed_unimplemented() {
        let (_dir, persist) = fresh_persist_with_workspace("ws-no-side", "NoSide").await;
        let server = MaestroMcpServer::with_read_handles(Arc::new(persist), system_clock_cache());
        let err = server
            .dispatch_tool(CallToolRequestParams::new("notify_user"))
            .expect_err("notify_user keeps the frozen seam without side handles");
        assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert!(err.message.contains("407"));
    }

    /// With side handles wired, `notify_user` over the in-process transport
    /// returns the frozen success AND lands a real notification row that
    /// surfaces via the same `NotificationHandle` inbox — proving the live sink
    /// is routed end-to-end through `call_tool` → `dispatch_side`.
    #[tokio::test]
    async fn call_tool_notify_user_lands_live_notification() {
        use crate::maestro::tools::side::LiveNotifySink;
        use crate::notifications::handle::{NoEvents, NotificationHandle};
        use crate::notifications::push::ExpoPushBackend;
        use std::time::Duration;

        let (_dir, persist) = fresh_persist_with_workspace("ws-notify", "Notify").await;
        let persist = Arc::new(persist);
        let notif = NotificationHandle::new(
            Arc::clone(&persist),
            Arc::new(ExpoPushBackend::new(None)),
            Arc::new(NoEvents),
        );
        let sink = LiveNotifySink::new(notif.clone(), Some("sess-1".into()));
        let server =
            MaestroMcpServer::with_read_handles(Arc::clone(&persist), system_clock_cache())
                .with_side_handles(sink, super::super::tools::side::ChipSlate::new());

        let (server_io, client_io) = tokio::io::duplex(8192);
        let (sr, sw) = tokio::io::split(server_io);
        let (cr, cw) = tokio::io::split(client_io);

        let server_task = tokio::spawn(async move {
            server
                .serve(AsyncRwTransport::<RoleServer, _, _>::new(sr, sw))
                .await
                .expect("server connects")
                .waiting()
                .await
                .ok();
        });

        let body = async {
            let client =
                ().serve(AsyncRwTransport::<rmcp::RoleClient, _, _>::new(cr, cw))
                    .await
                    .expect("client connects");

            let mut params = CallToolRequestParams::new("notify_user");
            let mut args = serde_json::Map::new();
            args.insert("text".into(), serde_json::json!("deploy finished"));
            args.insert("severity".into(), serde_json::json!("medium"));
            params.arguments = Some(args);

            let result = client
                .call_tool(params)
                .await
                .expect("notify_user returns Ok, not a typed error");
            assert_ne!(
                result.is_error,
                Some(true),
                "live notify_user must succeed (the frozen Ok)"
            );

            client.cancel().await.ok();
        };

        tokio::time::timeout(Duration::from_secs(20), body)
            .await
            .expect("notify_user round-trip finishes under the guard");

        // The spawned `notify()` lands a real row in the shared persistence.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let inbox = notif.get_inbox(None, None, false, 50).await.expect("inbox");
            if let Some(n) = inbox.first() {
                assert_eq!(n.body, "deploy finished");
                assert_eq!(n.subject_id, "sess-1");
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "notify_user live row never landed"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        server_task.abort();
    }

    /// A write tool stays the frozen typed-unimplemented error even on a
    /// handle-bearing server (Milestone 2 fills it, not this task).
    #[tokio::test]
    async fn write_tool_stays_typed_unimplemented_on_live_server() {
        let (_dir, persist) = fresh_persist_with_workspace("ws-w", "W").await;
        let server = MaestroMcpServer::with_read_handles(Arc::new(persist), system_clock_cache());
        let err = server
            .dispatch_tool(CallToolRequestParams::new("create_workspace"))
            .expect_err("write tool keeps the frozen seam");
        assert_eq!(err.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert!(err.message.contains("Task 406"));
    }

    // -- Milestone 1: the UDS accept loop ----------------------------------

    /// The full socket path: bind the listener, dial it with a raw `UnixStream`
    /// (as the Task 1 bridge does), run an MCP client over it, and assert
    /// `list_workspaces` returns the seeded workspace — proving
    /// socket → accept → fresh server clone → `dispatch_read`.
    #[tokio::test]
    async fn listener_serves_live_read_over_socket() {
        use std::time::Duration;
        use tokio::net::UnixStream;

        let (_dir, persist) = fresh_persist_with_workspace("ws-socket", "Socketed").await;
        let template = MaestroMcpServer::with_read_handles(Arc::new(persist), system_clock_cache());

        let sockdir = tempfile::tempdir().expect("sockdir");
        let socket = sockdir.path().join("maestro-mcp.sock");

        let listener_task = tokio::spawn(serve_maestro_mcp_listener(socket.clone(), template));

        // Wait for the socket to appear (bind happens inside the spawned task).
        let bound = async {
            loop {
                if socket.exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), bound)
            .await
            .expect("listener binds the socket");

        // 0600 perms on the socket.
        let mode = std::fs::metadata(&socket)
            .expect("stat socket")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "socket must be owner-only (0600)");

        let body = async {
            let stream = UnixStream::connect(&socket).await.expect("dial socket");
            let (r, w) = stream.into_split();
            let client =
                ().serve(AsyncRwTransport::<rmcp::RoleClient, _, _>::new(r, w))
                    .await
                    .expect("client connects over socket");

            let result = client
                .call_tool(CallToolRequestParams::new("list_workspaces"))
                .await
                .expect("list_workspaces over socket returns Ok");
            assert_ne!(result.is_error, Some(true));
            let serialized = serde_json::to_string(&result).expect("serialize");
            assert!(
                serialized.contains("ws-socket"),
                "socket round-trip must surface the seeded workspace, got: {serialized}"
            );

            client.cancel().await.ok();
        };

        tokio::time::timeout(Duration::from_secs(20), body)
            .await
            .expect("socket round-trip finishes under the guard");

        listener_task.abort();
    }

    /// The listener removes a stale socket left by a prior run and still binds
    /// + sets 0600 (the lighter bind/perms guard).
    #[tokio::test]
    async fn listener_binds_over_stale_socket_with_0600() {
        use std::os::unix::fs::FileTypeExt;
        use std::time::Duration;

        let template = MaestroMcpServer::new();
        let sockdir = tempfile::tempdir().expect("sockdir");
        let socket = sockdir.path().join("stale.sock");
        // Leave a stale regular file at the path; bind must clear it first.
        std::fs::write(&socket, b"stale").expect("write stale file");

        let listener_task = tokio::spawn(serve_maestro_mcp_listener(socket.clone(), template));

        let bound = async {
            loop {
                if let Ok(meta) = std::fs::metadata(&socket) {
                    // A bound UDS is a socket, not a regular file.
                    if meta.file_type().is_socket() {
                        break meta;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        let meta = tokio::time::timeout(Duration::from_secs(5), bound)
            .await
            .expect("listener rebinds over the stale socket");
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);

        listener_task.abort();
    }
}
