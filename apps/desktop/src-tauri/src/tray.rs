//! macOS menu-bar tray icon (Task 48).
//!
//! The tray is hosted in-process by `concerto-desktop` for V0.1. The
//! sidecar-process split called out in `design/15 §3.7` and
//! `design/01 §3.5` is deferred to V1.0; running everything inside the
//! Tauri app keeps the wire shape simple (the renderer process and the
//! tray menu share the same persistent gRPC channel via
//! [`crate::core_client`]).
//!
//! Surface:
//! - Status row: dynamic text reflecting `Runtime.GetServerCapabilities`
//!   liveness (online / offline) — polled every [`POLL_INTERVAL`].
//! - Up to [`MAX_WORKAREA_ITEMS`] non-archived workareas, each clickable
//!   to focus the main window and emit a
//!   `concerto://focus-workarea/<id>` event the renderer listens for.
//! - Separator + "Open Concerto" + "Quit Concerto".
//!
//! The main window's close button is rebound to hide-on-close on macOS,
//! matching standard Mac behaviour (close = hide; quit via Cmd-Q or the
//! tray's Quit item). Quitting the Desktop does NOT stop the Core —
//! the Core daemon runs independently per `design/01 §3.1`.

use std::sync::Mutex;
use std::time::Duration;

use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::workareas_client::WorkareasClient;
use concerto_proto::v1::ListWorkareasRequest;
use serde::Serialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuEvent};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Emitter, Manager, Wry};
use tokio::time::sleep;

use crate::core_client::{default_socket_path, get_or_connect, reset_channel};

/// How often the tray polls the Core for status + workareas. 5s matches
/// the cadence called out in `tasks/48-desktop-tray-icon.md` — tight
/// enough to feel live, lazy enough to stay invisible in the CPU
/// profile.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Maximum number of workarea entries rendered in the tray menu. Top of
/// the `Workareas.ListWorkareas(include_archived=false)` response wins.
const MAX_WORKAREA_ITEMS: usize = 5;

/// Stable menu id for "Open Concerto".
const MENU_ID_OPEN: &str = "open-concerto";
/// Stable menu id for "Quit Concerto".
const MENU_ID_QUIT: &str = "quit-concerto";
/// Stable menu id for the status row (kept disabled — display-only).
const MENU_ID_STATUS: &str = "status";
/// Prefix for workarea menu ids; suffix is the workarea uuid so the
/// handler can extract the target id from the [`MenuEvent`] id alone.
const MENU_ID_WORKAREA_PREFIX: &str = "workarea:";

/// Tray icon assets — 16x16 monochrome PNGs baked into the binary. We
/// embed via `include_bytes!` so the tray works regardless of the
/// current working directory; Tauri's resource-path resolution would
/// otherwise require wiring `tauri.conf.json -> bundle.resources` and
/// the `tauri::path::PathResolver` lookup.
const TRAY_ACTIVE_PNG: &[u8] = include_bytes!("../../icons/tray-active.png");
const TRAY_INACTIVE_PNG: &[u8] = include_bytes!("../../icons/tray-inactive.png");

/// Process-wide handle to the live tray icon. Populated by [`install`]
/// at startup; the poll loop reads from this slot every tick to rebuild
/// the menu in place via `TrayIcon::set_menu` / `set_icon`.
static TRAY: Mutex<Option<tauri::tray::TrayIcon<Wry>>> = Mutex::new(None);

/// Snapshot the poll loop hands to the menu builder. Kept tiny on
/// purpose — the goal is "what does the tray need to render right
/// now", not "complete domain model".
#[derive(Debug, Clone)]
struct TraySnapshot {
    online: bool,
    workareas: Vec<TrayWorkarea>,
}

#[derive(Debug, Clone)]
struct TrayWorkarea {
    id: String,
    label: String,
}

/// Payload emitted to the renderer when a workarea item is clicked.
/// Renderer-side code listens on the event name returned by
/// [`focus_workarea_event_name`].
#[derive(Debug, Clone, Serialize)]
struct FocusWorkareaPayload {
    workarea_id: String,
}

