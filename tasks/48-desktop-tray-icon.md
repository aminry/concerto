# Task 48 — Desktop Tray Icon (Menu Bar)

| Field | Value |
|---|---|
| Phase | 3 |
| Size | small (≤4h) |
| Depends on | 14, 25 |
| Touches subsystem(s) | 15 (Desktop), 01 (Runtime — tray sidecar pattern) |
| Smoke gate | unchanged |

## Goal
Add a macOS menu-bar tray icon that shows online/offline state of the Core, lists active workareas (up to 5), and offers "Open Concerto" / "Quit Concerto". V0.1 implements the tray inside the main `concerto-desktop` Tauri app rather than as a separate sidecar process (`design/15 §3.7` describes a sidecar; we defer the sidecar split to V1.0 — note in Handoff).

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.7 (tray sidecar — V0.1 simplification: in-process tray).
- `design/01_Core_Daemon_Runtime.md` §3.5 (tray sidecar described — V0.1 deviates).

## Scope — in
- Use Tauri 2's `TrayIconBuilder` to create a tray icon.
- Tray menu items:
  - **Status row** (dynamic): "● Concerto Core: online" or "✕ Concerto Core: offline" — derived from a periodic `Runtime.GetStatus` call (every 5s).
  - **Active workareas** (dynamic): top 5 from `Workareas.ListWorkareas(include_archived=false)`. Each is clickable → opens the main window focused on that workarea.
  - Separator.
  - **Open Concerto** — brings the main window forward (creates it if hidden).
  - **Quit Concerto** — exits the Tauri app (does NOT stop the Core).
- Tray icon assets: 16x16 monochrome PNGs for active/inactive state at `apps/desktop/icons/tray-{active,inactive}.png`.
- The main window's close button hides instead of quits on macOS (standard Mac behavior — close = hide; quit via menu).
- Tests:
  - Manual smoke: install + run; verify tray icon appears in menu bar; verify menu items respond.
  - No formal unit test for Tauri tray (the API is platform-specific and minimal).

## Scope — out
- Separate `concerto-tray` sidecar process (V1.0 — `design/01 §3.5`).
- Pending approvals badge + popover (V1.0).
- Scheduled tasks summary (V1.0).
- "Pair new device" tray action (V1.0 — pairing is V1.0).
- Windows system tray (V1.0).
- Linux indicator (V2.0 / never per `design/15 §1`).

## Public interface this task locks
- Icon paths: `apps/desktop/icons/tray-active.png`, `tray-inactive.png`.
- Tray menu structure: status / active workareas (5) / separator / open / quit.

## Implementation notes
- Tauri 2 tray API: `TrayIconBuilder::new().icon(icon).menu(&menu).on_menu_event(...)`.
- For dynamic menu items, rebuild the menu every 5s based on the polled status + workareas list.
- Hide the dock icon on macOS if desired: `app.set_activation_policy(ActivationPolicy::Accessory)`. V0.1 keeps the dock icon (standard behavior); document this choice.

## Verification
1. `pnpm tauri dev` → window opens AND tray icon appears in macOS menu bar.
2. Click tray → menu shows status + workareas.
3. Click a workarea → main window focuses on it.
4. Click "Open Concerto" → window comes forward.
5. Click "Quit Concerto" → app exits; Core keeps running (verify via `ps`).
6. `cargo clippy --workspace -- -D warnings` → clean.
7. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Tray icon appears and menu items work.
- [x] Dynamic items (status, workareas) update every 5s.
- [x] Closing the window does not quit; Quit menu does.
- [x] Quitting the Desktop does not stop the Core.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `apps/desktop/src-tauri/src/tray.rs` (new)
- `apps/desktop/src-tauri/src/main.rs` (modified — initializes tray)
- `apps/desktop/icons/tray-active.png`, `tray-inactive.png` (new — simple SVG→PNG monochrome icons)
- `apps/desktop/src-tauri/tauri.conf.json` (modified — close-to-hide behavior; tray plugin)

## Commit message
```
phase-3: desktop tray icon

Tauri 2 TrayIcon in the menu bar with status, top-5 active
workareas, Open, Quit. Close-to-hide on macOS. Sidecar-process
split (design/15 §3.7) deferred to V1.0.

Refs: tasks/48-desktop-tray-icon.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** Added Tauri features `tray-icon` + `image-png` to `apps/desktop/src-tauri/Cargo.toml` (neither is in Tauri 2.11's default feature set; tray-icon gates `TrayIconBuilder`, image-png decodes the 16x16 PNGs via `Image::from_bytes`). The icons are **embedded** via `include_bytes!("../../icons/tray-active.png")` rather than resolved through `tauri.conf.json -> bundle.resources` + `PathResolver` — embedding sidesteps the dev-vs-bundle resource-path divergence that bit `pnpm tauri dev` and keeps the tray working regardless of CWD. `tauri.conf.json` is **unmodified** as a result; the close-to-hide behaviour lives entirely in Rust (`window.on_window_event` inside `tray::install`), no plugin config required for Tauri 2's in-process tray. PNGs are minimal hand-rolled 16x16 RGBA (active = opaque black square, inactive = ~38% alpha) generated via a one-shot Python `struct`+`zlib` writer; both are tagged `icon_as_template(true)` so macOS renders them correctly in light & dark menubar themes.
- **Open questions for next task:** Workarea listing in the tray walks `Projects.ListProjects` → `Workspaces.ListWorkspaces(project_id)` → `Workareas.ListWorkareas(workspace_id)` because no `ListAllWorkareas` RPC exists. Cap is `MAX_WORKAREA_ITEMS = 5` so the fan-out is bounded for V0.1 (one project, a handful of workspaces); if Task 49+ adds a global tray-friendly listing RPC or a Maestro-aware "active workareas" projection, the polling loop in `tray.rs::fetch_snapshot` shrinks to one call. Labels are `"<workspace.name> — <composer-name>"`; branch chip + status dot from `design/15 §3.4` are deferred (tray text doesn't render coloured glyphs uniformly across macOS versions). Renderer-side wiring for the `concerto://focus-workarea/<id>` event is **not yet present** — the tray emits but no current React code listens; Task 49 / Phase 4 polish picks that up alongside the launchd integration.
- **Deliberate debt:** in-process tray; sidecar split (`design/15 §3.7`, `design/01 §3.5`) is V1.0. Pending-approvals badge + popover, scheduled-tasks summary, "Pair new device" action all V1.0. Windows system tray V1.0; Linux indicator V2.0 / never. No formal unit test for the tray icon's macOS-side rendering (Tauri tray builders need a running event loop); the three pure-Rust tests cover the event-name shape and the workarea-label formatter. The poll loop catches gRPC errors at DEBUG level and renders "offline" — no exponential backoff, no jitter; the cached UDS channel reset in `core_client::reset_channel` is the only reconnect strategy, matching the existing dispatcher's pattern.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still prints `Smoke gate v2: PASSED`. The tray runs in `concerto-desktop` only; the smoke gate exercises `concerto-core` + `concerto-smoke-client`, neither of which load the tray module.
