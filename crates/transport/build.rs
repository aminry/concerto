//! Generates the trivial `Loopback` gRPC service the Tier-2 loopback double
//! (`tests/loopback.rs`) drives over the hand-rolled adapter + Noise. tonic 0.12
//! codegen — the production generator — so the test exercises real framing. The
//! generated module is included by the test via `tonic::include_proto!`.
//!
//! This is the ONLY codegen in the transport crate; the production transport is
//! proto-free (it serves the Core's services through the [`ApiDispatcher`] seam,
//! not its own proto).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/loopback.proto"], &["proto"])?;
    Ok(())
}
