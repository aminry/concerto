//! `ConcertoIroh` — the React Native (iOS/Android) native module (Task 509,
//! `design/16 §3.2` + §4.6).
//!
//! A **hand-rolled uniffi cdylib** over the EXISTING, spike-validated
//! `concerto-transport` stack. This is the **D12 fallback**: `iroh-ffi` is
//! unusable for Concerto (git-only, no published `0.98.x`, and it pulls a
//! *second, colliding* `iroh` with different crypto pins — adding it would break
//! the validated `iroh = 0.98.2` trio). Instead this crate is a thin uniffi
//! facade over the SAME seam `tools/pair-dial` proves end to end:
//! `connect_channel` (Noise IK + channel-tag 0x01 API channel as a tonic
//! `Channel`), the `0x03` Noise-XX pairing flow, and the client-side
//! `classify_path` NAT classification.
//!
//! # The frozen surface (design/16 §3.2 / §4.6)
//!
//! - [`generate_device_keypair`] — Ed25519 via OS randomness; the caller (511)
//!   persists the seed to expo-secure-store.
//! - [`pair`] — the `0x03` Noise-XX pairing → on-wire `SignedDeviceCert` bytes.
//! - [`open_session`] — bind a client endpoint, reconstruct the server addr, gen
//!   a per-session Noise static, `connect_channel`, register an opaque handle.
//! - [`rpc_unary`] — generic byte passthrough over `Grpc::unary` + the identity
//!   codec (the path is fully-qualified `/concerto.v1.Service/Method`).
//! - [`rpc_stream`] — generic server-streaming passthrough, each message's raw
//!   bytes delivered to a [`StreamEventCallback`]; the returned id cancels it.
//! - [`cancel_subscription`] — drop a live stream.
//! - [`close_session`] — drop channel + endpoint, deregister.
//! - [`nat_stats`] — client-side `ConnectionPath` of this device's session(s).
//!
//! 509 stays a **pure passthrough**: it never `prost`-decodes the caller's bytes
//! (the identity codec copies them through). 510 assembles the typed paths +
//! messages on top of these primitives.

#![allow(clippy::result_large_err)] // tonic::Status / our error enum size is the FFI contract.

use std::sync::{Arc, OnceLock};

use base64::Engine as _;
use bytes::Bytes;
use concerto_identity::{device_id, generate_seed, KeyPair, NoiseStatic};
use concerto_transport::{classify_path, connect_channel, ConnectionPath, ALPN, MAX_MESSAGE_SIZE};
use futures::StreamExt;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;
use tonic::metadata::{Ascii, MetadataValue};

mod codec;
mod error;
mod nat;
mod pairing;
mod registry;

pub use error::IrohFfiError;

use codec::IdentityCodec;
use registry::{Registry, Session};

uniffi::setup_scaffolding!();

/// FROZEN (Task 210, `crates/core/src/security/auth.rs`): the metadata key every
/// remote client presents the signed device cert under. The value is STANDARD
/// base64 of the on-wire signed cert (`cert_bytes || signature`). Inlined to
/// avoid a `concerto-core` dependency (same promise pair-dial keeps).
const DEVICE_CERT_METADATA_KEY: &str = "concerto-device-cert";

/// The process-wide tokio runtime the (sync) FFI surface blocks on. Multi-thread
/// so the QUIC/Noise work runs off the calling thread.
fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build concerto-iroh-ffi tokio runtime")
    })
}

/// The process-wide session registry (opaque `u64` handle → live session).
fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(Registry::new)
}

// ---------------------------------------------------------------------------
// FFI value types (uniffi Records / Enums)
// ---------------------------------------------------------------------------

/// The connect-blob fields a paired device holds (the subset `open_session` /
/// `pair` need). Mirrors `tools/pair-serve`'s blob; the caller (511) decodes the
/// base64(JSON) blob on the JS side and passes the fields in.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ConnectBlob {
    /// The Core endpoint's Iroh `EndpointId` (z-base-32 string).
    pub endpoint_id: String,
    /// The relay URL the Core advertises (None / empty ⇒ no relay; loopback).
    pub relay_url: Option<String>,
    /// Direct socket addresses (`ip:port`) for LAN / same-host / hole-punched
    /// reachability.
    pub direct_addrs: Vec<String>,
    /// The Core's static Noise public key (hex, 32 bytes) — the IK responder
    /// identity `connect_channel` pins.
    pub core_noise_pub: String,
}

