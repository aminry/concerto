//! Task 10 (Maestro Live-Integration): the CI-runnable **end-to-end gate** for
//! the Maestro MCP TOOL-SERVING half — the one piece Phase 4 lacked.
//!
//! ## What this proves
//!
//! It drives the live loop exactly the way the real Claude CLI does in
//! production: it spawns the **REAL `concerto-maestro-bridge` binary** as an MCP
//! stdio server and speaks newline-delimited JSON-RPC (the MCP stdio framing)
//! over its stdin/stdout. The bridge relays that stream to the Core's Maestro
//! MCP UDS, where `serve_maestro_mcp_listener` accepts the connection, serves a
//! handle-bearing `MaestroMcpServer`, and routes read tools to the live
//! `dispatch_read` against a real on-disk `Persistence`. So a single round-trip
//! exercises the whole chain end to end:
//!
//! ```text
//! rmcp client  →  REAL bridge bin (stdio)  →  UDS  →  serve_maestro_mcp_listener
//!              →  MaestroMcpServer::call_tool  →  dispatch_read  →  live Persistence
//! ```
//!
//! In production the chain is `Claude CLI → bridge → UDS → server`; the bridge is
//! a **transparent stdio↔UDS relay** (`concerto_maestro_bridge::relay`), so an
//! rmcp client over the bridge's stdio is exactly equivalent to a client over the
//! socket — and additionally exercises the real bridge binary in the loop, which
//! the existing in-crate `listener_serves_live_read_over_socket` test (a raw
//! `UnixStream` straight to the socket) does NOT.
//!
//! ## Transport choice (and why not rmcp's child-process transport)
//!
//! rmcp 1.7.0 *does* ship a `TokioChildProcess` child-process transport, but it
//! lives behind the `transport-child-process` feature, which pulls in a brand-new
//! external dependency tree (`process-wrap` + deps) that is not yet vetted in
//! `deny.toml`. The task explicitly sanctions the dependency-free alternative:
//! spawn the bridge ourselves with `tokio::process::Command` (piped stdin/stdout)
//! and run the rmcp client over those pipes via `AsyncRwTransport` — the same
//! transport every other Maestro MCP test already uses (the `transport-io`
//! feature, no new crates, no `cargo deny` churn). Functionally this is identical
//! to the production Claude-CLI→bridge stdio dial; it is the most faithful path
//! that adds zero un-vetted dependencies.
//!
//! ## What is NOT covered here (by design)
//!
//! The OTHER half of the live loop — freeform text → live LLM session → streamed
//! reply — is hardwired to the real `claude` CLI (`AgentKind::Maestro` →
//! `ClaudeCliProvider`) and so is NOT CI-testable. It is covered by the MANUAL
//! Tier-3 checklist in
//! `docs/superpowers/plans/2026-06-11-maestro-live-integration.md`. This file is
//! strictly the tool-serving gate.
//!
//! Unix-only: the whole Maestro spine (UDS + `rmcp` server) is `#[cfg(unix)]`.

#![cfg(unix)]

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::ServiceExt;
use tokio::process::Command;
use tokio::sync::Mutex;

use concerto_core::maestro::mcp::serve_maestro_mcp_listener;
use concerto_core::maestro::summary::SummaryCache;
use concerto_core::maestro::MaestroMcpServer;
use concerto_persist::{NewWorkspace, Persistence, PersistenceConfig, WorkspaceId};

