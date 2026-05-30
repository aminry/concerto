# Embedded-Core Dev Loop, Daemon Control & Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a one-command hot-reload dev loop for frontend+Core against real data, an easy way to stop the standalone Core daemon, and a shippable "Concerto Embedded" edition (local build target + CI release artifact).

**Architecture:** Pure tooling/packaging on top of the existing `embedded-core` Cargo feature. A `cargo watch` wrapper script drives `pnpm tauri dev -f embedded-core`; a macOS shell script stops the launchd daemon and releases the PID lock; a Tauri config overlay gives the embedded build a distinct product name/identifier; the release workflow gains an `edition` matrix axis.

**Tech Stack:** Bash, GNU Make, Tauri 2 CLI, cargo-watch, GitHub Actions, launchd.

---

## File Structure

| Path | Responsibility | Action |
|---|---|---|
| `scripts/stop-core.sh` | Stop the launchd daemon + SIGTERM any bare PID-file process. | Create |
| `scripts/dev-embedded.sh` | Preflight (cargo-watch present, daemon-running warning) + exec the `cargo watch` dev loop. | Create |
| `apps/desktop/src-tauri/tauri.embedded.conf.json` | Config overlay: distinct product name + identifier for the embedded edition. | Create |
| `Makefile` | `dev-embedded` (real data), `dev-embedded-scratch`, `stop-core`, `build-embedded` targets + help. | Modify |
| `.github/workflows/release.yml` | Add `edition` matrix axis; parameterize build/sign/notarize by edition. | Modify |
| `README.md` | Document the new targets + embedded edition. | Modify |

---

## Task 1: `make stop-core` — stop the standalone daemon

**Files:**
- Create: `scripts/stop-core.sh`
- Modify: `Makefile`

- [ ] **Step 1: Create `scripts/stop-core.sh`**

```bash
#!/usr/bin/env bash
# Stop the standalone Concerto Core so embedded-real mode can take over.
#
#   1. launchctl bootout the LaunchAgent (best-effort; "not loaded" is fine).
#   2. If the PID lock still points at a live process (e.g. a bare,
#      directly-launched Core), SIGTERM it.
#
# macOS-only (launchd). Honors CONCERTO_CONFIG_DIR for the PID-file path.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
    echo "stop-core: macOS-only (launchd); nothing to do on $(uname -s)" >&2
    exit 0
fi

SERVICE_TARGET="gui/$(id -u)/com.concerto.core"
if launchctl bootout "$SERVICE_TARGET" 2>/dev/null; then
    echo "stop-core: stopped launchd service ($SERVICE_TARGET)"
else
    echo "stop-core: launchd service not loaded ($SERVICE_TARGET)"
fi

PID_FILE="${CONCERTO_CONFIG_DIR:-$HOME/.concerto}/core.pid"
if [ -f "$PID_FILE" ]; then
    # core.pid is JSON: {"pid":N,"version":"...","start_epoch_secs":N}.
    PID="$(sed -n 's/.*"pid"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$PID_FILE" | head -1)"
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
        kill "$PID"
        echo "stop-core: sent SIGTERM to bare Core process (pid $PID)"
    else
        echo "stop-core: PID lock present but no live process; nothing to kill"
    fi
fi

echo "stop-core: done"
```

- [ ] **Step 2: Make it executable and verify the no-op path**

```bash
chmod +x scripts/stop-core.sh
CONCERTO_CONFIG_DIR="$(mktemp -d)" ./scripts/stop-core.sh; echo "exit=$?"
```
Expected: prints "launchd service not loaded" (or "stopped" if you happen to have one), no PID-file message, "done", and `exit=0`.

- [ ] **Step 3: Verify the dead-PID path is a safe no-op**

```bash
TMPCFG="$(mktemp -d)"
printf '{"pid":999999,"version":"0.0.1","start_epoch_secs":1}' > "$TMPCFG/core.pid"
CONCERTO_CONFIG_DIR="$TMPCFG" ./scripts/stop-core.sh; echo "exit=$?"
```
Expected: prints "PID lock present but no live process; nothing to kill", "done", `exit=0` (PID 999999 is not live, so no `kill` error).

- [ ] **Step 4: Add the `stop-core` target to `Makefile`**

Add `stop-core` to the `.PHONY` line (line 9) and append this target at the end of the file:

```makefile
## Stop the standalone Core daemon (launchd) and release its PID lock so
## embedded-real mode can boot in-process. macOS-only.
stop-core:
	@./scripts/stop-core.sh
```