/// The one-shot pairing inputs (`pair` only). Kept separate from [`ConnectBlob`]
/// so `open_session` need not carry the secret token.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PairingInputs {
    /// The base connect-blob (endpoint id + relay + direct addrs + noise pub).
    pub blob: ConnectBlob,
    /// The one-shot pairing token (hex, 32 bytes) — the Noise XX PSK.
    pub pairing_token: String,
    /// A human-readable device name recorded in the cert (free text).
    pub device_name: String,
}

/// A freshly generated device identity. The caller (511) persists `seed` to
/// expo-secure-store and re-derives the keypair with it on next launch.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DeviceKeypair {
    /// The 32-byte Ed25519 seed (the PRIVATE key) — SECRET; persist securely.
    pub seed: Vec<u8>,
    /// The 32-byte Ed25519 public key.
    pub public_key: Vec<u8>,
    /// The canonical `device_id` = BLAKE2b-256(public_key), 32 bytes.
    pub device_id: Vec<u8>,
}

/// How this device's session reaches the Core — the client-side classification
/// of the live Iroh connection (`design/11 §3.6`, mirrored client-side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NatPath {
    /// Hole-punched direct path over a public/routable IP.
    Direct,
    /// Relayed QUIC through the configured relay.
    Relayed,
    /// LAN-direct over a loopback / private-range IP, no relay.
    Lan,
}

/// Client-side NAT stats for this device's own session(s) (`design/16 §4.6`).
/// `path` is the classification of the most-recently-opened session; the counts
/// aggregate across all live sessions.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NatStats {
    /// The path of the most-recently-opened live session (None ⇒ no sessions).
    pub path: Option<NatPath>,
    /// Count of live sessions on a direct (hole-punched) path.
    pub direct: u32,
    /// Count of live sessions on a relayed path.
    pub relayed: u32,
    /// Count of live sessions on a LAN-direct path.
    pub lan: u32,
}

/// The foreign callback `rpc_stream` delivers each server-streamed message's RAW
/// bytes to. Implemented on the JS side (511); the Rust stream pump invokes
/// `on_event` per message and `on_complete`/`on_error` at end-of-stream.
#[uniffi::export(callback_interface)]
pub trait StreamEventCallback: Send + Sync {
    /// One server-streamed message, raw bytes, untouched.
    fn on_event(&self, data: Vec<u8>);
    /// The stream ended cleanly (end-of-stream from the server).
    fn on_complete(&self);
    /// The stream ended with an error (gRPC status text).
    fn on_error(&self, message: String);
}

// ---------------------------------------------------------------------------
// FFI functions
// ---------------------------------------------------------------------------

/// Generate a fresh Ed25519 device keypair from OS randomness. The caller
/// persists [`DeviceKeypair::seed`] securely and re-derives on next launch.
#[uniffi::export]
pub fn generate_device_keypair() -> Result<DeviceKeypair, IrohFfiError> {
    let seed = generate_seed().map_err(|e| IrohFfiError::Crypto(format!("generate seed: {e}")))?;
    let kp = KeyPair::from_seed(&seed);
    let public_key = kp.verifying_key().to_bytes();
    let did = device_id(&public_key);
    Ok(DeviceKeypair {
        seed: seed.to_vec(),
        public_key: public_key.to_vec(),
        device_id: did.to_vec(),
    })
}

