//! Tier-2 integration tests for Task 204 — the Connect-Web bridge.
//!
//! **Test double:** an in-process **headless gRPC-Web client** (built on
//! `reqwest` over HTTP/1.1, framing protobuf messages in the gRPC-Web wire
//! format) dialing the Core's loopback `tonic-web` bridge on an
//! OS-assigned port. No real browser. This proves the SPA *data path*:
//!
//! - **(a) unary** `Runtime.GetServerCapabilities` returns
//!   `transport_kind == WSS_BRIDGE` — i.e. the Task-201 `ConnTransport`
//!   tag flows through the bridge's interceptor and the handler reports it.
//! - **(b) server-streaming** `Streams.Subscribe(workspace.events)`
//!   delivers a `WorkspaceEvent` after a `Workspaces.CreateWorkspace`.
//! - **(c) unary `AckOffset`** (Task 202) round-trips over gRPC-Web.
//!
//! What this double does NOT cover (→ Phase-2 Tier-3 manual checklist):
//! (1) a **real browser** driving the bridge via Playwright against the
//! actual `apps/web` SPA (Task 519/520); (2) the **remote WSS-via-relay
//! Path B** (Task 215) with browser-side Noise IK and the relay seeing
//! ciphertext only.
//!
//! Unix-only gate: the full in-process Core boot (`boot::start`) brings up
//! the agent-supervisor-backed `Streams`/`Workspaces` services, which are
//! `#[cfg(unix)]` until the Windows supervisor ports land. The *bridge
//! module itself* is cross-platform (Task 113 Windows lane checks that via
//! `cargo check`); this end-to-end test exercises the live streaming
//! surface, which is Unix-only today — same gate as `streams_reconnect.rs`.

#![cfg(unix)]

use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::path::Path;
use std::time::Duration;

use concerto_core::boot::{self, BootOutcome};
use concerto_core::runtime::RuntimeConfig;
use concerto_proto::v1::event::Body;
use concerto_proto::v1::{
    AckOffsetRequest, CreateWorkspaceRequest, Event, ServerCapabilities, SubscribeRequest,
    TransportKind, Workspace,
};
use prost::Message;
use tokio::process::Command;

const WORKSPACE_EVENTS: &str = "workspace.events";

// ---------------------------------------------------------------------------
// gRPC-Web wire helpers.
//
// A gRPC-Web message frame is a 5-byte prefix (1 flag byte + 4-byte
// big-endian length) followed by `length` bytes of payload. The trailers
// frame has the high bit of the flag set (0x80); its payload is the
// HTTP/1.1-style trailer block ("grpc-status: 0\r\n...").
// ---------------------------------------------------------------------------

