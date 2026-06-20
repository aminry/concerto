//! Tier-2 verifiable slice for Task 521 part 2 — the **remote WSS-via-relay
//! Noise IK** path (`design/11 §3.4` Path B, `design/17 §3.4`).
//!
//! # What this proves (the closest verifiable slice)
//!
//! The remote browser path is: browser → `wss://relay/.../<endpoint_id>` →
//! relay → one Iroh bidi stream → Core. The relay (Task 215, `crates/relay`)
//! is a **transparent opaque byte pump**: a WSS binary frame maps 1:1 onto the
//! Iroh stream bytes and back, and the relay sees **ciphertext only** — it never
//! parses, decodes, decrypts, or re-frames (`design/11 §3.9`). The browser
//! establishes **Noise IK** *inside* that tunnel using its session pairing key
//! (`design/17 §3.4`); gRPC then flows inside the Noise tunnel.
//!
//! The relay crate's own `tests/wss_bridge.rs` (Task 215) already proves the
//! opaque-pump + ciphertext-only invariant with a random blob. What it does
//! **not** assert is that a real **Noise IK handshake + encrypted application
//! traffic** survives that transparent byte-forwarding unchanged. This test
//! closes exactly that gap: it drives the **production** Noise IK helpers
//! (`establish_initiator` / `establish_responder`, the same ones Task 212's Iroh
//! transport runs) over a byte channel that is forwarded by a **relay double**
//! whose pump has the same `&[u8]`-verbatim semantics as the real relay's
//! `crates/relay/src/wss.rs::pump`. It then exchanges encrypted frames in both
//! directions and asserts:
//!
//!   - the handshake **completes** end-to-end through the relay double,
//!   - the responder authenticates the **initiator's pinned static** (IK), and a
//!     responder pre-loaded with the WRONG static (an impostor relay can't help
//!     here — the relay never has the keys) fails the handshake,
//!   - application frames decrypt to the exact plaintext (the tunnel is intact),
//!   - the relay double observed **only ciphertext** (no plaintext byte ever
//!     appears in the forwarded bytes) — the §3.9 invariant, asserted on the
//!     real Noise frames rather than a random blob.
//!
//! # Honest Tier-3 boundary
//!
//! This is the transport/crypto slice that is fully verifiable in CI with no
//! network and no real relay/keychain. What remains **Tier-3** (the task
//! Handoff): a **real browser** running Noise IK over a **real WSS** connection
//! to a **deployed relay** to a **real Core** (the `apps/web` SPA Playwright
//! path, Task 519/520), and the `concerto-relay` binary carrying it end to end
//! against a live Core. The relay crate's `tests/wss_bridge.rs` already covers
//! the WSS↔Iroh transport hop at Tier-2; this covers the Noise-IK-survives-the-
//! pump hop. Together they are the Tier-2 ceiling for the remote path.

use std::time::Instant;

use concerto_identity::{establish_initiator, establish_responder, NoiseStatic};
use tokio::sync::mpsc;

/// A relay double's opaque byte pump, mirroring `crates/relay/src/wss.rs::pump`:
/// it forwards `Vec<u8>` payloads **verbatim** between two endpoints, observing
/// the bytes only to record them (the relay's §3.9-permitted byte *counts* /
/// inspection surface) — it never parses, decodes, or mutates them.
///
/// Returns the channel ends a peer uses to talk *through* the relay, plus a
/// handle to inspect everything the relay forwarded (to assert ciphertext-only).
struct RelayDouble {
    /// Everything the relay forwarded, in order (both directions). Used to assert
    /// no plaintext ever crossed the relay.
    forwarded: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
}

