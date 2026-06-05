//! `pair-serve` — the Core side of the two-process, cross-machine Iroh pairing
//! verification (sibling of `tools/split-host-loopback`).
//!
//! Where `split-host-loopback` runs the WHOLE flow in one process on one host
//! with relays disabled (loopback only), `pair-serve` + `pair-dial` split it
//! across two machines and (by default) use relays so it works across a real
//! NAT. `pair-serve`:
//!
//!   1. Boots an Iroh-enabled Core in-process (`boot::start` with
//!      `CONCERTO_ENABLE_IROH=1`, Task 217.5), keychain-backed. The Core's own
//!      endpoint registers with the default relay (cross-machine) unless
//!      `--no-relays` (LAN/loopback validation).
//!   2. Seeds a project -> repo -> workspace -> workarea over the Core's
//!      co-located UDS server (implicit-admin, no cert) so the remote dial side
//!      has a real `workarea_id` to Files into.
//!   3. Arms a pairing (`start_pairing()` -> one-shot token).
//!   4. Builds the **relay-bearing** server `EndpointAddr` (id + relay url +
//!      direct socket addrs) a REMOTE peer can dial through a NAT, and prints a
//!      single-line, greppable `PAIR-BLOB:` connect-blob (base64(JSON)).
//!   5. Stays up (the token TTL is ~60 s; the listener must stay live for the
//!      dial) until SIGINT or the `--ttl` deadline, then shuts down cleanly.
//!
//! # macOS-only at runtime, cross-platform at build
//!
//! Like `split-host-loopback`, the booted Iroh path is keychain-backed (the
//! Core's Ed25519 cert issuer + Noise static), and the `keyring` backend only
//! works on macOS in V1.0. On a keychain-less env `RunningCore::iroh()` is
//! `None`; this bin then prints `pair-serve: iroh-unavailable` and exits 0.

#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use concerto_core::boot::{self, BootOutcome};
use concerto_core::runtime::RuntimeConfig;
use concerto_proto::v1::repositories_client::RepositoriesClient;
use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::workareas_client::WorkareasClient;
use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{
    AddRepoRequest, CloneRequest, CreateWorkareaRequest, CreateWorkspaceRequest,
};
use futures::StreamExt;
use hyper_util::rt::TokioIo;
use iroh::{EndpointAddr, Watcher};
use serde::Serialize;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};

/// How long to wait for the co-located UDS socket to come up after boot.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(20);
/// Per-seed-RPC budget.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long to wait for the Iroh endpoint to learn an address / relay.
const ADDR_TIMEOUT: Duration = Duration::from_secs(30);

/// The connect-blob a remote `pair-dial` decodes. Printed as `PAIR-BLOB: <b64>`
/// where `<b64>` is base64(JSON of this).
#[derive(Serialize)]
struct ConnectBlob {
    /// The server Iroh endpoint id (string form) the client dials.
    endpoint_id: String,
    /// The relay URL the server registered with (string), or null under
    /// `--no-relays`. A remote peer behind a NAT reaches the server via this.
    relay_url: Option<String>,
    /// The server's learned direct socket addresses (string form). Loopback +
    /// LAN addrs; used directly for the `--no-relays` same-host validation.
    direct_addrs: Vec<String>,
    /// The one-shot pairing token (hex) the Noise XX runs over.
    pairing_token: String,
    /// The Core's X25519 Noise static public key (hex) for the IK API channel.
    core_noise_pub: String,
    /// The seeded workarea the dial side Files into.
    workarea_id: String,
    /// The seeded project id.
    project_id: String,
    /// The seeded repo id.
    repo_id: String,
}

struct Args {
    data_dir: PathBuf,
    config_dir: PathBuf,
    bare_repo: String,
    relays: bool,
    ttl: Duration,
}