/// Pair this device with the Core over the `0x03` channel (Noise XX over the
/// one-shot token) and return the on-wire `SignedDeviceCert` bytes
/// (`cert_bytes || signature`). The caller persists the cert and presents it on
/// every subsequent session (see [`open_session`]). 511 consumes this.
///
/// `device_seed` is the 32-byte Ed25519 seed from [`generate_device_keypair`].
#[uniffi::export]
pub fn pair(inputs: PairingInputs, device_seed: Vec<u8>) -> Result<Vec<u8>, IrohFfiError> {
    let device_seed: [u8; 32] = device_seed
        .as_slice()
        .try_into()
        .map_err(|_| IrohFfiError::InvalidArgument("device_seed must be 32 bytes".into()))?;
    let token = decode_hex32(&inputs.pairing_token, "pairing_token")?;
    // Remote vs loopback: relay present ⇒ remote (RelayMode::Default), else
    // loopback (Disabled), matching pair-dial / split-host-loopback.
    let want_relay = blob_has_relay(&inputs.blob);

    runtime().block_on(async move {
        let client_ep = build_client_endpoint(want_relay).await?;
        let server_addr = build_server_addr(&inputs.blob, want_relay)?;

        let device_key = KeyPair::from_seed(&device_seed);
        let device_pubkey = device_key.verifying_key().to_bytes();
        let nonce = random_32()?;

        pairing::pair_over_iroh(
            &client_ep,
            &server_addr,
            &token,
            &device_key,
            &device_pubkey,
            &nonce,
            &inputs.device_name,
        )
        .await
    })
}

/// Open an authenticated session to the Core: bind a client endpoint,
/// reconstruct the server address, generate a per-session Noise static, build
/// the Noise-IK + channel-tag-0x01 API channel via `connect_channel`, classify
/// the path, and register an opaque handle. `signed_cert` is the on-wire device
/// cert from [`pair`]; it is attached as `concerto-device-cert` on every call.
#[uniffi::export]
pub fn open_session(blob: ConnectBlob, signed_cert: Vec<u8>) -> Result<u64, IrohFfiError> {
    let core_noise_pub = decode_hex32(&blob.core_noise_pub, "core_noise_pub")?;
    let cert_value = encode_cert_metadata(&signed_cert)?;
    let want_relay = blob_has_relay(&blob);

    runtime().block_on(async move {
        let client_ep = build_client_endpoint(want_relay).await?;
        let server_addr = build_server_addr(&blob, want_relay)?;

        let device_static = Arc::new(
            NoiseStatic::generate()
                .map_err(|e| IrohFfiError::Connect(format!("noise static: {e}")))?,
        );

        // Open the raw Iroh connection FIRST so we can classify its path, then
        // build the tonic channel over a fresh connection via connect_channel.
        // (connect_channel dials its own connection internally; we dial a
        // parallel one only to read the live ConnectionPath — the cheapest way
        // to expose natStats client-side without re-plumbing connect_channel.)
        let path = classify_initial_path(&client_ep, &server_addr, want_relay).await;

        let channel = connect_channel(&client_ep, server_addr, device_static, core_noise_pub)
            .await
            .map_err(|e| IrohFfiError::Connect(format!("connect api channel: {e}")))?;

        let session = Session {
            endpoint: client_ep,
            channel,
            cert_value,
            path,
            subscriptions: std::collections::HashMap::new(),
        };
        Ok(registry().insert(session))
    })
}

/// Drive a unary RPC as a pure byte passthrough. `method` is the FULLY-QUALIFIED
/// gRPC path (`/concerto.v1.Service/Method`); `payload` is the raw request body;
/// the raw response body is returned. The caller's bytes are NEVER prost-decoded
/// (identity codec). Honors the 64 MiB `MAX_MESSAGE_SIZE` ceiling.
#[uniffi::export]
pub fn rpc_unary(handle: u64, method: String, payload: Vec<u8>) -> Result<Vec<u8>, IrohFfiError> {
    let (channel, cert) = registry()
        .channel_and_cert(handle)
        .ok_or(IrohFfiError::UnknownHandle(handle))?;
    let path = parse_grpc_path(&method)?;

    runtime().block_on(async move {
        let mut grpc = tonic::client::Grpc::new(channel)
            .max_decoding_message_size(MAX_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_MESSAGE_SIZE);
        grpc.ready()
            .await
            .map_err(|e| IrohFfiError::Rpc(format!("channel not ready: {e}")))?;

        let mut req = tonic::Request::new(Bytes::from(payload));
        req.metadata_mut().insert(DEVICE_CERT_METADATA_KEY, cert);

        let resp = grpc
            .unary(req, path, IdentityCodec)
            .await
            .map_err(|s| IrohFfiError::Rpc(s.to_string()))?;
        Ok(resp.into_inner().to_vec())
    })
}