- [ ] **Step 5: Verify the target expands**

Run: `make -n stop-core`
Expected: prints `./scripts/stop-core.sh` (no Makefile parse error).

- [ ] **Step 6: Commit**

```bash
git add scripts/stop-core.sh Makefile
git commit -s -m "feat: add make stop-core to stop the standalone Core daemon"
```

---

## Task 2: Combined hot-reload dev loop (real data + cargo-watch)

**Files:**
- Create: `scripts/dev-embedded.sh`
- Modify: `Makefile`

- [ ] **Step 1: Create `scripts/dev-embedded.sh`**

```bash
#!/usr/bin/env bash
# Run the desktop app with Core embedded in-process, hot-reloading the
# frontend (Vite HMR), the src-tauri crate (Tauri's own watcher), AND
# crates/core (cargo watch, which restarts `tauri dev` on a Core change).
#
# Data root: real ~/concerto unless CONCERTO_HOME is set (the scratch
# variant sets it). Run `make stop-core` first if a standalone daemon is
# live, or embedded-real will detect the PID lock and dial it instead.
set -euo pipefail

# Preflight: cargo-watch must be installed.
if ! cargo watch --version >/dev/null 2>&1; then
    echo "dev-embedded: cargo-watch not found." >&2
    echo "  Install it with: cargo install cargo-watch" >&2
    exit 1
fi

# Warn (don't block) if a standalone daemon holds the lock in real-data mode.
if [ -z "${CONCERTO_HOME:-}" ]; then
    PID_FILE="${CONCERTO_CONFIG_DIR:-$HOME/.concerto}/core.pid"
    if [ -f "$PID_FILE" ]; then
        echo "dev-embedded: note — $PID_FILE exists; a standalone Core may be" >&2
        echo "  running. Run 'make stop-core' first so embedded mode boots" >&2
        echo "  in-process instead of dialing the daemon." >&2
    fi
fi

cd "$(dirname "$0")/../apps/desktop"
# cargo watch watches ONLY crates/core; Vite HMR + src-tauri rebuilds are
# handled by `tauri dev` itself. A crates/core edit restarts the whole
# dev session (full Core rebuild + relaunch).
exec cargo watch -w ../../crates/core -s 'pnpm tauri dev -f embedded-core'
```

- [ ] **Step 2: Make it executable and verify the cargo-watch preflight**

```bash
chmod +x scripts/dev-embedded.sh
# Simulate cargo-watch missing by shadowing PATH so `cargo` isn't found:
PATH=/usr/bin CONCERTO_HOME=/tmp/x ./scripts/dev-embedded.sh; echo "exit=$?"
```
Expected: prints the "cargo-watch not found" install hint and `exit=1`. (Do NOT run the script with cargo-watch present — it would launch the GUI and block. The `make -n` checks below cover the happy path.)

- [ ] **Step 3: Replace the `dev-embedded` target and add `dev-embedded-scratch` in `Makefile`**

Replace the existing `dev-embedded` target and its `##` comment block (current lines 45–54) with:

```makefile
## Run the desktop app with Core embedded in-process against your REAL
## ~/concerto data (the same folder the standalone daemon uses). Hot-reloads
## the frontend (Vite HMR), the src-tauri crate, and crates/core (via
## cargo watch). Run `make stop-core` first if the daemon is running.
## Requires cargo-watch: `cargo install cargo-watch`.
dev-embedded:
	@./scripts/dev-embedded.sh

## Same hot-reload loop, but against an isolated scratch data root so it
## never touches ~/concerto. Use for throwaway/isolated testing.
dev-embedded-scratch:
	@CONCERTO_HOME="$${CONCERTO_HOME:-$$(mktemp -d -t concerto-dev.XXXXXX)}" ./scripts/dev-embedded.sh
```

- [ ] **Step 4: Update `.PHONY` and `help` in `Makefile`**

Add `dev-embedded-scratch` to the `.PHONY` line (line 9). Replace the `dev-embedded` help line (line 18) with:

```makefile
	@echo "  make dev-embedded         Run desktop + embedded Core (real ~/concerto data)"
	@echo "  make dev-embedded-scratch Same, but against an isolated scratch data root"
	@echo "  make stop-core            Stop the standalone Core daemon (macOS)"
```

(Keep the existing `make smoke-embedded` help line.)

- [ ] **Step 5: Verify the targets expand**

Run: `make -n dev-embedded && make -n dev-embedded-scratch && make help`
Expected: `dev-embedded` expands to `./scripts/dev-embedded.sh`; `dev-embedded-scratch` expands to the `CONCERTO_HOME=... ./scripts/dev-embedded.sh` line; `make help` prints without parse errors and lists the new targets.

