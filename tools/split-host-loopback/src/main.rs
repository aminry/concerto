//! `split-host-loopback` — the Tier-2 capstone driver for the Phase-2
//! transport spine (Task 220).
//!
//! It brings up **two Iroh endpoints on one host with relays disabled** (the
//! spike's direct-loopback model — no NAT, no relay, no network) and drives the
//! full remote-client path over the Iroh transport + Noise IK, in one process:
//!
//!   1. **Boot** an Iroh-enabled Core in-process (`boot::start` with
//!      `CONCERTO_ENABLE_IROH=1`, Task 217.5), keychain-isolated.
//!   2. **Pair** a synthetic device over the real `0x03` pairing channel —
//!      Noise XX over the one-shot token, then the length-prefixed
//!      `PairingRequest` frame — and receive a `SignedDeviceCert`.
//!   3. Over the authenticated Iroh + Noise IK API channel, presenting the
//!      device cert in metadata, run three steps:
//!      - **unary** — `Runtime.GetServerCapabilities`, assert
//!        `transport_kind == IROH` (the Task-201 per-connection tag fired on
//!        the Iroh listener);
//!      - **stream** — `Streams.Subscribe(workspace.events)`, then create a
//!        workspace and assert the event frame is captured;
//!      - **Files** — `Files.Upload` a fixture blob into the workarea's
//!        `.context/`, `Files.Download` it back, and assert byte-identical +
//!        matching BLAKE2b-256.
//!   4. **Tear down** the Core (and its Iroh endpoint) cleanly via the shutdown
//!      token — no leaked endpoint/process.
//!
//! The chain that produces the workarea (project → repo → workspace →
//! workarea) is set up over the **same authenticated Iroh channel** from a
//! caller-seeded local bare repo (`--bare-repo`, `file://`, no network), so the
//! whole flow exercises the Iroh RPC surface end to end.
//!
//! # macOS-only at runtime, cross-platform at build
//!
//! This bin BUILDS on every lane (nothing is `#[cfg]`-gated). But the booted
//! Iroh path is **keychain-backed** (the Core's Ed25519 cert issuer + its Noise
//! static), and the `keyring` backend only works on macOS in V1.0 (Task 217.5
//! Handoff). On a keychain-less env (Linux/Windows CI) `RunningCore::iroh()`
//! degrades to `None`; this bin then prints `split-host-loopback:
//! iroh-unavailable` and exits 0 (a clean no-op). The smoke wrapper
//! (`scripts/smoke.d/94-split-host-loopback.sh`) additionally skips cleanly on
//! non-macOS so the ubuntu lane never even builds + boots a Core for nothing.
//!
//! What this double does **NOT** cover (→ Phase-2 Tier-3 manual checklist,
//! `design/11 §10`): real cross-machine split-host, real NAT diversity /
//! direct-connection %, relay fallback, Wi-Fi↔LTE migration, throughput-vs-UDS.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use concerto_core::boot::{self, BootOutcome};
use concerto_core::runtime::RuntimeConfig;
use concerto_core::security::auth::{encode_cert_metadata, DEVICE_CERT_METADATA_KEY};
use concerto_identity::{KeyPair, NoiseHandshake, NoiseStatic};
use concerto_proto::v1::event::Body as EventBody;
use concerto_proto::v1::files_client::FilesClient;
use concerto_proto::v1::repositories_client::RepositoriesClient;
use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::streams_client::StreamsClient;
use concerto_proto::v1::upload_chunk::Body as UploadBody;
use concerto_proto::v1::workareas_client::WorkareasClient;
use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{
    AddRepoRequest, CloneRequest, CreateWorkareaRequest, CreateWorkspaceRequest, DownloadRequest,
    SubscribeRequest, TransportKind, UploadChunk, UploadFinalize, UploadHeader,
};
use concerto_transport::api::write_channel_tag;
use concerto_transport::{
    connect_channel, direct_endpoint_addr, ChannelTag, IrohDuplex, IrohTransport, ALPN,
};
use futures::StreamExt;
use iroh::EndpointAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tonic::transport::Channel;

type Blake2b256 = Blake2b<U32>;

