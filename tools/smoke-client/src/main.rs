//! `smoke-client` — the canonical end-to-end client used by
//! `scripts/smoke.sh` (Task 15).
//!
//! Behaviour:
//!
//! 1. Parses `--socket <path>` from argv.
//! 2. Connects to a Tonic `Runtime` service at that UDS path.
//! 3. Calls `GetServerCapabilities`, wrapped in a 5 s `tokio::time::timeout`
//!    so a broken Core can never wedge CI.
//! 4. Prints a one-line JSON object to stdout with the fields the smoke
//!    gate greps for, then exits 0.
//! 5. On any failure, prints a `smoke-client: <reason>` line to stderr
//!    and exits non-zero. Non-zero is sticky — the script's `set -e`
//!    catches it.
//!
//! The connector pattern is copied from
//! `crates/core/tests/grpc_runtime.rs::connect_client`: a placeholder
//! HTTP URI feeds Tonic's `Endpoint`, and `connect_with_connector`
//! overrides every dial with a `UnixStream::connect` wrapped in
//! `hyper_util::rt::TokioIo`. See `tasks/13-grpc-uds-server.md`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::TransportKind;
use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Endpoint, Uri};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_TIMEOUT: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    let socket = match parse_args() {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("smoke-client: {msg}");
            return ExitCode::from(2);
        }
    };

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("smoke-client: failed to build tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };

    match rt.block_on(run(socket)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("smoke-client: {e}");
            ExitCode::from(1)
        }
    }
}

/// Parse `--socket <path>` out of argv. Anything else is a usage error.
///
/// Kept handwritten rather than pulling in `clap` because there's
/// exactly one flag and we don't want to add a dep to the workspace
/// for it.
fn parse_args() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    let mut socket: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--socket" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--socket requires a path argument".to_string())?;
                socket = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                println!("smoke-client --socket <path>");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    socket.ok_or_else(|| "missing required --socket <path>".to_string())
}

async fn run(socket: PathBuf) -> Result<(), String> {
    let mut client = connect(socket.clone()).await?;

    let resp = tokio::time::timeout(RPC_TIMEOUT, client.get_server_capabilities(()))
        .await
        .map_err(|_| format!("GetServerCapabilities timed out after {RPC_TIMEOUT:?}"))?
        .map_err(|status| format!("GetServerCapabilities rpc error: {status}"))?;

    let caps = resp.into_inner();

    // Render a JSON object whose `transport_kind` field is the proto
    // enum's string name (e.g. `"TRANSPORT_KIND_UDS"`) rather than the
    // raw `i32` that the auto-derived serde impl would emit. The smoke
    // script greps for the string form.
    let transport_kind_str = TransportKind::try_from(caps.transport_kind)
        .map(|k| k.as_str_name().to_string())
        .unwrap_or_else(|_| format!("UNKNOWN({})", caps.transport_kind));

    let out = serde_json::json!({
        "server_version": caps.server_version,
        "schema_version": caps.schema_version,
        "transport_kind": transport_kind_str,
        "core_host_os": caps.core_host_os,
        "core_hostname": caps.core_hostname,
        "limits": caps.limits.map(|l| serde_json::json!({
            "max_concurrent_streams": l.max_concurrent_streams,
            "max_payload_bytes": l.max_payload_bytes,
        })),
    });

    println!("{out}");
    Ok(())
}

/// Build a `RuntimeClient` whose underlying channel dials `socket_path`
/// over UDS. The HTTP URI is a placeholder — Tonic requires *some*
/// authority to parse, but `connect_with_connector` short-circuits it
/// every dial.
async fn connect(socket_path: PathBuf) -> Result<RuntimeClient<tonic::transport::Channel>, String> {
    let endpoint = Endpoint::try_from("http://[::1]:50051")
        .map_err(|e| format!("endpoint init: {e}"))?
        .connect_timeout(CONNECT_TIMEOUT);

    let channel = tokio::time::timeout(
        CONNECT_TIMEOUT,
        endpoint.connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = socket_path.clone();
            async move {
                let stream = UnixStream::connect(&p).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        })),
    )
    .await
    .map_err(|_| format!("connect timed out after {CONNECT_TIMEOUT:?}"))?
    .map_err(|e| format!("connect: {e}"))?;

    Ok(RuntimeClient::new(channel))
}
