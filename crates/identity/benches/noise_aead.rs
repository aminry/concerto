//! Second-AEAD overhead benchmark (Task 208) — the line spike 102
//! (`design/spikes/tonic-iroh-findings.md` §3/§7) explicitly deferred.
//!
//! Spike 102 measured only Iroh's TLS pass and STUBBED the inner Noise IK
//! layer, noting Task 208 must benchmark the second AEAD pass. This bench
//! measures [`NoiseSession`] `encrypt`+`decrypt` throughput on:
//!
//! - **unary** — 64-byte payloads (the spike's unary `Echo` size), and
//! - **streaming** — 1 MiB chunks (the `session.io` chunk size, `design/10
//!   §5.2`).
//!
//! Reported MB/s is interpreted exactly as the spike interprets its numbers:
//! the bar is **`> 1 MB/s` for `session.io`** (`design/11 §10`). The AES-256-GCM
//! throughput is orders of magnitude above the bar, so the conclusion is "the
//! second AEAD pass does not breach the `session.io` bar" — it does not drag the
//! combined path under 1 MB/s vs the spike's ~70–230 MB/s Iroh-TLS streaming
//! numbers.
//!
//! **Not a CI gate.** CI runs `cargo bench --no-run` (compile only); loopback
//! timing is environment-sensitive (Task 102 treats sub-ms / throughput numbers
//! the same way). Run measurements manually with `cargo bench -p
//! concerto-identity`. Reported numbers in the Task 208 Handoff name the host
//! class (Apple M-series), per the spike's honesty discipline.

use std::time::Instant;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use concerto_identity::{NoiseIkHandshake, NoiseSession, NoiseStatic};

/// Establish a loopback initiator/responder session pair for benching.
fn establish_pair() -> (NoiseSession, NoiseSession) {
    let dev = NoiseStatic::generate().expect("device static");
    let core = NoiseStatic::generate().expect("core static");
    let core_pub = core.public();

    let mut ini = NoiseIkHandshake::initiator(&dev, &core_pub).expect("initiator");
    let mut res = NoiseIkHandshake::responder(&core).expect("responder");

    let m1 = ini.write_message(&[]).expect("m1");
    res.read_message(&m1).expect("read m1");
    let m2 = res.write_message(&[]).expect("m2");
    ini.read_message(&m2).expect("read m2");

    let now = Instant::now();
    (
        ini.into_session(now).expect("ini session"),
        res.into_session(now).expect("res session"),
    )
}

/// A Noise transport message is capped at 65535 bytes (incl. the 16-byte AEAD
/// tag), so a bulk transfer is split into ≤ `NOISE_FRAME` plaintext frames —
/// exactly how Task 212's transport will chunk a `session.io` payload onto the
/// inner AEAD. 64 KiB minus tag headroom.
const NOISE_FRAME: usize = 64 * 1024 - 64;

/// One encrypt (sender) + decrypt (receiver) round over `payload`, split into
/// ≤ `NOISE_FRAME` Noise frames — the full second-AEAD pass a transfer incurs
/// end-to-end. A pinned clock keeps the rekey timer from tripping
/// mid-measurement so we bench steady-state AEAD, not rekey.
fn aead_roundtrip(
    sender: &mut NoiseSession,
    receiver: &mut NoiseSession,
    payload: &[u8],
    at: Instant,
) {
    for chunk in payload.chunks(NOISE_FRAME) {
        let ct = sender.encrypt_at(chunk, at).expect("encrypt");
        let pt = receiver.decrypt_at(&ct, at).expect("decrypt");
        debug_assert_eq!(pt.len(), chunk.len());
    }
}

fn bench_second_aead(c: &mut Criterion) {
    let mut group = c.benchmark_group("noise_ik_second_aead");

    // The benchmark clock: pinned so the 1 h rekey timer never trips during a
    // measurement window. (Byte accounting can still trip at 1 GB, but no single
    // bench iteration moves 1 GB.)
    let at = Instant::now();

    // (a) unary-sized payloads (64 B, the spike's Echo size).
    {
        let payload = vec![0xABu8; 64];
        group.throughput(Throughput::Bytes(payload.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("unary_encrypt_decrypt", "64B"),
            &payload,
            |b, payload| {
                let (mut s, mut r) = establish_pair();
                b.iter(|| aead_roundtrip(&mut s, &mut r, payload, at));
            },
        );
    }

    // (b) streaming bulk (1 MiB chunks, the session.io chunk size).
    {
        let payload = vec![0xCDu8; 1024 * 1024];
        group.throughput(Throughput::Bytes(payload.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("streaming_encrypt_decrypt", "1MiB"),
            &payload,
            |b, payload| {
                let (mut s, mut r) = establish_pair();
                b.iter(|| aead_roundtrip(&mut s, &mut r, payload, at));
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_second_aead);
criterion_main!(benches);
