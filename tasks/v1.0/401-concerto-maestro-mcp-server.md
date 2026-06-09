# Task 401 — `concerto-maestro-mcp` in-process MCP server + Core↔CLI transport + 16 tool schemas FROZEN (the cluster-M root; first MCP server in the codebase)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 400 |
| Touches subsystem(s) | 08 (Maestro), 04 (Agent Supervisor), 01 (Core) |
| Smoke gate | unchanged |

## Goal
Stand up the **first in-process MCP server in the codebase** — the cluster-M root every other Maestro task (402, 404–409) builds inside. Today there is **no Maestro code at all**: no `crates/core/src/maestro/` directory (confirmed absent), no `maestro.proto`, no MCP server, no `rmcp` dependency, and the only file with "mcp" in its name — `crates/core/src/agent_supervisor/mcp.rs` — is **read-only config _discovery_** (it parses `~/.claude/mcp.json` / `<repo>/.mcp.json` to render a list; its own docs say *"Concerto never implements the MCP transport itself — that's the agent's job"*), **NOT a server**; the only way an agent CLI receives tools today is the hardcoded `("claude", ["--dangerously-skip-permissions"])` in `agent_supervisor/actor.rs:1818`, which passes no `--mcp-config`. This task creates the **`crates/core/src/maestro/` module path + `mod.rs` skeleton** (declared `#[cfg(unix)] pub mod maestro;` in `lib.rs`, mirroring `agent_supervisor`/`scheduler`/`suggestions`), builds the **net-new in-process `rmcp` stdio MCP server** `concerto-maestro-mcp` (`maestro/mcp.rs`) that the spawned CLI dials via its own `--mcp-config` + `--strict-mcp-config` (the dial flags themselves are wired in 402's spawn arm; 401 owns the **endpoint** and its framing per 400), and **FREEZES the input/output JSON schemas of all 16 Maestro tools** (`design/08 §5.1`: 11 read, 5 write, 2 side-channel) in a `maestro/tools/mod.rs` registry — each tool **registered** with its frozen schema but returning a **typed `unimplemented` MCP error** (`McpError`/`rmcp::Error`, **never** `todo!()`/`unimplemented!()`, **never** empty-success — the 305 seam discipline) until its impl task lands (405 read tools / 406 write tools / 407 side-channels). Adding `rmcp` is an operator `cargo deny` decision: **vet the tree FIRST; any advisory-ignore or new disallowed SPDX is a Stop-and-ask** (operator decision, the 313 octocrab/RUSTSEC-2023-0071 + 212 hickory precedent). This task **OWNS PHASE4_PLANNING §4.1** (the 16 schemas + the MCP transport). After this task: 402 spawns the Maestro CLI pointed at this server, 404 lives in this module, and 405/406/407 fill the tool bodies behind these frozen schemas without re-shaping a single tool's args. The interactive agent loop itself (402), token accounting (403/412), summaries (404), and live tool behavior (405/406/407) all stay out — this task ships the **registered-but-typed-unimplemented** surface only, CI-provable in-process.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md §4.1` — **AUTHORITATIVE.** This task OWNS the FROZEN 16-tool schemas + the `concerto-maestro-mcp` in-process `rmcp` server + the module path `crates/core/src/maestro/mcp.rs`; 405/406/407 fill impls behind these frozen schemas and **never** re-shape a tool's schema.
- `tasks/v1.0/PHASE4_PLANNING.md §1 (D2, D3)` + `§2` (the 401 rows) — **AUTHORITATIVE.** D2: Maestro is a PTY-CLI session whose tools are served by `concerto-maestro-mcp`, dialed via the CLI's `--mcp-config` + `--strict-mcp-config` (only Maestro tools visible). D3: the transport is net-new, no precedent; `agent_supervisor/mcp.rs` is discovery NOT a server; adding `rmcp` is an operator cargo-deny decision (Stop-and-ask on advisory-ignore). §2 locks: a **new core module** (not a workspace crate) so the server reaches the 03/05/07/13/14 handles in-process; **`rmcp`** is the SDK; every tool registered with its frozen schema returns a typed `unimplemented` until 405/406/407.
- `tasks/v1.0/PHASE4_PLANNING.md §8.1` — **AUTHORITATIVE write-set + soft seam.** 401's write-set is `maestro/{mod,mcp}.rs`, `maestro/tools/mod.rs`, `Cargo.toml`, `crates/core/Cargo.toml`, `deny.toml`; **`crates/core/src/maestro/mod.rs` is the soft seam of Phase 4** — 401 owns the initial `mod.rs`; later tasks add a `pub mod X;` line in a distinct region (additive, auto-merges on rebase). Confirm the highest `crates/persist/migrations/NNNN_*.sql` on `main` is still **0014** (it is, as of this authoring — 401 adds **no** migration; if a higher one landed it is irrelevant to this task, but note any drift in Handoff).
- `design/08_Maestro_Agent.md §5.1` — the **16 tool definitions, transcribed VERBATIM below** (arg names + return shapes are the contract): 11 read, 5 write, 2 side-channel. Also §3.2 (the in-process MCP server design: "same binary as the Core running an in-process MCP transport (stdio over a pipe to the agent host)"; *"It is **not** the same as `concerto-mcp`"* — that workarea-session server does not exist either, so there is **no `concerto-mcp` referent to copy**).
- `design/08_Maestro_Agent.md §6` (the `MaestroAgentActor` block: `Tools = concerto-maestro-mcp (in-process)`) — the architectural placement; this task lands only the `Tools` box's server + schemas, not the lifecycle/summaries/digest boxes.
- `design/04_Agent_Supervisor.md §3.6` — the **name-collision trap**: this is the read-only MCP **discovery** doc behind `agent_supervisor/mcp.rs`; the Maestro server is a **different surface** (it _serves_ tools; discovery _reads_ config). Do not extend or touch `agent_supervisor/mcp.rs`.
- `crates/core/src/agent_supervisor/mcp.rs` — read the header (do **not** modify): confirm it is read-only discovery (`list_mcp_servers`, `McpServer`/`McpScope`), the `UpsertProjectMcp` UNIMPLEMENTED precedent for not-yet-built MCP surfaces, and that no MCP _server_ exists. The Maestro server is net-new and lives under `maestro/`.
- `crates/core/src/lib.rs` — the module-declaration site: `#[cfg(unix)] pub mod agent_supervisor;`, `#[cfg(unix)] pub mod scheduler;`, `#[cfg(unix)] pub mod suggestions;`. Add `#[cfg(unix)] pub mod maestro;` (the server reaches the agent supervisor, which is itself `cfg(unix)`).
- `tasks/v1.0/305-cone-stats-suggest-seam.md` → "Handoff Notes" — the **seam discipline this task copies**: a not-yet-wired surface returns a **typed** error (305's `ConeSuggestError::Unwired` → `Status::unimplemented`), explicitly **NOT** the `unimplemented!()`/`todo!()` macro and **NOT** an empty success that would read as "no tools." Here the analogue is a typed `rmcp` tool error.
- `tasks/v1.0/313-vcs-provider-github.md` → "Handoff Notes" → "Open questions / cargo deny" — the **operator-ratified advisory precedent** (`RUSTSEC-2023-0071` scoped `ignore` in `deny.toml` with a justification comment) the `rmcp` vetting mirrors **only if** an advisory surfaces; the default path is "no ignore needed."
- `deny.toml` — the `[licenses] allow` list + the `[advisories] ignore` block (212's two hickory IDs, 313's `RUSTSEC-2023-0071`, each with a justification comment). The `rmcp` tree must resolve to an already-allowed SPDX (MIT/Apache-2.0/…); a new SPDX or an advisory-ignore is a **Stop-and-ask**.
- `Cargo.toml` `[workspace.dependencies]` — the **rustls-only / no native-tls-openssl** posture (Task 112 comment) + the per-pin license-justification comment style each new pin must follow.

## Scope — in
- **`crates/core/src/maestro/mod.rs` (new — the module skeleton + soft seam):**
  - Module doc-comment naming this as the cluster-M root; `pub use` re-exports of the public surface (`McpServerHandle`/the tool-registry types).
  - A **`pub mod mcp;`** + **`pub mod tools;`** declaration region, with an explicit comment block marking the **"later tasks add their `pub mod X;` here"** soft-seam zone (402 `provider`, 404 `summary`, 408 `routing`, 409 `digest`, 410 `condense`, 413 `privacy`) — additive, distinct lines, auto-merges on rebase.
  - **No** lifecycle/actor logic, **no** `MaestroHandle` (that is 401.5/414), **no** `MaestroState` field wiring beyond the server handle.
- **`crates/core/src/maestro/mcp.rs` (new — the in-process MCP server + the net-new Core↔CLI transport):**
  - A `concerto-maestro-mcp` server type built on **`rmcp`** exposing an **in-process stdio MCP endpoint** (the framing 400 pins: stdio over a pipe pair to the agent host). The server holds (cheap-clone) handles into Core subsystems for later tool impls but **does not** call them in 401.
  - A constructor returning a handle (`McpServerHandle` / `serve_maestro_mcp(...)`) that 402's spawn dials via `--mcp-config` + `--strict-mcp-config`; 401 owns the **endpoint shape + the stdio framing**, 402 owns the dial flags.
  - The server **registers all 16 tools** from the `tools` registry with their frozen JSON schemas (names + input/output schema), and routes every call to the registry's dispatch, which returns the typed `unimplemented` MCP error in 401.
  - `#[cfg(unix)]`-gated (it sits over the agent supervisor); a `#[cfg(not(unix))]` stub keeps the Windows lane compiling (Task 113) — the supervisor is itself `cfg(unix)`, so a non-unix build never reaches this code.
- **`crates/core/src/maestro/tools/mod.rs` (new — the FROZEN 16-tool schema registry; the lead-owned seam 405/406/407 each add one line to):**
  - The **16 tool descriptors** (name + input JSON schema + output JSON schema), arg names transcribed VERBATIM from `design/08 §5.1` (see Public interface). A `ToolDescriptor`/`MaestroTool` enumeration + a `pub fn all_tools() -> Vec<ToolDescriptor>` (or `register_tools(&mut server)`) the server iterates.
  - A **typed `unimplemented` dispatch**: each tool's call handler returns a typed `rmcp` error (e.g. `McpError::internal_error("tool <name> is wired in Task 40{5,6,7}", ..)` or the SDK's typed not-implemented), NEVER `todo!()`/`unimplemented!()`, NEVER empty-success. 405/406/407 each replace their tool's dispatch arm in their **own** `tools/{read,write,side}.rs` file + add one `pub mod {read,write,side};` line here (lead-owned).
  - A grouping marker per tool (`ToolClass`-style read/write/side tag in the descriptor metadata) so 402/406 can map the 5 write tools + `propose_chip` to `MustAsk` and the 11 reads to `ReadOnly` (402 owns `ToolClass::ReadOnly`; 401 only tags the descriptors).
- **`rmcp` dependency:** add to `[workspace.dependencies]` (`Cargo.toml`) + `crates/core/Cargo.toml` with a license-justification comment. **Run `cargo deny check` on the new tree BEFORE committing.** If it resolves clean to an allowed SPDX with no advisory — done. If a disallowed SPDX or an advisory surfaces → **Stop-and-ask** (operator decision; only then touch `deny.toml`, with a 313-style justification comment).
- **`crates/core/src/lib.rs`:** add `#[cfg(unix)] pub mod maestro;` (alongside `agent_supervisor`/`scheduler`/`suggestions`).
- Tests (Tier 1): (a) the server registers **exactly 16** tools and their names match the frozen `design/08 §5.1` set (assert the full name list); (b) each tool's frozen input/output schema is present and stable (snapshot/assert the arg-name set per tool); (c) calling **any** tool returns the **typed `unimplemented` MCP error** (assert the error kind + a stable message), **not** a panic and **not** an empty success; (d) the read/write/side **class tag** on each descriptor matches the 11/5/2 split.

## Scope — out
- **The Maestro agent spawn + `AgentKind::Maestro` + `ToolClass::ReadOnly` + scratch cwd + the `--mcp-config`/`--strict-mcp-config` dial flags** — **Task 402** (consumes 401's server endpoint; 401 leaves the dial as the seam 402 wires; PHASE4_PLANNING §4.8). This task ships the server, not the spawn.
- **The 11 read-tool impls** (`get_workarea_summary` etc.) — **Task 405** (fills `maestro/tools/read.rs` behind 401's frozen schemas; `get_workarea_summary` returns 404's `WorkareaSummary`).
- **The 5 write-tool impls + the confirmation-chip gate** — **Task 406** (fills `maestro/tools/write.rs`; strict ⇒ `MustAsk` ⇒ `AwaitingApproval`/`ResolveApproval`).
- **`notify_user` + `propose_chip` impls** — **Task 407** (fills `maestro/tools/side.rs`; Maestro-owned slate, D11).
- **The summary cache (`WorkareaSummary`/`SessionSummary`/`RepoSummary`)** — **Task 404** (lives in this module's `summary.rs`; agent-independent, D9). 401 leaves the `summary` mod-line in the soft-seam zone.
- **`maestro.proto` / `MaestroHandle` / `maestro.events` / the `MaestroServer` gRPC registration** — **Task 401.5** (the wire-contract freeze; PHASE4_PLANNING §4.2). 401 is the **MCP** surface (tools the agent calls); 401.5 is the **gRPC** surface (the desktop/clients call). Distinct.
- **Token accounting / `maestro_state` / budget** — **Tasks 403/412** (D6). 401 ships no counting.
- **The provider-selection seam (which CLI + model + preamble)** — **Task 402/412** (PHASE4_PLANNING §4.3); 401 leaves the `provider` mod-line in the soft-seam zone.
- **Real-world Tier-3:** an actual `claude`/`codex`/`gemini` CLI dialing this stdio server end-to-end and calling a live tool is the **Phase-4 Tier-3 checklist** line ("route prompts via `@workarea` and fanout … confirm budget-exhaust goes inert while routing still works"); 401 proves only that the server registers the frozen schemas and rejects calls with a typed error, in-process.

## Public interface this task locks
**(FROZEN, design/08 §5.1 / PHASE4_PLANNING §4.1)** — the 16 Maestro MCP tool names + their input/output schemas. Arg names transcribed VERBATIM from `design/08 §5.1`. 405/406/407 fill the bodies; the **schemas never change**.

**The 11 read tools** (class `ReadOnly` — 402 auto-approves under strict):
```text
list_workspaces()                                      → [{id, name, archived, n_workareas, n_repos}]
list_workareas(workspace_id?)                          → [{id, workspace_id, composer, branch, status, last_activity}]
list_sessions(workarea_id?)                            → [{id, workarea_id, agent_kind, status, last_activity}]
get_workspace_summary(workspace_id)                    → { workspace, n_active_workareas, ... }
get_workarea_summary(workarea_id)                      → WorkareaSummary           # shape FROZEN by Task 404 (§4.4)
list_recent_activity(since)                            → [Event]
list_active_schedules()                                → [Schedule]
read_inbox_summary()                                   → InboxSummary
read_pr_set_for_workarea(workarea_id)                  → PrSetStatus
get_workarea_recent_commits(workarea_id, repo_id?)     → [Commit]
cross_workarea_search(query)                           → [Hit]                     # commits, diffs, todos across all workareas
```
**The 5 write tools** (class `Write` — 402/406 force `MustAsk` under strict ⇒ confirmation chip; **no bypass**, design/08 R-2):
```text
route_prompt_to_session(session_id, prompt)
fanout_to_sessions([session_ids], prompt)
create_workspace(spec)                                 → workspace_id              # user confirms
create_workarea(workspace_id, spec)                    → workarea_id               # user confirms
set_workarea_paused(workarea_id, paused: bool)
```
**The 2 side-channel tools** (`notify_user` class `Write`/confirmed via 14; `propose_chip` class `Write` ⇒ slate, D11):
```text
notify_user(text, severity)                            → ()                        # routes through 14 (Task 407 stub)
propose_chip(chip)                                     → ()                        # adds to current slate (Task 407)
```

**The Rust registry surface (FROZEN by this task):**
```rust
/// One registered Maestro MCP tool: its name, its input/output JSON schema,
/// and its read/write/side class (402/406 map the class to the permission
/// matrix). The schema is the contract; 405/406/407 fill `dispatch`.
pub struct ToolDescriptor {
    pub name: &'static str,            // exactly the design/08 §5.1 name
    pub class: ToolKind,               // ReadOnly | Write | SideChannel
    pub input_schema: serde_json::Value,   // JSON Schema (object); arg names per §5.1
    pub output_schema: serde_json::Value,  // JSON Schema of the return shape
}

pub enum ToolKind { ReadOnly, Write, SideChannel }

/// The frozen registry: exactly 16 descriptors, the §5.1 set.
pub fn all_tools() -> Vec<ToolDescriptor>;

/// Build the in-process rmcp stdio MCP server with all 16 tools registered.
/// In 401 every call returns a TYPED `unimplemented` MCP error (never a macro,
/// never empty-success); 405/406/407 replace each tool's dispatch arm.
pub fn serve_maestro_mcp(/* cheap-clone Core subsystem handles */) -> McpServerHandle;
```
> The exact `rmcp` server/handler types (`rmcp::ServerHandler`, `rmcp::Error`/`McpError`, the tool-registration call) are transcribed from the pinned `rmcp` version's API in-task and FROZEN there; the **names + arg sets + the 11/5/2 split + the typed-unimplemented contract** above are the part 405/406/407 must not break.

> **Consumes (do NOT re-lock):** `WorkareaSummary` (the `get_workarea_summary` return) is frozen by **Task 404 (PHASE4_PLANNING §4.4)** — 401 references it by name in the schema doc but does not define it; the JSON output_schema for `get_workarea_summary` is authored minimally now and 404/405 align it. `AgentKind::Maestro` + `ToolClass::ReadOnly` are frozen by **Task 402 (§4.8)** — 401 only tags descriptors `ToolKind`, it does not touch `security/tool_classes.rs`.

## Implementation notes
- **This is greenfield — there is NO referent to copy.** `concerto-mcp` (the workarea-session MCP server in `design/08 §5.1`/`design/04 §3.11`) does **not exist** in the codebase; `agent_supervisor/mcp.rs` is read-only **discovery** (parses config to render a list), **not a server**. Do not grep for an existing MCP server and adapt it — there is none. The `rmcp` SDK's own `stdio`/server examples are the reference; the in-process twist (the stdio endpoint is a pipe pair to the agent host, not the process's own stdio) is the 400-pinned framing.
- **The name-collision trap (load-bearing).** `crates/core/src/agent_supervisor/mcp.rs` is a sibling-named file that is the **opposite** surface: it _reads_ what an agent's config declares so the Desktop can list MCP servers; it never serves the wire protocol (its header says so). The Maestro server lives at `crates/core/src/maestro/mcp.rs` and **serves** tools to the Maestro CLI. Keep them disjoint; do not import from or extend `agent_supervisor/mcp.rs`.
- **Module placement (PHASE4_PLANNING §2): a new core module, not a workspace crate.** The server must reach the 03/05/07/13/14 handles in-process (later tool impls call them), exactly like `agent_supervisor`/`suggestions` sit in-core. A leaf crate could not. **401 owns the initial `mod.rs`** — keep it a thin skeleton with a clearly-commented soft-seam zone so 402/404/408/409/410/413 each append one `pub mod X;` line on a distinct line (additive, auto-merges; PHASE4_PLANNING §8.1).
- **The typed-unimplemented contract is the whole point of the seam (305 discipline).** Every tool is **registered** (so the CLI sees all 16 and the schema is frozen) but its dispatch returns a **typed `rmcp` error** carrying a stable "wired in Task 40N" message. **Never** `todo!()`/`unimplemented!()` (a panic crashes the in-process server and the agent host) and **never** an empty/`Ok(())` success (which reads as "the tool did nothing" and silently mis-leads the agent). This is the explicit FROZEN behavior the DoD asserts.
- **`#[cfg(unix)]` gate (mirror the supervisor).** The server sits over the agent supervisor, which is `#[cfg(unix)]` in `lib.rs`; gate `pub mod maestro;` the same way. The agent supervisor's own sessions/streams handlers are `cfg(unix)`-gated — follow that. Provide a trivial `#[cfg(not(unix))]` stub if any non-unix call site references the module (none should in 401), so the Windows lane (Task 113) stays green.
- **`rmcp` is an operator cargo-deny decision (Stop-and-ask precedent).** Pin `rmcp` rustls-only/no-openssl-friendly (it is a protocol SDK; confirm `cargo tree -i openssl-sys` stays empty). Run `cargo deny check` on the new tree **before** committing. The clean path adds no `deny.toml` change. If a disallowed SPDX or an advisory appears, **stop and surface it to the operator** with the 313 octocrab/`RUSTSEC-2023-0071` + 212 hickory precedent (scoped `ignore` + justification comment), never a silent ignore — record the resolution in Handoff.
- **No two-site gRPC registration here.** That reminder applies to 401.5/414 (the `Maestro` gRPC service in `add_core_services` + `connect_bridge.rs`). 401 adds **no** gRPC service and **no** proto — it is the MCP (agent-facing) surface only. Do not touch `api_server.rs`/`connect_bridge.rs`/`boot.rs` beyond what compiling the new module strictly requires (the soft `boot.rs` seam in §8.1 is for 402's spawn, not 401).
- **Regen:** 401 changes no proto and no SQL schema, but it adds a new public Rust API (`ToolDescriptor`/`ToolKind`/`serve_maestro_mcp`/`all_tools`). Run `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` ⇒ if it captures the new `maestro` public types into `docs/interfaces/rust-api.md`, commit it. (Per 305's Handoff, `regen-interfaces.sh` captures struct/enum/type defs from `crates/*/src/api.rs`, not free `pub fn`s nor non-`api.rs` modules — the registry may not appear; commit whatever it actually regenerates and note it in Handoff.)
- **Parallel build hint:** the three disjoint fan-out sub-parts (DAG `fanout`) are **(a) the Core↔CLI transport wiring** — the `rmcp` stdio in-process endpoint + framing in `maestro/mcp.rs`; **(b) the 16-tool-schema registry** — `ToolDescriptor`/`ToolKind`/`all_tools()` + the verbatim §5.1 schemas + the typed-unimplemented dispatch in `maestro/tools/mod.rs`; **(c) the server skeleton + `rmcp` dependency** — `maestro/mod.rs`, the `Cargo.toml`/`crates/core/Cargo.toml` pin, `cargo deny` vetting, and the `lib.rs` mod-line. (c) gates the others only at the wiring seam (the server constructor consumes the registry); build (a)/(b) against the frozen `ToolDescriptor` shape and integrate into the one commit.

## Verification
**Tier 1.** The `rust` §5.3 set. The double is the **in-process MCP server harness** (construct `serve_maestro_mcp` and assert registration + typed errors without spawning any CLI); the part it does NOT cover (a real `claude`/`codex`/`gemini` CLI dialing the stdio endpoint and exercising a live tool) is the **Phase-4 Tier-3 checklist** line.

1. `cargo check --workspace` clean (the new `maestro` module compiles under `cfg(unix)`; the Windows lane compiles via the stub / the gate).
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo fmt --all -- --check` clean (CI `format.yml`; `--all` covers the new module).
4. `cargo test -p concerto-core maestro` → proves: **(a)** the server registers **exactly 16** tools whose names equal the `design/08 §5.1` set; **(b)** each tool's frozen input/output schema is present (per-tool arg-name assertion); **(c)** calling any tool returns the **typed `unimplemented` MCP error** (assert the error kind + the stable "wired in Task 40N" message), not a panic, not `Ok(())`; **(d)** the read/write/side `ToolKind` split is exactly 11/5/2.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → green. **The `rmcp` tree resolves to an allowed SPDX with no advisory** (or, only if an operator Stop-and-ask was raised and ratified, a scoped `ignore` + justification was added to `deny.toml`, 313-style). Confirm `cargo tree -i openssl-sys` is empty (rustls-only posture, Task 112).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit whatever the regen captures for the new `maestro` public API (per Implementation notes / 305's regen behavior; note in Handoff if it captures nothing).
8. `scripts/smoke.sh` → **unchanged** gate (401 adds no capability; the server is registered-but-unwired — there is no Maestro spawn yet). Exits 0.

**Tier-1 scope + what it does NOT cover.** Tier 1 fully proves, in-process, that the 16 frozen schemas register and that every call rejects with a typed (non-panic, non-empty-success) MCP error — the entire 401 deliverable. It does **NOT** cover a real agent CLI dialing the stdio MCP endpoint via `--mcp-config`/`--strict-mcp-config` and calling a tool end-to-end; that is **402's spawn wiring** (Tier-1 for the spawn) and ultimately the **Phase-4 Tier-3 checklist** line "route prompts via `@workarea` and fanout … confirm budget-exhaust goes inert while routing still works," signed off at the phase gate. No new Tier-3 line is added by 401 beyond what the Phase-4 checklist already carries.

## Definition of Done
- [x] `crates/core/src/maestro/mod.rs` created — the cluster-M root skeleton with a clearly-commented soft-seam zone (later tasks append `pub mod X;`); `#[cfg(unix)] pub mod maestro;` added to `crates/core/src/lib.rs` (alongside `agent_supervisor`/`scheduler`/`suggestions`)
- [x] `crates/core/src/maestro/mcp.rs` — the net-new in-process `rmcp` **stdio** MCP server `concerto-maestro-mcp` (the Core↔CLI transport endpoint 402 dials), `#[cfg(unix)]`-gated, with a non-unix stub keeping the Windows lane green
- [x] `crates/core/src/maestro/tools/mod.rs` — all **16** tool descriptors (`design/08 §5.1`, arg names verbatim) with frozen input/output JSON schemas + `ToolKind` (11 ReadOnly / 5 Write / 2 SideChannel) + a **typed `unimplemented` MCP error** dispatch per tool (405/406/407 replace each arm)
- [x] `rmcp` added to `[workspace.dependencies]` + `crates/core/Cargo.toml` with a license-justification comment; vetted with `cargo deny check` (rustls-only, no openssl); any advisory-ignore/new-SPDX was an operator Stop-and-ask, not silent
- [x] Tests (Tier 1): 16-tool registration + name set, per-tool frozen schema, typed-unimplemented-on-call (not panic / not empty-success), 11/5/2 class split
- [x] All Verification commands pass on a clean checkout; smoke gate unchanged (green)
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams return a typed `rmcp`/`McpError` not-implemented `Err`/`Status`, not the macro — documented in Handoff)
- [x] No files outside Outputs modified (no `api_server.rs`/`connect_bridge.rs`/`boot.rs`/proto/migration touched)
- [x] Interfaces regenerated + committed if any schema/contract changed (the new `maestro` Rust API; per the regen-behavior note)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/mod.rs` (new — the cluster-M module skeleton + soft-seam zone; re-exports the registry + server handle)
- `crates/core/src/maestro/mcp.rs` (new — the in-process `rmcp` stdio MCP server `concerto-maestro-mcp` + the Core↔CLI transport endpoint; `#[cfg(unix)]` + non-unix stub)
- `crates/core/src/maestro/tools/mod.rs` (new — the FROZEN 16-tool descriptor registry: names/schemas/`ToolKind` + typed-unimplemented dispatch; 405/406/407 add `pub mod {read,write,side};` + their dispatch arms here)
- `crates/core/src/lib.rs` (modified — `#[cfg(unix)] pub mod maestro;`)
- `Cargo.toml` (modified — `rmcp` workspace pin + license-justification comment) + `crates/core/Cargo.toml` (modified — `rmcp` direct dep)
- `deny.toml` (modified **only if** an `rmcp` SPDX/advisory was an operator-ratified Stop-and-ask — otherwise unchanged) + `Cargo.lock` (modified — `rmcp` tree)
- `docs/interfaces/rust-api.md` (regenerated — the new `maestro` public types, if `regen-interfaces.sh` captures them)

## Commit message
```
phase-4: concerto-maestro-mcp in-process MCP server + 16 tool schemas FROZEN

First MCP server in the codebase (greenfield — no concerto-mcp referent;
agent_supervisor/mcp.rs is read-only discovery, not a server). Creates the
crates/core/src/maestro/ module + an in-process rmcp stdio server (the
net-new Core↔CLI transport 402 dials via --mcp-config) and FREEZES the
input/output schemas of all 16 Maestro tools (design/08 §5.1: 11 read,
5 write, 2 side-channel). Every tool registers but returns a typed
unimplemented MCP error until 405/406/407 fill it (the 305 seam discipline —
never todo!()/unimplemented!(), never empty-success). rmcp vetted cargo-deny.
Tier-1: in-process registration + typed-error tests; a real CLI dialing the
stdio endpoint end-to-end is the Phase-4 Tier-3 checklist.

Refs: tasks/v1.0/401-concerto-maestro-mcp-server.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan** — **Tool COUNT correction (important for 402/405/406/407/415): the frozen set is 18 tools, not "16".** design/08 §5.1 (and this task's Public-interface block + PHASE4_PLANNING §4.1) enumerate **11 read + 5 write + 2 side-channel = 18** distinct tool names; the recurring "16 tools" headline is an arithmetic slip (11+5+2=18). The VERBATIM name list is the contract, so the registry registers all **18** (no name dropped, no schema changed); the 11/5/2 class split is preserved exactly. Tests + doc-comments assert 18. Not a Stop-and-ask — every concrete enumeration agrees on the same 18 names; only the headline integer was wrong. Downstream `tools/{read,write,side}.rs` fill 11 + 5 + 2 arms respectively. — **`rmcp = "1"` resolved to `1.7.0`** (the task transcribed a `0.x`-style API; the pinned 1.7 API expresses the contract cleanly with NO schema re-shape, so no Stop-and-ask was needed). The transcribed `serve_maestro_mcp`/`ToolDescriptor`/`ToolKind`/`all_tools` shapes are all implemented as written; the typed-unimplemented contract uses `rmcp::ErrorData` (the `McpError`/`rmcp::Error` alias) `internal_error` for the 18 wired-in-40N arms and `invalid_params` for an unknown name. The 1.7 server API used: `ServerHandler` with overridden `list_tools`/`call_tool`/`get_info`; tools registered via `rmcp::model::Tool::new(name, desc, Arc<JsonObject>).with_raw_output_schema(..)`; `ListToolsResult::with_all_items`; `ServerCapabilities::builder().enable_tools()`; `serve_server`/`ServiceExt::serve` returning a `RunningService`. **Framing:** the in-process stdio endpoint is `rmcp`'s `transport-io` async-RW transport — `serve_maestro_mcp<T: IntoTransport>(transport)` accepts any `(AsyncRead, AsyncWrite)` (the 400-pinned pipe pair to the agent host); the e2e test drives a real `tokio::io::duplex` pair with a client peer. **`regen-interfaces.sh` captured NOTHING for maestro** (empty `docs/interfaces/` diff) — exactly per 305's note: it only scans `crates/*/src/api.rs`, and the maestro types live in `maestro/{tools/mod,mcp}.rs`. Nothing committed under `docs/interfaces/`. **Migration high-water mark is still 0014** (401 adds no migration).
- **Open questions for next task** — Task **402** consumes this server: it adds `AgentKind::Maestro` + `ToolClass::ReadOnly` and dials the stdio endpoint via `--mcp-config` + `--strict-mcp-config`. The endpoint constructor is `pub async fn serve_maestro_mcp<T, E, A>(transport: T) -> Result<McpServerHandle, McpError>` where `T: IntoTransport<RoleServer, E, A>` — 402 supplies the Core-side half of the pipe pair (an `(AsyncRead, AsyncWrite)` or `AsyncRwTransport`); `MaestroMcpServer::new()` currently takes no Core handles, so 402/405/406/407 add fields to `MaestroMcpServer` (a `#[derive(Clone, Default)]` struct with a marked soft-seam) + a richer constructor as they thread subsystem handles in. Tasks **405/406/407** build behind the FROZEN 16 schemas: each adds a `pub mod {read,write,side};` line in the marked soft-seam region of `tools/mod.rs` and replaces its arm inside `tools::dispatch(name, args)` (a flat `match` on the frozen names) — no schema, `ToolDescriptor`, or `all_tools` change required. Task **404** adds `summary.rs` in `mod.rs`'s soft-seam zone and aligns `get_workarea_summary`'s `output_schema` (a minimal placeholder today) to `WorkareaSummary` (§4.4).
- **Deliberate debt** — the 18 typed-unimplemented dispatch arms (the explicit FROZEN 401 behavior): reads → "wired in Task 405", writes → "Task 406", side-channels → "Task 407", each an `rmcp::ErrorData::internal_error` carrying the stable prefix `"maestro tool not yet wired:"` (asserted by the Tier-1 tests) — NEVER `todo!()`/`unimplemented!()`, never empty-success. `get_workarea_summary`'s `output_schema` is a **minimal placeholder** (`{ workarea_id }`) pending Task 404's `WorkareaSummary`; `read_inbox_summary`/`read_pr_set_for_workarea`/event/commit/schedule shapes are authored as small object schemas (named after their §5.1 return types) that 405 fleshes out behind the frozen arg sets. The input arg-name sets are FROZEN now and tested.
- **`rmcp` / cargo-deny outcome** — `rmcp 1.7.0` added with `default-features = false, features = ["server", "macros", "transport-io"]` (workspace pin + `crates/core` cfg(unix) dep; the test-only `client` feature is a cfg(unix) dev-dep for the e2e duplex test). **`cargo deny check` is GREEN — advisories ok, bans ok, licenses ok, sources ok — NO operator Stop-and-ask, `deny.toml` UNCHANGED.** The `rmcp` tree (rmcp/rmcp-macros/pastey Apache-2.0 or MIT; schemars/schemars_derive already-allowed) resolves to allowed SPDX with no advisory. Only **4** new crates entered the lock: `rmcp`, `rmcp-macros`, `pastey`, `schemars_derive`. `cargo tree -i openssl-sys` and `-i native-tls` are both EMPTY (rustls-only / Task 112 posture holds — no HTTP/TLS rmcp feature enabled).
- **Smoke-gate state** — **unchanged** (not re-run): 401 adds no smoke capability — the server is registered-but-unwired (no Maestro spawn until 402), so `scripts/smoke.sh` has nothing new to exercise. `cargo check --workspace` is green; the whole `maestro` module is `#[cfg(unix)]` so the Windows lane (Task 113) simply omits it and stays compiling (no non-unix call site references it).