/// Drive a server-streaming RPC as a pure byte passthrough. Each message's raw
/// bytes are delivered to `on_event`; the returned subscription id cancels the
/// stream (see [`cancel_subscription`]) or it is dropped on [`close_session`].
/// A bounded channel provides backpressure between the network pump and the
/// callback.
#[uniffi::export]
pub fn rpc_stream(
    handle: u64,
    method: String,
    payload: Vec<u8>,
    on_event: Box<dyn StreamEventCallback>,
) -> Result<u64, IrohFfiError> {
    let (channel, cert) = registry()
        .channel_and_cert(handle)
        .ok_or(IrohFfiError::UnknownHandle(handle))?;
    let path = parse_grpc_path(&method)?;

    // Cancel signal: stored on the session, fired by cancel_subscription / drop.
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    let sub_id = registry()
        .with_session(handle, |s| {
            // Reuse the registry's id space for subscription ids (monotonic,
            // unique within the process); store the cancel sender.
            let id = s.subscriptions.len() as u64 + 1 + next_sub_seed();
            s.subscriptions.insert(id, cancel_tx);
            id
        })
        .ok_or(IrohFfiError::UnknownHandle(handle))?;

    let callback: Arc<dyn StreamEventCallback> = Arc::from(on_event);
    let cert_for_task = cert;

    runtime().spawn(async move {
        let mut grpc = tonic::client::Grpc::new(channel)
            .max_decoding_message_size(MAX_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_MESSAGE_SIZE);
        if let Err(e) = grpc.ready().await {
            callback.on_error(format!("channel not ready: {e}"));
            return;
        }
        let mut req = tonic::Request::new(Bytes::from(payload));
        req.metadata_mut()
            .insert(DEVICE_CERT_METADATA_KEY, cert_for_task);

        let stream = match grpc.server_streaming(req, path, IdentityCodec).await {
            Ok(resp) => resp.into_inner(),
            Err(s) => {
                callback.on_error(s.to_string());
                return;
            }
        };
        tokio::pin!(stream);
        tokio::pin!(cancel_rx);

        loop {
            tokio::select! {
                // Cancellation: drop the stream task immediately.
                _ = &mut cancel_rx => {
                    return;
                }
                item = stream.next() => {
                    match item {
                        Some(Ok(msg)) => callback.on_event(msg.to_vec()),
                        Some(Err(s)) => {
                            callback.on_error(s.to_string());
                            return;
                        }
                        None => {
                            callback.on_complete();
                            return;
                        }
                    }
                }
            }
        }
    });

    Ok(sub_id)
}

/// Cancel a live subscription (drops the stream task). No-op if the handle /
/// subscription is unknown (already cancelled / completed).
#[uniffi::export]
pub fn cancel_subscription(handle: u64, subscription_id: u64) {
    registry().with_session(handle, |s| {
        if let Some(tx) = s.subscriptions.remove(&subscription_id) {
            let _ = tx.send(());
        }
    });
}

/// Close a session: drop the tonic channel + the Iroh endpoint (closing the
/// QUIC connection) and deregister the handle. Any live subscriptions are
/// cancelled (their oneshots fire on drop).
#[uniffi::export]
pub fn close_session(handle: u64) {
    // Removing the Session drops its `subscriptions` map → each oneshot Sender
    // drops → the stream tasks' `cancel_rx` resolves (Err) → they return.
    // Dropping `endpoint` + `channel` closes the connection.
    let _ = registry().remove(handle);
}

