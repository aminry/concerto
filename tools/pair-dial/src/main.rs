//! `pair-dial` — the client/dial side of the two-process, cross-machine Iroh
//! pairing verification (sibling of `tools/split-host-loopback`).
//!
//! It decodes the `pair-serve` connect-blob, reconstructs the **relay-bearing**
//! server `EndpointAddr` (so it can reach a NAT'd peer through the relay),
//! generates a device key, pairs over the real `0x03` channel (Noise XX over the
//! one-shot token -> `SignedDeviceCert`), opens the Noise-IK-wrapped API channel,
//! and then runs, printing each result on its own `pair-dial:` line:
//!
//!   1. `Runtime.GetServerCapabilities` -> assert `transport_kind == IROH`.
//!   2. `Runtime.GetNatStats` -> the direct/relayed/lan counts (real-NAT evidence).
//!   3. The connection path (Direct hole-punched vs Relayed), inferred from the
//!      NAT stats (the client cannot read the server's per-session path directly).
//!   4. `Files.Upload` a ~450 KiB fixture into the workarea's `.context/` then
//!      `Files.Download` it back; assert byte-identical + BLAKE2b-256 match.
//!   5. (optional) `Streams.Subscribe(workspace.events)` opens.
//!
//! On any failure it prints `pair-dial: FAILED: <reason>` and exits non-zero; on
//! full success it prints `pair-dial: ALL OK` and exits 0.
//!
//! # MINIMAL deps (no concerto-core)
//!
//! This bin depends only on `concerto-transport` / `-identity` / `-proto` (NOT
//! `concerto-core`), so it builds on a small box without the Core + keychain
//! stack. It inlines the `concerto-device-cert` metadata key + the base64 cert
//! encoding (the `crates/core/src/security/auth.rs` FROZEN surface) to avoid
//! pulling core in just for two constants.

#![allow(clippy::result_large_err)]

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use concerto_identity::{KeyPair, NoiseHandshake, NoiseStatic};
use concerto_proto::v1::files_client::FilesClient;
use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::streams_client::StreamsClient;
use concerto_proto::v1::upload_chunk::Body as UploadBody;
use concerto_proto::v1::{
    DownloadRequest, SubscribeRequest, TransportKind, UploadChunk, UploadFinalize, UploadHeader,
};
use concerto_transport::api::write_channel_tag;
use concerto_transport::{connect_channel, ChannelTag, IrohDuplex, ALPN};
use futures::StreamExt;
use iroh::{EndpointAddr, EndpointId, RelayUrl};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tonic::transport::Channel;

type Blake2b256 = Blake2b<U32>;

/// FROZEN (Task 210): the metadata key every remote client presents the signed
/// device cert under. Inlined to avoid a `concerto-core` dependency. The value
/// is STANDARD base64 of the on-wire signed cert (`cert_bytes || signature`).
const DEVICE_CERT_METADATA_KEY: &str = "concerto-device-cert";

/// Per-step wall-clock budget for the RPCs.
const STEP_TIMEOUT: Duration = Duration::from_secs(45);
/// Cap the whole pairing exchange (Noise XX + request/cert) separately.
const PAIR_TIMEOUT: Duration = Duration::from_secs(45);
/// Files fixture: ~450 KiB so the upload spans multiple frames.
const FILE_REL_PATH: &str = "pair-dial.bin";
const FILE_CHUNK: usize = 200 * 1024;
const FILE_SIZE: usize = 450 * 1024;

