# Task 49 — macOS launchd Install / Uninstall

| Field | Value |
|---|---|
| Phase | 4 |
| Size | small (≤4h) |
| Depends on | 11 |
| Touches subsystem(s) | 01 (Runtime), 18 (Distribution) |
| Smoke gate | unchanged |

## Goal
Add the launchd LaunchAgent plist + install/uninstall scripts so `concerto-core` runs as a user agent on macOS, surviving login/logout cycles. After this task, a developer can `make install` (or run `scripts/install-macos.sh`) and the Core is automatically started by launchd at login.

## Inputs to read before starting
- `design/01_Core_Daemon_Runtime.md` §3.1 (OS-integration daemonization), §10 (testing — launchd per-platform integration).
- `design/00_Architecture_Overview.md` §10 (V0.1 row: macOS launchd only).

## Scope — in
- Create `dist/macos/com.concerto.core.plist`:
  - `Label`: `com.concerto.core`.
  - `ProgramArguments`: path to the installed `concerto-core` binary.
  - `RunAtLoad`: true.
  - `KeepAlive`: true (with `Crashed: true`; restart on crash but not on clean exit).
  - `StandardOutPath` / `StandardErrorPath`: `~/concerto/logs/launchd-{out,err}.log`.
  - `EnvironmentVariables`: minimal — `HOME` is set by launchd.
  - `ProcessType`: `Interactive`.
- Create `scripts/install-macos.sh`:
  - Builds `concerto-core` (release).
  - Copies binary to `/usr/local/bin/concerto-core` (or `~/Applications/concerto/concerto-core` — pick a path; document why).
  - Templates the plist with the binary path; writes to `~/Library/LaunchAgents/com.concerto.core.plist`.
  - Runs `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.concerto.core.plist`.
  - Verifies the service is running via `launchctl print gui/$(id -u)/com.concerto.core`.
- Create `scripts/uninstall-macos.sh`:
  - Runs `launchctl bootout gui/$(id -u)/com.concerto.core`.
  - Removes the plist.
  - Optionally (with `--purge`) removes `~/concerto/`, `~/.concerto/`.
- Add a `Makefile` (or extend if it exists) with `install` / `uninstall` targets that call these scripts on macOS, or `make install-linux` for systemd later (V1.0).
- Document the choice between `/usr/local/bin` (system-wide, requires sudo) and `~/Applications/concerto/` (per-user, no sudo) in Handoff Notes; the install script defaults to `~/Applications/concerto/` to avoid sudo.

## Scope — out
- Linux systemd unit (V1.0).
- Windows Service Manager (V1.0).
- Pkg installer (.pkg / signed installer is Task 53 / V1.0 polish).
- Auto-update plumbing (Task 53).
- Tray sidecar registration (Task 48 covers in-process tray).

## Public interface this task locks
- Plist label: `com.concerto.core`.
- LaunchAgent location: `~/Library/LaunchAgents/com.concerto.core.plist`.
- Binary install path: `~/Applications/concerto/concerto-core` (per-user, default).
- Service control via `launchctl bootstrap` / `bootout` (`launchctl load` / `unload` deprecated on macOS 11+).

## Implementation notes
- Use heredoc in the install script to template the plist:
  ```sh
  cat > "$LAUNCH_AGENT_PATH" <<EOF
  <?xml version="1.0" encoding="UTF-8"?>
  ...
  EOF
  ```
- Use `plutil -lint` to validate the plist after writing.
- The uninstall must succeed even if the service was never installed (idempotent).
- For pre-existing instance: `launchctl print gui/$(id -u)/com.concerto.core 2>/dev/null` — bootstrap fails if already loaded. Bootout first; ignore errors; then bootstrap.

## Verification
1. `bash -n scripts/install-macos.sh && bash -n scripts/uninstall-macos.sh` → no syntax errors.
2. `shellcheck scripts/install-macos.sh scripts/uninstall-macos.sh` → clean.
3. `plutil -lint dist/macos/com.concerto.core.plist` (template) — note: the actual rendered plist is what runs; lint the rendered version too in a test script.
4. Manual on a Mac:
   - `scripts/install-macos.sh` → service starts; `launchctl list | grep concerto` shows it.
   - Reboot or `launchctl kickstart -k gui/$(id -u)/com.concerto.core` → service restarts.
   - `scripts/uninstall-macos.sh` → service stops, plist removed.
5. `scripts/smoke.sh` still passes (smoke gate uses ad-hoc-spawned Core, not the launchd one).

## Definition of Done
- [ ] Install + uninstall scripts work on a fresh Mac.
- [ ] `launchctl` reports the service active after install.
- [ ] `KeepAlive` restarts the Core if killed.
- [ ] Idempotent: re-running install doesn't break.
- [ ] No `TODO` / `FIXME` in scripts or plist.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `dist/macos/com.concerto.core.plist` (new — template)
- `scripts/install-macos.sh` (new, executable)
- `scripts/uninstall-macos.sh` (new, executable)
- `Makefile` (new or modified — `install` / `uninstall` targets)
- `dist/macos/README.md` (new — explains the install)

## Commit message
```
phase-4: macOS launchd install / uninstall

LaunchAgent plist at ~/Library/LaunchAgents/com.concerto.core.plist
runs concerto-core at login. install-macos.sh / uninstall-macos.sh
manage the service via launchctl bootstrap/bootout. Per-user
default install path (no sudo).

Refs: tasks/49-launchd-install.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no Linux / Windows install yet (V1.0); no .pkg installer (V1.0).
- **Smoke-gate state:** unchanged.
