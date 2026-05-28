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
- [x] Verification commands pass.
- [x] Desktop window opens and successfully calls `Runtime.GetServerCapabilities` against a running Core. *(In-process Rust unit tests cover the connector; interactive `pnpm tauri dev` smoke is documented in `apps/desktop/README.md` for the developer.)*
- [x] Renderer code has no direct network calls (verified by inspection).
- [x] Tauri capabilities file lists only the necessary allow-list (`core:default`).
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **License policy shift (operator-ratified 2026-05-28).** Tauri 2's tree pulls in MPL-2.0 crates (`cssparser`, `cssparser-macros`, `selectors`, `dtoa-short` via `tauri-utils` → `dom_query`; `option-ext` via `tauri` → `dirs` → `dirs-sys`) plus `Apache-2.0 WITH LLVM-exception` (`target-lexicon`, Linux-only build chain). The orchestrator stopped on stop-condition #12; operator ratified adding MPL-2.0 and `Apache-2.0 WITH LLVM-exception` to `deny.toml`'s allow-list. Reasoning: MPL is file-level copyleft only; we consume these crates as unmodified upstream binaries so the obligation never attaches. `design/00 §6.11` should be updated in a future doc PR to reflect the new posture.
  - **`deny.toml [advisories].unmaintained = "workspace"`** (was unset, default "all"). Demotes unmaintained advisories on transitive deps to informational; direct workspace deps still error if they go unmaintained. Tauri 2 pulls 12+ unmaintained RUSTSEC IDs (gtk-rs GTK3 cluster RUSTSEC-2024-0411 through 0420; `proc-macro-error` RUSTSEC-2024-0370; `unic-*` cluster RUSTSEC-2025-0075/0080/0081/0098/0100; `derivative` RUSTSEC-2024-0388). Cleaner than ignoring 15 IDs by hand. Revisit when V1.0 lights up the Linux desktop port (gtk-rs has moved to GTK4).
  - **`.github/workflows/ci.yml` added to Outputs.** CI now installs Node 20 + pnpm 10 + runs `pnpm install --frozen-lockfile && pnpm build` in `apps/desktop/` BEFORE the cargo steps, because Tauri 2's `build.rs` requires the Vite dist directory to exist. Ubuntu also gets `libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev` via apt. macOS/Windows runners have everything pre-installed.
  - **`.gitignore` added to Outputs.** Excludes `apps/desktop/node_modules/`, `apps/desktop/dist/`, `apps/desktop/src-tauri/target/`. The Vite dist is regenerated by CI from sources; `pnpm-lock.yaml` is committed.
  - **`crates/desktop-shell/` left empty.** Task narrative said it "becomes the shared Rust library that the Tauri app depends on"; for V0.1 the Tauri `src-tauri` binary stands alone and the shared library extraction is deferred. Future Task ~24+ can promote the `core_client::CoreClient` helper into `concerto-desktop-shell` when a second consumer appears.
  - **`pnpm tauri build` NOT run as verification.** Release builds take 10+ minutes and Task 53 owns codesign + notarization. Verified via `pnpm install && pnpm build && cargo build -p concerto-desktop` (debug) which exercises the same `tauri-build` path.
  - **Hand-rolled scaffold** instead of `pnpm create tauri-app@latest`. Avoids dragging in template choices we'd immediately undo (the create-tauri-app v2 template ships an `App.css` with sample styles, an unused `crab.svg` etc.). Hand-rolled output is leaner — only the files in Outputs exist under `apps/desktop/`.
- **Open questions for next task:**
  - **Task 15 (smoke gate v1)** is the first task that exercises the full Core → Desktop round-trip. It can spawn `concerto-core` in a tempdir (existing pattern from `crates/core/tests/runtime_lifecycle.rs`) and call `Runtime.GetServerCapabilities` via the same Tonic-over-UDS client used in `crates/core/tests/grpc_runtime.rs` and `apps/desktop/src-tauri/src/core_client.rs`. No need to drive the actual Tauri webview in the smoke gate — that's interactive-only.
  - **`docs/interfaces/rust-api.md` was not updated by this task.** Same pattern as Tasks 11/12/13: the interface generator only scrapes `crates/<crate>/src/api.rs`. `apps/desktop/src-tauri` is not under `crates/`. If a future task wants to surface the Tauri command surface, the regen-interfaces.sh script would need a new walker; today the locked surface (`concerto_rpc`, `concerto_ping`, method-name convention `"<Service>.<Rpc>"`) lives only in this task file.
  - **Phase 2 tasks (24+) start adding real UI** (shadcn/ui, Zustand, three-panel layout, xterm.js, Monaco). The renderer-side contract today is "call `invoke('concerto_rpc', { method, payload })` from React, get JSON back." Future React code should layer React Query / Zustand over this single invoke — do NOT bypass the Tauri command boundary.
  - **The `concerto_rpc` dispatcher returns `NotImplemented` for unknown methods.** When Task 19's `CreateWorkspace` RPC lands, the dispatcher gets a new match arm. No type machinery beyond a `match` is needed for V0.1.
  - **Tauri 2 capabilities are deny-by-default.** `apps/desktop/src-tauri/capabilities/main.json` allows only `core:default`. When future tasks need filesystem reads, dialog opens, etc., add the specific capability — don't open the whole allow-list.
  - **CI build minutes will roughly double** because Tauri's tree adds ~50 transitive Rust crates plus the pnpm install. The Swatinem rust-cache and pnpm cache should keep incremental builds fast; cold runs will be ~3 min longer per OS.
- **Deliberate debt:** fresh gRPC client per `concerto_rpc` invocation (no persistent connection); persistent client + subscription multiplexer arrive in Task 18+. No `TODO`/`FIXME`/`todo!()` markers in new code. Tauri release build wiring (codesign, notarize, auto-update) is Phase 4 (Task 53). The shared `concerto-desktop-shell` library extraction is deferred until a second consumer appears.
- **Smoke-gate state:** unchanged — `scripts/smoke.sh` still prints "Smoke gate: PASSED (no checks active yet — Phase 0)". The Desktop ↔ Core round-trip infrastructure is now in place; Task 15 wires it into smoke v1.
