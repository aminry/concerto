//! Tier-2 LOOPBACK integration test for the `ConcertoIroh` native module
//! (Task 509).
//!
//! **Double:** an Iroh-enabled in-process Core (booted via the test-harness env
//! pattern — `CONCERTO_ENABLE_IROH=1` + a fresh `CONCERTO_KEYCHAIN_SERVICE`),
//! relays disabled (direct loopback), driven entirely through the crate's PUBLIC
//! FFI surface (`pair` → `open_session` → `rpc_unary` / `rpc_stream` →
//! `nat_stats` → `close_session`). It mirrors `tools/split-host-loopback` and
//! `crates/core/tests/iroh_boot.rs`, but exercises the SHIPPED uniffi functions
//! rather than re-inlining the transport flow.
//!
//! **Belt-and-suspenders skip.** This test is NOT `#[cfg(target_os = "macos")]`
//! gated — it COMPILES + RUNS on every lane. When the booted Core has no Iroh
//! seam (`core.iroh()` is `None` — the keychain-less CI case, since the Iroh
//! boot path is keychain-backed and macOS-only in V1.0) it logs a skip and
//! returns cleanly (`Ok`/no panic). This is the same runtime degrade
//! split-host-loopback uses; it means the test is a real assertion on macOS and
//! a clean no-op everywhere else.
//!
//! **Nested-runtime note.** The crate's FFI functions block on their OWN global
//! multi-thread tokio runtime (`runtime().block_on(..)`). Calling `block_on`
//! from inside a tokio async context panics, so this test is a plain `#[test]`:
//! it builds a dedicated runtime for the Core boot/teardown and runs every FFI
//! call via `spawn_blocking` (whose threads are NOT inside an async context).

use std::sync::Arc;
use std::time::Duration;

use concerto_core::boot::{self, BootOutcome};
use concerto_core::runtime::RuntimeConfig;
use concerto_iroh_ffi::{
    cancel_subscription, close_session, nat_stats, open_session, pair, rpc_stream, rpc_unary,
    ConnectBlob, NatPath, PairingInputs, StreamEventCallback,
};
use concerto_transport::{direct_endpoint_addr, IrohTransport};

/// A callback that collects streamed event bytes into a shared vec, for the
/// `rpc_stream` assertion.
struct CollectingCallback {
    events: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    done: Arc<std::sync::atomic::AtomicBool>,
}