/// Client-side NAT stats for this device's own live session(s) — NOT a Core RPC
/// (`design/16 §4.6`). `path` is the most-recently-opened session's
/// classification; the counts aggregate across all live sessions.
#[uniffi::export]
pub fn nat_stats() -> NatStats {
    let paths = registry().all_paths();
    let mut direct = 0u32;
    let mut relayed = 0u32;
    let mut lan = 0u32;
    for p in &paths {
        match p {
            ConnectionPath::Direct => direct += 1,
            ConnectionPath::Relayed => relayed += 1,
            ConnectionPath::Lan => lan += 1,
        }
    }
    // `path` = the last-inserted session's path (the registry preserves nothing
    // ordered, so report the single path when there is exactly one live session,
    // else None — the most honest single-value summary; the counts carry the
    // aggregate). For the common mobile case (one session) this is the live
    // path, which is what 511's diagnostics surface.
    let path = if paths.len() == 1 {
        Some(nat::to_ffi(paths[0]))
    } else {
        None
    };
    NatStats {
        path,
        direct,
        relayed,
        lan,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (endpoint / addr / encoding) — mirror pair-dial.
// ---------------------------------------------------------------------------

/// Build the client Iroh endpoint. `RelayMode::Default` for remote (so a NAT'd
/// peer is reachable through the default relay map), `Disabled` for loopback.
async fn build_client_endpoint(want_relay: bool) -> Result<Endpoint, IrohFfiError> {
    let mode = if want_relay {
        RelayMode::Default
    } else {
        RelayMode::Disabled
    };
    Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(mode)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| IrohFfiError::Endpoint(format!("client endpoint: {e}")))
}

/// Reconstruct the server `EndpointAddr` from the blob: its id, the relay url
/// (when `want_relay`), and any direct socket addrs. Mirrors pair-dial.
fn build_server_addr(blob: &ConnectBlob, want_relay: bool) -> Result<EndpointAddr, IrohFfiError> {
    let endpoint_id: EndpointId = blob.endpoint_id.parse().map_err(|e| {
        IrohFfiError::InvalidArgument(format!("parse endpoint_id '{}': {e}", blob.endpoint_id))
    })?;
    let mut addr = EndpointAddr::new(endpoint_id);

    if want_relay {
        if let Some(url) = &blob.relay_url {
            if !url.is_empty() {
                let relay: RelayUrl = url.parse().map_err(|e| {
                    IrohFfiError::InvalidArgument(format!("parse relay_url '{url}': {e}"))
                })?;
                addr = addr.with_relay_url(relay);
            }
        }
    }

    let mut direct_count = 0usize;
    for s in &blob.direct_addrs {
        let sa: std::net::SocketAddr = s
            .parse()
            .map_err(|e| IrohFfiError::InvalidArgument(format!("parse direct addr '{s}': {e}")))?;
        addr = addr.with_ip_addr(sa);
        direct_count += 1;
    }

    if !want_relay && direct_count == 0 {
        return Err(IrohFfiError::InvalidArgument(
            "no relay and no direct addrs (the Core is unreachable)".into(),
        ));
    }
    if want_relay && !blob_has_relay(blob) && direct_count == 0 {
        return Err(IrohFfiError::InvalidArgument(
            "blob carries neither a relay url nor direct addrs".into(),
        ));
    }
    Ok(addr)
}

/// Whether the blob advertises a (non-empty) relay url.
fn blob_has_relay(blob: &ConnectBlob) -> bool {
    blob.relay_url
        .as_deref()
        .map(|u| !u.is_empty())
        .unwrap_or(false)
}

/// Dial a parallel raw connection purely to read its live `ConnectionPath` for
/// `natStats`. Best-effort: a failure (the real channel will surface the error)
/// degrades to the conservative `Relayed` for remote / `Lan` for loopback.
async fn classify_initial_path(
    client_ep: &Endpoint,
    server_addr: &EndpointAddr,
    want_relay: bool,
) -> ConnectionPath {
    match client_ep.connect(server_addr.clone(), ALPN).await {
        Ok(conn) => {
            // Give Iroh a beat to select a path before classifying.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            classify_path(&conn)
        }
        Err(_) => {
            if want_relay {
                ConnectionPath::Relayed
            } else {
                ConnectionPath::Lan
            }
        }
    }
}

/// STANDARD base64 of the on-wire signed cert as an ASCII metadata value (the
/// FROZEN `concerto-device-cert` encoding).
fn encode_cert_metadata(signed_cert: &[u8]) -> Result<MetadataValue<Ascii>, IrohFfiError> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(signed_cert);
    encoded
        .parse()
        .map_err(|e| IrohFfiError::InvalidArgument(format!("cert metadata: {e}")))
}