/// Tauri event name the renderer subscribes to when a tray menu item
/// asks to focus a specific workarea.
fn focus_workarea_event_name(workarea_id: &str) -> String {
    format!("concerto://focus-workarea/{workarea_id}")
}

/// Install the tray icon, wire up the menu handler + close-to-hide on
/// macOS, and kick off the 5s poll loop. Call exactly once during
/// `tauri::Builder::setup`.
pub fn install(app: &mut App) -> tauri::Result<()> {
    // Initial menu reflects "offline + no workareas" — the poll loop
    // will overwrite within ~5s once the Core responds.
    let initial = TraySnapshot {
        online: false,
        workareas: Vec::new(),
    };
    let menu = build_menu(app.handle(), &initial)?;

    let tray = TrayIconBuilder::new()
        .icon(Image::from_bytes(TRAY_INACTIVE_PNG)?)
        // macOS template icons render correctly in both light and dark
        // menubar themes. The PNGs we ship are pure black-on-transparent
        // so flagging as template is safe.
        .icon_as_template(true)
        .tooltip("Concerto")
        .menu(&menu)
        .on_menu_event(handle_menu_event)
        .build(app)?;

    *TRAY.lock().expect("tray mutex poisoned") = Some(tray);

    // Rebind the main window's close button to hide-on-close on macOS
    // so the app keeps running in the tray. Other platforms keep the
    // standard close-quits behaviour.
    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window("main") {
        let window_for_handler = window.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Embedded: close means quit. Signal Core to stop (releases
                // PID lock, flushes audit, stops agents) and let the window
                // close — do NOT prevent_close, so the process exits.
                #[cfg(feature = "embedded-core")]
                {
                    if let Some(h) = window_for_handler
                        .app_handle()
                        .try_state::<crate::embedded::EmbeddedHandle>()
                    {
                        h.shutdown.cancel();
                        return;
                    }
                }
                // Reached when Core is external (no embedded handle in state)
                // or the lean build: keep the standard macOS close-to-hide.
                api.prevent_close();
                let _ = window_for_handler.hide();
            }
        });
    }

    // Spawn the poll loop. `tauri::async_runtime` reuses the tokio
    // runtime already running for the gRPC client, so this costs us a
    // single extra task — no new runtime.
    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        poll_loop(app_handle).await;
    });

    Ok(())
}

/// Top-level menu-event router. Stable ids only; unknown ids are
/// logged and dropped (a future menu addition that forgets to wire up
/// its handler stays loud in logs without crashing the tray).
fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id().as_ref();
    match id {
        MENU_ID_OPEN => {
            if let Err(e) = show_and_focus_main(app) {
                tracing::warn!(error = %e, "tray: failed to surface main window");
            }
        }
        MENU_ID_QUIT => {
            // V0.1: exiting the Desktop must not stop the Core daemon.
            // `app.exit(0)` only tears down this process; the Core's
            // launchd job (Task 49) is responsible for its own
            // lifecycle.
            app.exit(0);
        }
        other if other.starts_with(MENU_ID_WORKAREA_PREFIX) => {
            let workarea_id = other.trim_start_matches(MENU_ID_WORKAREA_PREFIX);
            if workarea_id.is_empty() {
                tracing::warn!("tray: workarea menu id missing payload");
                return;
            }
            if let Err(e) = show_and_focus_main(app) {
                tracing::warn!(error = %e, "tray: failed to surface main window for workarea");
            }
            let event_name = focus_workarea_event_name(workarea_id);
            if let Err(e) = app.emit(
                &event_name,
                FocusWorkareaPayload {
                    workarea_id: workarea_id.to_string(),
                },
            ) {
                tracing::warn!(error = %e, event = %event_name, "tray: failed to emit focus event");
            }
        }
        MENU_ID_STATUS => {
            // Status row is disabled — clicks shouldn't fire, but if
            // the platform synthesises one we silently ignore it.
        }
        unknown => {
            tracing::warn!(id = %unknown, "tray: unknown menu id");
        }
    }
}