impl StreamEventCallback for CollectingCallback {
    fn on_event(&self, data: Vec<u8>) {
        self.events.lock().unwrap().push(data);
    }
    fn on_complete(&self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn on_error(&self, _message: String) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A callback that only tracks completion (the Clone stream is drained to EOS;
/// its progress frames are not asserted).
struct DrainCallback {
    done: Arc<std::sync::atomic::AtomicBool>,
    err: Arc<std::sync::Mutex<Option<String>>>,
}

impl StreamEventCallback for DrainCallback {
    fn on_event(&self, _data: Vec<u8>) {}
    fn on_complete(&self) {
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn on_error(&self, message: String) {
        *self.err.lock().unwrap() = Some(message);
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn loopback_pair_open_unary_stream_natstats_close() {
    // --- Keychain isolation + Iroh toggle (KEYCHAIN-IN-CI hazard) ----------
    std::env::set_var(
        "CONCERTO_KEYCHAIN_SERVICE",
        format!("concerto-test-{}-iroh-ffi-loopback", std::process::id()),
    );
    std::env::set_var("CONCERTO_ENABLE_IROH", "1");

    // A dedicated runtime for the Core boot/teardown (NOT the FFI runtime).
    let core_rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("core runtime");

    core_rt.block_on(async {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_dir = tmp.path().join("data");
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&config_dir).unwrap();

        let config = RuntimeConfig {
            data_dir,
            config_dir,
            shutdown_grace: Duration::from_secs(5),
        };

        // The Core boot resolves the `concerto-agent-host` sibling binary. When
        // this test is run via a bare `cargo test -p concerto-iroh-ffi` the
        // sibling bin may not be built; point boot at it if we can find it in the
        // standard target dir, else let boot's own resolution try.
        ensure_agent_host_env();

        // --- Belt-and-suspenders: a boot failure (missing sibling bin, no
        // keychain, sandboxed CI) is a clean SKIP, not a test failure. The
        // load-bearing assertions only run once a real Iroh-enabled Core is up.
        let core = match boot::start(config).await {
            Ok(BootOutcome::Started(c)) => c,
            Ok(BootOutcome::AlreadyRunning { pid }) => {
                panic!("unexpected live instance pid={pid}")
            }
            Err(e) => {
                eprintln!(
                    "loopback: SKIP — Core boot failed in this environment ({e}). \
                     The loopback Tier-2 check runs only where a full Core can boot \
                     (macOS + keychain + the concerto-agent-host sibling bin). Clean no-op."
                );
                return;
            }
        };

        // --- Belt-and-suspenders: skip cleanly if the Core has no Iroh ------
        let iroh = match core.iroh() {
            Some(iroh) => iroh,
            None => {
                eprintln!(
                    "loopback: SKIP — Core has no Iroh seam (core.iroh() is None; the Iroh \
                     boot path is keychain-backed + macOS-only in V1.0). Clean no-op."
                );
                shutdown(core).await;
                return;
            }
        };

        let server_transport: Arc<IrohTransport> = Arc::clone(&iroh.transport);
        let core_noise_pub = server_transport.core_noise_public();
        let endpoint_id = server_transport.endpoint_id().to_string();
        let server_addr = direct_endpoint_addr(&server_transport.endpoint())
            .await
            .expect("server iroh addr");

        // The direct (loopback) socket addrs the FFI rebuilds the EndpointAddr
        // from — relays disabled, so the direct addrs are the only path.
        let direct_addrs: Vec<String> = server_addr.ip_addrs().map(|sa| sa.to_string()).collect();
        assert!(
            !direct_addrs.is_empty(),
            "loopback server must advertise at least one direct addr"
        );

        // --- Arm a pairing (mints token + opens the 0x03 listener) ----------
        let challenge = iroh
            .pairing_responder
            .start_pairing()
            .expect("start_pairing");
        let pairing_token = hex::encode(challenge.pairing_token);
        let core_noise_pub_hex = hex::encode(core_noise_pub);

        let blob = ConnectBlob {
            endpoint_id: endpoint_id.clone(),
            relay_url: None, // loopback → no relay
            direct_addrs: direct_addrs.clone(),
            core_noise_pub: core_noise_pub_hex.clone(),
        };

        // === Drive the FFI surface (each call off the async context) ========

        // (1) generate keypair + pair() → signed cert.
        let kp = tokio::task::spawn_blocking(concerto_iroh_ffi::generate_device_keypair)
            .await
            .unwrap()
            .expect("generate_device_keypair");

        let pairing_inputs = PairingInputs {
            blob: blob.clone(),
            pairing_token,
            device_name: "iroh-ffi loopback".to_string(),
        };
        let seed = kp.seed.clone();
        let signed_cert = tokio::task::spawn_blocking(move || pair(pairing_inputs, seed))
            .await
            .unwrap()
            .expect("pair");
        assert!(
            signed_cert.len() > 1,
            "pair() must return a real signed cert, not a refusal byte"
        );

        // (2) open_session() → opaque handle.
        let blob_for_open = blob.clone();
        let cert_for_open = signed_cert.clone();
        let handle =
            tokio::task::spawn_blocking(move || open_session(blob_for_open, cert_for_open))
                .await
                .unwrap()
                .expect("open_session");
        assert!(handle >= 1, "handle ids start at 1");

        // (3) rpc_unary(GetServerCapabilities) → RAW bytes decode to IROH.
        let empty_req: Vec<u8> = prost::Message::encode_to_vec(&());
        let h = handle;
        let raw = tokio::task::spawn_blocking(move || {
            rpc_unary(
                h,
                "/concerto.v1.Runtime/GetServerCapabilities".to_string(),
                empty_req,
            )
        })
        .await
        .unwrap()
        .expect("rpc_unary GetServerCapabilities");

        // Decode the RAW response bytes OUT-OF-BAND (the module returned opaque
        // bytes) and assert transport_kind == IROH.
        use concerto_proto::v1::{ServerCapabilities, TransportKind};
        let caps = <ServerCapabilities as prost::Message>::decode(raw.as_slice())
            .expect("decode ServerCapabilities from raw bytes");
        assert_eq!(
            caps.transport_kind,
            TransportKind::Iroh as i32,
            "the raw unary response over Iroh must decode to transport_kind == IROH"
        );

        // (4) rpc_stream(Subscribe workspace.events) → >= 1 raw event. The
        // `workspace.events` subject is live-only (no snapshot on subscribe), so
        // we trigger exactly one event by creating a workspace — mirroring the
        // proven split-host-loopback flow: seed a local `file://` bare repo (no
        // network), AddRepository → Clone (drain) over the SAME FFI channel,
        // subscribe to `workspace.events`, then CreateWorkspace to emit the
        // `created` event. Each FFI call runs off the async context.
        let bare_repo = seed_bare_repo(tmp.path());

        // AddRepository → repo id (raw passthrough, decode the Repository reply).
        let add_req = encode_add_repo(&format!("file://{bare_repo}"));
        let h_add = handle;
        let repo_raw = tokio::task::spawn_blocking(move || {
            rpc_unary(
                h_add,
                "/concerto.v1.Repositories/AddRepository".to_string(),
                add_req,
            )
        })
        .await
        .unwrap()
        .expect("rpc_unary AddRepository");
        let repo_id = {
            use concerto_proto::v1::Repository;
            <Repository as prost::Message>::decode(repo_raw.as_slice())
                .expect("decode Repository")
                .id
        };

        // Clone (streaming) — drain to EOS via rpc_stream.
        let clone_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let clone_err = Arc::new(std::sync::Mutex::new(Option::<String>::None));
        let clone_cb = Box::new(DrainCallback {
            done: clone_done.clone(),
            err: clone_err.clone(),
        });
        let clone_req = encode_clone_request(&repo_id);
        let h_clone = handle;
        let _clone_sub = tokio::task::spawn_blocking(move || {
            rpc_stream(
                h_clone,
                "/concerto.v1.Repositories/Clone".to_string(),
                clone_req,
                clone_cb,
            )
        })
        .await
        .unwrap()
        .expect("rpc_stream Clone");
        for _ in 0..400 {
            if clone_done.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            clone_done.load(std::sync::atomic::Ordering::SeqCst),
            "Clone stream must reach EOS"
        );
        assert!(
            clone_err.lock().unwrap().is_none(),
            "Clone must not error: {:?}",
            clone_err.lock().unwrap()
        );

        // Subscribe to workspace.events.
        let events = Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cb = Box::new(CollectingCallback {
            events: events.clone(),
            done: done.clone(),
        });
        let subscribe_req = encode_subscribe_request("workspace.events");
        let h2 = handle;
        let sub_id = tokio::task::spawn_blocking(move || {
            rpc_stream(
                h2,
                "/concerto.v1.Streams/Subscribe".to_string(),
                subscribe_req,
                cb,
            )
        })
        .await
        .unwrap()
        .expect("rpc_stream Subscribe");

        // Give the subscription a beat to register before triggering the event.
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Trigger one workspace.events frame by creating a workspace.
        let cw_req = encode_create_workspace("iroh-ffi-ws", &repo_id);
        let h_cw = handle;
        let _ = tokio::task::spawn_blocking(move || {
            rpc_unary(
                h_cw,
                "/concerto.v1.Workspaces/CreateWorkspace".to_string(),
                cw_req,
            )
        })
        .await
        .unwrap()
        .expect("rpc_unary CreateWorkspace");

        // Wait (bounded) for at least one raw event.
        let mut got_event = false;
        for _ in 0..400 {
            if !events.lock().unwrap().is_empty() {
                got_event = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            got_event,
            "rpc_stream must deliver >= 1 raw event on workspace.events after \
             CreateWorkspace; none arrived"
        );

        // Cancel the subscription (drops the stream task).
        let h3 = handle;
        tokio::task::spawn_blocking(move || cancel_subscription(h3, sub_id))
            .await
            .unwrap();

        // (5) nat_stats() == Lan (loopback path).
        let stats = tokio::task::spawn_blocking(nat_stats).await.unwrap();
        assert_eq!(
            stats.path,
            Some(NatPath::Lan),
            "a loopback session classifies as Lan (got {:?})",
            stats.path
        );
        assert_eq!(stats.lan, 1, "exactly one live LAN session");

        // (6) close_session() → handle removed; a follow-up rpc_unary fails.
        let h4 = handle;
        tokio::task::spawn_blocking(move || close_session(h4))
            .await
            .unwrap();
        let h5 = handle;
        let after = tokio::task::spawn_blocking(move || {
            rpc_unary(
                h5,
                "/concerto.v1.Runtime/GetServerCapabilities".to_string(),
                Vec::new(),
            )
        })
        .await
        .unwrap();
        assert!(
            after.is_err(),
            "a closed handle must not serve further RPCs"
        );

        // nat_stats now empty.
        let stats_after = tokio::task::spawn_blocking(nat_stats).await.unwrap();
        assert_eq!(
            stats_after.lan, 0,
            "session count drops to zero after close"
        );

        // --- Clean shutdown (no leaked endpoint) ----------------------------
        shutdown(core).await;
        eprintln!("loopback: OK (paired, unary IROH, stream event, natStats Lan, closed)");
    });
}

/// Encode a `SubscribeRequest { subject }` to its prost bytes (the test builds
/// the typed request out-of-band; the FFI surface stays a pure byte
/// passthrough).
fn encode_subscribe_request(subject: &str) -> Vec<u8> {
    use concerto_proto::v1::SubscribeRequest;
    let req = SubscribeRequest {
        subject: subject.to_string(),
        filter: None,
        since_offset: None,
    };
    prost::Message::encode_to_vec(&req)
}

/// Encode an `AddRepoRequest` for a `file://` URL (full clone, no sparse).
fn encode_add_repo(url: &str) -> Vec<u8> {
    use concerto_proto::v1::AddRepoRequest;
    let req = AddRepoRequest {
        name: "iroh-ffi-repo".to_string(),
        url: url.to_string(),
        default_branch: "main".to_string(),
        ..Default::default()
    };
    prost::Message::encode_to_vec(&req)
}

/// Encode a `CloneRequest`.
fn encode_clone_request(repository_id: &str) -> Vec<u8> {
    use concerto_proto::v1::CloneRequest;
    let req = CloneRequest {
        repository_id: repository_id.to_string(),
    };
    prost::Message::encode_to_vec(&req)
}

/// Encode a `CreateWorkspaceRequest` referencing one repo (no sparse cones).
fn encode_create_workspace(name: &str, repository_id: &str) -> Vec<u8> {
    use concerto_proto::v1::{CreateWorkspaceRequest, WorkspaceRepoSpec};
    let req = CreateWorkspaceRequest {
        name: name.to_string(),
        repos: vec![WorkspaceRepoSpec {
            repository_id: repository_id.to_string(),
            sparse_cones: vec![],
        }],
        permission_mode: None,
        description: None,
        icon: None,
    };
    prost::Message::encode_to_vec(&req)
}

/// Seed a local bare git repo with one commit on `main` (a `file://` source — no
/// network) and return its path. Mirrors the split-host-loopback smoke seed.
fn seed_bare_repo(root: &std::path::Path) -> String {
    let bare = root.join("bare.git");
    let seed = root.join("seed");
    let run = |args: &[&str], cwd: Option<&std::path::Path>| {
        let mut c = std::process::Command::new("git");
        c.args(args);
        if let Some(d) = cwd {
            c.current_dir(d);
        }
        let out = c.output().expect("run git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "--bare", "--quiet", bare.to_str().unwrap()], None);
    run(
        &[
            "-C",
            bare.to_str().unwrap(),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ],
        None,
    );
    run(
        &[
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            seed.to_str().unwrap(),
        ],
        None,
    );
    std::fs::write(seed.join("README.md"), "# iroh-ffi loopback\n").unwrap();
    run(&["-C", seed.to_str().unwrap(), "add", "-A"], None);
    run(
        &[
            "-C",
            seed.to_str().unwrap(),
            "-c",
            "user.email=test@test",
            "-c",
            "user.name=Test",
            "commit",
            "-m",
            "seed",
            "--quiet",
        ],
        None,
    );
    run(
        &[
            "-C",
            seed.to_str().unwrap(),
            "push",
            "--quiet",
            "origin",
            "main",
        ],
        None,
    );
    bare.to_str().unwrap().to_string()
}

/// Point `CONCERTO_AGENT_HOST_BIN` at the built `concerto-agent-host` sibling if
/// it exists in the standard target dir and the var is not already set. Boot
/// resolves this binary; a bare `cargo test -p concerto-iroh-ffi` does not build
/// sibling bins, so we help it find one if a previous `cargo build` produced it.
/// If none is found we leave the var unset and let boot's own resolution try
/// (and, if it fails, the test SKIPs cleanly above).
fn ensure_agent_host_env() {
    if std::env::var_os("CONCERTO_AGENT_HOST_BIN").is_some() {
        return;
    }
    // The test binary lives at target/<profile>/deps/<name>; the agent-host bin
    // is at target/<profile>/concerto-agent-host. Walk up from the test exe.
    if let Ok(exe) = std::env::current_exe() {
        // .../target/debug/deps/loopback-XXXX  →  .../target/debug
        if let Some(profile_dir) = exe.parent().and_then(|p| p.parent()) {
            let candidate = profile_dir.join("concerto-agent-host");
            if candidate.exists() {
                std::env::set_var("CONCERTO_AGENT_HOST_BIN", candidate);
            }
        }
    }
}

/// Trigger an orderly shutdown and wait for the runtime to stop.
async fn shutdown(core: concerto_core::boot::RunningCore) {
    let token = core.shutdown_token();
    let join = tokio::spawn(async move { core.run_until_shutdown().await });
    token.cancel();
    let res = tokio::time::timeout(Duration::from_secs(10), join).await;
    assert!(res.is_ok(), "run_until_shutdown should return after cancel");
    res.unwrap().expect("join").expect("clean shutdown");
}
