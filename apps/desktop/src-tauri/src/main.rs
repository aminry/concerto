// Suppress the extra console window on Windows in non-dev builds.
// V0.1 ships macOS-only so this is forward-compat hygiene only.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `concerto-desktop` — Tauri 2 shell entry point (Task 14, extended
//! by Task 24).
//!
//! Owns the native window, hosts the WebView renderer, and exposes
//! the renderer-facing Tauri commands (`concerto_rpc`,
//! `concerto_ping`, `concerto_subscribe`, `concerto_unsubscribe`).
//! All gRPC plumbing lives in `core_client.rs`; dispatch + the
//! subscription registry live in `commands.rs`.
//!
//! Phase scope:
//!
//! - V0.1 (Task 14): window opens, renderer round-trips
//!   `Runtime.GetServerCapabilities` over UDS.
//! - Phase 2 (Task 24): persistent gRPC channel, wider dispatcher
//!   (Projects.ListProjects, Workspaces.{List,Get},
//!   Workareas.GetWorkarea, Sessions.ListSessions stub), and the
//!   `concerto_subscribe`/`unsubscribe` bridge over Streams.
//! - Phase 4 (Task 49+, 53): launchd integration, auto-update, code
//!   signing.

mod commands;
mod core_client;
#[cfg(feature = "embedded-core")]
mod embedded;
mod tray;

fn main() {
    // Tauri 2's `tauri::Builder` runs the event loop on the calling
    // thread; the closure inside `.invoke_handler` is hot path only
    // for IPC, not init.
    tauri::Builder::default()
        // Task 53: auto-update. The plugin reads `plugins.updater` from
        // `tauri.conf.json`; with `endpoints: []` it is a no-op at
        // runtime, so unsigned / self-host builds (no manifest) don't
        // error. The renderer drives the check via
        // `@tauri-apps/plugin-updater` (see `src/hooks/useAutoUpdate.ts`).
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            #[cfg(feature = "embedded-core")]
            {
                use tauri::Manager;
                let args: Vec<String> = std::env::args().collect();
                let mode = embedded::resolve_mode(
                    &args,
                    std::env::var("CONCERTO_EMBEDDED").ok().as_deref(),
                    std::env::var("CONCERTO_HOME").ok().as_deref(),
                );
                // Block setup until Core is booting so the renderer's first
                // RPC never races the socket override being installed.
                let handle = tauri::async_runtime::block_on(embedded::start(mode));
                if let Some(h) = handle {
                    app.manage(h);
                }
            }
            commands::manage_subscriptions(app);
            // Task 48: install the menu-bar tray. The call also wires
            // close-to-hide on macOS and spawns the 5s status poll.
            tray::install(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::concerto_ping,
            commands::concerto_rpc,
            commands::concerto_subscribe,
            commands::concerto_unsubscribe,
            commands::clone_repository,
            commands::check_command,
        ])
        .run(tauri::generate_context!())
        .expect("error while running concerto-desktop");
}