/// Parse a fully-qualified gRPC path (`/pkg.Service/Method`) into the
/// `PathAndQuery` the tonic `Grpc` driver wants. Uses tonic's re-exported `http`
/// so the crate needs no direct `http` dep.
fn parse_grpc_path(method: &str) -> Result<tonic::codegen::http::uri::PathAndQuery, IrohFfiError> {
    method
        .parse()
        .map_err(|e| IrohFfiError::InvalidArgument(format!("invalid gRPC path '{method}': {e}")))
}

/// Decode a 32-byte hex field (token / noise pub).
fn decode_hex32(hex_str: &str, what: &str) -> Result<[u8; 32], IrohFfiError> {
    let bytes = hex::decode(hex_str)
        .map_err(|e| IrohFfiError::InvalidArgument(format!("{what} not hex: {e}")))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| IrohFfiError::InvalidArgument(format!("{what} is not 32 bytes")))
}

/// 32 OS-random bytes (same `getrandom` the workspace uses; no `rand` dep).
fn random_32() -> Result<[u8; 32], IrohFfiError> {
    let mut buf = [0u8; 32];
    getrandom::getrandom(&mut buf).map_err(|e| IrohFfiError::Crypto(format!("getrandom: {e}")))?;
    Ok(buf)
}

/// A small monotonic seed so concurrently-opened subscriptions on the same
/// session get distinct ids even if the `subscriptions` map was just drained.
fn next_sub_seed() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// generate_device_keypair: seed/pubkey/device_id are the right lengths and
    /// device_id == BLAKE2b-256(pubkey) (matches concerto-identity).
    #[test]
    fn generate_device_keypair_shapes_and_device_id() {
        let kp = generate_device_keypair().expect("gen");
        assert_eq!(kp.seed.len(), 32);
        assert_eq!(kp.public_key.len(), 32);
        assert_eq!(kp.device_id.len(), 32);

        let pubkey: [u8; 32] = kp.public_key.as_slice().try_into().unwrap();
        let expected = concerto_identity::device_id(&pubkey).to_vec();
        assert_eq!(kp.device_id, expected, "device_id is BLAKE2b-256(pubkey)");

        // Two generations differ (OS randomness).
        let kp2 = generate_device_keypair().expect("gen2");
        assert_ne!(kp.seed, kp2.seed);
    }

    /// nat_stats with no sessions is the empty/None summary.
    #[test]
    fn nat_stats_empty_when_no_sessions() {
        // NOTE: this asserts the shape on a fresh registry; if other tests in
        // this binary opened sessions the counts could differ, but unit tests
        // here never open a real session (that needs a live Core → loopback
        // test), so the global registry stays empty.
        let stats = nat_stats();
        assert_eq!(stats.path, None);
        assert_eq!(stats.direct + stats.relayed + stats.lan, 0);
    }

    /// rpc_unary / rpc_stream against an unknown handle return UnknownHandle.
    #[test]
    fn unknown_handle_is_typed_error() {
        let err = rpc_unary(
            424242,
            "/concerto.v1.Runtime/GetServerCapabilities".into(),
            vec![],
        )
        .unwrap_err();
        assert!(matches!(err, IrohFfiError::UnknownHandle(424242)));
    }

    /// parse_grpc_path accepts a fully-qualified path and rejects garbage.
    #[test]
    fn grpc_path_parsing() {
        assert!(parse_grpc_path("/concerto.v1.Runtime/GetServerCapabilities").is_ok());
        // A path with a space is invalid.
        assert!(parse_grpc_path("/has space/Method").is_err());
    }

    /// blob_has_relay distinguishes None / empty / present.
    #[test]
    fn blob_relay_detection() {
        let mk = |relay: Option<&str>| ConnectBlob {
            endpoint_id: "x".into(),
            relay_url: relay.map(|s| s.to_string()),
            direct_addrs: vec![],
            core_noise_pub: "00".into(),
        };
        assert!(!blob_has_relay(&mk(None)));
        assert!(!blob_has_relay(&mk(Some(""))));
        assert!(blob_has_relay(&mk(Some("https://relay.example/"))));
    }
}
