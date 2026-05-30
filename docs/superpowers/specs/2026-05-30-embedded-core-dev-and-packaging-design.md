# Embedded-Core dev loop, daemon control, and packaging

**Status:** Design approved — ready for implementation planning
**Date:** 2026-05-30
**Branch:** `kill-port-5173-error` (continues the embedded-core feature; PR #59)

## Problem

The embedded-core mode (PR #59) added the ability to boot Core in-process under
the `embedded-core` Cargo feature. Three ergonomics/packaging gaps remain:

1. **No one-command dev loop that hot-reloads both frontend and Core.**
   `make dev-embedded` gives Vite HMR and `src-tauri` rebuilds, but edits to
   `crates/core` are not watched (Tauri's dev watcher only watches `src-tauri`),
   and the loop uses a throwaway scratch data dir rather than the real workspace.
2. **No easy way to stop the standalone Core daemon.** Running embedded mode
   against real data requires the launchd daemon to be stopped first (otherwise
   the PID lock makes embedded mode dial the daemon instead of embedding). The
   only existing control is a full uninstall.
3. **No shippable single-binary artifact.** People who don't want to install
   Core separately have no Desktop-with-embedded-Core build to download.

## Decisions (locked during brainstorming)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Dev-loop watch strategy | `cargo watch` scoped to `crates/core`, wrapping `pnpm tauri dev`. Vite HMR + `src-tauri` rebuilds stay with Tauri; only Core edits trigger a full restart. |
| 2 | Dev-loop data folder | `make dev-embedded` uses real `~/concerto`; the scratch sandbox moves to `make dev-embedded-scratch`. |
| 3 | "Kill the standalone core" scope | Stop the launchd service **and** SIGTERM any bare process holding the PID lock. |
| 4a | Where the shippable artifact is produced | Wire the embedded edition into the CI release workflow (`release.yml`), plus a local `make build-embedded` for convenience. |
| 4b | Embedded build identity | Distinct edition: `productName` "Concerto Embedded", `identifier` `app.concerto.desktop.embedded`; installable side-by-side with the normal app. |

## Component 1 — Combined dev loop

### `make dev-embedded` (real data)

```makefile
dev-embedded:
	cd apps/desktop && cargo watch -w ../../crates/core \
		-s 'pnpm tauri dev -f embedded-core'
```

- **No `CONCERTO_HOME`** → `embedded::resolve_mode` returns `EmbeddedReal` →
  Core boots against the real `~/concerto` / `~/.concerto` (the same data the
  standalone daemon uses).
- `cargo watch` watches **only `crates/core`**. On a change there it restarts
  the whole `tauri dev` (full Core rebuild + app relaunch — unavoidable since
  Core is a compiled dependency). Vite frontend HMR and `src-tauri` Rust
  rebuilds are handled by `tauri dev` itself and are unaffected.
- **Preflight checks** (small wrapper, e.g. `scripts/dev-embedded.sh` invoked by
  the target, or inline in the recipe):
  - If `cargo watch` is not installed, print `cargo install cargo-watch` and exit
    non-zero.
  - If `~/.concerto/core.pid` exists, print a one-line notice to run
    `make stop-core` first (embedded-real will otherwise detect the live daemon
    via the PID lock and dial it instead of embedding). This is a warning, not a
    hard stop.

### `make dev-embedded-scratch` (isolated sandbox)

Same `cargo watch` wrapper, but sets `CONCERTO_HOME` to a fresh tempdir so it
runs against an isolated data root (today's `dev-embedded` behavior). Core edits
hot-reload here too.

```makefile
dev-embedded-scratch:
	cd apps/desktop && CONCERTO_HOME=$${CONCERTO_HOME:-$$(mktemp -d -t concerto-dev.XXXXXX)} \
		cargo watch -w ../../crates/core -s 'pnpm tauri dev -f embedded-core'
```

`CONCERTO_HOME` is set in the outer shell before `cargo watch`, so it stays
stable across restarts (the tempdir is allocated once per `make` invocation).

The cargo-watch fallback comment currently in the `Makefile` is superseded by
these targets and should be simplified.

## Component 2 — `make stop-core` → `scripts/stop-core.sh`

macOS-only (mirrors the platform guard in `install-macos.sh`). Steps:

1. `launchctl bootout gui/$(id -u)/com.concerto.core` — best-effort; a non-zero
   exit ("service not loaded") is fine and reported as such. Stops the daemon
   for the session; it returns on next login unless uninstalled.
2. Resolve the PID file: `${CONCERTO_CONFIG_DIR:-$HOME/.concerto}/core.pid`.
   If it exists, extract the PID from the JSON record (`{"pid":N,"version":...,
   "start_epoch_secs":N}`) with a `jq`-free `sed`/`grep` (first integer after
   `"pid":`). If that PID is live (`kill -0`), `kill` (SIGTERM) it. Idempotent:
   missing file / dead PID are no-ops.

Exit 0 on success or when nothing was running. Surfaced via:

```makefile
stop-core:
	@./scripts/stop-core.sh
```

`.PHONY` and `help:` get the new target lines (`dev-embedded-scratch`,
`stop-core`, `build-embedded`).

## Component 3 — Embedded edition packaging

### Config overlay

New `apps/desktop/src-tauri/tauri.embedded.conf.json`, merged on top of the base
config via `tauri build --config`:

```json
{
  "productName": "Concerto Embedded",
  "identifier": "app.concerto.desktop.embedded"
}
```

Leaves the base `tauri.conf.json` and the normal release path untouched.
Produces `Concerto Embedded.app` and `Concerto Embedded_<version>_<arch>.dmg`,
installable side-by-side with a normally-installed Concerto.

### Local build — `make build-embedded`

```makefile
build-embedded:
	cd apps/desktop && pnpm tauri build -f embedded-core \
		--config src-tauri/tauri.embedded.conf.json
```

Produces an unsigned local bundle for testing/distribution without cutting a tag.

### CI release — `release.yml`

Add an `edition` axis to the build matrix, crossed with the existing arch
targets (`aarch64-apple-darwin`, `x86_64-apple-darwin`):

- `edition.key = normal`: build args `""`, app file `Concerto.app`.
- `edition.key = embedded`: build args
  `-f embedded-core --config src-tauri/tauri.embedded.conf.json`,
  app file `Concerto Embedded.app`.

Parameterize the existing steps by the edition:

- **Build Tauri bundle**: append the edition's build args to `pnpm tauri build`.
- **Codesign / Notarize**: `APP_PATH` derives from the edition's app filename
  (note the space in "Concerto Embedded.app" — quote paths). Same secrets.
- **Upload to release**: the glob over `bundle/macos/*.app.tar.gz(.sig)` and
  `bundle/dmg/*.dmg` already captures whichever product name was built, so each
  matrix leg uploads its own artifacts. Result: a tagged `v*` release publishes
  **four** artifacts (2 arch × 2 editions).

GitHub Actions matrix note: `edition` is a list of objects
(`[{key, build_args, app}, ...]`); reference fields as `matrix.edition.key`
etc. Preserve the existing per-target `arch` via the matrix `include`.

### Validation caveat

`release.yml` changes are only fully exercised by a real `v*` tag with signing
secrets. Offline-verifiable parts: YAML validity (and `actionlint` if available),
the config-overlay merge, and the local `make build-embedded` bundle. The
signed/notarized CI path itself is validated only on an actual release.

## Testing

**Headless / offline:**
- `make build-embedded` produces `Concerto Embedded.app`; assert its
  `Contents/Info.plist` carries `CFBundleIdentifier = app.concerto.desktop.embedded`
  and the "Concerto Embedded" product name.
- `scripts/stop-core.sh` exits 0 and is a no-op when no service is loaded and no
  PID file exists (run in an env with `CONCERTO_CONFIG_DIR` pointed at an empty
  tempdir). With a fake `core.pid` whose PID is dead, it must not error.
- `make -n dev-embedded`, `make -n dev-embedded-scratch`, `make -n stop-core`,
  `make -n build-embedded` expand without Makefile parse errors; `make help`
  still parses and lists the new targets.
- `release.yml` is valid YAML; `actionlint` clean if installed.

**Manual (needs a Mac / a tag):**
- `make dev-embedded` opens the app on an in-process Core against real
  `~/concerto`; editing a `crates/core` file triggers a rebuild + relaunch;
  editing frontend code hot-reloads without restart.
- `make stop-core` frees the PID lock so `make dev-embedded` embeds rather than
  dialing the daemon.
- A tagged release produces four signed artifacts (normal + embedded, per arch).

## Non-goals (YAGNI)

- Auto-stopping the daemon from `dev-embedded` (a warning is enough; stopping is
  explicit via `make stop-core`).
- Linux/Windows dev or packaging (V0.1 is macOS-only).
- A separate updater channel/manifest for the embedded edition (it ships as a
  download; auto-update parity is out of scope here).
- Split-process dev (persistent Vite + bare `cargo run`) — rejected in favor of
  the simpler `cargo watch` wrapper (decision 1).

## Risks

- **cargo-watch restart cleanliness.** Restarting `tauri dev` must release Vite's
  port 5173; `cargo watch` SIGTERMs the child, which normally tears down Vite.
  Documented as a known restart cost.
- **release.yml only testable on a real tag** (see validation caveat).
- **Bundling behavior.** The base config has `bundle.active: false`, yet the
  existing release relies on `tauri build` producing bundles — implying Tauri v2
  bundles by default here. The embedded build uses the same `tauri build` path,
  so it inherits whatever the normal release does; no change to bundling
  semantics is introduced.
