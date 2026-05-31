//! `core` endpoint for the Iroh NAT-diversity spike (Task 101).
//!
//! Stands up a long-lived Iroh endpoint, prints its `EndpointId` (the ticket
//! the `client` dials), and answers each incoming connection with a one-token
//! echo. On every accepted connection it logs whether the selected path is
//! direct or relayed and dumps Iroh's per-path detail.
//!
//! Run on the machine playing the "Core" role for a given network-matrix row:
//!
//!   cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin core
//!   cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin core -- --relay <url>

use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use iroh::endpoint::{Connection, Incoming};
use iroh::Watcher;
use iroh_nat_spike::{
    build_endpoint, init_tracing, observe_settled_path, RelayChoice, PING, PONG,
};

#[derive(Parser, Debug)]
#[command(about = "Iroh NAT-diversity spike — core (listening) endpoint")]
struct Args {
    /// Relay selection: `default` (n0 public relays), `disabled`, or a custom
    /// relay URL (e.g. an operator's throwaway `iroh-relay`). See the crate
    /// README for standing one up.
    #[arg(long)]
    relay: Option<String>,

    /// How long (seconds) to let each connection settle before recording its
    /// direct-vs-relay verdict.
    #[arg(long, default_value_t = 8)]
    settle_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let relay = RelayChoice::parse(args.relay.as_deref())?;

    let endpoint = build_endpoint(&relay).await?;
    let id = endpoint.id();

    // The home relay the endpoint registered with (the standby path clients
    // fall back to). Logged so the operator can confirm a relay is reachable.
    // Registration is async, so give it a few seconds to populate before
    // reporting (unless the operator disabled relays, in which case we don't
    // wait).
    report_home_relay(&endpoint, &relay).await;

    println!();
    println!("=== iroh-nat-spike CORE ready ===");
    println!("relay mode  : {relay:?}");
    println!("EndpointId  : {id}");
    println!();
    println!("On the CLIENT machine run:");
    println!(
        "  cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin client -- {id}{}",
        match &relay {
            RelayChoice::Custom(u) => format!(" --relay {u}"),
            RelayChoice::Disabled => " --relay disabled".to_string(),
            RelayChoice::Default => String::new(),
        }
    );
    println!();
    println!("(Ctrl-C to stop.)");
    println!();

    let settle = Duration::from_secs(args.settle_secs);

    loop {
        let Some(incoming) = endpoint.accept().await else {
            tracing::info!("endpoint closed; exiting");
            break;
        };
        tokio::spawn(async move {
            if let Err(err) = handle_connection(incoming, settle).await {
                tracing::warn!(?err, "connection handler error");
            }
        });
    }

    let _ = endpoint;
    Ok(())
}

/// Wait briefly for the endpoint to register a home relay and report it, so
/// the operator can confirm a relayed-fallback path is reachable for this row.
async fn report_home_relay(endpoint: &iroh::Endpoint, relay: &RelayChoice) {
    if matches!(relay, RelayChoice::Disabled) {
        tracing::info!("relay disabled by operator (direct-only)");
        return;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(url) = endpoint.watch_addr().get().relay_urls().next().cloned() {
            tracing::info!(%url, "home relay registered");
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("no home relay registered after 5s (relayed fallback may be unavailable)");
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn handle_connection(incoming: Incoming, settle: Duration) -> Result<()> {
    let conn: Connection = incoming.await.context("accepting incoming connection")?;
    let peer = conn.remote_id();
    tracing::info!(%peer, "incoming connection accepted");

    // Echo the client's ping on a single bidi stream.
    let (mut send, mut recv) = conn.accept_bi().await.context("accept_bi")?;
    let mut buf = [0u8; PING.len()];
    recv.read_exact(&mut buf).await.context("read ping")?;
    if buf == PING {
        send.write_all(PONG).await.context("write pong")?;
    } else {
        send.write_all(b"????").await.ok();
    }
    send.finish().context("finish send stream")?;

    // Let the path settle, then record the verdict from Iroh's own signal.
    let kind = observe_settled_path(&conn, settle).await;
    println!("CORE  : peer={peer} path={}", kind.label());

    // Keep the connection briefly so the client can read its verdict too.
    tokio::time::sleep(Duration::from_millis(500)).await;
    conn.close(0u32.into(), b"done");
    Ok(())
}
