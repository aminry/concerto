//! Compiles `proto/bench.proto` with tonic 0.12 / prost 0.13 codegen — the
//! same generator the production Core uses — so the spike's framing overhead
//! is representative.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        // Map `bytes` fields to `bytes::Bytes` so the firehose clones chunks
        // zero-copy and the throughput number isn't dominated by per-chunk
        // allocation — representative of the product's `session.io` path.
        .bytes(["."])
        .compile_protos(&["proto/bench.proto"], &["proto"])?;
    Ok(())
}
