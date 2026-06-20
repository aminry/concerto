# Concerto — Desktop (`apps/desktop/`)

Tauri 2 + React + Vite + Tailwind shell for the Concerto desktop client.

V0.1 scope (Task 14): a window opens, the renderer can round-trip
`Runtime.GetServerCapabilities` over UDS via the `concerto_rpc` Tauri
command. Real UI (three-panel layout, Monaco diff, xterm.js terminal)
lands in Phase 2 (Task 24+).

## Dev workflow

`apps/desktop` is a member of the root pnpm workspace (Task 523 — it consumes
the shared `@concerto/ui` inbox renderer + `@concerto/client`). Install from the
repo root, then run the dev server from here:

```sh
# from the repo root (one install for the whole JS workspace)
pnpm install

# from this directory
pnpm tauri dev
```

`pnpm tauri dev` builds the Rust shell (`concerto-desktop`), starts
the Vite dev server on `http://localhost:5173`, and opens the native
window pointed at it. Hot reload works for the renderer; the Rust
shell rebuilds on `Ctrl-C` + rerun.

The shell expects a running Core. In another terminal:

```sh
cargo run --bin concerto-core
```

The Core binds `~/.concerto/core.sock`; the shell connects to the same
path on every RPC call (no persistent client in V0.1 — see
`apps/desktop/src-tauri/src/core_client.rs`).

## Layout

```
apps/desktop/
├── package.json            # renderer deps + scripts
├── index.html              # Vite entry
├── tsconfig.json           # TS strict + bundler resolution
├── vite.config.ts          # Vite + @vitejs/plugin-react
├── tailwind.config.ts      # Tailwind v3 (locked, see Cargo.toml)
├── postcss.config.js
├── src/                    # React renderer
│   ├── main.tsx
│   ├── App.tsx
│   └── index.css
└── src-tauri/              # Rust shell (workspace member)
    ├── Cargo.toml
    ├── build.rs            # tauri_build::build()
    ├── tauri.conf.json     # app id, window, frontendDist
    ├── capabilities/
    │   └── main.json       # deny-by-default capability set
    └── src/
        ├── main.rs         # Tauri entry
        ├── commands.rs     # concerto_rpc + concerto_ping
        └── core_client.rs  # UDS Tonic connector
```

## What's locked (do not change without a revision task)

- Path: `apps/desktop/` is the Tauri app root.
- Tauri commands: `concerto_rpc(method, payload)` is the single
  dispatch entry point. Method-name convention is
  `"<Service>.<Rpc>"` (e.g., `"Runtime.GetServerCapabilities"`).
- App identifier: `app.concerto.desktop`.
- Window default size: 1200×800. Window-state persistence is V1.0.

## Capability posture

`capabilities/main.json` allows only `core:default` — no fs, no shell,
no http, no notification, no dialog. Future phases add named
permissions one at a time; the renderer cannot escape sandboxed
default behavior. See `design/15 §3.1` for the long-term shape.

## Release builds

`pnpm tauri build` produces signed/notarized artifacts. V0.1 defers
release builds to Phase 4 (Task 53) — don't run them in CI.