fn parse_args() -> Result<Args, String> {
    let mut data_dir = None;
    let mut config_dir = None;
    let mut bare_repo = None;
    let mut relays = true;
    let mut ttl = Duration::from_secs(300);
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = Some(PathBuf::from(next(&mut it, "--data-dir")?)),
            "--config-dir" => config_dir = Some(PathBuf::from(next(&mut it, "--config-dir")?)),
            "--bare-repo" => bare_repo = Some(next(&mut it, "--bare-repo")?),
            "--relays" => relays = true,
            "--no-relays" => relays = false,
            "--ttl" => {
                let secs: u64 = next(&mut it, "--ttl")?
                    .parse()
                    .map_err(|e| format!("--ttl must be a number of seconds: {e}"))?;
                ttl = Duration::from_secs(secs);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        data_dir: data_dir.ok_or("missing --data-dir <path>")?,
        config_dir: config_dir.ok_or("missing --config-dir <path>")?,
        bare_repo: bare_repo.ok_or("missing --bare-repo <path>")?,
        relays,
        ttl,
    })
}

fn next(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} requires a value"))
}

fn main() -> std::process::ExitCode {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("pair-serve: build runtime: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    match rt.block_on(run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pair-serve: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;

    // `CONCERTO_ENABLE_IROH=1` is what arms the Core's Iroh listener at boot
    // (Task 217.5). Set it in-process so the operator does not have to.
    std::env::set_var("CONCERTO_ENABLE_IROH", "1");

    let config = RuntimeConfig {
        data_dir: args.data_dir.clone(),
        config_dir: args.config_dir.clone(),
        shutdown_grace: Duration::from_secs(5),
    };
    let core = match boot::start(config)
        .await
        .map_err(|e| format!("boot: {e}"))?
    {
        BootOutcome::Started(c) => c,
        BootOutcome::AlreadyRunning { pid } => {
            return Err(format!("unexpected live instance pid={pid}"));
        }
    };

    // The live Iroh seam. `None` => keychain-less env (Linux/Windows): clean
    // no-op (the Core's Iroh listener degraded to OFF).
    let iroh = match core.iroh() {
        Some(iroh) => iroh,
        None => {
            println!(
                "pair-serve: iroh-unavailable \
                 (RunningCore::iroh() is None — Iroh boot is keychain-backed, macOS-only \
                 until the Linux/Windows keychain backends land)"
            );
            shutdown(core).await?;
            return Ok(());
        }
    };

    let transport: Arc<concerto_transport::IrohTransport> = Arc::clone(&iroh.transport);
    let core_noise_pub = transport.core_noise_public();
    let endpoint = transport.endpoint();

    // --- Seed the chain over the co-located UDS server (implicit admin) -----
    let socket_path = core.socket_path().to_path_buf();
    let chain = seed_chain(&socket_path, &args.data_dir, &args.bare_repo)
        .await
        .map_err(|e| format!("seed chain: {e}"))?;

    // --- Arm a pairing (mints token + opens the 0x03 listener) -------------
    let challenge = iroh
        .pairing_responder
        .start_pairing()
        .map_err(|e| format!("start_pairing: {e}"))?;
    let token = challenge.pairing_token;
    if challenge.lan_endpoint != transport.endpoint_id().to_string() {
        return Err("pairing challenge lan_endpoint != live endpoint id".to_string());
    }

    // --- Build the RELAY-BEARING server EndpointAddr a remote peer can dial -
    // `direct_endpoint_addr` (the transport helper) returns only direct/loopback
    // IPs (no relay) — fine for `--no-relays` same-host, but a NAT'd remote needs
    // the relay url too. So we read the endpoint's OWN full `EndpointAddr`
    // (`watch_addr()`) which carries the relay url AND the learned direct addrs.
    let server_addr = resolve_server_addr(&endpoint, args.relays)
        .await
        .map_err(|e| format!("server addr: {e}"))?;

    let relay_url = server_addr.relay_urls().next().map(|u| u.to_string());
    let direct_addrs: Vec<String> = server_addr.ip_addrs().map(|a| a.to_string()).collect();

    if args.relays && relay_url.is_none() {
        // The cross-machine path NEEDS a relay url for a NAT'd peer to dial. If
        // the endpoint never registered one, fail loud rather than print a blob
        // a remote cannot use.
        return Err(
            "endpoint registered no relay url within budget (cross-machine dial would fail). \
             Try again, or use --no-relays for same-host validation."
                .to_string(),
        );
    }

    let blob = ConnectBlob {
        endpoint_id: transport.endpoint_id().to_string(),
        relay_url,
        direct_addrs,
        pairing_token: hex::encode(token),
        core_noise_pub: hex::encode(core_noise_pub),
        workarea_id: chain.workarea_id,
        project_id: chain.project_id,
        repo_id: chain.repo_id,
    };
    let json = serde_json::to_vec(&blob).map_err(|e| format!("encode blob json: {e}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&json);
    println!("PAIR-BLOB: {encoded}");
    println!(
        "pair-serve: armed (relays={}, ttl={}s) — keep running for the dial",
        if args.relays { "on" } else { "off" },
        args.ttl.as_secs()
    );
    // Flush so the operator's grep sees the blob immediately.
    use std::io::Write as _;
    let _ = std::io::stdout().flush();

    // --- Stay up until SIGINT or the TTL deadline -------------------------
    let ttl = args.ttl;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            println!("pair-serve: SIGINT — shutting down");
        }
        _ = tokio::time::sleep(ttl) => {
            println!("pair-serve: ttl reached ({}s) — shutting down", ttl.as_secs());
        }
    }

    shutdown(core).await?;
    println!("pair-serve: stopped");
    Ok(())
}

/// The seeded chain ids the dial side needs.
struct Chain {
    project_id: String,
    repo_id: String,
    workarea_id: String,
}

/// Seed project -> repo -> (clone) -> workspace -> workarea over the Core's
/// co-located UDS server. The UDS path is kernel-attested implicit admin, so no
/// device cert is needed here.
async fn seed_chain(socket_path: &Path, data_dir: &Path, bare_repo: &str) -> Result<Chain, String> {
    let channel = connect_uds(socket_path).await?;

    // A liveness probe so we fail fast if the Core's gRPC server never bound.
    let mut runtime = RuntimeClient::new(channel.clone());
    timeout_rpc("GetStatus", runtime.get_status(())).await?;

    // No Projects.CreateProject RPC in V1.0 — insert the row directly (same as
    // the split-host-loopback driver).
    let project_id = insert_project(data_dir).await?;

    let mut repos = RepositoriesClient::new(channel.clone());
    let repo_id = timeout_rpc(
        "AddRepository",
        repos.add_repository(AddRepoRequest {
            project_id: project_id.clone(),
            name: "pair-serve-repo".to_string(),
            url: format!("file://{bare_repo}"),
            default_branch: "main".to_string(),
        }),
    )
    .await?
    .into_inner()
    .id;

    // Clone (streaming) — drain to EOS within the step budget.
    {
        let clone_fut = async {
            let resp = RepositoriesClient::<_>::clone(
                &mut repos,
                CloneRequest {
                    repository_id: repo_id.clone(),
                },
            )
            .await
            .map_err(|s| format!("Clone rpc error: {s}"))?;
            let mut stream = resp.into_inner();
            while let Some(item) = stream.next().await {
                item.map_err(|s| format!("Clone stream error: {s}"))?;
            }
            Ok::<(), String>(())
        };
        tokio::time::timeout(STEP_TIMEOUT, clone_fut)
            .await
            .map_err(|_| "Clone stalled".to_string())??;
    }

    let mut ws = WorkspacesClient::new(channel.clone());
    let workspace_id = timeout_rpc(
        "CreateWorkspace",
        ws.create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "pair-serve-ws".to_string(),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: None,
        }),
    )
    .await?
    .into_inner()
    .id;

    let mut wa = WorkareasClient::new(channel);
    let workarea_id = timeout_rpc(
        "CreateWorkarea",
        wa.create_workarea(CreateWorkareaRequest {
            workspace_id,
            permission_mode: None,
        }),
    )
    .await?
    .into_inner()
    .id;

    Ok(Chain {
        project_id,
        repo_id,
        workarea_id,
    })
}

