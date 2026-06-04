//! Generates the trivial `RelayRoute` gRPC service the Tier-2 relay-route double
//! (`tests/relay_route.rs`) drives over the Task-212 adapter + Noise, routed
//! through the in-process `concerto-relay`. tonic 0.12 codegen — the production
//! generator — so the test exercises real framing over the relay. The generated
//! module is included by the test via `tonic::include_proto!`.
//!
//! This is the ONLY codegen in the relay crate; the relay itself is proto-free
//! (it embeds iroh-relay's wire protocol — R-7 — and exposes a Prometheus text
//! endpoint, not gRPC).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/relay_route.proto"], &["proto"])?;
    Ok(())
}
