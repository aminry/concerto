# macOS LaunchAgent install

This directory contains the LaunchAgent template that runs `concerto-core`
as a per-user background service on macOS.

## Files

| Path | Purpose |
|---|---|
| `com.concerto.core.plist` | LaunchAgent template. The install script substitutes `__BIN_PATH__` and `__HOME__` and writes the result to `~/Library/LaunchAgents/com.concerto.core.plist`. |
| `../../scripts/install-macos.sh` | Build + install + bootstrap. |
| `../../scripts/uninstall-macos.sh` | Bootout + remove (with optional `--purge`). |

## Install

From the repo root:

```sh
make install            # or: ./scripts/install-macos.sh
```

This will:

1. `cargo build --release -p concerto-core`.
2. Copy the binary to `~/Applications/concerto/concerto-core` — per-user,
   no sudo. (We picked the per-user path over `/usr/local/bin` so the
   install never prompts for a password.)
3. Render the plist into `~/Library/LaunchAgents/com.concerto.core.plist`.
4. `launchctl bootout gui/$(id -u)/com.concerto.core` (best-effort), then
   `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.concerto.core.plist`.

Logs land at `~/concerto/logs/launchd-{out,err}.log`.

## Verify

```sh
launchctl print gui/$(id -u)/com.concerto.core
```

The service should be listed as running. `KeepAlive` is set to
`{ Crashed: true }`, so the agent will be restarted if Core crashes but
will not fight a clean shutdown.

## Uninstall

```sh
make uninstall                       # service + plist + binary
./scripts/uninstall-macos.sh --purge # also wipes ~/concerto + ~/.concerto
```

Both scripts are idempotent — re-running them on a clean machine is a
no-op.

## Why `bootstrap` / `bootout` instead of `load` / `unload`

`launchctl load` and `unload` are deprecated on macOS 11+. The modern
`bootstrap` / `bootout` verbs take a domain target (`gui/<uid>`) and a
service target (`gui/<uid>/com.concerto.core`) and play correctly with
the per-user GUI session.

## Plist lint

The install script runs `plutil -lint` on the rendered plist before
moving it into place. `plutil` ships with macOS, so this is always
available on the target platform; on non-Darwin hosts the lint step is
skipped (the script also refuses to run on non-Darwin up front).