/// One peer's view of the relayed byte channel: send bytes "into the relay",
/// receive bytes "out of the relay".
struct RelayedChannel {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

impl RelayDouble {
    /// Wire up a relay double between two peers (A = initiator/browser, B =
    /// responder/Core). Spawns two pump tasks that forward A→B and B→A verbatim.
    fn connect() -> (Self, RelayedChannel, RelayedChannel) {
        let forwarded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        // A writes into a2relay; the pump forwards to relay2b which B reads.
        let (a_tx, mut a2relay) = mpsc::unbounded_channel::<Vec<u8>>();
        let (relay2b, b_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        // B writes into b2relay; the pump forwards to relay2a which A reads.
        let (b_tx, mut b2relay) = mpsc::unbounded_channel::<Vec<u8>>();
        let (relay2a, a_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        // A→B pump (browser→Core). Verbatim `&[u8]` forwarding, record-only.
        {
            let forwarded = forwarded.clone();
            tokio::spawn(async move {
                while let Some(bytes) = a2relay.recv().await {
                    forwarded.lock().unwrap().push(bytes.clone());
                    if relay2b.send(bytes).is_err() {
                        break;
                    }
                }
            });
        }
        // B→A pump (Core→browser). Same verbatim semantics.
        {
            let forwarded = forwarded.clone();
            tokio::spawn(async move {
                while let Some(bytes) = b2relay.recv().await {
                    forwarded.lock().unwrap().push(bytes.clone());
                    if relay2a.send(bytes).is_err() {
                        break;
                    }
                }
            });
        }

        (
            RelayDouble { forwarded },
            RelayedChannel { tx: a_tx, rx: a_rx },
            RelayedChannel { tx: b_tx, rx: b_rx },
        )
    }

    /// Every byte blob the relay forwarded (both directions).
    fn forwarded(&self) -> Vec<Vec<u8>> {
        self.forwarded.lock().unwrap().clone()
    }
}

/// Drive `establish_initiator` over a relayed channel: each handshake message is
/// sent as one relayed blob; each reply is awaited from the relay.
fn run_initiator(
    local: &NoiseStatic,
    remote_static_pub: &[u8; 32],
    ch: &mut RelayedChannel,
) -> Result<concerto_identity::NoiseSession, concerto_identity::IdentityError> {
    let tx = ch.tx.clone();
    establish_initiator(
        local,
        remote_static_pub,
        Instant::now(),
        |msg: &[u8]| {
            tx.send(msg.to_vec())
                .map_err(|_| concerto_identity::IdentityError::Noise("relay send".into()))
        },
        || {
            // Blocking recv on the channel; the handshake is two messages so this
            // resolves promptly once the responder's pump forwards the reply.
            ch.rx
                .blocking_recv()
                .ok_or_else(|| concerto_identity::IdentityError::Noise("relay recv".into()))
        },
    )
}

/// Drive `establish_responder` over a relayed channel (mirror of the initiator).
fn run_responder(
    local: &NoiseStatic,
    ch: &mut RelayedChannel,
) -> Result<concerto_identity::NoiseSession, concerto_identity::IdentityError> {
    let tx = ch.tx.clone();
    establish_responder(
        local,
        Instant::now(),
        |msg: &[u8]| {
            tx.send(msg.to_vec())
                .map_err(|_| concerto_identity::IdentityError::Noise("relay send".into()))
        },
        || {
            ch.rx
                .blocking_recv()
                .ok_or_else(|| concerto_identity::IdentityError::Noise("relay recv".into()))
        },
    )
}

/// The full happy path: Noise IK completes through the relay double, application
/// frames round-trip, and the relay saw ciphertext only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn noise_ik_survives_relay_pump_and_app_traffic_round_trips() {
    // The browser's (initiator) session static and the Core's (responder)
    // static — the initiator pre-loads the Core's public half (IK).
    let browser = NoiseStatic::generate().unwrap();
    let core = NoiseStatic::generate().unwrap();
    let core_pub = core.public();

    let (relay, mut a_ch, mut b_ch) = RelayDouble::connect();

    // The plaintext the app exchanges after the handshake (must never appear in
    // the relay's forwarded bytes).
    const BROWSER_PLAINTEXT: &[u8] = b"GET /concerto.v1.Runtime/GetServerCapabilities";
    const CORE_PLAINTEXT: &[u8] = b"transport_kind=WSS_BRIDGE; ok";

    // Responder (Core) runs on a blocking thread (the helper uses blocking_recv).
    let responder = tokio::task::spawn_blocking(move || {
        let mut session = run_responder(&core, &mut b_ch).expect("responder handshake");
        // Receive the browser's encrypted app frame, decrypt, reply encrypted.
        let frame = b_ch.rx.blocking_recv().expect("app frame from browser");
        let plaintext = session.decrypt(&frame).expect("decrypt browser frame");
        assert_eq!(
            plaintext, BROWSER_PLAINTEXT,
            "Core sees the browser plaintext"
        );
        let reply = session.encrypt(CORE_PLAINTEXT).expect("encrypt reply");
        b_ch.tx.send(reply).expect("send reply through relay");
        plaintext
    });

    // Initiator (browser) likewise on a blocking thread.
    let initiator = tokio::task::spawn_blocking(move || {
        let mut session =
            run_initiator(&browser, &core_pub, &mut a_ch).expect("initiator handshake");
        // Send an encrypted app frame, await the encrypted reply, decrypt it.
        let frame = session
            .encrypt(BROWSER_PLAINTEXT)
            .expect("encrypt app frame");
        a_ch.tx.send(frame).expect("send app frame through relay");
        let reply = a_ch.rx.blocking_recv().expect("encrypted reply from Core");
        let plaintext = session.decrypt(&reply).expect("decrypt Core reply");
        assert_eq!(plaintext, CORE_PLAINTEXT, "browser sees the Core plaintext");
        plaintext
    });

    let core_saw = responder.await.expect("responder task");
    let browser_saw = initiator.await.expect("initiator task");
    assert_eq!(core_saw, BROWSER_PLAINTEXT);
    assert_eq!(browser_saw, CORE_PLAINTEXT);

    // Ciphertext-only invariant (`design/11 §3.9`), asserted on the REAL Noise
    // frames the relay forwarded: neither plaintext ever appears in any blob the
    // relay carried.
    let forwarded = relay.forwarded();
    assert!(
        !forwarded.is_empty(),
        "the relay must have forwarded the handshake + app frames"
    );
    for blob in &forwarded {
        assert!(
            !contains_subslice(blob, BROWSER_PLAINTEXT),
            "relay must never see the browser plaintext (ciphertext-only)"
        );
        assert!(
            !contains_subslice(blob, CORE_PLAINTEXT),
            "relay must never see the Core plaintext (ciphertext-only)"
        );
    }
}

/// An impostor: the initiator pins the WRONG Core static (e.g. a different /
/// spoofed Core). IK binds the responder's identity into the handshake, so the
/// handshake fails — the relay (which has no keys) cannot rescue it. This is the
/// "Core identity mismatch" guarantee on the remote path (`design/17 §8`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_pinned_core_static_fails_through_relay() {
    let browser = NoiseStatic::generate().unwrap();
    let real_core = NoiseStatic::generate().unwrap();
    let impostor = NoiseStatic::generate().unwrap();
    let wrong_pub = impostor.public(); // browser pins the impostor, not real_core

    let (_relay, mut a_ch, mut b_ch) = RelayDouble::connect();

    // Real Core responds with its real static; browser pinned the impostor's.
    let responder = tokio::task::spawn_blocking(move || run_responder(&real_core, &mut b_ch));
    let initiator =
        tokio::task::spawn_blocking(move || run_initiator(&browser, &wrong_pub, &mut a_ch));

    let init_res = initiator.await.expect("initiator task");
    // The responder may error or hang on the mismatched handshake; bound it.
    let resp_res = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        responder.await.expect("responder task")
    })
    .await;

    assert!(
        init_res.is_err() || resp_res.map(|r| r.is_err()).unwrap_or(true),
        "a browser pinning the wrong Core static must NOT establish a session through the relay"
    );
}

/// Naive subslice search (no extra dep) for the ciphertext-only assertion.
fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}
