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
    ENV_RELAY_LISTEN_ADDR, ENV_WSS_LISTEN_ADDR,
};
use concerto_relay::{Relay, RelayConfig};

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
    {ENV_WSS_LISTEN_ADDR}               host:port for the WSS bridge. RESERVED for
                                  Task 215 — parsed/validated here, not yet served.
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
        if let Err(e) = relay.run_until_signal().await {
            eprintln!("error: relay exited with error: {e}");
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    })
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
