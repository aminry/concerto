# Maestro Live-Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the Maestro live agent loop end-to-end so a user can chat with a live Claude session (using the 11 read tools), route `@composer` prompts to the workspace they're viewing, and get `/digest` — delivered with the end-to-end tests Phase 4 missed.

**Architecture:** The Core hosts the in-process `rmcp` Maestro MCP server over a dedicated UDS (`~/.concerto/maestro-mcp.sock`); a tiny `concerto-maestro-bridge` binary, named in the CLI's `.mcp.json`, copies bytes between the spawned Claude CLI's stdio and that socket. Boot ensures a hidden system workspace/workarea to satisfy the `sessions.workarea_id NOT NULL` FK, spawns the long-lived `AgentKind::Maestro` session bound to a `kind='maestro'` chat, and writes `.mcp.json`. `MaestroMessageRequest` carries an optional `workspace_id` scope hint so routing targets the viewed workspace.

**Tech Stack:** Rust (tokio, rmcp, sqlx/SQLite, tonic/prost), the Concerto agent-supervisor + persistence crates, the Tauri desktop shell (TS + `src-tauri` Rust).

**Reference spec:** `docs/superpowers/specs/2026-06-11-maestro-live-integration-design.md`

**Scope:** Milestone 1 only (read-capable Maestro). Write MCP tools + confirmation chips are Milestone 2 and out of scope here — the 5 write + 2 side-channel tools keep returning their existing typed-unimplemented MCP error (safe, no panic).

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/maestro-bridge/` (new crate) | Standalone bin: copy bytes between stdio and a UDS. Zero MCP knowledge. | Create |
| `crates/core/src/maestro/mcp.rs` | `MaestroMcpServer` gains read-tool handles; `call_tool` routes read tools to `dispatch_read`; new `serve_maestro_mcp_listener` binds + accept-loops the dedicated UDS. | Modify |
| `crates/core/src/maestro/mod.rs` | `write_maestro_mcp_json` (compose + write `.mcp.json`); `maestro_mcp_socket_path`. | Modify |
| `crates/core/src/maestro/system_workarea.rs` (new) | `ensure_system_workspace_and_workarea` returning the reserved ids; the sentinel id constants. | Create |
| `crates/core/src/maestro/handle.rs` | `spawn_maestro_session` (creates `kind='maestro'` chat, binds session); `forward_freeform`/routing honor a `workspace_id` hint. | Modify |
| `crates/core/src/boot.rs` | After building `MaestroHandle`: ensure system ws/wa, bind MCP listener, write `.mcp.json`, spawn the session; degrade-to-inert. | Modify |
| `crates/persist/src/workspaces.rs` / `workareas.rs` | List queries exclude the sentinel id (or a helper the callers use). | Modify |
| `crates/proto/proto/concerto/v1/maestro.proto` | Add `optional string workspace_id = 3` to `MaestroMessageRequest`. | Modify |
| `crates/core/src/handlers/maestro.rs` | Thread `workspace_id` from request into the handle call. | Modify |
| `apps/desktop/src-tauri/src/rpc.rs` | Formalize the Maestro.* dispatch arms; pass `workspace_id`. | Modify |
| `apps/desktop/src/api/maestro.ts` + composer | Pass active workspace id into `SendToMaestro`. | Modify |
| `apps/desktop/src/...workspace list` | Filter the sentinel workspace from UI lists. | Modify |

---

## Task 1: `concerto-maestro-bridge` binary

**Files:**
- Create: `crates/maestro-bridge/Cargo.toml`
- Create: `crates/maestro-bridge/src/main.rs`
- Modify: root `Cargo.toml` workspace `members` (add `crates/maestro-bridge`)
- Test: `crates/maestro-bridge/tests/relay.rs`

The bridge is the command the Claude CLI spawns (named in `.mcp.json`). It connects to the Core's Maestro-MCP UDS and relays bytes both directions: process stdin → socket, socket → process stdout. MCP stdio framing (newline-delimited JSON-RPC) flows through transparently.

- [ ] **Step 1: Create the crate manifest**

`crates/maestro-bridge/Cargo.toml`:
```toml
[package]
name = "concerto-maestro-bridge"
version = "0.1.0"
edition = "2021"
publish = false

[[bin]]
name = "concerto-maestro-bridge"
path = "src/main.rs"

[dependencies]
clap = { workspace = true, features = ["derive"] }
tokio = { workspace = true, features = ["rt", "macros", "io-std", "io-util", "net"] }

[dev-dependencies]
tempfile = { workspace = true }
```
Add `"crates/maestro-bridge"` to the workspace `members` array in the root `Cargo.toml`.

- [ ] **Step 2: Write the failing relay test**

`crates/maestro-bridge/tests/relay.rs`:
```rust
// Spawns a UnixListener that echoes a line back, runs the relay() against it
// with in-memory stdin/stdout, and asserts the echoed bytes reach stdout.
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

