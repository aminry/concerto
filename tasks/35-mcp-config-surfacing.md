# Task 35 — MCP Configuration Surfacing (Read-Only)

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 22 |
| Touches subsystem(s) | 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
Read each agent's existing MCP configs (`~/.claude/mcp.json`, `~/.codex/config.toml`, per-repo `.mcp.json`) and surface them via `Sessions.ListMcpServers(scope)`. V0.1 is read-only — writing project-level `.mcp.json` is V1.0. After this task, the Desktop can show a list of MCP servers per scope without us implementing the MCP wire protocol.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.6 (MCP: read four scopes, surface, V1.0 write project-level; do not implement MCP wire).
- `design/00_Architecture_Overview.md` §6.4 (locked: read agents' existing config).

## Scope — in
- Implement `crates/core/src/agent_supervisor/mcp.rs`:
  - `pub struct McpServer { pub name: String, pub scope: McpScope, pub command: String, pub args: Vec<String>, pub env: BTreeMap<String, String>, pub source_path: PathBuf }`
  - `pub enum McpScope { Personal, Project(RepositoryId), Plugin, Enterprise }`
  - `pub async fn list_mcp_servers(scope: ScopeFilter) -> Result<Vec<McpServer>>`:
    - Personal: read `~/.claude/mcp.json` and `~/.codex/config.toml` (TOML, has an `[mcp]` table).
    - Project: read each repo's `.mcp.json` (path: `<repo_worktree>/.mcp.json`).
    - Plugin / Enterprise: V0.1 stubs — return empty; document file paths in code comments.
  - Tolerant parsing: a malformed file at one scope produces a warning + empty list for that scope, not an error.
- gRPC: `Sessions.ListMcpServers(McpScopeRequest)` already declared in `design/10` but not implemented; implement handler.
- Proto:
  ```proto
  message McpScopeRequest { optional string scope = 1; optional string repository_id = 2; }
  message ListMcpResponse { repeated McpServer servers = 1; }
  message McpServer {
    string name = 1; string scope = 2; string command = 3;
    repeated string args = 4; map<string, string> env = 5;
    string source_path = 6;
  }
  ```
- No write path in V0.1 — `Sessions.UpsertProjectMcp` is declared in the proto but the handler returns `NOT_IMPLEMENTED`. Document this in Handoff Notes.
- Tests:
  - Parse a fixture `mcp.json` with two servers; assert list contains them.
  - Tolerate malformed JSON: produce empty + warn.
  - Per-repo `.mcp.json`: place fixture in a temp dir, call with `scope=Project`, assert correct result.

## Scope — out
- Writing project-level `.mcp.json` (V1.0).
- Implementing the MCP wire protocol (never owned here — that's the agent's job).
- Plugin and enterprise scopes (V1.0+).
- The Concerto-specific in-process `concerto-mcp` server (V1.0 — `design/04 §3.6` notes this is in-process; we're not building it in V0.1).

## Public interface this task locks
- Rust: `McpServer`, `McpScope` types in `crates/core/src/agent_supervisor/mcp.rs`. FROZEN.
- Proto: `McpServer` message + `Sessions.ListMcpServers` RPC. Field numbers frozen.
- File paths read: `~/.claude/mcp.json`, `~/.codex/config.toml`, `<repo>/.mcp.json`.

## Implementation notes
- For TOML parsing, add `toml = "0.8"` as a dep. JSON via `serde_json` (already present).
- Use `dirs::home_dir()` for `~`.
- For `~/.codex/config.toml`, the MCP section is under a `[mcp]` or `[mcp_servers]` table — check the Codex docs at task time and document which key is canonical in Handoff. Codex's exact format may have moved.
- Make the parsers tolerant: serde with `#[serde(default)]` on optional fields; recover from missing keys.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core mcp` → tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: place a fixture `mcp.json` in `~/.claude/`; call `Sessions.ListMcpServers(scope="personal")`; verify the parsed list.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass.
- [x] Malformed-file tolerance verified.
- [x] Personal + Project scopes return parsed servers.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/core/src/agent_supervisor/mcp.rs` (new)
- `crates/proto/proto/concerto/v1/sessions.proto` (modified)
- `crates/core/src/handlers/sessions.rs` (modified)
- `crates/core/tests/mcp_listing.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md` (regenerated)

## Commit message
```
phase-3: MCP config surfacing (read-only)

ListMcpServers reads ~/.claude/mcp.json, ~/.codex/config.toml, and
per-repo .mcp.json. Tolerant parsing — malformed files warn and
return empty. Writing project-level configs is V1.0.

Refs: tasks/35-mcp-config-surfacing.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`home::home_dir()` instead of `dirs::home_dir()`.** Task 05 already
    swapped to the `home` crate workspace-wide to keep the license
    posture permissive-only (MPL-2.0 `option-ext` is banned). To make
    the function testable without mocking a global, `list_mcp_servers`
    takes an explicit `home_dir: Option<&Path>` parameter; production
    callers (`SessionsHandler::list_mcp_servers`) pass `None` and the
    impl falls back to `home::home_dir()`. The integration test in
    `crates/core/tests/mcp_listing.rs` passes `Some(tempdir.path())`
    so the test never reads the developer's real `~/.claude/`.
  - **`McpScopeFilter::All` does NOT sweep every repository.** A full
    sweep would require enumerating `repositories` and reading every
    `<local_path>/.mcp.json`, which can hit dozens of files on a real
    workspace. Per the pre-decision, `All` reads only the personal
    scope (Claude + Codex) plus the V0.1 plugin/enterprise stubs
    (which return empty). Callers that want per-project results must
    request `Project(repository_id)` explicitly. Documented inline on
    `McpScopeFilter::All`.
  - **Codex TOML canonical key is `[mcp_servers.<name>]`.** Task spec
    asked us to confirm. Current Codex CLI (`~/.codex/config.toml`)
    uses the nested-table form. The legacy `[mcp]` table is accepted
    via `#[serde(alias = "mcp")]` so older installs still surface.
  - **`mcpServers` is the JSON key, not `mcp_servers`.** Claude's
    on-disk schema uses camelCase. `ClaudeMcpFile` declares
    `#[serde(rename = "mcpServers", alias = "mcp_servers")]` to
    accept both. Test 1 (`personal_scope_parses_claude_and_codex_fixtures`)
    failed against the original alias-only form and was the trigger
    for adding the explicit `rename`.
- **Open questions for next task:**
  - The Desktop will probably want a "refresh on file change" affordance
    for the MCP config list. V0.1 is pull-only via gRPC; a file-watcher
    that emits on `streams.mcp.<scope>` is a clean V1.0 addition.
  - `UpsertProjectMcp` (V1.0) needs to think about merge semantics
    when a `.mcp.json` already exists at `<local_path>/.mcp.json`:
    overwrite vs. merge-by-name vs. fail-on-conflict. Worth a design
    note before the writer lands.
  - When `Plugin` / `Enterprise` scopes ship, decide whether they
    appear under `McpScopeFilter::All` (currently they're stubbed to
    return empty, so `All` is effectively `Personal`).
- **Deliberate debt:** read-only; `UpsertProjectMcp` declared in the
  proto with field numbers locked but the handler returns
  `UNIMPLEMENTED` ("mcp.upsert: writing project-level .mcp.json is V1.0").
  Plugin + Enterprise scopes return empty lists with documented file
  paths waiting in the module doc-comment.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0
  with "Smoke gate v2: PASSED".