/// Build a Tonic channel over the Core's UDS socket, waiting for the socket to
/// come up (boot returns before the gRPC server binds it).
async fn connect_uds(socket_path: &Path) -> Result<Channel, String> {
    let deadline = tokio::time::Instant::now() + SOCKET_TIMEOUT;
    loop {
        if socket_path.exists() {
            match try_connect_uds(socket_path).await {
                Ok(ch) => return Ok(ch),
                Err(_) if tokio::time::Instant::now() < deadline => {}
                Err(e) => return Err(format!("uds connect: {e}")),
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "core socket {} never became dialable within {SOCKET_TIMEOUT:?}",
                socket_path.display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn try_connect_uds(socket_path: &Path) -> Result<Channel, String> {
    let owned = socket_path.to_path_buf();
    let endpoint = Endpoint::try_from("http://[::1]:50051")
        .map_err(|e| format!("endpoint init: {e}"))?
        .connect_timeout(Duration::from_secs(5));
    endpoint
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = owned.clone();
            async move {
                let stream = UnixStream::connect(&p).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
        .map_err(|e| format!("connect: {e}"))
}

/// Resolve the server's full `EndpointAddr` for a remote dial. When `relays` is
/// set, wait until the endpoint has learned a relay url (so a NAT'd peer can
/// reach it); otherwise wait for a direct/loopback IP. `watch_addr()` carries
/// BOTH the relay url and the learned direct addrs.
async fn resolve_server_addr(
    endpoint: &iroh::Endpoint,
    relays: bool,
) -> Result<EndpointAddr, String> {
    let deadline = tokio::time::Instant::now() + ADDR_TIMEOUT;
    loop {
        let addr = endpoint.watch_addr().get();
        let has_relay = addr.relay_urls().next().is_some();
        let has_ip = addr.ip_addrs().next().is_some();
        // Cross-machine: relay url is the must-have (direct addrs are a bonus
        // for same-LAN hole-punching). Same-host (--no-relays): a direct addr is
        // the must-have.
        let ready = if relays { has_relay || has_ip } else { has_ip };
        if ready && (has_relay || has_ip) {
            // Give the relay a brief extra moment to register when we have an IP
            // but not yet a relay url and relays are requested.
            if relays && !has_relay && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            return Ok(addr);
        }
        if tokio::time::Instant::now() >= deadline {
            if has_relay || has_ip {
                return Ok(addr);
            }
            return Err(format!(
                "endpoint learned neither a relay url nor a socket addr within {ADDR_TIMEOUT:?}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Insert a `projects` row directly (no `Projects.CreateProject` RPC in V1.0;
/// mirrors the split-host-loopback driver). The Core's migrations already ran at
/// boot, so the DB exists; we open it `create_if_missing(false)`.
async fn insert_project(data_dir: &Path) -> Result<String, String> {
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{ConnectOptions, Connection};

    let db_path = data_dir.join("concerto.db");
    let id = uuid::Uuid::now_v7().to_string();
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("clock before epoch: {e}"))?
        .as_millis() as i64;

    let opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(false);
    let mut conn = opts
        .connect()
        .await
        .map_err(|e| format!("open {}: {e}", db_path.display()))?;
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, ?, ?)")
        .bind(&id)
        .bind("pair-serve")
        .bind(created_at)
        .execute(&mut conn)
        .await
        .map_err(|e| format!("insert project: {e}"))?;
    conn.close().await.map_err(|e| format!("close db: {e}"))?;
    Ok(id)
}

/// Trigger an orderly shutdown and wait for the runtime to stop (no leaked
/// endpoint/process).
async fn shutdown(core: concerto_core::boot::RunningCore) -> Result<(), String> {
    let token = core.shutdown_token();
    let join = tokio::spawn(async move { core.run_until_shutdown().await });
    token.cancel();
    tokio::time::timeout(Duration::from_secs(10), join)
        .await
        .map_err(|_| "shutdown stalled".to_string())?
        .map_err(|e| format!("shutdown join: {e}"))?
        .map_err(|e| format!("shutdown: {e}"))
}

/// Await a unary RPC under [`STEP_TIMEOUT`], mapping both the timeout and the
/// gRPC error into a `String`.
async fn timeout_rpc<T>(
    what: &str,
    fut: impl std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>>,
) -> Result<tonic::Response<T>, String> {
    tokio::time::timeout(STEP_TIMEOUT, fut)
        .await
        .map_err(|_| format!("{what} timed out after {STEP_TIMEOUT:?}"))?
        .map_err(|s| format!("{what} rpc error: {s}"))
}