/// Per-step wall-clock budget. Generous so an unattended CI runner (the
/// `--ci-mode` invocation) under load does not flake; short enough to fail fast
/// when the Iroh path is wedged.
const STEP_TIMEOUT: Duration = Duration::from_secs(20);
/// Cap the whole pairing exchange (Noise XX + request/cert) separately.
const PAIR_TIMEOUT: Duration = Duration::from_secs(20);
/// Files fixture: ~450 KiB so the upload spans multiple ≤256 KiB frames.
const FILE_REL_PATH: &str = "split-host-loopback.bin";
const FILE_CHUNK: usize = 200 * 1024;

fn blake2b_256(bytes: &[u8]) -> Vec<u8> {
    let mut h = Blake2b256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

/// 4-byte-BE length + body — the `0x03`-channel framing the Core's pairing
/// responder locks (Task 217.5).
async fn write_frame(duplex: &mut IrohDuplex, bytes: &[u8]) -> Result<(), String> {
    duplex
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(|e| format!("pair: write len: {e}"))?;
    duplex
        .write_all(bytes)
        .await
        .map_err(|e| format!("pair: write body: {e}"))?;
    duplex
        .flush()
        .await
        .map_err(|e| format!("pair: flush: {e}"))?;
    Ok(())
}

async fn read_frame(duplex: &mut IrohDuplex) -> Result<Vec<u8>, String> {
    let mut len = [0u8; 4];
    duplex
        .read_exact(&mut len)
        .await
        .map_err(|e| format!("pair: read len: {e}"))?;
    let n = u32::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    duplex
        .read_exact(&mut buf)
        .await
        .map_err(|e| format!("pair: read body: {e}"))?;
    Ok(buf)
}

/// `device_pubkey(32) || nonce(32) || signature(64) || device_name(utf8)` —
/// the encrypted `PairingRequest` the Core decodes (Task 217.5 framing).
fn encode_pairing_request(
    device_pubkey: &[u8; 32],
    nonce: &[u8; 32],
    signature: &[u8; 64],
    device_name: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + device_name.len());
    out.extend_from_slice(device_pubkey);
    out.extend_from_slice(nonce);
    out.extend_from_slice(signature);
    out.extend_from_slice(device_name.as_bytes());
    out
}

struct Args {
    data_dir: PathBuf,
    config_dir: PathBuf,
    bare_repo: String,
}