/// Open a fresh on-disk `Persistence` seeded with one workspace, mirroring the
/// `mcp.rs` / `tools/read.rs` fixture. The tempdir is returned so the caller
/// keeps the DB file alive for the duration of the test.
async fn fresh_persist_with_workspace(
    id: &str,
    name: &str,
) -> (tempfile::TempDir, Persistence) {
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

/// End-to-end: the REAL bridge bin relays an rmcp client's MCP traffic over its
/// stdio to a live Maestro MCP server bound on a UDS, and a read tool returns
/// live DB data.
#[tokio::test(flavor = "multi_thread")]
async fn bridge_bin_serves_live_read_end_to_end() {
    // 1. Live Persistence seeded with one workspace the read tool will surface.
    let (_dir, persist) = fresh_persist_with_workspace("ws-e2e", "E2E").await;

    // 2. Handle-bearing server: the 11 read tools route to the live
    //    `dispatch_read`; write/side tools keep the frozen typed-unimplemented
    //    seam (Milestone-1 boundary).
    let template = MaestroMcpServer::with_read_handles(
        Arc::new(persist),
        Arc::new(Mutex::new(SummaryCache::with_system_clock())),
    );

    // 3. Bind a temp socket and serve the accept loop in a background task. The
    //    tempdir auto-cleans the socket on drop.
    let sockdir = tempfile::tempdir().expect("sockdir");
    let socket = sockdir.path().join("maestro-mcp.sock");
    let listener_task = tokio::spawn(serve_maestro_mcp_listener(socket.clone(), template));

    // Wait for the listener to bind (bind happens inside the spawned task).
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

    // 4. Resolve the REAL bridge bin cross-crate. `concerto-maestro-bridge` is a
    //    separate workspace member (not a dependency of `concerto-core`), so
    //    `CARGO_BIN_EXE_*` is not set for this test. `assert_cmd`'s `cargo_bin`
    //    only *locates* the already-built binary in the workspace target dir — it
    //    does not build it. The CI gate runs `cargo test/nextest --workspace`,
    //    which builds every member bin (incl. the bridge) before any test, so it
    //    is always present; a bare `cargo test -p concerto-core --test maestro_e2e`
    //    on a clean target must build the bridge first (the assert below names it).
    let bridge_bin = assert_cmd::cargo::cargo_bin("concerto-maestro-bridge");
    assert!(
        bridge_bin.exists(),
        "bridge bin should be built at {}",
        bridge_bin.display()
    );

    // 5. Spawn the bridge as our MCP stdio server: `concerto-maestro-bridge
    //    --socket <socket>`, piped stdin/stdout, and run the rmcp CLIENT over
    //    those pipes. The bridge transparently relays our MCP frames to/from the
    //    UDS, so this is the production Claude-CLI→bridge stdio dial.
    let mut child = Command::new(&bridge_bin)
        .arg("--socket")
        .arg(&socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Inherit stderr so a bridge panic / dial failure surfaces in test logs.
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn concerto-maestro-bridge");

    let child_stdin = child.stdin.take().expect("bridge stdin piped");
    let child_stdout = child.stdout.take().expect("bridge stdout piped");

    let body = async {
        // Client reads the bridge's stdout, writes to the bridge's stdin —
        // i.e. it speaks MCP to the bridge, which relays to the server. The
        // `serve` call performs the MCP `initialize` handshake.
        let client = ()
            .serve(AsyncRwTransport::<rmcp::RoleClient, _, _>::new(
                child_stdout,
                child_stdin,
            ))
            .await
            .expect("rmcp client handshakes through the real bridge");

        // (a) Sanity: the frozen 18-tool registry crosses the full loop.
        let tools = client.list_all_tools().await.expect("list_tools over bridge");
        assert_eq!(
            tools.len(),
            18,
            "client sees all 18 frozen Maestro tools through the real bridge"
        );

        // (b) The live read tool: `list_workspaces` must SUCCEED and surface the
        //     seeded workspace id — proving bridge → UDS → server → dispatch_read
        //     → live Persistence end to end.
        let result = client
            .call_tool(CallToolRequestParams::new("list_workspaces"))
            .await
            .expect("list_workspaces over bridge returns Ok, not a typed error");
        assert_ne!(
            result.is_error,
            Some(true),
            "live read tool must be a success across the real bridge"
        );
        let serialized = serde_json::to_string(&result).expect("serialize result");
        assert!(
            serialized.contains("ws-e2e"),
            "end-to-end result must surface the seeded workspace id, got: {serialized}"
        );

        // (c) Milestone-1 boundary: a WRITE tool still returns the typed
        //     unimplemented error across the same loop (never a panic, never an
        //     empty success).
        let write = client
            .call_tool(CallToolRequestParams::new("create_workspace"))
            .await;
        let err = write.expect_err("write tool returns a typed error, not Ok");
        let msg = format!("{err}");
        assert!(
            msg.contains("wired in Task 40"),
            "write tool must keep the frozen typed-unimplemented seam, got: {msg}"
        );

        client.cancel().await.ok();
    };

    tokio::time::timeout(Duration::from_secs(30), body)
        .await
        .expect("end-to-end round-trip through the real bridge must finish under the guard");

    // 6. Clean teardown: the client is cancelled (drops the bridge's stdin, so
    //    the bridge relay returns and the child exits — `kill_on_drop` is a
    //    backstop), abort the listener task, and let the tempdirs auto-clean.
    let _ = child.wait().await;
    listener_task.abort();
}
