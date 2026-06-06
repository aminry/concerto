//! `concerto-relay` binary entry point (`design/11 §6.3`, Task 214).
//!
//! A thin Twelve-Factor wrapper over the [`concerto_relay`] library: parse the
//! env-var config, start the relay (embedded `iroh-relay`, Prometheus endpoint,
//! routing-table sweep), then run until a Ctrl-C / SIGTERM signal triggers a
//! clean shutdown. No config file, no flags beyond `--help` / `--version`
//! (`design/11 §6.3`).
//!
//! Task 215 wraps the same [`concerto_relay::Relay::start`] in its own binary
//! loop to add the WSS↔Iroh bridge on the reserved `WSS_LISTEN_ADDR`.

use std::process::ExitCode;

use concerto_relay::config::{
    ENV_BANDWIDTH_CAP_PER_ENDPOINT, ENV_MAX_ROUTES, ENV_PROMETHEUS_LISTEN_ADDR,
    ENV_RELAY_LISTEN_ADDR, ENV_WEBHOOK_LISTEN_ADDR, ENV_WSS_LISTEN_ADDR,
};
use concerto_relay::{Relay, RelayConfig, RelayError, WssTlsConfig};

/// Additive env var (Task 215): the PEM cert chain the WSS bridge terminates TLS
/// with. Paired with [`ENV_WSS_TLS_KEY_PATH`]. Unset ⇒ an ephemeral self-signed
/// cert is generated (dev / loopback only; production supplies a real cert).
const ENV_WSS_TLS_CERT_PATH: &str = "WSS_TLS_CERT_PATH";
/// Additive env var (Task 215): the PEM private key paired with the cert above.
const ENV_WSS_TLS_KEY_PATH: &str = "WSS_TLS_KEY_PATH";
/// SAN for the self-signed fallback cert when no operator cert is supplied.
const WSS_SELF_SIGNED_SAN: &str = "localhost";

/// Build flags beyond which there are none (Twelve-Factor strictness). We hand-
/// roll `--help` / `--version` so the binary pulls no arg parser and stays env-
/// only.
fn print_help() {
    let pkg = env!("CARGO_PKG_NAME");
    let ver = env!("CARGO_PKG_VERSION");
    println!(
        "\
{pkg} {ver} — Concerto self-hosted relay (embeds iroh-relay; design/11 §3.2, §6.3)

Twelve-Factor: configured by ENVIRONMENT VARIABLES only. No config file, no
flags other than --help / --version.

USAGE:
    {pkg}                 # reads config from the environment, then runs

ENVIRONMENT VARIABLES (design/11 §6.3):
    {ENV_RELAY_LISTEN_ADDR}             host:port for the iroh-relay HTTP server
                                  (the relay protocol endpoint). Default 0.0.0.0:80.
    {ENV_WSS_LISTEN_ADDR}               host:port for the WSS<->Iroh bridge
                                  (design/11 §3.4 Path B). Set ⇒ the bridge serves
                                  wss://<host>/wss/<endpoint_id>; unset ⇒ Iroh-only.
    {ENV_WSS_TLS_CERT_PATH}             PEM cert chain the WSS bridge terminates TLS
                                  with. Unset ⇒ ephemeral self-signed (dev only).
    {ENV_WSS_TLS_KEY_PATH}              PEM private key paired with the cert above.
    {ENV_WEBHOOK_LISTEN_ADDR}           host:port for the inbound-webhook route
                                  (design/11 §3.4.1). Set ⇒ the relay serves
                                  POST /webhook/github/<endpoint_id>, opening an
                                  ephemeral 0x04 Webhook bidi to the addressed
                                  Core; unset ⇒ no webhook route. Reuses the WSS
                                  TLS cert/key above.
    {ENV_MAX_ROUTES}                  max routing-table entries (a node handles
                                  10k-50k). Default 50000.
    {ENV_BANDWIDTH_CAP_PER_ENDPOINT}   max forwarded bytes per endpoint. Unset =
                                  unlimited.
    {ENV_PROMETHEUS_LISTEN_ADDR}       host:port for the Prometheus /metrics endpoint.
                                  Default 0.0.0.0:9090.

The relay is ciphertext-only (design/11 §3.9): it forwards encrypted QUIC and
exposes only metadata (routes, bytes forwarded, hole-punch success by region)."
    );
}

