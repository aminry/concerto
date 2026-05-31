# Task 109 — `concerto` CLI Skeleton

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 108 |
| Touches subsystem(s) | 10 (Client API Protocol) |
| Smoke gate | extends:cli |

## Goal
Turn the `crates/cli` placeholder (currently a 7-line `main.rs` that prints "concerto placeholder") into a real CLI that wraps the gRPC API over UDS. `design/10` mandates the `concerto` CLI as a V1.0 deliverable (R-6) and later tasks build on it (`concerto pair` in Task 713, `concerto backup` in Task 111). This task delivers the skeleton: a client that dials the Core's UDS socket and implements a first set of read commands plus the command scaffolding everything else hangs off.

## Inputs to read before starting
- `crates/cli/src/main.rs` (the placeholder).
- `tools/smoke-client/` (the existing dev gRPC client — the reference for how to dial Core's UDS and call services; reuse its connection approach).
- `crates/proto/` + `docs/interfaces/proto.md` (the services/messages: `Runtime.GetServerCapabilities`/`GetStatus`, `Workspaces.List`, `Sessions` list).
- `apps/desktop/src-tauri/src/core_client.rs` (how the desktop resolves the socket path + the `set_socket_override` convention — mirror the default socket path logic).
- `design/10_Local_API_Protocol.md` §1 (CLI mandate, R-6).
- `tasks/v1.0/108-smoke-gate-refactor.md` → "Handoff Notes" — the `scripts/smoke.d/` layout + `scripts/smoke.manifest` format this task's `cli` check plugs into.

## Scope — in
- Replace `crates/cli` with a `clap`-based binary `concerto` exposing:
  - `concerto status` — calls `Runtime.GetServerCapabilities` + `GetStatus`, prints version/uptime/transport_kind/actors.
  - `concerto workspace ls` — calls `Workspaces.List`, prints a table.
  - `concerto session ls [--workarea <id>]` — lists sessions.
  - `concerto --socket <path>` global flag + `CONCERTO_SOCKET` env to point at a non-default Core (mirroring the desktop's socket override).
  - `--json` global flag for machine-readable output.
- A small internal `client` module that establishes the UDS gRPC channel (factor it so Tasks 111/713 reuse it).
- Helpful errors when Core isn't running (name the socket path tried).
- Integration test: spawn a Core via the `crates/test-harness`, run `concerto status` against it, assert output.

## Scope — out
- `concerto pair` (Task 713 — needs the pairing flow from Phase 2).
- `concerto backup` (Task 111 — separate task, builds on this client module).
- Any write/mutating commands beyond what's listed (create workspace, start session) — keep the skeleton read-only; mutations come with their subsystems.
- Iroh/remote transport (CLI is local-UDS only in Phase 1; remote is a later concern).

## Public interface this task locks
- Rust: `crates/cli` binary name `concerto`; the `client` module's connect function signature; the global flags `--socket`/`--json` and `CONCERTO_SOCKET`.
- The command surface above (subcommands may be added later; these names are stable).

## Implementation notes
- Copy the channel-setup approach from `tools/smoke-client`, but keep the CLI's `client` module **self-contained** — do not refactor `tools/smoke-client` (that's outside this task's `Outputs`). Factor the module cleanly so Tasks 111/713 reuse it within `crates/cli`.
- Keep output rendering separated from the RPC calls so `--json` is a thin switch.
- Default socket path must match what Core writes and what the desktop reads — derive it the same way (don't hardcode a second source of truth).

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo build -p concerto-cli` → produces `concerto`.
4. `cargo test -p concerto-cli` → the spawn-core-and-`status` integration test passes.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `scripts/smoke.sh` → add a `cli` capability check (`extends:cli`): with Core running, `concerto status` exits 0 and prints the version. Exits 0.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit any regen.

## Definition of Done
- [x] `concerto status` / `workspace ls` / `session ls` work against a live Core over UDS
- [x] `--socket`/`CONCERTO_SOCKET`/`--json` honored; Core-down error names the socket
- [x] Reusable `client` module factored for Tasks 111/713
- [x] Integration test + `cli` smoke check pass
- [x] Verification commands pass; smoke gate green
- [x] Single commit created with the message below

## Outputs
- `crates/cli/src/main.rs` (rewritten)
- `crates/cli/src/client.rs`, `src/commands/*.rs` (new)
- `crates/cli/Cargo.toml` (modified — clap, tonic client deps)
- `crates/cli/tests/status.rs` (new)
- `scripts/smoke.d/<NN>-cli.sh` + manifest line (new)
- `docs/interfaces/rust-api.md` (regenerated if pub surface changed)

## Commit message
```
phase-1: concerto CLI skeleton over UDS gRPC

Replaces the crates/cli placeholder with a clap-based `concerto` binary:
status / workspace ls / session ls, --socket/--json flags, and a
reusable UDS client module for later `pair` and `backup` subcommands.

Refs: tasks/v1.0/109-concerto-cli-skeleton.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **Reusable `client` connect-fn signature (LOCKED — Tasks 111/713 reuse this):**
    ```rust
    // crates/cli/src/client.rs
    pub async fn connect(socket: &std::path::Path)
        -> Result<tonic::transport::Channel, crate::client::ClientError>;
    ```
    Build typed service clients on the returned channel, e.g.
    `RuntimeClient::new(client::connect(&socket).await?)`. Supporting helpers also
    LOCKED in the same module: `pub fn default_socket_path() -> Result<PathBuf, ClientError>`,
    `pub fn resolve_socket_path(flag: Option<PathBuf>) -> Result<PathBuf, ClientError>`,
    `pub const SOCKET_ENV: &str = "CONCERTO_SOCKET"`, `pub const CONNECT_TIMEOUT: Duration`.
    `ClientError` variants: `NoHome`, `EndpointInit`, `ConnectTimeout{socket}`,
    `Connect{socket, source}` — the Core-down message names the socket and the env var.
  - **Windows CI cfg-gating (post-impl fix):** the UDS dial in `client.rs` (the `UnixStream`
    import + the `Endpoint`/`connect_with_connector` body) is `#[cfg(unix)]`-gated, with a
    `#[cfg(not(unix))]` `connect` of the *identical* signature that returns a new
    `ClientError::Unsupported` variant ("the `concerto` CLI uses a local Unix-domain socket,
    which is not available on this platform; remote transport support arrives in a later
    phase"). The `concerto-test-harness` dev-dep moved to `[target.'cfg(unix)'.dev-dependencies]`
    and `tests/status.rs` is `#![cfg(unix)]`, so `concerto-cli` builds clean on the Windows
    `--all-targets` lane without touching the CI exclude list. All Unix behavior is byte-for-byte
    unchanged.
  - **Default-socket derivation:** single source of truth in `client::default_socket_path`,
    matching `apps/desktop/src-tauri/src/core_client.rs::default_socket_path`. Precedence:
    `--socket` flag > `$CONCERTO_SOCKET` (when set & non-empty) > `<HOME>/.concerto/core.sock`.
    No second hardcoded path. (The desktop uses an in-process `set_socket_override`; the CLI
    uses the `$CONCERTO_SOCKET` env var as the equivalent override channel — same resolved
    default.)
  - **`actors` not exposed over the wire:** the task prose asked `status` to print `actors`,
    but the frozen `runtime.proto` (`ServerCapabilities` / `RuntimeStatus`) has **no** actors
    field — the supervision-tree roster is not on the wire in V0.1. `concerto status` prints
    version / uptime / transport_kind plus `ServerCapabilities.optional_services` (rendered as
    `services:`), the closest advertised facet. If a future task wants a real actor roster in
    `status`, it needs a proto addition to `Runtime` (new task; re-lock at a new version).
  - **`Workspaces.List` / `Sessions` list take an id argument:** the frozen RPCs are
    `Workspaces.ListWorkspaces(project_id)` and `Sessions.ListSessions(workarea_id)` — there is
    no global list RPC. So `workspace ls` grew an optional `--project <id>`; with no flag it
    enumerates `Projects.ListProjects` and unions each project's workspaces. `session ls`
    keeps the spec'd `--workarea <id>`; with no flag it walks
    projects→workspaces→workareas→sessions and unions. No new RPCs were added.
  - **smoke.d file added:** `scripts/smoke.d/95-cli.sh` (capability `cli`, read-only, runs
    last) + `cli` appended to `scripts/smoke.manifest`. Driver `scripts/smoke.sh` untouched.
- **Open questions for next task:**
  - Task 111 (`concerto backup`) / 713 (`concerto pair`) add their subcommands under
    `crates/cli/src/commands/<name>.rs`, register them in `src/commands/mod.rs` + the clap
    `Command` enum in `main.rs`, and dial via `client::connect(&socket)` exactly as above
    (signature spelled out under Drift). They get `--socket`/`--json` for free (global flags).
    `backup` will need the `--json` view structs + a non-read RPC surface (its own concern);
    `pair` (Phase 7) needs the pairing RPCs that don't exist until Phase 2 (`Devices`).
  - If `status` should ever surface a live actor/health roster, that's a `Runtime` proto
    change (see Drift) — flag it when Task 709 (diagnostics RPCs) lands.
- **Deliberate debt:** none. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code. The
  global `session ls` / `workspace ls` fan-out issues one RPC per project/workspace/workarea
  (N+1 walk); fine for the read-only skeleton at V1.0 scale, and `--project`/`--workarea`
  scope it down. A dedicated cross-tree list RPC, if ever wanted, is a separate task.
- **Smoke-gate state:** `extends:cli`. Added `scripts/smoke.d/95-cli.sh` (defines `check_cli`:
  builds + runs the `concerto` binary's `status` against `--socket "$SOCKET"` from
  `00-core-boot`, asserts exit 0 and a `version:` line, echoes `PASS cli`/`FAIL cli`) and
  appended `cli` to `scripts/smoke.manifest` after `mcp` (read-only, last). `scripts/smoke.sh
  --list` shows `cli`; `scripts/smoke.sh --ci-mode` exits 0 with `PASS cli`; `shellcheck
  scripts/smoke.d/95-cli.sh` is clean.
