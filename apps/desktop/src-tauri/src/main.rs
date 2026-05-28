// Suppress the extra console window on Windows in non-dev builds.
// V0.1 ships macOS-only so this is forward-compat hygiene only.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `concerto-desktop` — Tauri 2 shell entry point (Task 14).
//!
//! Owns the native window, hosts the WebView renderer, and exposes the
//! `concerto_rpc` / `concerto_ping` Tauri commands. All gRPC plumbing
//! lives in `core_client.rs`; dispatch lives in `commands.rs`.
//!
//! Phase scope:
//!
//! - V0.1 (this task): window opens, renderer round-trips
//!   `Runtime.GetServerCapabilities` over UDS.
//! - Phase 2 (Task 24+): real workspace UI (shadcn/ui, Zustand,
//!   React Query, xterm.js).
//! - Phase 4 (Task 49+, 53): launchd integration, auto-update, code
//!   signing.

mod commands;
mod core_client;

fn main() {
    // Tauri 2's `tauri::Builder` runs the event loop on the calling
    // thread; the closure inside `.invoke_handler` is hot path only
    // for IPC, not init.
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::concerto_ping,
            commands::concerto_rpc,
        ])
        .run(tauri::generate_context!())
        .expect("error while running concerto-desktop");
}
