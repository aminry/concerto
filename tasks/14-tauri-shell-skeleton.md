# Task 14 — Tauri Desktop Shell Skeleton

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 13 |
| Touches subsystem(s) | 15 (Desktop), 10 (Local API) |
| Smoke gate | unchanged |

## Goal
Scaffold the Tauri 2 desktop application with a minimal React renderer. After this task, `cargo tauri dev` opens a window, the renderer calls a Tauri command that proxies a gRPC `GetServerCapabilities` over the UDS to a running Core, and the result renders as JSON in the window. No real UI yet — just the wire from React → Tauri command → gRPC → Core proven.

## Inputs to read before starting
- `design/15_Desktop_Client.md` §1 (purpose), §3.1 (Tauri capabilities: deny-by-default), §3.2 (IPC: Tauri commands wrap CoreClient), §5.1 (Tauri command surface), §6.1 (shell startup sequence), §6.5 (cold-start budget).
- `design/00_Architecture_Overview.md` §6.8 (Tauri 2 + React + Vite + shadcn/ui + Tailwind locked).
- `tasks/13-grpc-uds-server.md` → "Handoff Notes".

## Scope — in
- Initialize a Tauri 2 project under `apps/desktop/` (NOT `crates/desktop-shell/` — Tauri's scaffolding wants its own layout; the existing `crates/desktop-shell` becomes the shared Rust library that the Tauri app depends on).
  - Use `pnpm create tauri-app@latest` with: React + TypeScript + Vite template; or hand-scaffold:
    - `apps/desktop/src-tauri/Cargo.toml` declaring the binary `concerto-desktop`.
    - `apps/desktop/src-tauri/tauri.conf.json` with `productName = "Concerto"`, `identifier = "app.concerto.desktop"`, window 1200x800, dev URL `http://localhost:5173`.
    - `apps/desktop/package.json` with React 18, Vite, TypeScript, Tailwind, `@tauri-apps/api`.
    - `apps/desktop/src/main.tsx` + `apps/desktop/src/App.tsx`.
- Add the directory to the Cargo workspace's `members` (the `src-tauri` crate joins the workspace).
- In `src-tauri/Cargo.toml` add deps: `tauri = { version = "2", features = [...] }`, `concerto-proto`, `tonic`, `tokio`, `tower`, `concerto-error`.
- Implement `src-tauri/src/main.rs` with two Tauri commands:
  - `concerto_rpc(method: String, payload: serde_json::Value) -> Result<serde_json::Value>` that maps a single method `"Runtime.GetServerCapabilities"` to a gRPC call against the local UDS at `~/.concerto/core.sock`. Other methods return `NotImplemented` error for now.
  - `concerto_ping() -> Result<String>` returning `"pong"` (for smoke-testing the IPC bridge).
- The renderer:
  - `App.tsx` renders a single button "Connect" that calls `invoke('concerto_rpc', { method: 'Runtime.GetServerCapabilities', payload: {} })` and displays the result as JSON in a `<pre>`. On error displays the message.
  - Tailwind is installed and configured (one demo class on the page to verify it works).
- Capabilities file (`apps/desktop/src-tauri/capabilities/main.json`) is configured per `design/15 §3.1` deny-by-default — explicit allow for `core:default`, `dialog:default`, and the custom `concerto:rpc` permission.
- Add a `README.md` in `apps/desktop/` documenting `pnpm install && pnpm tauri dev` as the dev workflow.

## Scope — out
- shadcn/ui setup (Phase 2 — Task 24 brings real UI).
- Zustand / React Query (Phase 2).
- Three-panel layout (Phase 2 / Phase 3).
- xterm.js / Monaco (Phase 2 / Phase 3).
- Auto-update (Phase 4).
- Tray sidecar (Phase 4 or deferred to V1.0).
- macOS code signing config (Phase 4 — Task 53).
- Windows build (V1.0).

## Public interface this task locks
- Path: `apps/desktop/` is the Tauri app root.
- Tauri commands: `concerto_rpc(method, payload)` is the single dispatch entry point. The method-name convention is `"<Service>.<Rpc>"` (e.g., `"Runtime.GetServerCapabilities"`). Future tasks add cases to this dispatcher.
- Tauri app identifier: `app.concerto.desktop`.
- Window default size: 1200x800. Persistence of window state is deferred.

## Implementation notes
- Tauri 2 needs Rust 1.77+; ensure `rust-toolchain.toml` from Task 01 is current.
- The gRPC client over UDS in Tauri Rust code: use the same `tonic` client as the Core's integration test from Task 13. Connect to `unix://<HOME>/.concerto/core.sock`. Use `tower::service_fn` to wrap a `UnixStream` connector. This will be reused, so put the connection logic in `src-tauri/src/core_client.rs`.
- Don't keep a long-lived connection in V0.1 — open a fresh client per Tauri-command invocation. Persistent connection comes in Task 18 or later.
- Tailwind setup: `pnpm add -D tailwindcss postcss autoprefixer && pnpm tailwindcss init -p`. Configure `content: ['./index.html', './src/**/*.{ts,tsx}']`.
- The renderer should NOT do any direct network I/O — all I/O goes through Tauri commands.
- Use TypeScript strict mode (`"strict": true` in tsconfig).

## Verification
1. `cd apps/desktop && pnpm install` → succeeds.
2. `cargo build -p concerto-desktop` → succeeds.
3. `cargo check --workspace` → clean.
4. `cargo clippy --workspace -- -D warnings` → clean.
5. Manual: start Core in one terminal (`cargo run --bin concerto-core`); start Desktop in another (`cd apps/desktop && pnpm tauri dev`); click "Connect"; observe JSON of `ServerCapabilities` in the window.
6. `concerto_ping` command works (the renderer has a test button that exercises it).
7. The window title is "Concerto" (verifies tauri.conf.json applied).
8. `pnpm build` (the renderer Vite build) produces an output directory; Tauri's release build (`pnpm tauri build --debug`) succeeds — though we don't ship the binary.
9. `cargo deny check` → still clean (Tauri's tree must satisfy the license allow-list; if anything's unhappy, that's a real signal).

## Definition of Done
- [ ] Verification commands pass.
- [ ] Desktop window opens and successfully calls `Runtime.GetServerCapabilities` against a running Core.
- [ ] Renderer code has no direct network calls (verified by inspection).
- [ ] Tauri capabilities file lists only the necessary allow-list.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `apps/desktop/package.json` (new)
- `apps/desktop/pnpm-lock.yaml` (new, generated)
- `apps/desktop/tsconfig.json`, `apps/desktop/vite.config.ts` (new)
- `apps/desktop/tailwind.config.ts`, `apps/desktop/postcss.config.js` (new)
- `apps/desktop/index.html` (new)
- `apps/desktop/src/main.tsx`, `apps/desktop/src/App.tsx`, `apps/desktop/src/index.css` (new)
- `apps/desktop/src-tauri/Cargo.toml` (new)
- `apps/desktop/src-tauri/tauri.conf.json` (new)
- `apps/desktop/src-tauri/capabilities/main.json` (new)
- `apps/desktop/src-tauri/src/main.rs` (new)
- `apps/desktop/src-tauri/src/commands.rs` (new)
- `apps/desktop/src-tauri/src/core_client.rs` (new)
- `apps/desktop/src-tauri/build.rs` (new — `tauri-build::build()`)
- `apps/desktop/README.md` (new)
- `Cargo.toml` (workspace root, modified — adds `apps/desktop/src-tauri` to members)

## Commit message
```
phase-1: tauri desktop shell skeleton

Scaffolds apps/desktop/ as a Tauri 2 + React + Vite + Tailwind app
with a single concerto_rpc Tauri command that proxies
Runtime.GetServerCapabilities over UDS to the Core. Capabilities
deny-by-default per design/15 §3.1.

Refs: tasks/14-tauri-shell-skeleton.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** fresh gRPC client per command call (no persistent connection); persistent client+subscription multiplexer comes in Task 18+.
- **Smoke-gate state:** infrastructure exists; first integration check in Task 15.