fn main() -> ExitCode {
    // Hand-rolled flag handling — env-only otherwise (Twelve-Factor).
    let mut args = std::env::args().skip(1);
    if let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--version" | "-V" => {
                println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!(
                    "error: unexpected argument '{other}'. {} is configured by environment \
                     variables only (Twelve-Factor); run with --help.",
                    env!("CARGO_PKG_NAME")
                );
                return ExitCode::FAILURE;
            }
        }
    }

    init_tracing();

    // Parse + validate config first so a misconfigured deploy fails fast BEFORE
    // we touch the network (`design/11 §6.3`).
    let config = match RelayConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: building tokio runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    runtime.block_on(async move {
        let relay = match Relay::start(config).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: starting relay: {e}");
                return ExitCode::FAILURE;
            }
        };

        // WSS↔Iroh bridge (Task 215, `design/11 §3.4`): opt-in on the reserved
        // `WSS_LISTEN_ADDR`. Unset ⇒ Iroh-only relay (unchanged from Task 214).
        // Kept alive for the relay's lifetime; its background loop runs until the
        // relay's shutdown token fires.
        // The WSS bridge + the inbound-webhook route (Task 315) share the same
        // outer TLS material; build it once. Both are opt-in (their respective
        // listen-addr env vars); unset ⇒ that path is simply not served.
        let tls = match build_wss_tls() {
            Ok(tls) => tls,
            Err(e) => {
                eprintln!("error: loading WSS/webhook TLS material: {e}");
                return ExitCode::FAILURE;
            }
        };
        let _wss_bridge = match relay.start_wss_bridge(tls.clone()).await {
            Ok(Some(bridge)) => {
                tracing::info!(wss_listen = %bridge.local_addr(), "WSS bridge listening");
                Some(bridge)
            }
            Ok(None) => None, // WSS_LISTEN_ADDR unset — Iroh-only.
            Err(e) => {
                eprintln!("error: starting WSS bridge: {e}");
                return ExitCode::FAILURE;
            }
        };
        // The inbound-webhook route (`design/11 §3.4.1`): opt-in on
        // `WEBHOOK_LISTEN_ADDR`. Kept alive for the relay's lifetime.
        let _webhook_route = match relay.start_webhook_route(tls).await {
            Ok(Some(route)) => {
                tracing::info!(webhook_listen = %route.local_addr(), "webhook route listening");
                Some(route)
            }
            Ok(None) => None, // WEBHOOK_LISTEN_ADDR unset — no webhook route.
            Err(e) => {
                eprintln!("error: starting webhook route: {e}");
                return ExitCode::FAILURE;
            }
        };

        if let Err(e) = relay.run_until_signal().await {
            eprintln!("error: relay exited with error: {e}");
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    })
}

/// Build the WSS bridge's TLS material (Task 215): the operator's PEM cert/key
/// from `WSS_TLS_CERT_PATH` / `WSS_TLS_KEY_PATH` if both are set, otherwise an
/// ephemeral self-signed pair (dev / loopback only — not browser-trusted). Only
/// consulted when `WSS_LISTEN_ADDR` is set; cheap to build unconditionally.
fn build_wss_tls() -> Result<WssTlsConfig, RelayError> {
    let cert_path = std::env::var(ENV_WSS_TLS_CERT_PATH).ok();
    let key_path = std::env::var(ENV_WSS_TLS_KEY_PATH).ok();
    match (cert_path, key_path) {
        (Some(cert), Some(key)) => {
            let cert_pem = std::fs::read(&cert).map_err(|e| {
                RelayError::Config(format!("{ENV_WSS_TLS_CERT_PATH}='{cert}': {e}"))
            })?;
            let key_pem = std::fs::read(&key)
                .map_err(|e| RelayError::Config(format!("{ENV_WSS_TLS_KEY_PATH}='{key}': {e}")))?;
            Ok(WssTlsConfig { cert_pem, key_pem })
        }
        (None, None) => WssTlsConfig::self_signed(WSS_SELF_SIGNED_SAN),
        _ => Err(RelayError::Config(format!(
            "{ENV_WSS_TLS_CERT_PATH} and {ENV_WSS_TLS_KEY_PATH} must be set together (or both unset for a self-signed dev cert)"
        ))),
    }
}

/// Initialize tracing from `RUST_LOG` (default `info`). iroh-relay emits WARNs
/// for normal send-queue backpressure under load; the default filter keeps the
/// relay's own logs at info and quiets that noise.
fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,iroh_relay::server::client=off"));
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
}
