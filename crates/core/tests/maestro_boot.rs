//! Task 7 (Maestro Live-Integration): proves `boot::start` wires the live
//! Maestro spine — the MCP listener binds its dedicated UDS, the CLI-facing
//! `.mcp.json` is written into the scratch dir naming the bridge command +
//! `--socket`, and boot completes cleanly even though no real `claude` binary
//! is present (the long-lived Maestro session degrades to inert, not fatal).
//!
//! Unix-only: the Maestro spine (and the rest of the UDS-bound Core) is
//! `#[cfg(unix)]`, so this test is gated the same way as `embedded_boot`.
//!
//! Isolation: the Maestro MCP socket (`~/.concerto/maestro-mcp.sock`) and the
//! scratch dir (`~/concerto/maestro/`) both resolve off `home::home_dir()`,
//! which honours `$HOME` on Unix. We point `$HOME` at the tempdir so this test
//! never touches (or collides with) a developer's real `~/.concerto`.

#![cfg(unix)]

use std::time::Duration;

use concerto_core::boot::{self, BootOutcome};
use concerto_core::runtime::RuntimeConfig;

#[tokio::test(flavor = "multi_thread")]
async fn boot_wires_maestro_mcp_listener_and_mcp_json() {
    // Isolate the Core-identity keychain access to a unique throwaway service
    // (same reason as `embedded_boot`: a headless macOS CI runner would
    // otherwise block on a Keychain Access prompt).
    std::env::set_var(
        "CONCERTO_KEYCHAIN_SERVICE",
        format!("concerto-test-{}-maestro-boot", std::process::id()),
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path().join("home");
    let data_dir = tmp.path().join("data");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();

    // Redirect every `home::home_dir()`-derived Maestro path into the tempdir.
    std::env::set_var("HOME", &home);

    // Make the agent-host bin resolution deterministic regardless of the test
    // exe's on-disk layout: `resolve_maestro_bridge_bin` reuses the agent-host
    // resolver and swaps the file stem, so pinning the override pins the bridge
    // path to the same target dir. `cargo_bin` guarantees the bin is built as a
    // dependency of this test (the canonical "find a workspace bin from a test"
    // pattern, mirrored from `agent_spawn`).
    let host_bin = assert_cmd::cargo::cargo_bin("concerto-agent-host");
    std::env::set_var("CONCERTO_AGENT_HOST_BIN", &host_bin);
    let expected_bridge = host_bin
        .parent()
        .expect("agent-host bin has a parent dir")
        .join("concerto-maestro-bridge");

    let config = RuntimeConfig {
        data_dir: data_dir.clone(),
        config_dir: config_dir.clone(),
        shutdown_grace: Duration::from_secs(5),
    };

    // (c) Boot completes without error even though no `claude` binary is
    // present — the Maestro session is best-effort and degrades to inert.
    let core = match boot::start(config).await.expect("boot::start should not fail") {
        BootOutcome::Started(c) => c,
        BootOutcome::AlreadyRunning { pid } => panic!("unexpected live instance pid={pid}"),
    };

    // (a) The maestro-mcp socket exists after boot (the listener bound it). The
    // listener binds inside a detached task that runs concurrently with
    // `boot::start` returning, so poll for it like `embedded_boot` polls the
    // gRPC UDS.
    let socket = home.join(".concerto").join("maestro-mcp.sock");
    let bound = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        bound.is_ok(),
        "maestro mcp listener should bind {} shortly after boot",
        socket.display()
    );

    // (b) `.mcp.json` exists in the scratch dir and names the bridge command +
    // `--socket`. The scratch write happens synchronously before `boot::start`
    // returns, so it is already present here.
    let mcp_json = home.join("concerto").join("maestro").join(".mcp.json");
    assert!(
        mcp_json.exists(),
        "maestro .mcp.json should be written to {}",
        mcp_json.display()
    );
    let body = std::fs::read_to_string(&mcp_json).expect("read .mcp.json");
    let v: serde_json::Value = serde_json::from_str(&body).expect(".mcp.json is valid JSON");

    // Locate the single registered MCP server entry (the composer keys it under
    // the Maestro server name) and assert its command + args.
    let servers = v
        .get("mcpServers")
        .and_then(|s| s.as_object())
        .expect(".mcp.json has an mcpServers object");
    assert_eq!(servers.len(), 1, "exactly one MCP server registered");
    let entry = servers.values().next().expect("one server entry");

    let command = entry
        .get("command")
        .and_then(|c| c.as_str())
        .expect("server entry has a string command");
    assert_eq!(
        command,
        expected_bridge.to_string_lossy(),
        "command should be the bridge bin sitting next to the agent-host bin"
    );

    let args: Vec<String> = entry
        .get("args")
        .and_then(|a| a.as_array())
        .expect("server entry has an args array")
        .iter()
        .map(|a| a.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        args.iter().any(|a| a == "--socket"),
        "args should pass --socket, got {args:?}"
    );
    let socket_str = socket.to_string_lossy().to_string();
    assert!(
        args.iter().any(|a| a == &socket_str),
        "args should name the maestro mcp socket path, got {args:?}"
    );

    // Clean shutdown (also tears down the detached listener task with the
    // runtime), matching `embedded_boot`'s lifecycle assertion.
    let token = core.shutdown_token();
    let join = tokio::spawn(async move { core.run_until_shutdown().await });
    token.cancel();
    let res = tokio::time::timeout(Duration::from_secs(10), join).await;
    assert!(res.is_ok(), "run_until_shutdown should return after cancel");
    res.unwrap().expect("join").expect("clean shutdown");
}