fn parse_args() -> Result<Args, String> {
    let mut data_dir = None;
    let mut config_dir = None;
    let mut bare_repo = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => data_dir = Some(PathBuf::from(next(&mut it, "--data-dir")?)),
            "--config-dir" => config_dir = Some(PathBuf::from(next(&mut it, "--config-dir")?)),
            "--bare-repo" => bare_repo = Some(next(&mut it, "--bare-repo")?),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        data_dir: data_dir.ok_or("missing --data-dir <path>")?,
        config_dir: config_dir.ok_or("missing --config-dir <path>")?,
        bare_repo: bare_repo.ok_or("missing --bare-repo <path>")?,
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
            eprintln!("split-host-loopback: build runtime: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    match rt.block_on(run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("split-host-loopback: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;

    // --- Boot an Iroh-enabled Core in-process (Task 217.5 toggle) ----------
    // The smoke wrapper sets CONCERTO_ENABLE_IROH=1 + a unique
    // CONCERTO_KEYCHAIN_SERVICE before launching us.
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

    // The live Iroh seam. `None` ⇒ keychain-less env (Linux/Windows CI): the
    // Iroh listener degraded to OFF (Task 217.5). Clean no-op, exit 0 — the
    // wrapper already skips on non-macOS; this is the belt-and-suspenders.
    let iroh = match core.iroh() {
        Some(iroh) => iroh,
        None => {
            println!(
                "split-host-loopback: iroh-unavailable \
                 (RunningCore::iroh() is None — Iroh boot is keychain-backed, macOS-only \
                 until the Linux/Windows keychain backends land)"
            );
            shutdown(core).await?;
            return Ok(());
        }
    };

    let server_transport: Arc<IrohTransport> = Arc::clone(&iroh.transport);
    let core_noise_pub = server_transport.core_noise_public();
    let server_addr = direct_endpoint_addr(&server_transport.endpoint())
        .await
        .map_err(|e| format!("server iroh addr: {e}"))?;

    // --- Arm a pairing (mints token + opens the 0x03 listener) -------------
    let challenge = iroh
        .pairing_responder
        .start_pairing()
        .map_err(|e| format!("start_pairing: {e}"))?;
    let token = challenge.pairing_token;
    if challenge.lan_endpoint != server_transport.endpoint_id().to_string() {
        return Err("pairing challenge lan_endpoint != live endpoint id".to_string());
    }

    // --- Device endpoint (relay disabled → direct loopback) ----------------
    let client_ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .map_err(|e| format!("client endpoint: {e}"))?;

    // --- Pair over the REAL 0x03 channel → SignedDeviceCert ----------------
    let device_key = KeyPair::from_seed(&[0x42u8; 32]);
    let device_pubkey = device_key.verifying_key().to_bytes();
    let nonce = [0x24u8; 32];
    let signed_cert = match tokio::time::timeout(
        PAIR_TIMEOUT,
        pair_over_iroh(
            &client_ep,
            &server_addr,
            &token,
            &device_key,
            &device_pubkey,
            &nonce,
        ),
    )
    .await
    {
        Ok(res) => res?,
        Err(_) => return Err("pairing stalled (no cert within budget)".to_string()),
    };
    println!(
        "split-host-loopback: paired ({} byte cert)",
        signed_cert.len()
    );

    // --- Authenticated Iroh + Noise IK API channel ------------------------
    let device_static =
        Arc::new(NoiseStatic::generate().map_err(|e| format!("noise static: {e}"))?);
    let channel = connect_channel(&client_ep, server_addr, device_static, core_noise_pub)
        .await
        .map_err(|e| format!("connect api channel: {e}"))?;
    let attach_cert = cert_interceptor(&signed_cert)?;

    // (a) unary — GetServerCapabilities == IROH ----------------------------
    let mut runtime_client = RuntimeClient::with_interceptor(channel.clone(), attach_cert.call());
    let caps = timeout_rpc(
        "GetServerCapabilities",
        runtime_client.get_server_capabilities(()),
    )
    .await?
    .into_inner();
    if caps.transport_kind != TransportKind::Iroh as i32 {
        return Err(format!(
            "unary over Iroh reported transport_kind={} (want IROH={})",
            caps.transport_kind,
            TransportKind::Iroh as i32
        ));
    }
    println!("split-host-loopback: unary GetServerCapabilities == IROH");

    // --- Set up the chain over the Iroh channel ---------------------------
    let project_id = insert_project(&args.data_dir).await?;
    let mut repos_client =
        RepositoriesClient::with_interceptor(channel.clone(), attach_cert.call());
    let repo_id = timeout_rpc(
        "AddRepository",
        repos_client.add_repository(AddRepoRequest {
            project_id: project_id.clone(),
            name: "split-host-repo".to_string(),
            url: format!("file://{}", args.bare_repo),
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
                &mut repos_client,
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

    let mut ws_client = WorkspacesClient::with_interceptor(channel.clone(), attach_cert.call());
    let workspace_id = timeout_rpc(
        "CreateWorkspace",
        ws_client.create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "split-host-ws".to_string(),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: None,
        }),
    )
    .await?
    .into_inner()
    .id;

    let mut wa_client = WorkareasClient::with_interceptor(channel.clone(), attach_cert.call());
    let workarea_id = timeout_rpc(
        "CreateWorkarea",
        wa_client.create_workarea(CreateWorkareaRequest {
            workspace_id: workspace_id.clone(),
            permission_mode: None,
        }),
    )
    .await?
    .into_inner()
    .id;

    // (b) stream — Subscribe(workspace.events) then trigger an event --------
    stream_step(channel.clone(), &attach_cert, &project_id, &repo_id).await?;
    println!("split-host-loopback: stream Streams.Subscribe(workspace.events) captured an event");

    // (c) Files — Upload then Download into the workarea's .context/ --------
    files_step(channel.clone(), &attach_cert, &workarea_id).await?;
    println!("split-host-loopback: Files.Upload/Download round-trip + BLAKE2b-256 verified");

    // --- Clean shutdown (no leaked endpoint) ------------------------------
    shutdown(core).await?;
    println!("split-host-loopback: OK");
    Ok(())
}

/// Run the Noise-XX pairing handshake over the `0x03` channel and return the
/// on-wire signed device cert.
async fn pair_over_iroh(
    client_ep: &iroh::Endpoint,
    server_addr: &EndpointAddr,
    token: &[u8; 32],
    device_key: &KeyPair,
    device_pubkey: &[u8; 32],
    nonce: &[u8; 32],
) -> Result<Vec<u8>, String> {
    let conn = client_ep
        .connect(server_addr.clone(), ALPN)
        .await
        .map_err(|e| format!("pair connect: {e}"))?;
    let (send, recv) = conn
        .open_bi()
        .await
        .map_err(|e| format!("open bidi: {e}"))?;
    let duplex = IrohDuplex::new(send, recv);
    let mut duplex = write_channel_tag(duplex, ChannelTag::Pairing)
        .await
        .map_err(|e| format!("write 0x03 tag: {e}"))?;

    // Noise XX initiator over the one-shot token.
    let mut hs = NoiseHandshake::initiator(token).map_err(|e| format!("xx initiator: {e}"))?;
    let m1 = hs.write_message(&[]).map_err(|e| format!("m1: {e}"))?;
    write_frame(&mut duplex, &m1).await?;
    let m2 = read_frame(&mut duplex).await?;
    hs.read_message(&m2).map_err(|e| format!("read m2: {e}"))?;
    let m3 = hs.write_message(&[]).map_err(|e| format!("m3: {e}"))?;
    write_frame(&mut duplex, &m3).await?;
    let mut noise = hs
        .into_transport()
        .map_err(|e| format!("xx transport: {e}"))?;

    // Sign `token || nonce || device_pubkey`, send the encrypted request.
    let mut payload = Vec::with_capacity(96);
    payload.extend_from_slice(token);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(device_pubkey);
    let signature = device_key.sign(&payload).to_bytes();
    let req = encode_pairing_request(device_pubkey, nonce, &signature, "Split-Host Loopback");
    let ct = noise
        .write_message(&req)
        .map_err(|e| format!("encrypt request: {e}"))?;
    write_frame(&mut duplex, &ct).await?;

    // Read the encrypted signed cert (a refusal would be a single byte).
    let reply_ct = read_frame(&mut duplex).await?;
    let signed_cert = noise
        .read_message(&reply_ct)
        .map_err(|e| format!("decrypt cert reply: {e}"))?;
    if signed_cert.len() <= 1 {
        return Err("pairing refused (single-byte reply, not a cert)".to_string());
    }
    Ok(signed_cert)
}

/// Subscribe to `workspace.events` over Iroh, create a workspace to emit an
/// event, and assert the stream captured at least one frame.
async fn stream_step(
    channel: Channel,
    attach_cert: &CertInterceptorFactory,
    project_id: &str,
    repo_id: &str,
) -> Result<(), String> {
    let mut streams_client = StreamsClient::with_interceptor(channel.clone(), attach_cert.call());
    let sub = timeout_rpc(
        "Subscribe(workspace.events)",
        streams_client.subscribe(SubscribeRequest {
            subject: "workspace.events".to_string(),
            filter: None,
            since_offset: None,
        }),
    )
    .await?;
    let mut stream = sub.into_inner();

    // Trigger exactly one event by creating a workspace on the same channel.
    let mut ws_client = WorkspacesClient::with_interceptor(channel, attach_cert.call());
    let _ = timeout_rpc(
        "CreateWorkspace(stream-trigger)",
        ws_client.create_workspace(CreateWorkspaceRequest {
            project_id: project_id.to_string(),
            name: "split-host-stream-trigger".to_string(),
            repository_ids: vec![repo_id.to_string()],
            permission_mode: None,
            description: None,
        }),
    )
    .await?;

    // Wait for a workspace.events frame (the `created` event).
    let got = tokio::time::timeout(STEP_TIMEOUT, async {
        while let Some(item) = stream.next().await {
            let event = item.map_err(|s| format!("workspace.events stream error: {s}"))?;
            if matches!(event.body, Some(EventBody::Workspace(_))) {
                return Ok::<bool, String>(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|_| "workspace.events stream stalled (no event within budget)".to_string())??;
    if !got {
        return Err("workspace.events stream closed without a Workspace event".to_string());
    }
    Ok(())
}

/// Upload a multi-chunk fixture into the workarea's `.context/`, download it
/// back, and assert byte-identical + matching BLAKE2b-256.
async fn files_step(
    channel: Channel,
    attach_cert: &CertInterceptorFactory,
    workarea_id: &str,
) -> Result<(), String> {
    let mut files = FilesClient::with_interceptor(channel, attach_cert.call());

    let payload: Vec<u8> = (0..450 * 1024).map(|i| (i % 251) as u8).collect();
    let digest = blake2b_256(&payload);

    let mut frames = vec![UploadChunk {
        body: Some(UploadBody::Header(UploadHeader {
            workarea_id: workarea_id.to_string(),
            repository_id: None,
            relative_path: FILE_REL_PATH.to_string(),
            expected_size: payload.len() as u64,
            content_type: "application/octet-stream".to_string(),
        })),
    }];
    for piece in payload.chunks(FILE_CHUNK) {
        frames.push(UploadChunk {
            body: Some(UploadBody::Data(piece.to_vec())),
        });
    }
    frames.push(UploadChunk {
        body: Some(UploadBody::Finalize(UploadFinalize {
            blake2b: digest.clone(),
        })),
    });

    let uploaded = timeout_rpc("Files.Upload", files.upload(futures::stream::iter(frames)))
        .await?
        .into_inner();
    if uploaded.size != payload.len() as u64 {
        return Err(format!(
            "Upload reported size {} but payload was {} bytes",
            uploaded.size,
            payload.len()
        ));
    }

    let resp = timeout_rpc(
        "Files.Download",
        files.download(DownloadRequest {
            workarea_id: workarea_id.to_string(),
            repository_id: None,
            relative_path: FILE_REL_PATH.to_string(),
            offset: None,
            length: None,
        }),
    )
    .await?;
    let mut stream = resp.into_inner();
    let mut downloaded = Vec::with_capacity(payload.len());
    let drain = tokio::time::timeout(STEP_TIMEOUT, async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|s| format!("Download stream error: {s}"))?;
            downloaded.extend_from_slice(&chunk.data);
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "Download stalled".to_string());
    drain??;

    if downloaded != payload {
        return Err(format!(
            "Files round-trip mismatch: downloaded {} bytes, expected {}",
            downloaded.len(),
            payload.len()
        ));
    }
    if blake2b_256(&downloaded) != digest {
        return Err("Files round-trip BLAKE2b-256 mismatch".to_string());
    }
    Ok(())
}

/// Insert a `projects` row directly (no `Projects.CreateProject` RPC in V1.0;
/// mirrors `smoke-client add-project`). The Core's migrations already ran at
/// boot, so the DB exists; we open it `create_if_missing(false)`.
async fn insert_project(data_dir: &std::path::Path) -> Result<String, String> {
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
        .bind("split-host")
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

/// Build a cert-attaching interceptor factory. Each [`CertInterceptorFactory::call`]
/// hands out a fresh `FnMut` interceptor (one per gRPC client) while sharing the
/// immutable parsed cert metadata value.
fn cert_interceptor(signed_cert: &[u8]) -> Result<CertInterceptorFactory, String> {
    let value: tonic::metadata::MetadataValue<tonic::metadata::Ascii> =
        encode_cert_metadata(signed_cert)
            .parse()
            .map_err(|e| format!("cert metadata: {e}"))?;
    Ok(CertInterceptorFactory { value })
}

/// A cheap factory so each gRPC client gets a fresh `FnMut` interceptor while
/// sharing the (immutable) parsed cert value.
#[derive(Clone)]
struct CertInterceptorFactory {
    value: tonic::metadata::MetadataValue<tonic::metadata::Ascii>,
}

impl CertInterceptorFactory {
    fn call(
        &self,
    ) -> impl FnMut(tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> + Clone {
        let value = self.value.clone();
        #[allow(clippy::result_large_err)] // `tonic::Status` size is the interceptor contract.
        move |mut req: tonic::Request<()>| {
            req.metadata_mut()
                .insert(DEVICE_CERT_METADATA_KEY, value.clone());
            Ok(req)
        }
    }
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