/// Bring the main window forward — create-if-hidden semantics. On
/// macOS we explicitly call `show()` because the close-to-hide handler
/// leaves the window in `hidden` state.
fn show_and_focus_main(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

/// Render the dynamic menu for the current snapshot. Kept pure — no
/// I/O — so the poll loop and the initial install path share the
/// builder. The returned [`Menu`] is consumed by `TrayIcon::set_menu`.
fn build_menu(app: &AppHandle, snapshot: &TraySnapshot) -> tauri::Result<Menu<Wry>> {
    let status_text = if snapshot.online {
        "● Concerto Core: online"
    } else {
        "✕ Concerto Core: offline"
    };

    // The status row is rendered as a disabled text item — it's a
    // label, not a clickable action. Tauri's `MenuBuilder::text` adds
    // an enabled item; we fall through to `MenuItemBuilder` for the
    // `enabled(false)` knob.
    let status_item = tauri::menu::MenuItemBuilder::with_id(MENU_ID_STATUS, status_text)
        .enabled(false)
        .build(app)?;

    let mut builder = MenuBuilder::new(app).item(&status_item).separator();

    if snapshot.workareas.is_empty() {
        let empty = tauri::menu::MenuItemBuilder::with_id("workareas-empty", "No active workareas")
            .enabled(false)
            .build(app)?;
        builder = builder.item(&empty).separator();
    } else {
        for wa in &snapshot.workareas {
            let id = format!("{MENU_ID_WORKAREA_PREFIX}{}", wa.id);
            builder = builder.text(id, &wa.label);
        }
        builder = builder.separator();
    }

    let menu = builder
        .text(MENU_ID_OPEN, "Open Concerto")
        .text(MENU_ID_QUIT, "Quit Concerto")
        .build()?;
    Ok(menu)
}

/// Poll loop — every [`POLL_INTERVAL`] dial the Core, refresh the
/// snapshot, and ask the tray to rebuild its menu + swap its icon.
/// Errors here are logged at WARN and skipped; the user sees the
/// "offline" status row until the next tick succeeds.
async fn poll_loop(app: AppHandle) {
    loop {
        match fetch_snapshot().await {
            Ok(snapshot) => apply_snapshot(&app, &snapshot),
            Err(err) => {
                tracing::debug!(error = %err, "tray: poll failed; rendering offline");
                let offline = TraySnapshot {
                    online: false,
                    workareas: Vec::new(),
                };
                apply_snapshot(&app, &offline);
            }
        }
        sleep(POLL_INTERVAL).await;
    }
}

/// Hand the snapshot to the live `TrayIcon`. Locking the static is
/// fast; we never await while the guard is held.
fn apply_snapshot(app: &AppHandle, snapshot: &TraySnapshot) {
    let tray = {
        let guard = TRAY.lock().expect("tray mutex poisoned");
        guard.clone()
    };
    let Some(tray) = tray else { return };

    match build_menu(app, snapshot) {
        Ok(menu) => {
            if let Err(e) = tray.set_menu(Some(menu)) {
                tracing::warn!(error = %e, "tray: set_menu failed");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "tray: rebuild_menu failed");
        }
    }

    let icon_bytes = if snapshot.online {
        TRAY_ACTIVE_PNG
    } else {
        TRAY_INACTIVE_PNG
    };
    match Image::from_bytes(icon_bytes) {
        Ok(icon) => {
            if let Err(e) = tray.set_icon(Some(icon)) {
                tracing::warn!(error = %e, "tray: set_icon failed");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "tray: decode icon failed");
        }
    }
}

/// Single poll round-trip: `Runtime.GetServerCapabilities` for
/// liveness, then `Workareas.ListWorkareas` across each workspace the
/// user has open. V0.1 keeps the snapshot intentionally shallow — the
/// per-workspace list call is omitted because there's no
/// `ListAllWorkareas` RPC; instead we list every workspace and union
/// their non-archived workareas, capped at [`MAX_WORKAREA_ITEMS`].
async fn fetch_snapshot() -> Result<TraySnapshot, FetchError> {
    let socket = default_socket_path().ok_or(FetchError::NoSocketPath)?;
    let channel = match get_or_connect(&socket).await {
        Ok(ch) => ch,
        Err(e) => {
            reset_channel();
            return Err(FetchError::Transport(e.to_string()));
        }
    };

    // Liveness probe.
    let mut runtime = RuntimeClient::new(channel.clone());
    if let Err(status) = runtime.get_server_capabilities(()).await {
        reset_channel();
        return Err(FetchError::Rpc(format!(
            "GetServerCapabilities: {}: {}",
            status.code(),
            status.message()
        )));
    }

    // Workarea list. Workspaces are now a global, flat registry (the
    // Project layer was collapsed away), so we list every workspace once
    // and ask each for its workareas until we've filled
    // [`MAX_WORKAREA_ITEMS`]. The shape is small in V0.1 (a handful of
    // workspaces) so the cost is bounded.
    let mut workareas: Vec<TrayWorkarea> = Vec::new();
    let mut workspaces_client =
        concerto_proto::v1::workspaces_client::WorkspacesClient::new(channel.clone());
    let ws_list = workspaces_client
        .list_workspaces(concerto_proto::v1::ListWorkspacesRequest {
            include_archived: false,
        })
        .await
        .map_err(|s| {
            reset_channel();
            FetchError::Rpc(format!("ListWorkspaces: {}: {}", s.code(), s.message()))
        })?
        .into_inner();

    let mut workareas_client = WorkareasClient::new(channel.clone());
    for ws in &ws_list.workspaces {
        if workareas.len() >= MAX_WORKAREA_ITEMS {
            break;
        }
        let wa_list = workareas_client
            .list_workareas(ListWorkareasRequest {
                workspace_id: ws.id.clone(),
                include_archived: false,
            })
            .await
            .map_err(|s| {
                reset_channel();
                FetchError::Rpc(format!("ListWorkareas: {}: {}", s.code(), s.message()))
            })?
            .into_inner();
        for wa in wa_list.workareas {
            if workareas.len() >= MAX_WORKAREA_ITEMS {
                break;
            }
            let label = workarea_label(&ws.name, &wa);
            workareas.push(TrayWorkarea { id: wa.id, label });
        }
    }

    Ok(TraySnapshot {
        online: true,
        workareas,
    })
}

/// Render a single workarea menu label. Falls back to the composer
/// name when the workspace name + branch combo would be empty.
fn workarea_label(workspace_name: &str, workarea: &concerto_proto::v1::Workarea) -> String {
    let composer = if workarea.composer_name.is_empty() {
        "(unnamed)"
    } else {
        workarea.composer_name.as_str()
    };
    if workspace_name.is_empty() {
        composer.to_string()
    } else {
        format!("{workspace_name} — {composer}")
    }
}

/// Internal error type for the poll loop. We don't surface these to
/// the renderer — the tray simply renders "offline" until the next
/// successful poll.
#[derive(Debug, thiserror::Error)]
enum FetchError {
    #[error("HOME not set — cannot resolve ~/.concerto/core.sock")]
    NoSocketPath,
    #[error("transport: {0}")]
    Transport(String),
    #[error("rpc: {0}")]
    Rpc(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_workarea_event_name_is_stable() {
        assert_eq!(
            focus_workarea_event_name("abc-123"),
            "concerto://focus-workarea/abc-123"
        );
    }

    #[test]
    fn workarea_label_falls_back_when_composer_empty() {
        let wa = concerto_proto::v1::Workarea {
            composer_name: String::new(),
            ..Default::default()
        };
        let label = workarea_label("wsp", &wa);
        assert_eq!(label, "wsp — (unnamed)");
    }

    #[test]
    fn workarea_label_joins_workspace_and_composer() {
        let wa = concerto_proto::v1::Workarea {
            composer_name: "blue-otter".into(),
            ..Default::default()
        };
        let label = workarea_label("wsp", &wa);
        assert_eq!(label, "wsp — blue-otter");
    }
}