#[tokio::test]
async fn relay_copies_both_directions() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("mcp.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    // Server: read a line, echo it back with a suffix, close.
    let server = tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 64];
        let n = conn.read(&mut buf).await.unwrap();
        let mut out = buf[..n].to_vec();
        out.extend_from_slice(b"-pong\n");
        conn.write_all(&out).await.unwrap();
        conn.flush().await.unwrap();
    });

    // Client side: feed "ping\n" as stdin, capture stdout.
    let input = &b"ping\n"[..];
    let mut output: Vec<u8> = Vec::new();
    concerto_maestro_bridge::relay(&sock, input, &mut output)
        .await
        .unwrap();

    server.await.unwrap();
    assert_eq!(output, b"ping\n-pong\n");
}
```

- [ ] **Step 3: Run the test, verify it fails**

Run: `cargo test -p concerto-maestro-bridge --test relay`
Expected: FAIL — `concerto_maestro_bridge::relay` does not exist (no lib target yet).

- [ ] **Step 4: Implement `relay` + `main`**

`crates/maestro-bridge/src/main.rs`:
```rust
//! `concerto-maestro-bridge` — a dumb stdio↔UDS relay. The Claude CLI spawns
//! this (named in the Maestro `.mcp.json`); it connects to the Core's
//! Maestro-MCP unix socket and copies bytes both directions. It has NO MCP
//! knowledge: MCP stdio framing (newline-delimited JSON-RPC) passes through
//! transparently. Unix-only (the Maestro is `#[cfg(unix)]`).

use std::path::{Path, PathBuf};

use clap::Parser;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

#[derive(Parser, Debug)]
#[command(name = "concerto-maestro-bridge", version, about = "Stdio↔UDS relay for the Concerto Maestro MCP server.")]
struct Cli {
    /// The Maestro-MCP unix socket the Core listens on.
    #[arg(long)]
    socket: PathBuf,
}

/// Relay `input` → socket and socket → `output` concurrently until either side
/// reaches EOF. Generic over the std streams so the test can drive it with
/// in-memory buffers.
pub async fn relay<R, W>(socket: &Path, mut input: R, mut output: W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let stream = UnixStream::connect(socket).await?;
    let (mut sock_r, mut sock_w) = stream.into_split();

    let up = async {
        tokio::io::copy(&mut input, &mut sock_w).await?;
        sock_w.shutdown().await
    };
    let down = async {
        tokio::io::copy(&mut sock_r, &mut output).await?;
        output.shutdown().await
    };
    tokio::try_join!(up, down)?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    relay(&cli.socket, stdin, stdout).await
}
```
To make `relay` importable by the integration test, add a thin lib target: create `crates/maestro-bridge/src/lib.rs` with `pub use` of `relay`, OR (simpler) keep `relay` in `main.rs` and add `[lib] path = "src/main.rs"` is not valid — instead create `src/lib.rs` holding `relay` + the `relay<R,W>` body, and have `main.rs` `use concerto_maestro_bridge::relay;`. Pick the lib+bin split: move `relay` into `src/lib.rs`, leave `Cli`/`main` in `src/main.rs`, and add to `Cargo.toml`:
```toml
[lib]
name = "concerto_maestro_bridge"
path = "src/lib.rs"
```

- [ ] **Step 5: Run the test, verify it passes**

Run: `cargo test -p concerto-maestro-bridge --test relay`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/maestro-bridge Cargo.toml Cargo.lock
git commit -m "feat(maestro): concerto-maestro-bridge stdio↔UDS relay binary"
```

---

## Task 2: Maestro-MCP socket path + `.mcp.json` writer

**Files:**
- Modify: `crates/core/src/maestro/mod.rs`
- Test: in-module `#[cfg(test)]` in `mod.rs`

The `.mcp.json` the CLI dials must name the bridge command (absolute path), the `--socket` arg (the Maestro-MCP socket), and register it under the frozen server name `concerto-maestro-mcp`.

- [ ] **Step 1: Write the failing test**

Add to `crates/core/src/maestro/mod.rs` `#[cfg(test)] mod tests`:
```rust
#[test]
fn mcp_json_names_bridge_and_socket_under_server_name() {
    let json = compose_maestro_mcp_json(
        std::path::Path::new("/opt/concerto/concerto-maestro-bridge"),
        std::path::Path::new("/home/u/.concerto/maestro-mcp.sock"),
    );
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let server = &v["mcpServers"][SERVER_NAME];
    assert_eq!(server["command"], "/opt/concerto/concerto-maestro-bridge");
    assert_eq!(server["args"][0], "--socket");
    assert_eq!(server["args"][1], "/home/u/.concerto/maestro-mcp.sock");
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p concerto-core maestro::tests::mcp_json_names_bridge_and_socket`
Expected: FAIL — `compose_maestro_mcp_json` undefined.

- [ ] **Step 3: Implement the path helper + the JSON composer + writer**

In `crates/core/src/maestro/mod.rs` (near `ensure_maestro_scratch_dir`):
```rust
use crate::maestro::mcp::SERVER_NAME;

/// The Core's Maestro-MCP unix socket path (`~/.concerto/maestro-mcp.sock`).
/// The bridge dials this; the Core listens on it (Task 5).
pub fn maestro_mcp_socket_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        Error::Internal("cannot resolve home dir for the maestro-mcp socket".into())
    })?;
    Ok(home.join(".concerto").join("maestro-mcp.sock"))
}

/// Compose the `.mcp.json` body that points the spawned CLI at the bridge.
/// `--strict-mcp-config` (in the launch args) restricts the CLI to exactly the
/// server registered here.
pub fn compose_maestro_mcp_json(bridge_bin: &Path, socket: &Path) -> String {
    let v = serde_json::json!({
        "mcpServers": {
            SERVER_NAME: {
                "command": bridge_bin.to_string_lossy(),
                "args": ["--socket", socket.to_string_lossy()],
            }
        }
    });
    serde_json::to_string_pretty(&v).expect("serializing a json! literal never fails")
}

/// Write the Maestro `.mcp.json` into `scratch_cwd` (the path the launch spec's
/// `--mcp-config` points at: `scratch_cwd/.mcp.json`). Idempotent (overwrites).
pub fn write_maestro_mcp_json(scratch_cwd: &Path, bridge_bin: &Path, socket: &Path) -> Result<PathBuf> {
    let path = scratch_cwd.join(".mcp.json");
    std::fs::write(&path, compose_maestro_mcp_json(bridge_bin, socket)).map_err(Error::Io)?;
    Ok(path)
}
```
Confirm `dirs` is already a dependency of `concerto-core` (it is used elsewhere); if not, use the same home-dir resolution `maestro_scratch_dir` uses.