- [ ] **Step 6: Commit**

```bash
git add scripts/dev-embedded.sh Makefile
git commit -s -m "feat: dev-embedded hot-reloads frontend+core against real data"
```

---

## Task 3: Embedded edition config overlay + `make build-embedded`

**Files:**
- Create: `apps/desktop/src-tauri/tauri.embedded.conf.json`
- Modify: `Makefile`

- [ ] **Step 1: Create the config overlay**

Create `apps/desktop/src-tauri/tauri.embedded.conf.json`:

```json
{
  "productName": "Concerto Embedded",
  "identifier": "app.concerto.desktop.embedded"
}
```

- [ ] **Step 2: Verify it is valid JSON and carries the distinct identity**

```bash
node -e "const c=require('./apps/desktop/src-tauri/tauri.embedded.conf.json'); if(c.productName!=='Concerto Embedded'||c.identifier!=='app.concerto.desktop.embedded'){process.exit(1)}; console.log('overlay ok')"
```
Expected: prints `overlay ok`, exit 0.

- [ ] **Step 3: Add the `build-embedded` target to `Makefile`**

Add `build-embedded` to the `.PHONY` line and append at the end of the file:

```makefile
## Build a self-contained "Concerto Embedded" .app/.dmg (Desktop + Core in
## one binary) for local distribution. Unsigned. The config overlay gives it
## a distinct product name + identifier so it installs alongside the normal app.
build-embedded:
	cd apps/desktop && pnpm tauri build -f embedded-core \
		--config src-tauri/tauri.embedded.conf.json
```

Also add a help line in the `help:` block, after the `smoke-embedded` line:

```makefile
	@echo "  make build-embedded       Build the standalone 'Concerto Embedded' app bundle"
```

- [ ] **Step 4: Verify the target expands**

Run: `make -n build-embedded`
Expected: prints the `cd apps/desktop && pnpm tauri build -f embedded-core --config src-tauri/tauri.embedded.conf.json` command (no parse error). Do NOT run the full build here — a release `tauri build` is slow; the bundle-identity check is the manual verification in Step 5.

- [ ] **Step 5: (Manual, slow — macOS) Verify the produced bundle's identity**

This is the real end-to-end check; run it once on a Mac with the Tauri toolchain. It takes several minutes.

```bash
make build-embedded
APP="apps/desktop/src-tauri/target/release/bundle/macos/Concerto Embedded.app"
/usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" "$APP/Contents/Info.plist"
```
Expected: the bundle exists at "Concerto Embedded.app" and `CFBundleIdentifier` prints `app.concerto.desktop.embedded`. (If `bundle.active` ends up suppressing bundling in this Tauri version, append `--bundles app,dmg` to the build command; see the Risks note in the spec.)

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri/tauri.embedded.conf.json Makefile
git commit -s -m "feat: add Concerto Embedded edition overlay + make build-embedded"
```

---

## Task 4: Wire the embedded edition into the release workflow

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] **Step 1: Add the `edition` matrix axis**

Replace the `strategy:` block (current lines 29–36) with:

```yaml
    strategy:
      fail-fast: false
      matrix:
        target: [aarch64-apple-darwin, x86_64-apple-darwin]
        edition:
          - key: normal
            build_args: ""
            app: "Concerto.app"
          - key: embedded
            build_args: "-f embedded-core --config src-tauri/tauri.embedded.conf.json"
            app: "Concerto Embedded.app"
        include:
          - target: aarch64-apple-darwin
            arch: arm64
          - target: x86_64-apple-darwin
            arch: x64
```

This cross-products to 4 jobs (2 targets × 2 editions); the `include` entries attach `arch` to every edition of each target.

- [ ] **Step 2: Reflect the edition in the job name**

Change the job `name:` (current line 27) from:

```yaml
    name: bundle / ${{ matrix.target }}
```
to:
```yaml
    name: bundle / ${{ matrix.target }} (${{ matrix.edition.key }})
```

- [ ] **Step 3: Pass the edition's build args to `tauri build`**

Change the Build step's run line (current line 73) from:

```yaml
        run: pnpm tauri build --target ${{ matrix.target }}
```
to:
```yaml
        run: pnpm tauri build --target ${{ matrix.target }} ${{ matrix.edition.build_args }}