fn blake2b_256(bytes: &[u8]) -> Vec<u8> {
    let mut h = Blake2b256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

/// The connect-blob `pair-serve` prints (base64(JSON)). Field set must match
/// `pair-serve`'s `ConnectBlob`.
#[derive(Deserialize)]
struct ConnectBlob {
    endpoint_id: String,
    relay_url: Option<String>,
    direct_addrs: Vec<String>,
    pairing_token: String,
    core_noise_pub: String,
    workarea_id: String,
    #[allow(dead_code)]
    project_id: String,
    #[allow(dead_code)]
    repo_id: String,
}

struct Args {
    blob: String,
    relays: bool,
    revoke_self: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut blob = None;
    let mut relays = true;
    let mut revoke_self = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--blob" => blob = Some(next(&mut it, "--blob")?),
            "--relays" => relays = true,
            "--no-relays" => relays = false,
            // After the round-trip, RevokeDevice(self) and prove the server
            // severs this session (a follow-up RPC must fail).
            "--revoke-self" => revoke_self = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        blob: blob.ok_or("missing --blob <base64-blob>")?,
        relays,
        revoke_self,
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
            eprintln!("pair-dial: FAILED: build runtime: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    match rt.block_on(run()) {
        Ok(()) => {
            println!("pair-dial: ALL OK");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            println!("pair-dial: FAILED: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;

    // --- Decode the connect-blob ------------------------------------------
    let json = base64::engine::general_purpose::STANDARD
        .decode(args.blob.trim())
        .map_err(|e| format!("blob is not valid base64: {e}"))?;
    let blob: ConnectBlob =
        serde_json::from_slice(&json).map_err(|e| format!("blob json decode: {e}"))?;

    let token = decode_token(&blob.pairing_token)?;
    let core_noise_pub = decode_pub(&blob.core_noise_pub)?;

    // --- Build the client endpoint (relay vs disabled) --------------------
    let client_ep = build_client_endpoint(args.relays).await?;

    // --- Reconstruct the server EndpointAddr (id + relay url + direct addrs)
    let server_addr = build_server_addr(&blob, args.relays)?;
    println!(
        "pair-dial: dialing endpoint_id={} via {} ({} direct addr(s))",
        blob.endpoint_id,
        blob.relay_url.as_deref().unwrap_or("<no relay>"),
        blob.direct_addrs.len()
    );

    // --- Generate a device key + nonce (random per dial) ------------------
    let device_seed = random_32().map_err(|e| format!("device seed rng: {e}"))?;
    let device_key = KeyPair::from_seed(&device_seed);
    let device_pubkey = device_key.verifying_key().to_bytes();
    let nonce = random_32().map_err(|e| format!("nonce rng: {e}"))?;

    // --- Pair over the REAL 0x03 channel -> SignedDeviceCert --------------
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
    println!("pair-dial: paired ({} byte cert)", signed_cert.len());

    // --- Authenticated Iroh + Noise IK API channel ------------------------
    let device_static =
        Arc::new(NoiseStatic::generate().map_err(|e| format!("noise static: {e}"))?);
    let channel = tokio::time::timeout(
        STEP_TIMEOUT,
        connect_channel(
            &client_ep,
            server_addr.clone(),
            device_static,
            core_noise_pub,
        ),
    )
    .await
    .map_err(|_| "connect api channel stalled".to_string())?
    .map_err(|e| format!("connect api channel: {e}"))?;
    let attach_cert = cert_interceptor(&signed_cert)?;

    // (1) unary — GetServerCapabilities == IROH ----------------------------
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
    println!("pair-dial: unary OK, transport_kind=IROH");

    // (2) GetNatStats — the real-NAT path evidence -------------------------
    let nat = timeout_rpc("GetNatStats", runtime_client.get_nat_stats(()))
        .await?
        .into_inner();
    println!(
        "pair-dial: nat_stats direct={} relayed={} lan={}",
        nat.direct_today, nat.relayed_today, nat.lan_today
    );

    // (3) Connection path — inferred from the NAT counters. The client cannot
    // query the server's per-session ConnectionPath directly, so we infer from
    // GetNatStats: a relayed session bumps relayed_today; a hole-punched one
    // bumps direct_today; a same-host/LAN one bumps lan_today.
    //
    // NOTE: in the 217.5 boot path the Core's Runtime handler is built with the
    // default `NoNatStats` source (the live `IrohTransport` is NOT yet attached
    // via `with_nat_stats`), so `GetNatStats` returns all-zero counters even
    // though a real Iroh session is live. When every counter is zero we report
    // `unknown (stats not wired)` rather than guessing — the load-bearing
    // real-NAT evidence is the live IROH unary + the Files round-trip below; the
    // direct-vs-relayed split here only becomes meaningful once the Core attaches
    // the transport as its NatStatsSource.
    let total = nat.direct_today + nat.relayed_today + nat.lan_today;
    let path = if total == 0 {
        "unknown (stats not wired: Core uses NoNatStats in the 217.5 boot path)"
    } else if nat.relayed_today > 0 && nat.direct_today == 0 && nat.lan_today == 0 {
        "Relayed"
    } else if nat.direct_today > 0 {
        "Direct"
    } else {
        "Lan"
    };
    println!("pair-dial: connection path inferred from nat_stats: {path}");

    // (4) Files round-trip into the workarea's .context/ -------------------
    files_round_trip(channel.clone(), &attach_cert, &blob.workarea_id).await?;
    println!("pair-dial: Files round-trip OK");

    // (5) optional — Streams.Subscribe(workspace.events) just opens --------
    match stream_opens(channel.clone(), &attach_cert).await {
        Ok(()) => println!("pair-dial: stream Streams.Subscribe(workspace.events) opened"),
        Err(e) => println!("pair-dial: stream step skipped ({e})"),
    }

    // (6) optional — RevokeDevice(self) → the Core severs THIS session ------
    // The real teardown path end-to-end over the network: this device revokes
    // its OWN device row over the authenticated channel; the Core's
    // DeviceManager marks it revoked and the IrohSessionCloser tears the live
    // Iroh session down (fingerprint→DeviceId). Proof = a follow-up RPC over
    // the same channel MUST fail (connection severed and/or cert now revoked).
    if args.revoke_self {
        use concerto_proto::v1::devices_client::DevicesClient;
        use concerto_proto::v1::RevokeDeviceRequest;
        let device_id_hex: String = concerto_identity::device_id(&device_pubkey)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        println!("pair-dial: revoking self (device_id={device_id_hex}) ...");
        let mut devices = DevicesClient::with_interceptor(channel.clone(), attach_cert.call());
        // The revoke handler severs THIS very connection, so the RPC itself may
        // return Ok OR a transport error (self-severance mid-response). Either
        // outcome is acceptable — the load-bearing proof is the follow-up RPC.
        match tokio::time::timeout(
            STEP_TIMEOUT,
            devices.revoke_device(RevokeDeviceRequest {
                device_id: device_id_hex,
            }),
        )
        .await
        {
            Ok(Ok(_)) => println!("pair-dial: RevokeDevice(self) returned Ok"),
            Ok(Err(e)) => println!(
                "pair-dial: RevokeDevice(self) connection self-severed (code={:?})",
                e.code()
            ),
            Err(_) => println!("pair-dial: RevokeDevice(self) timed out (likely self-severed)"),
        }
        // Give the SessionCloser a beat, then a follow-up RPC over the SAME
        // channel MUST fail — that is the teardown proof.
        tokio::time::sleep(Duration::from_millis(750)).await;
        let mut runtime_after = RuntimeClient::with_interceptor(channel, attach_cert.call());
        match tokio::time::timeout(
            Duration::from_secs(15),
            runtime_after.get_server_capabilities(()),
        )
        .await
        {
            Ok(Ok(_)) => {
                return Err("revoke teardown FAILED: a post-revoke RPC still succeeded \
                     (session was NOT severed)"
                    .to_string())
            }
            Ok(Err(e)) => println!(
                "pair-dial: post-revoke RPC correctly FAILED (code={:?}) — session severed ✓",
                e.code()
            ),
            Err(_) => {
                println!("pair-dial: post-revoke RPC timed out — session severed / unreachable ✓")
            }
        }
        println!("pair-dial: revoke→teardown demonstrated over the live network ✓");
    }

    Ok(())
}

/// Build the client Iroh endpoint. `RelayMode::Default` for cross-machine (so the
/// client can reach a NAT'd peer through the default relay map), `Disabled` for
/// same-host `--no-relays` validation.
async fn build_client_endpoint(relays: bool) -> Result<iroh::Endpoint, String> {
    let mode = if relays {
        iroh::RelayMode::Default
    } else {
        iroh::RelayMode::Disabled
    };
    iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(mode)
        .bind()
        .await
        .map_err(|e| format!("client endpoint: {e}"))
}

/// Reconstruct the server `EndpointAddr` from the blob: its id, the relay url
/// (so a NAT'd peer reaches it), and any direct socket addrs (LAN hole-punch /
/// same-host). For `--no-relays` the direct addrs are the only reachable path.
fn build_server_addr(blob: &ConnectBlob, relays: bool) -> Result<EndpointAddr, String> {
    let endpoint_id: EndpointId = blob
        .endpoint_id
        .parse()
        .map_err(|e| format!("parse endpoint_id '{}': {e}", blob.endpoint_id))?;
    let mut addr = EndpointAddr::new(endpoint_id);

    if relays {
        if let Some(url) = &blob.relay_url {
            let relay: RelayUrl = url
                .parse()
                .map_err(|e| format!("parse relay_url '{url}': {e}"))?;
            addr = addr.with_relay_url(relay);
        }
    }

    let mut direct_count = 0usize;
    for s in &blob.direct_addrs {
        match s.parse::<std::net::SocketAddr>() {
            Ok(sa) => {
                addr = addr.with_ip_addr(sa);
                direct_count += 1;
            }
            Err(e) => return Err(format!("parse direct addr '{s}': {e}")),
        }
    }

    if !relays && direct_count == 0 {
        return Err(
            "--no-relays but the blob carries no direct addrs (the peer is unreachable)"
                .to_string(),
        );
    }
    if relays && blob.relay_url.is_none() && direct_count == 0 {
        return Err("blob carries neither a relay url nor direct addrs".to_string());
    }
    Ok(addr)
}

/// Run the Noise-XX pairing handshake over the `0x03` channel and return the
/// on-wire signed device cert. Mirrors the split-host-loopback driver's
/// `pair_over_iroh` (Task 217.5 framing).
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
    let req = encode_pairing_request(device_pubkey, nonce, &signature, "Pair-Dial Cross-Machine");
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

/// Upload a multi-chunk fixture into the workarea's `.context/`, download it
/// back, and assert byte-identical + matching BLAKE2b-256.
async fn files_round_trip(
    channel: Channel,
    attach_cert: &CertInterceptorFactory,
    workarea_id: &str,
) -> Result<(), String> {
    let mut files = FilesClient::with_interceptor(channel, attach_cert.call());

    let payload: Vec<u8> = (0..FILE_SIZE).map(|i| (i % 251) as u8).collect();
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
    tokio::time::timeout(STEP_TIMEOUT, async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|s| format!("Download stream error: {s}"))?;
            downloaded.extend_from_slice(&chunk.data);
        }
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "Download stalled".to_string())??;

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

/// Open a `workspace.events` subscription and confirm it comes up (the optional
/// stream step). We only assert the subscribe RPC opens; we do not drive an
/// event (the dial side does not create workspaces).
async fn stream_opens(
    channel: Channel,
    attach_cert: &CertInterceptorFactory,
) -> Result<(), String> {
    let mut streams_client = StreamsClient::with_interceptor(channel, attach_cert.call());
    let sub = timeout_rpc(
        "Subscribe(workspace.events)",
        streams_client.subscribe(SubscribeRequest {
            subject: "workspace.events".to_string(),
            filter: None,
            since_offset: None,
        }),
    )
    .await?;
    // Just confirm the server-streaming response opened.
    let _stream = sub.into_inner();
    Ok(())
}

/// `device_pubkey(32) || nonce(32) || signature(64) || device_name(utf8)` — the
/// encrypted `PairingRequest` the Core decodes (Task 217.5 framing).
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

fn decode_token(hex_str: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("pairing_token not hex: {e}"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| "pairing_token is not 32 bytes".to_string())
}

fn decode_pub(hex_str: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("core_noise_pub not hex: {e}"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| "core_noise_pub is not 32 bytes".to_string())
}

/// 32 OS-random bytes (the same `getrandom` the rest of the workspace uses; no
/// `rand` dep).
fn random_32() -> Result<[u8; 32], String> {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).map_err(|e| format!("getrandom: {e}"))?;
    Ok(buf)
}

/// Build a cert-attaching interceptor factory. The value is STANDARD base64 of
/// the on-wire signed cert under [`DEVICE_CERT_METADATA_KEY`] — the FROZEN
/// `crates/core/src/security/auth.rs` encoding, inlined here.
fn cert_interceptor(signed_cert: &[u8]) -> Result<CertInterceptorFactory, String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(signed_cert);
    let value: tonic::metadata::MetadataValue<tonic::metadata::Ascii> =
        encoded.parse().map_err(|e| format!("cert metadata: {e}"))?;
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