- [ ] **Step 4: Run, verify it passes**

Run: `cargo test -p concerto-core maestro::tests::mcp_json_names_bridge_and_socket`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/maestro/mod.rs
git commit -m "feat(maestro): .mcp.json composer + maestro-mcp socket path"
```

---

## Task 3: Hidden system workspace + workarea

**Files:**
- Create: `crates/core/src/maestro/system_workarea.rs`
- Modify: `crates/core/src/maestro/mod.rs` (add `mod system_workarea;` + re-export)
- Test: in-module `#[cfg(test)]`

The Maestro session needs a host workarea to satisfy `sessions.workarea_id NOT NULL REFERENCES workareas(id)`. Reserve a sentinel workspace + workarea with stable ids; ensure idempotently at boot.

- [ ] **Step 1: Write the failing test**

`crates/core/src/maestro/system_workarea.rs` `#[cfg(test)] mod tests` (use the `fresh()` Persistence fixture pattern from `tools/read.rs` tests):
```rust
#[tokio::test]
async fn ensure_is_idempotent_and_returns_sentinel_ids() {
    let (_dir, persist) = fresh().await;
    let (ws1, wa1) = ensure_system_workspace_and_workarea(&persist).await.unwrap();
    let (ws2, wa2) = ensure_system_workspace_and_workarea(&persist).await.unwrap();
    assert_eq!(ws1, ws2);
    assert_eq!(wa1, wa2);
    assert_eq!(ws1.0, SYSTEM_WORKSPACE_ID);
    assert_eq!(wa1.0, SYSTEM_WORKAREA_ID);
    // The workarea row actually exists (FK target is real).
    assert!(concerto_persist::workareas::get(persist.readers(), &wa1).await.unwrap().is_some());
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p concerto-core maestro::system_workarea`
Expected: FAIL — module/functions undefined.

- [ ] **Step 3: Implement ensure**

`crates/core/src/maestro/system_workarea.rs`:
```rust
//! The reserved, UI-hidden system workspace + workarea that hosts the global
//! Maestro session. Satisfies `sessions.workarea_id NOT NULL REFERENCES
//! workareas(id)` without a schema change (design spec Fork B1). The sentinel
//! ids are filtered from every user-facing list (persistence list queries).

use concerto_persist::{Persistence, WorkareaId, WorkspaceId};
use concerto_error::Result;

/// Reserved workspace id hosting the Maestro. Filtered from UI lists.
pub const SYSTEM_WORKSPACE_ID: &str = "__maestro_system__";
/// Reserved workarea id the Maestro session FKs to.
pub const SYSTEM_WORKAREA_ID: &str = "__maestro_system_wa__";