```

- [ ] **Step 4: Parameterize the Codesign + Notarize app paths**

Change BOTH `APP_PATH` env values (current lines 99 and 118) from:

```yaml
          APP_PATH: apps/desktop/src-tauri/target/${{ matrix.target }}/release/bundle/macos/Concerto.app
```
to:
```yaml
          APP_PATH: apps/desktop/src-tauri/target/${{ matrix.target }}/release/bundle/macos/${{ matrix.edition.app }}
```

- [ ] **Step 5: Confirm the sign/notarize scripts quote `APP_PATH`**

The embedded app path contains a space ("Concerto Embedded.app"). Check both scripts reference it quoted:

Run: `grep -n 'APP_PATH' scripts/sign-macos.sh scripts/notarize-macos.sh`
Expected: every use is `"$APP_PATH"` (double-quoted). If any unquoted use exists, quote it (edit that line so the spaced path survives word-splitting). The Upload step already globs `bundle/macos/*.app.tar.gz*` and `bundle/dmg/*.dmg`, so it needs no change — each leg uploads whatever product name it built.

- [ ] **Step 6: Verify the workflow is valid YAML**

```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release.yml')); print('yaml ok')"
```
Expected: prints `yaml ok`. If `actionlint` is installed, also run `actionlint .github/workflows/release.yml` and expect no errors. (The signed/notarized path itself is only exercised by a real `v*` tag with secrets — note this in the commit body.)

- [ ] **Step 7: Commit**

```bash
git add .github/workflows/release.yml
git commit -s -m "ci(release): build + ship the Concerto Embedded edition

Adds an edition matrix axis (normal | embedded); a tagged release now
produces four artifacts (2 arch x 2 editions). Full signed/notarized
path is only exercised on a real v* tag."
```

---

## Task 5: Document the new workflow in the README

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Update the "Embedded mode" section**

In `README.md`, find the embedded-mode section (the table of launch modes and the `make dev-embedded` block added previously). Replace the dev-loop + smoke paragraph (from "Fast dev loop" through the `scripts/smoke-embedded.sh` line) with:

````markdown
Fast dev loop — hot-reloads the frontend (Vite HMR), the `src-tauri` crate,
and `crates/core` (via `cargo watch`), running against your **real**
`~/concerto` data:

```sh
make stop-core      # stop the standalone daemon so embedded mode can boot
make dev-embedded   # requires: cargo install cargo-watch
```

`make dev-embedded-scratch` runs the same loop against an isolated scratch
data root instead. `make stop-core` stops the launchd daemon and releases its
PID lock (macOS).

To build a self-contained **Concerto Embedded** app (Desktop + Core in one
binary, installable alongside a normal Concerto) for people who don't want to
install Core separately:

```sh
make build-embedded
```

Tagged releases (`v*`) also publish signed `Concerto Embedded` artifacts
automatically. A headless smoke check for the embedded boot path lives at
`scripts/smoke-embedded.sh` (also `make smoke-embedded`).
````

- [ ] **Step 2: Verify the section reads correctly**

Run: `grep -n "make dev-embedded\|make stop-core\|make build-embedded\|Concerto Embedded" README.md`
Expected: shows the new commands and the "Concerto Embedded" mentions in the embedded-mode section.

- [ ] **Step 3: Commit**

```bash
git add README.md
git commit -s -m "docs: document dev-embedded real-data loop, stop-core, and embedded edition"
```

---

## Self-Review Notes

- **Spec coverage:** Component 1 (dev loop) → Tasks 2 (+5 docs); Component 2 (stop-core) → Task 1 (+5 docs); Component 3 (embedded edition: overlay + local build + CI release) → Tasks 3 & 4 (+5 docs); Testing section → per-task verification steps, with the slow/manual bundle-identity and tag-only release paths explicitly marked. Every spec decision (1, 2, 3, 4a, 4b) maps to a task.
- **Type/name consistency:** `scripts/stop-core.sh`, `scripts/dev-embedded.sh`, `tauri.embedded.conf.json`, the `matrix.edition.{key,build_args,app}` fields, and the product name "Concerto Embedded" / identifier `app.concerto.desktop.embedded` are used identically across tasks. The config path passed to `--config` is `src-tauri/tauri.embedded.conf.json` everywhere (relative to the `apps/desktop` working dir used by both `make build-embedded` and the release build step).
- **Placeholders:** none — every script and edit is shown in full; the only deferred verifications are explicitly labeled manual/slow (Task 3 Step 5) or tag-only (Task 4), with offline checks provided for both.
- **YAGNI:** no daemon auto-stop, no embedded updater channel, no Linux/Windows — matching the spec's non-goals.