/// Encode a protobuf message into a single gRPC-Web DATA frame.
fn grpc_web_frame<M: Message>(msg: &M) -> Vec<u8> {
    let body = msg.encode_to_vec();
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(0u8); // flag: data frame, uncompressed
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// A parsed gRPC-Web response: zero+ message payloads and the trailer map.
#[derive(Default)]
struct GrpcWebResponse {
    messages: Vec<Vec<u8>>,
    trailers: std::collections::HashMap<String, String>,
}

/// Split a complete gRPC-Web response body into its DATA frames and the
/// trailing trailer frame. Returns whatever it could parse (used both for
/// unary responses and the fully-buffered server-stream).
fn parse_grpc_web(body: &[u8]) -> GrpcWebResponse {
    let mut out = GrpcWebResponse::default();
    let mut i = 0usize;
    while i + 5 <= body.len() {
        let flag = body[i];
        let len = u32::from_be_bytes([body[i + 1], body[i + 2], body[i + 3], body[i + 4]]) as usize;
        i += 5;
        if i + len > body.len() {
            break;
        }
        let payload = &body[i..i + len];
        i += len;
        if flag & 0x80 != 0 {
            // Trailer frame: parse "key: value" lines.
            let text = String::from_utf8_lossy(payload);
            for line in text.split("\r\n").flat_map(|l| l.split('\n')) {
                if let Some((k, v)) = line.split_once(':') {
                    out.trailers
                        .insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
            }
        } else {
            out.messages.push(payload.to_vec());
        }
    }
    out
}

/// Assert the gRPC-Web call succeeded (status header or trailer is `0`).
fn assert_grpc_ok(http_status: reqwest::StatusCode, resp: &GrpcWebResponse, ctx: &str) {
    assert!(
        http_status.is_success(),
        "{ctx}: HTTP status {http_status} (expected 2xx)"
    );
    if let Some(s) = resp.trailers.get("grpc-status") {
        assert_eq!(s, "0", "{ctx}: grpc-status trailer = {s} (expected 0)");
    }
}

/// Perform a unary gRPC-Web call: POST a single framed request and parse
/// the framed response. `path` is `/<package>.<Service>/<Method>`.
async fn grpc_web_unary<Req: Message, Resp: Message + Default>(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    req: &Req,
) -> Resp {
    let url = format!("{base}{path}");
    let resp = client
        .post(&url)
        .header("content-type", "application/grpc-web+proto")
        .header("accept", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(grpc_web_frame(req))
        .send()
        .await
        .unwrap_or_else(|e| panic!("POST {path}: {e}"));
    let status = resp.status();
    let body = resp.bytes().await.expect("read body");
    let parsed = parse_grpc_web(&body);
    assert_grpc_ok(status, &parsed, path);
    let msg = parsed
        .messages
        .first()
        .unwrap_or_else(|| panic!("{path}: no message frame in response"));
    Resp::decode(&msg[..]).unwrap_or_else(|e| panic!("{path}: decode: {e}"))
}

/// Pick a free loopback port by binding `:0`, reading the port, and
/// dropping the listener so the bridge can rebind it. Inherently a small
/// TOCTOU window, acceptable for a single serial test.
fn free_loopback_addr() -> SocketAddr {
    let l = StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let addr = l.local_addr().expect("local_addr");
    drop(l);
    addr
}

async fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: stderr={}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Seed a project + one repository directly in SQLite (mirrors
/// `streams_reconnect.rs::seed_project_repo`) so `CreateWorkspace` has a
/// valid project + repo to reference. Returns `(project_id, repo_id)`.
async fn seed_project_repo(db_path: &Path, repos_root: &Path, slug: &str) -> (String, String) {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use tempfile::TempDir;

    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
    let bare_url = format!("file://{}", bare.path().display());
    git(&["remote", "add", "origin", bare_url.as_str()], work.path()).await;
    git(&["push", "-u", "origin", "main"], work.path()).await;

    let project_id = format!("proj-{slug}");
    let repo_id = format!("repo-{slug}");
    let local_path = repos_root.join(&repo_id);

    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open db write pool");
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, 'test', 0)")
        .bind(&project_id)
        .execute(&pool)
        .await
        .expect("insert project");
    sqlx::query(
        "INSERT INTO repositories (id, project_id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, ?, 'full', 'main')",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(format!("name-{slug}"))
    .bind(&bare_url)
    .bind(local_path.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .expect("insert repository");
    pool.close().await;

    std::mem::forget(bare);
    std::mem::forget(work);
    (project_id, repo_id)
}

/// Boot a full in-process Core with the Connect-Web bridge bound to
/// `bridge_addr`. Returns the running core + its data dir.
async fn boot_with_bridge(bridge_addr: SocketAddr) -> (boot::RunningCore, std::path::PathBuf) {
    // The bridge config is read from the process env by the api-server
    // actor. This test is the sole owner of these vars (one serial test).
    std::env::set_var("CONCERTO_CONNECT_BRIDGE", "1");
    std::env::set_var("CONCERTO_CONNECT_BRIDGE_ADDR", bridge_addr.to_string());
    // Isolate the Core-identity keychain access (Task 206 establishes the
    // identity in `boot::start`) to a unique throwaway service, so this test
    // binary only ever touches an item it created — otherwise a headless
    // macOS CI runner blocks forever on a Keychain Access prompt.
    std::env::set_var(
        "CONCERTO_KEYCHAIN_SERVICE",
        format!("concerto-test-{}-cwb", std::process::id()),
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    // Leak the tempdir so the DB/worktrees outlive this fn; the process is
    // short-lived and the OS reclaims on exit.
    let root = tmp.keep();
    let data_dir = root.join("data");
    let config_dir = root.join("config");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();

    let config = RuntimeConfig {
        data_dir: data_dir.clone(),
        config_dir: config_dir.clone(),
        shutdown_grace: Duration::from_secs(5),
    };
    let core = match boot::start(config).await.expect("boot::start") {
        BootOutcome::Started(c) => c,
        BootOutcome::AlreadyRunning { pid } => panic!("unexpected live instance pid={pid}"),
    };
    (core, data_dir)
}

/// One serial end-to-end test covering all three required surfaces over
/// the gRPC-Web bridge. Single test because the bridge config is read from
/// process-global env; running serially avoids cross-test env races.
#[tokio::test(flavor = "multi_thread")]
async fn connect_web_bridge_serves_grpc_web() {
    let bridge_addr = free_loopback_addr();
    let (core, data_dir) = boot_with_bridge(bridge_addr).await;
    let base = format!("http://{bridge_addr}");

    // Bound every HTTP call so a stalled request on a loaded CI runner fails
    // fast (a rerunnable error) instead of hanging the whole test forever.
    // The unary gRPC-Web calls below have no other timeout of their own; the
    // server-streaming read is consumed and dropped within its own 10s
    // deadline, well under this 30s ceiling, so it is never cut short in the
    // happy path. (Without this, an intermittent macOS-runner stall in a
    // unary `.send()/.bytes()` blocked indefinitely — there is no GUI/CI
    // watchdog to kill it.)
    let http = reqwest::Client::builder()
        .http1_only()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    // Wait for the bridge to accept connections (it binds inside the
    // supervised actor shortly after boot returns).
    let ready = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if tokio::net::TcpStream::connect(bridge_addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(ready.is_ok(), "bridge should accept TCP shortly after boot");

    // ---- (a) unary: GetServerCapabilities → transport_kind == WSS_BRIDGE.
    let caps: ServerCapabilities = grpc_web_unary(
        &http,
        &base,
        "/concerto.v1.Runtime/GetServerCapabilities",
        &(),
    )
    .await;
    assert_eq!(
        caps.transport_kind,
        TransportKind::WssBridge as i32,
        "bridge connection must report WSS_BRIDGE (Task 201 seam through the bridge interceptor)"
    );
    assert_eq!(caps.schema_version, "concerto.v1");
    assert_eq!(caps.core_host_os, std::env::consts::OS);

    // Seed a project + repo so CreateWorkspace has something to reference.
    let db_path = data_dir.join("concerto.db");
    let repos_root = data_dir.join("repos");
    let (project_id, repo_id) = seed_project_repo(&db_path, &repos_root, "cw-bridge").await;

    // ---- (b) server-streaming: subscribe to workspace.events, then create
    // a workspace over gRPC-Web and read the streamed event.
    //
    // Open the streaming POST first so the subscription is live before the
    // event is emitted (no since_offset → live-from-head).
    let sub = SubscribeRequest {
        subject: WORKSPACE_EVENTS.to_string(),
        filter: None,
        since_offset: None,
    };
    let stream_resp = http
        .post(format!("{base}/concerto.v1.Streams/Subscribe"))
        .header("content-type", "application/grpc-web+proto")
        .header("accept", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(grpc_web_frame(&sub))
        .send()
        .await
        .expect("subscribe POST");
    assert!(stream_resp.status().is_success(), "subscribe HTTP status");
    // Drive the stream body on a task: read framed bytes until we see a
    // workspace event frame or time out.
    let stream_task = tokio::spawn(async move {
        use futures::StreamExt;
        let mut byte_stream = stream_resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            // Try to parse a complete workspace event out of whatever we
            // have buffered so far.
            let parsed = parse_grpc_web(&buf);
            for m in &parsed.messages {
                if let Ok(ev) = Event::decode(&m[..]) {
                    if matches!(ev.body, Some(Body::Workspace(_))) {
                        return Some(ev);
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            match tokio::time::timeout(Duration::from_millis(500), byte_stream.next()).await {
                Ok(Some(Ok(chunk))) => buf.extend_from_slice(&chunk),
                Ok(Some(Err(_))) | Ok(None) => {
                    // Stream ended; one last parse attempt then give up.
                    let parsed = parse_grpc_web(&buf);
                    for m in &parsed.messages {
                        if let Ok(ev) = Event::decode(&m[..]) {
                            if matches!(ev.body, Some(Body::Workspace(_))) {
                                return Some(ev);
                            }
                        }
                    }
                    return None;
                }
                Err(_) => { /* tick: re-loop to re-check the deadline */ }
            }
        }
    });

    // Give the subscription a beat to attach before emitting the event.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let created: Workspace = grpc_web_unary(
        &http,
        &base,
        "/concerto.v1.Workspaces/CreateWorkspace",
        &CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "bridge-ws".to_string(),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: None,
        },
    )
    .await;
    assert!(
        !created.id.is_empty(),
        "CreateWorkspace over gRPC-Web should return an id"
    );

    let streamed = stream_task.await.expect("stream task join");
    let ev = streamed.expect("a workspace.events frame should arrive over gRPC-Web");
    match ev.body {
        Some(Body::Workspace(w)) => assert_eq!(w.kind, "created"),
        other => panic!("expected workspace event, got {other:?}"),
    }

    // ---- (c) unary AckOffset (Task 202) round-trips over gRPC-Web. The
    // first workspace.events offset is 0; acking it is a well-formed,
    // in-range ack and must return Empty (grpc-status 0).
    let _empty: () = grpc_web_unary(
        &http,
        &base,
        "/concerto.v1.Streams/AckOffset",
        &AckOffsetRequest {
            subject: WORKSPACE_EVENTS.to_string(),
            offset: 0,
        },
    )
    .await;

    // Clean shutdown.
    let token = core.shutdown_token();
    let join = tokio::spawn(async move { core.run_until_shutdown().await });
    token.cancel();
    let res = tokio::time::timeout(Duration::from_secs(10), join).await;
    assert!(res.is_ok(), "run_until_shutdown should return after cancel");
    res.unwrap().expect("join").expect("clean shutdown");

    std::env::remove_var("CONCERTO_CONNECT_BRIDGE");
    std::env::remove_var("CONCERTO_CONNECT_BRIDGE_ADDR");
}