/// Idempotently ensure the reserved system workspace + workarea exist, returning
/// their ids. Safe to call on every boot.
pub async fn ensure_system_workspace_and_workarea(
    persist: &Persistence,
) -> Result<(WorkspaceId, WorkareaId)> {
    let ws_id = WorkspaceId(SYSTEM_WORKSPACE_ID.to_string());
    let wa_id = WorkareaId(SYSTEM_WORKAREA_ID.to_string());

    let mut conn = persist.writer().await?; // match the existing writer-conn accessor
    if concerto_persist::workspaces::get(persist.readers(), &ws_id).await?.is_none() {
        concerto_persist::workspaces::insert(&mut conn, NewWorkspace {
            id: Some(ws_id.clone()),
            name: "Maestro (system)".to_string(),
            // …remaining NewWorkspace fields with inert defaults (no repos, archived_at = None)
        }).await?;
    }
    if concerto_persist::workareas::get(persist.readers(), &wa_id).await?.is_none() {
        concerto_persist::workareas::insert(&mut conn, NewWorkarea {
            id: Some(wa_id.clone()),
            workspace_id: ws_id.clone(),
            composer_name: "__maestro__".to_string(),
            branch_name: "__maestro__".to_string(),
            worktree_root: maestro_scratch_dir()?, // the scratch dir; never a repo worktree
            status: /* the workarea's idle/system status enum value */,
            // …remaining NewWorkarea fields with inert defaults
        }).await?;
    }
    Ok((ws_id, wa_id))
}
```
**Executor note:** open `crates/persist/src/workspaces.rs` (`NewWorkspace`) and `workareas.rs` (`NewWorkarea`, the `status` enum) to fill the exact struct fields. `insert` takes `&mut SqliteConnection`; use the same writer-connection accessor the rest of `core` uses (grep `persist.writer(` / `acquire_writer` in `crates/core/src`). If `NewWorkspace.id`/`NewWorkarea.id` are not `Option`, use the variant that accepts an explicit id, or set the id field directly.

In `crates/core/src/maestro/mod.rs` add `pub mod system_workarea;` in its own region and re-export the two consts + the fn.

- [ ] **Step 4: Run, verify it passes**

Run: `cargo test -p concerto-core maestro::system_workarea`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/maestro/system_workarea.rs crates/core/src/maestro/mod.rs
git commit -m "feat(maestro): reserved hidden system workspace+workarea (FK host)"
```

---

## Task 4: Filter the sentinel from UI list queries

**Files:**
- Modify: `crates/persist/src/workspaces.rs` (`list_all`)
- Modify: `crates/persist/src/workareas.rs` (`list_by_workspace` / list_all if any)
- Test: in-module `#[cfg(test)]` in each

The reserved workspace/workarea must never appear in user-facing lists (workspace pane, the Maestro's own `list_workspaces` read tool may include or exclude it — exclude, since it is not a user workspace).

- [ ] **Step 1: Write the failing test** (workspaces)

In `crates/persist/src/workspaces.rs` tests: insert a normal workspace + one with id `__maestro_system__`; assert `list_all` returns only the normal one.
```rust
#[tokio::test]
async fn list_all_excludes_the_maestro_system_workspace() {
    let (_d, pool) = fresh_pool().await; // match existing persist test fixture
    insert_ws(&pool, "ws-real", "Real").await;
    insert_ws(&pool, "__maestro_system__", "Maestro (system)").await;
    let all = list_all(&pool).await.unwrap();
    assert!(all.iter().all(|w| w.id.0 != "__maestro_system__"));
    assert_eq!(all.len(), 1);
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p concerto-persist workspaces::tests::list_all_excludes`
Expected: FAIL — sentinel still returned.

- [ ] **Step 3: Add the exclusion**

In `list_all`'s SQL add `WHERE id <> '__maestro_system__'` (and preserve existing `archived_at IS NULL` etc with `AND`). Define the literal as a shared `pub const` (import the one from `maestro::system_workarea` is a layering inversion — persist must not depend on core; instead define `pub const MAESTRO_SYSTEM_WORKSPACE_ID: &str` in `concerto_persist` and have `core`'s `system_workarea.rs` re-export *that*). Update `core`'s consts in Task 3 to reference the persist-defined literal to keep them in sync.
Repeat for `workareas::list_by_workspace` excluding `__maestro_system_wa__` (define `MAESTRO_SYSTEM_WORKAREA_ID` in persist too).

- [ ] **Step 4: Run, verify it passes**

Run: `cargo test -p concerto-persist workspaces:: workareas::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/persist/src/workspaces.rs crates/persist/src/workareas.rs crates/core/src/maestro/system_workarea.rs
git commit -m "feat(maestro): exclude reserved system ids from list queries"
```

---

## Task 5: Serve the MCP server over a dedicated UDS listener

**Files:**
- Modify: `crates/core/src/maestro/mcp.rs`
- Test: `crates/core/src/maestro/mcp.rs` `#[cfg(test)]` (real UnixListener round-trip)

`serve_maestro_mcp` already serves over any `AsyncRwTransport`. Add a listener that binds the dedicated socket (0600), accepts connections, and serves a fresh `MaestroMcpServer` over each. Critically, give `MaestroMcpServer` the read-tool Core handles and route `call_tool` to `dispatch_read` (seam 1b).

- [ ] **Step 1: Write the failing test** (server returns live read data, not unimplemented)

Add to `mcp.rs` tests: build a `MaestroMcpServer` with a real `Persistence` (the `fresh()` fixture) seeded with one workspace, serve it over an in-memory `AsyncRwTransport` pair (mirror the existing test at line ~237), drive a client `call_tool("list_workspaces", {})`, and assert the result is a success containing the seeded workspace — NOT a typed-unimplemented error.
```rust
#[tokio::test]
async fn call_tool_list_workspaces_returns_live_data() {
    let (_dir, persist) = fresh_with_one_workspace().await;
    let cache = Arc::new(Mutex::new(SummaryCache::with_system_clock()));
    let server = MaestroMcpServer::with_read_handles(Arc::new(persist), cache, fixed_clock());
    // serve over an in-memory duplex, connect a client, call list_workspaces…
    let result = client.call_tool("list_workspaces", json!({})).await.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    // result content mentions the seeded workspace id
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p concerto-core maestro::mcp::tests::call_tool_list_workspaces`
Expected: FAIL — `with_read_handles` undefined; `call_tool` still hits the handle-less stub.

- [ ] **Step 3: Give the server read handles + route `call_tool` to `dispatch_read`**

In `mcp.rs`, replace the empty `MaestroMcpServer` fields:
```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use concerto_persist::Persistence;
use crate::maestro::summary::SummaryCache;
use crate::maestro::tools::read::dispatch_read;
use crate::maestro::tools::{self, ToolKind};

#[derive(Clone)]
pub struct MaestroMcpServer {
    handles: Option<ReadHandles>,
}

#[derive(Clone)]
struct ReadHandles {
    persist: Arc<Persistence>,
    cache: Arc<Mutex<SummaryCache>>,
    clock: crate::maestro::summary::Clock, // or however `now_ms` is sourced; reuse the cache's clock
}

impl Default for MaestroMcpServer {
    fn default() -> Self { Self { handles: None } }
}

impl MaestroMcpServer {
    pub fn new() -> Self { Self::default() }

    /// Construct with the read-tool Core handles wired (the live spine).
    pub fn with_read_handles(persist: Arc<Persistence>, cache: Arc<Mutex<SummaryCache>>, clock: crate::maestro::summary::Clock) -> Self {
        Self { handles: Some(ReadHandles { persist, cache, clock }) }
    }
}
```
Rewrite `call_tool` to be class-aware:
```rust
async fn call_tool(&self, request: CallToolRequestParams, _ctx: RequestContext<RoleServer>) -> Result<CallToolResult, McpError> {
    let class = tools::class_of(&request.name); // add a helper in tools/mod.rs returning Option<ToolKind>
    match class {
        Some(ToolKind::ReadOnly) => {
            let h = self.handles.as_ref().ok_or_else(|| McpError::internal_error("maestro read handles not wired", None))?;
            let now_ms = h.clock.now_ms();
            let cache = h.cache.lock().await;
            let value = dispatch_read(&request.name, request.arguments, &h.persist, &cache, now_ms).await?;
            Ok(value_to_call_result(value))
        }
        // Write + SideChannel: Milestone 2. Keep the frozen typed-unimplemented seam.
        Some(ToolKind::Write) | Some(ToolKind::SideChannel) => self.dispatch_tool(request),
        None => self.dispatch_tool(request), // unknown name → existing typed invalid_params
    }
}
```
Add `value_to_call_result(Value) -> CallToolResult` (wrap the JSON as MCP text/json content — mirror how `dispatch` builds `CallToolResult` today). Add `tools::class_of(name: &str) -> Option<ToolKind>` (look up `all_tools()` by name, return its `class`).

- [ ] **Step 4: Add the listener + accept loop**

```rust
use tokio::net::UnixListener;
use rmcp::transport::async_rw::AsyncRwTransport;

/// Bind `socket` (0600), and for each accepted connection serve a fresh
/// `MaestroMcpServer` (cheap-clone handles) over it. Runs until the returned
/// task is aborted (boot holds the JoinHandle for the Core's lifetime). Each
/// connection is the CLI's bridge dialing in for one MCP session.
pub async fn serve_maestro_mcp_listener(
    socket: std::path::PathBuf,
    template: MaestroMcpServer,
) -> std::io::Result<()> {
    let _ = std::fs::remove_file(&socket); // clear a stale socket from a prior run
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::os::unix::fs::Permissions::from_mode(0o600))?;
    loop {
        let (conn, _) = listener.accept().await?;
        let server = template.clone();
        tokio::spawn(async move {
            let (r, w) = conn.into_split();
            match serve_maestro_mcp(AsyncRwTransport::<RoleServer, _, _>::new(r, w)).await {
                Ok(handle) => { let _ = handle.waiting().await; }
                Err(e) => tracing::warn!(target: "concerto::maestro", error=%e, "maestro mcp serve failed"),
            }
        });
    }
}
```
Update `serve_maestro_mcp` to build the server from a passed-in `MaestroMcpServer` (so the listener supplies the handle-bearing template) instead of `MaestroMcpServer::new()` — change its body to `template.serve(transport)`, or add a sibling `serve_maestro_mcp_with(server, transport)` the listener calls and keep the old signature for the existing tests.

- [ ] **Step 5: Run, verify it passes**

Run: `cargo test -p concerto-core maestro::mcp`
Expected: PASS (the new live test + the existing registration/handshake tests).

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/maestro/mcp.rs crates/core/src/maestro/tools/mod.rs
git commit -m "feat(maestro): wire read-tool handles into MCP server + UDS accept loop"
```

---

## Task 6: Maestro session spawn bound to a `kind='maestro'` chat

**Files:**
- Modify: `crates/core/src/maestro/handle.rs`
- Modify: `crates/core/src/agent_supervisor/actor.rs` (allow `start_session` to bind a pre-created `kind='maestro'` chat — additive)
- Test: `crates/core/src/maestro/handle.rs` `#[cfg(test)]`

`start_session` today creates its own `kind='session'` chat, but `maestro_session_id()` looks up `kind='maestro'`. Provide a Maestro spawn path that creates (or reuses) the singleton `kind='maestro'` chat and binds the session to it.

- [ ] **Step 1: Write the failing test**

In `handle.rs` tests (with the supervisor's existing test harness / a fake agent bin like the `echo` pattern used in supervisor tests): call `spawn_maestro_session`, then assert `maestro_session_id()` resolves to a live session and the backing chat row has `kind='maestro'`.
```rust
#[tokio::test]
async fn spawn_creates_maestro_chat_and_resolvable_session() {
    let h = test_handle_with_echo_agent().await;
    h.spawn_maestro_session().await.unwrap();
    let sid = h.maestro_session_id().await.unwrap();
    let kind: String = sqlx::query_scalar("SELECT c.kind FROM sessions s JOIN chats c ON c.id=s.chat_id WHERE s.id=?")
        .bind(&sid.0).fetch_one(h.inner.persistence.readers()).await.unwrap();
    assert_eq!(kind, "maestro");
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p concerto-core maestro::handle::tests::spawn_creates_maestro_chat`
Expected: FAIL — `spawn_maestro_session` undefined.

- [ ] **Step 3: Implement the spawn path**

In `handle.rs`:
```rust
impl MaestroHandle {
    /// Spawn (or no-op if already live) the long-lived Maestro session: ensure the
    /// singleton `kind='maestro'` chat, then `start_session` bound to it on the
    /// reserved system workarea. Called once at boot; idempotent.
    pub async fn spawn_maestro_session(&self) -> Result<concerto_persist::SessionId> {
        if let Ok(existing) = self.maestro_session_id().await {
            return Ok(existing);
        }
        let chat_id = self.ensure_maestro_chat().await?; // INSERT OR IGNORE a kind='maestro' chat, return its id
        let (_ws, wa_id) = crate::maestro::system_workarea::ensure_system_workspace_and_workarea(&self.inner.persistence).await?;
        let scratch = crate::maestro::ensure_maestro_scratch_dir()?;
        crate::maestro::ensure_maestro_scratch_trusted(&scratch)?;
        let mut req = crate::maestro::maestro_start_request(wa_id, scratch);
        // Bind the session to the pre-created maestro chat (additive supervisor field).
        req.chat_id = Some(chat_id);
        self.inner.supervisor.start_session(req).await
    }
}
```
Add `ensure_maestro_chat()` (a `concerto_persist::chats` insert-or-get of the singleton `kind='maestro'` row; the `chats` CHECK already allows `kind='maestro'` with NULL `session_id`). In `actor.rs`, add an optional `chat_id: Option<ChatId>` to `StartSessionRequest`; when `Some`, bind the session to it instead of creating a fresh `kind='session'` chat (the existing path stays the default when `None`). Update `maestro_start_request` to default `chat_id: None`, and any exhaustive `StartSessionRequest { … }` constructions.

- [ ] **Step 4: Run, verify it passes**

Run: `cargo test -p concerto-core maestro::handle::tests::spawn_creates_maestro_chat`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/maestro/handle.rs crates/core/src/agent_supervisor/actor.rs
git commit -m "feat(maestro): spawn Maestro session bound to a kind='maestro' chat"
```

---

## Task 7: Boot wiring — bind listener, write `.mcp.json`, spawn

**Files:**
- Modify: `crates/core/src/boot.rs` (the maestro block ~995-1010)
- Test: a boot-level integration test under `crates/core/tests/` (or extend an existing boot test) with a fake CLI provider

Tie it together at boot: after building the `MaestroHandle`, ensure the system ws/wa, bind the MCP listener (holding its `JoinHandle`), resolve the bridge bin path, write `.mcp.json`, and spawn the session. All best-effort: a failure degrades the Maestro to inert (logged) and never crashes boot.

- [ ] **Step 1: Write the failing test**

`crates/core/tests/maestro_boot.rs`:
```rust
// Boot the Core with maestro_state.enabled=true and a fake CLI provider (an
// `echo`-style agent bin on PATH). Assert: (a) the maestro-mcp socket exists,
// (b) .mcp.json exists in the scratch dir, (c) a live Maestro session resolves.
#[tokio::test]
async fn boot_spawns_maestro_and_writes_mcp_json() {
    let env = boot_test_core_with_maestro_enabled().await; // helper: temp HOME, fake claude bin
    assert!(env.maestro_mcp_socket().exists());
    assert!(env.scratch_dir().join(".mcp.json").exists());
    assert!(env.core.maestro_session_is_live().await);
}
```

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p concerto-core --test maestro_boot`
Expected: FAIL — boot does none of this yet.

- [ ] **Step 3: Implement the boot wiring**

In `boot.rs`, in the `Some(...)` arm after constructing the handle, bind the handle to a local and add (guarded by `#[cfg(unix)]`, matching the maestro module gating):
```rust
let handle = crate::maestro::MaestroHandle::new(/* …existing args… */);

// Live spine (design spec C1): best-effort; any failure → inert + logged.
let socket = crate::maestro::maestro_mcp_socket_path();
if let Ok(socket) = socket.as_ref() {
    if let Some(parent) = socket.parent() { let _ = std::fs::create_dir_all(parent); }
    let template = {
        let cache = Arc::clone(&summary_cache);
        crate::maestro::mcp::MaestroMcpServer::with_read_handles(Arc::clone(&persistence), cache, /* clock */)
    };
    let listen_socket = socket.clone();
    // Hold the JoinHandle for the Core's lifetime (store alongside other boot tasks).
    let _mcp_listener = tokio::spawn(async move {
        if let Err(e) = crate::maestro::mcp::serve_maestro_mcp_listener(listen_socket, template).await {
            tracing::warn!(target: "concerto::maestro", error=%e, "maestro mcp listener exited");
        }
    });

    // .mcp.json: bridge bin sits next to the resolved agent-host bin.
    if let (Ok(scratch), Some(bridge)) = (crate::maestro::ensure_maestro_scratch_dir(), resolve_bridge_bin(/* agent-host path / target dir */)) {
        let _ = crate::maestro::write_maestro_mcp_json(&scratch, &bridge, socket);
    }

    // Spawn the long-lived session (idempotent). Inert on provider-unconfigured.
    match handle.spawn_maestro_session().await {
        Ok(sid) => tracing::info!(target: "concerto::maestro", session=%sid.0, "maestro session live"),
        Err(e) => tracing::warn!(target: "concerto::maestro", error=%e, "maestro session not spawned (inert)"),
    }
}

Some(handle)
```
Add `resolve_bridge_bin`: the bridge bin is built into the same dir as `concerto-agent-host`; reuse boot's existing agent-host-path resolution (grep boot for where `agent_bin`/agent-host path is resolved) and swap the file name to `concerto-maestro-bridge`. The `JoinHandle` should be stored in the same structure boot uses to keep supervised tasks alive (so it isn't dropped); follow the pattern of the other `tokio::spawn`s in boot. Source the `clock` the same way the summary cache does (`with_system_clock`).

- [ ] **Step 4: Run, verify it passes**

Run: `cargo test -p concerto-core --test maestro_boot`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/boot.rs crates/core/tests/maestro_boot.rs
git commit -m "feat(maestro): boot wires MCP listener, .mcp.json, and session spawn"
```

---

## Task 8: `workspace_id` scope hint on the wire

**Files:**
- Modify: `crates/proto/proto/concerto/v1/maestro.proto`
- Modify: `crates/core/src/handlers/maestro.rs`
- Modify: `crates/core/src/maestro/handle.rs` (routing uses the hint, falls back to default)
- Test: `handle.rs` `#[cfg(test)]`

- [ ] **Step 1: Add the proto field**

In `maestro.proto`:
```proto
message MaestroMessageRequest {
  string text = 1;
  repeated MaestroAttachment attachments = 2;
  optional string workspace_id = 3;  // scope hint: resolve bare @composer here first
}
```
Run the proto build (`cargo build -p concerto-proto`) to regenerate.

- [ ] **Step 2: Write the failing routing-scope test**

In `handle.rs` tests: two workspaces each with a composer; `@<name>` present only in workspace B; assert that with `workspace_id = Some(B)` routing resolves in B, and with `Some(A)` (where the name is absent) it returns the existing `NotFound`/`NoSuchWorkarea` (not a silent cross-workspace match).
```rust
#[tokio::test]
async fn routing_honors_workspace_hint() {
    let h = handle_with_two_workspaces().await; // A: composer "alpha"; B: composer "beta"
    let out = h.send_to_maestro("@beta hi", Some(workspace_b())).await.unwrap();
    assert!(matches!(out, SendOutcome::Routed { .. }));
    let err = h.send_to_maestro("@beta hi", Some(workspace_a())).await.unwrap_err();
    assert!(format!("{err:?}").contains("NoSuchWorkarea"));
}
```

- [ ] **Step 3: Run, verify it fails**

Run: `cargo test -p concerto-core maestro::handle::tests::routing_honors_workspace_hint`
Expected: FAIL — the send entrypoint ignores `workspace_id`.

- [ ] **Step 4: Thread the hint**

Change the handle's send/route entrypoint to take `workspace_id: Option<WorkspaceId>`; in routing, use it as the scope; when `None`, fall back to `default_workspace_id()`. In `handlers/maestro.rs`, read `req.workspace_id` (the new proto field, `Option<String>` → `WorkspaceId`) and pass it through.

- [ ] **Step 5: Run, verify it passes**

Run: `cargo test -p concerto-core maestro::handle::tests::routing_honors_workspace_hint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/proto/proto/concerto/v1/maestro.proto crates/core/src/handlers/maestro.rs crates/core/src/maestro/handle.rs
git commit -m "feat(maestro): workspace_id scope hint for @composer routing"
```

---

## Task 9: Formalize the desktop shell Maestro.* dispatch + pass workspace_id

**Files:**
- Modify: `apps/desktop/src-tauri/src/rpc.rs`
- Modify: `apps/desktop/src/api/maestro.ts` (+ the composer call site)
- Test: `cargo check -p concerto-desktop --no-default-features`; desktop unit test for the binding if one exists

This formalizes the live, uncommitted fix (seam 6) and adds the `workspace_id` pass-through. The reference for the exact arms is the uncommitted edit in the `wt-verify` worktree (`apps/desktop/src-tauri/src/rpc.rs`).

- [ ] **Step 1: Add the dispatch arms**

In `dispatch_over_channel`, before the `other => NotImplemented` arm, add:
```rust
"Maestro.SendToMaestro" => {
    #[derive(serde::Deserialize)]
    struct P { text: String, #[serde(default)] attachments: Vec<Att>, #[serde(default)] workspace_id: Option<String> }
    #[derive(serde::Deserialize)]
    struct Att { kind: String, #[serde(rename = "ref")] r#ref: String }
    let p: P = serde_json::from_value(payload)?;
    MaestroClient::new(channel)
        .send_to_maestro(MaestroMessageRequest {
            text: p.text,
            attachments: p.attachments.into_iter().map(|a| MaestroAttachment { kind: a.kind, r#ref: a.r#ref }).collect(),
            workspace_id: p.workspace_id,
        })
        .await
        .map(|_| Value::Null)
}
"Maestro.GetDigest" => MaestroClient::new(channel).get_digest(GetDigestRequest {}).await.map(|r| serde_json::to_value(r.into_inner()))?,
"Maestro.GetState" => MaestroClient::new(channel).get_state(GetStateRequest {}).await.map(|r| serde_json::to_value(r.into_inner()))?,
"Maestro.SetWorkareaVisibility" => { /* reads {workarea_id, visibility:i64} → VisibilityRequest */ }
```
Add the imports: `use concerto_proto::v1::maestro_client::MaestroClient;` and the request types. (Match the exact `into_inner`/error mapping the surrounding arms use.)

- [ ] **Step 2: Pass `workspace_id` from the frontend**

In `apps/desktop/src/api/maestro.ts`, add `workspaceId?: string` to the `sendToMaestro` args and include it (as `workspace_id`) in the payload; update the composer to pass the active workspace id (the same id the workspace pane is showing).

- [ ] **Step 3: Verify it compiles + binds**

Run: `cargo check -p concerto-desktop --no-default-features`
Expected: OK.
Run the desktop unit suite: `pnpm --filter desktop test` (if maestro bindings are covered).
Expected: PASS.

- [ ] **Step 4: Filter the sentinel workspace in the UI**

Wherever the workspace list is rendered, drop any workspace whose id is `__maestro_system__` (belt-and-suspenders with the persistence filter from Task 4).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/rpc.rs apps/desktop/src/api/maestro.ts apps/desktop/src
git commit -m "feat(desktop): formalize Maestro.* dispatch + pass workspace_id scope"
```

---

## Task 10: End-to-end test (the missing gate) + full suite

**Files:**
- Create/extend: `crates/core/tests/maestro_e2e.rs`

The test Phase 4 lacked: drive `SendToMaestro` through the handle against a live (fake-CLI) session and assert (a) a freeform turn reaches the session, and (b) a read-tool call over the live MCP socket returns live Core data. Uses a scripted fake CLI provider so CI needs no real Claude binary.

- [ ] **Step 1: Write the e2e test**

```rust
// 1. Boot a Core with maestro enabled + a scripted fake agent that, on receiving
//    input, performs an MCP list_workspaces over the .mcp.json socket and prints
//    the result. 2. Send a freeform message. 3. Assert the agent saw input and
//    the read tool returned the seeded workspace (proving server→dispatch_read).
#[tokio::test]
async fn maestro_live_loop_freeform_and_read_tool() {
    let env = boot_test_core_with_maestro_enabled().await;
    env.seed_workspace("ws-real", "Real").await;
    env.send_to_maestro("what are my workareas doing?", None).await.unwrap();
    // fake agent records: input received + an MCP read-tool round trip succeeded
    let trace = env.fake_agent_trace().await;
    assert!(trace.received_input);
    assert!(trace.read_tool_saw_workspace("ws-real"));
}
```
**Executor note:** the simplest fake agent is the `concerto-maestro-bridge` + a scripted MCP client driven by the test, OR a small test agent bin that links `rmcp` as a client and dials `.mcp.json`'s socket. Reuse the supervisor's existing fake-agent test harness if one exists (grep supervisor tests for `echo`/fake bin).

- [ ] **Step 2: Run, verify it fails, then passes as Tasks 1-9 land**

Run: `cargo test -p concerto-core --test maestro_e2e`
Expected: PASS once the spine is wired.

- [ ] **Step 3: Full gate**

Run: `cargo nextest run --workspace` (the repo's CI runner) and `cargo clippy --workspace --all-targets -- -D warnings` and `pnpm --filter desktop test`.
Expected: all green.

- [ ] **Step 4: Commit**

```bash
git add crates/core/tests/maestro_e2e.rs
git commit -m "test(maestro): end-to-end live loop (freeform + read tool over MCP)"
```

---

## Manual verification (Tier-3, after the suite is green)

Not automatable in CI — the gate that caught these seams. Run the real desktop app + real Claude CLI:

1. `cargo run --bin concerto-core` (Core daemon up; log shows "maestro session live").
2. `pnpm tauri dev` (desktop).
3. In the Maestro composer: `what are my workareas doing?` → a streamed Claude reply that used the read tools (no "no live Maestro session" error).
4. `@<composer> <msg>` for a composer in the **currently-viewed** workspace → routes (no `NoSuchWorkarea`).
5. `/digest` → a digest renders.
6. Confirm the reserved "Maestro (system)" workspace does **not** appear in the workspace pane.

---

## Self-Review

**Spec coverage:** A1 transport → Tasks 1,2,5,7. B1 host workarea → Tasks 3,4,6. C1 spawn-at-boot → Tasks 6,7. D1 workspace_id → Task 8. Seam 1b (read-tool wiring) → Task 5. Seam 6 (rpc.rs) → Task 9. Missing e2e test → Task 10. All spec sections map to tasks.

**Placeholder scan:** Code steps carry real code. Three steps carry explicit **executor notes** where exact struct fields / harness reuse must be read from the codebase during execution (Task 3 `NewWorkspace`/`NewWorkarea` fields, Task 7 `resolve_bridge_bin`/JoinHandle storage, Task 10 fake-agent harness) — these are genuine "open the file and match the local pattern" points, not hidden design decisions.

**Type consistency:** `MaestroMcpServer::with_read_handles` (Task 5) is consumed in Task 7. `spawn_maestro_session` (Task 6) is called in Task 7. `compose_maestro_mcp_json`/`write_maestro_mcp_json`/`maestro_mcp_socket_path` (Task 2) are consumed in Task 7. `ensure_system_workspace_and_workarea` (Task 3) is consumed in Tasks 6,7. `StartSessionRequest.chat_id` (Task 6) is set in Task 6's spawn. `workspace_id` proto field (Task 8) is consumed in Task 9. Sentinel id literals defined in `concerto_persist` (Task 4) and re-exported by `core` (Task 3) — consistent.
