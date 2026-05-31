//! `client` endpoint for the Iroh NAT-diversity spike (Task 101).
//!
//! Dials the `core`'s `EndpointId`, does a one-token ping/pong, then records —
//! from Iroh's own selected-path signal — whether the path is DIRECT (hole
//! punched) or RELAYED, plus the round-trip connect time and Iroh's per-path
//! detail. This is the side whose verdict feeds the findings matrix.
//!
//! Run on the machine playing the "client" role for a given network-matrix
//! row, passing the EndpointId the `core` printed:
//!
//!   cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin client -- <endpoint_id>
//!   cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin client -- <endpoint_id> --relay <url>

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Parser;
use iroh::EndpointId;
use iroh_nat_spike::{
    build_endpoint, init_tracing, observe_settled_path, RelayChoice, ALPN, PING, PONG,
};

#[derive(Parser, Debug)]
#[command(about = "Iroh NAT-diversity spike — client (dialing) endpoint")]
struct Args {
    /// The core's EndpointId (printed by the `core` binary).
    endpoint_id: String,

    /// Relay selection: must match the core's (`default`, `disabled`, or a
    /// custom relay URL).
    #[arg(long)]
    relay: Option<String>,

    /// How long (seconds) to let the path settle (hole-punch upgrade) before
    /// recording the verdict.
    #[arg(long, default_value_t = 8)]
    settle_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = Args::parse();
    let relay = RelayChoice::parse(args.relay.as_deref())?;
    let endpoint_id: EndpointId = args
        .endpoint_id
        .parse()
        .context("parsing core EndpointId argument")?;

    let endpoint = build_endpoint(&relay).await?;
    tracing::info!(self_id = %endpoint.id(), %endpoint_id, "dialing core");

    let connect_start = Instant::now();
    // Dial by EndpointId alone (no pre-shared socket address); n0 discovery +
    // relay resolve the route. This is the realistic remote case.
    let conn = endpoint
        .connect(endpoint_id, ALPN)
        .await
        .context("connecting to core")?;
    let connect_elapsed = connect_start.elapsed();

    // One-token ping/pong to prove the path actually carries application data.
    let (mut send, mut recv) = conn.open_bi().await.context("open_bi")?;
    send.write_all(PING).await.context("write ping")?;
    send.finish().context("finish send")?;
    let mut buf = [0u8; PONG.len()];
    recv.read_exact(&mut buf).await.context("read pong")?;
    if buf != PONG {
        bail!("unexpected echo from core: {:?}", buf);
    }

    let settle = Duration::from_secs(args.settle_secs);
    let kind = observe_settled_path(&conn, settle).await;

    println!();
    println!("=== iroh-nat-spike CLIENT result ===");
    println!("core EndpointId : {endpoint_id}");
    println!("relay mode      : {relay:?}");
    println!("PATH            : {}", kind.label());
    println!(
        "direct?         : {}",
        if kind.is_direct() { "YES" } else { "no" }
    );
    println!("connect time    : {connect_elapsed:?}");
    println!();
    println!(
        "MATRIX ROW      : direct={} | relayed={} | connect_ms={}",
        kind.is_direct(),
        kind == iroh_nat_spike::PathKind::Relayed,
        connect_elapsed.as_millis()
    );
    println!();

    conn.close(0u32.into(), b"done");
    endpoint.close().await;
    Ok(())
}
