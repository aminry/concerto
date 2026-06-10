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

use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer, RunningService};
use rmcp::transport::IntoTransport;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt};

use super::tools;

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
    // Soft seam: 405/406/407 add cheap-clone Core subsystem handles here
    // (e.g. `workspace_manager`, `scheduler`, `vcs`, `notifications`) and pass
    // them into their `tools::dispatch` arm. Intentionally empty in 401.
    _private: (),
}

impl MaestroMcpServer {
    /// Construct the server with no wired subsystem handles (Task 401). 405/406/407
    /// add a richer constructor as they thread Core handles in.
    pub fn new() -> Self {
        Self::default()
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
        self.dispatch_tool(request)
    }
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

            let result = client
                .call_tool(CallToolRequestParams::new("list_workspaces"))
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
}
