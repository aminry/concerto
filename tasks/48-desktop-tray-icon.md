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
- [ ] Tray icon appears and menu items work.
- [ ] Dynamic items (status, workareas) update every 5s.
- [ ] Closing the window does not quit; Quit menu does.
- [ ] Quitting the Desktop does not stop the Core.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** in-process tray; sidecar split is V1.0.
- **Smoke-gate state:** unchanged.
