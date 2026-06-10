# Task 402 — Maestro-as-agent: `AgentKind::Maestro`, `ToolClass::ReadOnly` + strict matrix, scratch cwd, `MaestroProvider` seam (Claude CLI live)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium |
| Depends on | 401 |
| Touches subsystem(s) | 04 (Agent Supervisor), 08 (Maestro), 12 (Security) |
| Smoke gate | unchanged |

## Goal
Spawn the long-lived **Maestro agent session** by reusing the existing Agent-Supervisor spine verbatim — `start_session`/host-survival/cold-resume — rather than building a bespoke orchestrator (`design/08 §3.1`, D2). Today there is **no Maestro agent kind** and no way to launch a restricted, MCP-tooled session: `AgentKind` is the closed V0.1 set `{Echo, Claude, Codex, Gemini}` (`crates/core/src/agent_supervisor/actor.rs:66`); `resolve_agent_bin` (`actor.rs:1818`) hardcodes `("claude", ["--dangerously-skip-permissions"])` and passes **no model / `--mcp-config` / preamble / permission-mode**; the permission matrix maps `(Strict, _) => MustAsk` for **every** tool including `Safe` reads (`security/permission.rs:451`), so a strict Maestro would prompt on every `list_workspaces`; `ToolClass` is the closed `{Safe, Restricted, Dangerous}` (`permission.rs:352`); and `parse_agent_kind` (`handlers/sessions.rs:405`) rejects everything but `echo|claude`. This task **FREEZES** (a) the new **`AgentKind::Maestro`** variant + all of its touch-sites, (b) a new **`ToolClass::ReadOnly`** bucket that auto-approves under strict while every write tool + `propose_chip` still hits `MustAsk` (the existing `AwaitingApproval`/`ResolveApproval` confirmation-chip flow), and (c) the **`MaestroProvider`** provider-selection trait (which CLI binary + model + Maestro preamble + `--mcp-config` + `--strict-mcp-config` + `permission_mode=strict` + scratch cwd to launch) with the **Claude CLI as the LIVE impl** — all per **`design/08 §3.1`/`§3.2`/`§3.10`** and **PHASE4_PLANNING §4.8 + §4.3** (amended by Task 400). The Maestro launches the chosen CLI with the **Maestro preamble** + the in-process MCP server config (401's `concerto-maestro-mcp`) + a **no-op/structured parser pack** (its tool calls ride MCP, not PTY-scrape — it must NOT reuse the fragile `ClaudeCodePack` regex scraper), `cwd = ~/concerto/maestro/` (a scratch dir, not a worktree, no edit-mutex). After this task, **Task 412** extends the same `MaestroProvider` seam with Codex/Gemini-LIVE + the frozen-unwired `DirectApiProvider` arm + the daily budget, and **Task 406** consumes the strict-matrix chip-gate for its write-tool confirmations. **Token accounting is net-new and stays out** — `AgentEvent::ContextUsage{pct}` is wired-but-never-emitted and is NOT the carrier; 403 freezes `maestro_state`/budget and 412 wires counting. The real interactive-LLM behavior (Sonnet quality, multi-turn routing) stays a Tier-3 phase-gate item.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md §4.8` — **AUTHORITATIVE**: this task OWNS+FREEZES `AgentKind::Maestro` (+ DB string `"maestro"`, parser-pack arm, cold-resume arm), `ToolClass::ReadOnly` (auto-approves the 11 read tools under strict), and the `~/concerto/maestro/` scratch convention. Everything in cluster M consumes these.
- `tasks/v1.0/PHASE4_PLANNING.md §4.3 + §1 D1 + D5` — **AUTHORITATIVE**: this task FREEZES the `MaestroProvider` interactive-agent seam with **Claude CLI LIVE**; 412 adds Codex/Gemini LIVE + the `DirectApiProvider` frozen-unwired arm + budget. Distinct from `OneShotLlm` (4.5) — that String-only/no-stream shape is the WRONG shape for the agent loop; do not conflate.
- `tasks/v1.0/PHASE4_PLANNING.md §2` (402 rows) — the exact touch-sites: `as_db_kind` (`"maestro"`), `resolve_agent_bin` (Maestro spawn arm), `from_db_kind` (cold-resume, `actor.rs ~1248`), the parser-pack selection match (`actor.rs ~1357`), `parse_agent_kind` (`handlers/sessions.rs ~405`); scratch cwd = `~/concerto/maestro/` (no edit-mutex, no worktree); the Maestro uses a **no-op/structured parser pack** (400 pins the parser story; do not reuse `ClaudeCodePack`).
- `tasks/v1.0/PHASE4_PLANNING.md §3` — migration reservation: **402 has NO row → adds no migration.** It is an author-check anchor only — **confirm the highest `crates/persist/migrations/NNNN_*.sql` on `main` is still `0014`** (0012/0013 landed in Phase 3, below 0015); if a Phase-4 migration drifted the block, note it in Handoff. This task touches no SQL.
- `tasks/v1.0/PHASE4_PLANNING.md §8.1` — write-set: shares the soft `maestro/mod.rs` seam with 401/404 (add your `pub mod provider;` in a distinct region) and shares `provider.rs` with 412 + `tool_classes.rs` with 406; serialize on those hard seams.
- `tasks/v1.0/400-maestro-architecture-reconciliation.md` — the design amendment that governs where built code diverges from `design/08`: Maestro-as-PTY-CLI-session, the Core↔CLI MCP-stdio transport (`--mcp-config`/`--strict-mcp-config`), the strict-mode ReadOnly-auto-approve rule, and Direct-API-deferred-as-Tier-1-seam. **400 + PHASE4_PLANNING govern over `design/08`'s idealized shapes.**
- `tasks/v1.0/401-maestro-mcp-server.md` (dep) — consume 401's FROZEN `crates/core/src/maestro/mcp.rs` `concerto-maestro-mcp` in-process `rmcp` stdio server + the 16-tool schema registry + the module path `crates/core/src/maestro/mod.rs`. 402 launches the chosen CLI dialed at 401's stdio MCP endpoint via `--mcp-config`; it does NOT re-shape any tool schema.
- `design/08_Maestro_Agent.md §3.1` (the Maestro is itself an agent process — strict mode, ReadOnly exception, no file-edit/shell/network, scratch cwd, "reuse 04's lifecycle/host-survival/cold-resume") + `§3.2` (MCP server hosted in-Core, the CLI configured with only this server) + `§3.9`/`§3.10` (pluggable backend, default Sonnet, `enterpriseDataPrivacy` + external model ⇒ Maestro disabled; routing still works) + `§5.1` (the 11 read / 5 write / 2 side-channel tool split — the ReadOnly bucket = the 11 reads).
- `design/04_Agent_Supervisor.md §3.10` — the permission matrix this task amends (the `(Strict, _) => MustAsk`-for-all rule and the `ToolClass` bucket vocabulary); `§3.2` for the AwaitingApproval/ResolveApproval intercept flow the write tools fall through to.
- `crates/core/src/agent_supervisor/actor.rs` — the real spine to extend: `AgentKind` enum (`:66`), `as_db_kind` (`:82`), `StartSessionRequest`/`start_session` (`:368`), the parser-pack `match req.agent_kind` at **three** sites (`:629` start_session, `:1357` cold-resume, `:2626` adopt-orphans free-fn), `from_db_kind` cold-resume `match row.agent_kind.as_str()` (`:1248`), `resolve_agent_bin` (`:1818`), `ensure_claude_trusts_dir` (`:1856`, the trust-preseed pattern). `send_input(&SessionId, Vec<u8>)` is the only send-prompt path; `AgentEvent::TurnComplete` rides `session.events.<sid>`.
- `crates/core/src/security/{permission.rs,tool_classes.rs}` — `ToolClass` enum (`permission.rs:352`), `decide()` matrix (`permission.rs:448`, the `(Strict,_)` arm at `:451`), `Decision {AutoApprove, AutoApproveOnce, MustAsk, AutoDeny}` (`:362`), and the `TOOL_CLASSES` `LazyLock<HashMap>` + `classify_tool` fallback-to-`Restricted` (`tool_classes.rs:42`/`:72`).
- `crates/core/src/security/managed.rs` — the provider reads `ManagedPolicy::default_model()` (`:343`, `Option<&str>`), `claude_executable_path()` (`:349`, `Option<&Path>`), and `enterprise_data_privacy()` (`:337`, `Option<bool>`) — all already parsed, currently unread on the Maestro path.
- `tasks/v1.0/313-vcs-provider-github.md` / `tasks/v1.0/305-cone-stats-suggest-seam.md` — the citation-dense FROZEN-marking register + the seam discipline (a not-yet-wired arm returns a **typed `Err`**, never `unimplemented!()`/`todo!()`, never empty-success).

## Scope — in
- **`crates/core/src/agent_supervisor/actor.rs` — `AgentKind::Maestro` + all touch-sites (FROZEN per PHASE4_PLANNING §4.8/§2):**
  - Add `Maestro` to the `AgentKind` enum (`:66`).
  - `as_db_kind` (`:82`): `AgentKind::Maestro => "maestro"` (the `sessions.agent_kind` CHECK **already** allows `'maestro'` — confirm, do NOT touch the schema).
  - `from_db_kind` cold-resume `match row.agent_kind.as_str()` (`:1248`): add `"maestro" => AgentKind::Maestro` so the singleton survives Core restart via `cold_resume_session`.
  - Parser-pack selection at **all three** sites (`:629` start_session, `:1357` cold-resume, `:2626` `adopt_orphans` free-fn): add an `AgentKind::Maestro` arm that uses a **no-op/structured parser pack** (a new `MaestroPack`, or `EchoPack` reused as the explicit safe pass-through if 400 pins that) — **NOT** `ClaudeCodePack`; the Maestro's tool calls travel over MCP, not the PTY scrape. Document the choice; the `:2626` `_ =>` arm already falls through to `EchoPack`, so an explicit `"maestro"` arm there is belt-and-suspenders honesty.
  - `resolve_agent_bin` (`:1818`): add the `AgentKind::Maestro` arm that delegates to the frozen `MaestroProvider` (below) to produce `(bin, args)` = the chosen CLI + `--mcp-config <401 stdio endpoint>` + `--strict-mcp-config` + the Maestro-preamble flag/file + the model flag. **No `--dangerously-skip-permissions`** — the Maestro runs `permission_mode=strict` and tool calls are gated by the resolver.
- **`crates/core/src/security/{tool_classes.rs,permission.rs}` — `ToolClass::ReadOnly` + strict arm (FROZEN per PHASE4_PLANNING §4.8 / D4):**
  - Add `ReadOnly` to `ToolClass` (`permission.rs:352`).
  - Amend `decide()` (`:448`): a new `(Strict, ToolClass::ReadOnly) => AutoApprove` arm **above** the catch-all `(Strict, _) => MustAsk` (which stays for `Safe`/`Restricted`/`Dangerous` — the existing PTY-session strict semantics are unchanged). `auto_decision_string()` already returns `"auto_strict"` for strict — reused unchanged.
  - Classify the **11 read tools** as `ReadOnly` in `TOOL_CLASSES` (`tool_classes.rs:42`): `list_workspaces`, `list_workareas`, `list_sessions`, `get_workspace_summary`, `get_workarea_summary`, `list_recent_activity`, `list_active_schedules`, `read_inbox_summary`, `read_pr_set_for_workarea`, `get_workarea_recent_commits`, `cross_workarea_search`. Leave the **5 write tools** (`route_prompt_to_session`, `fanout_to_sessions`, `create_workspace`, `create_workarea`, `set_workarea_paused`) + `propose_chip` UNclassified so `classify_tool` falls through to `Restricted` ⇒ strict ⇒ `MustAsk` ⇒ the `AwaitingApproval`/`ResolveApproval` chip flow (D4; `notify_user` may also stay `Restricted` — it has a user-visible side effect). Do NOT widen the existing `Safe`/`Restricted`/`Dangerous` tool rows.
- **`crates/core/src/maestro/provider.rs` (new) — the `MaestroProvider` seam (FROZEN per PHASE4_PLANNING §4.3 / D1/D5):**
  - A `pub trait MaestroProvider: Send + Sync` whose method resolves a launch spec — `fn resolve_launch(&self, ctx: &MaestroLaunchContext) -> Result<MaestroLaunchSpec>` — where `MaestroLaunchSpec` carries `{ bin: String, args: Vec<String>, model: String, preamble: String, mcp_config_path: PathBuf, strict_mcp_config: bool, permission_mode: "strict", scratch_cwd: PathBuf }` (the full "which CLI + model + preamble + mcp-config + strict + scratch-cwd to launch" tuple).
  - A LIVE `ClaudeCliProvider` impl that reads `ManagedPolicy::{default_model, claude_executable_path}` (falling back to `"claude"` on `$PATH` and the Sonnet default `claude-4.6-sonnet` per `design/08 R-1`), emits the Maestro preamble, and points `--mcp-config` at 401's stdio endpoint with `--strict-mcp-config`.
  - The Codex/Gemini live arms + the `DirectApiProvider` frozen-unwired arm are **412's** — leave them as a documented seam (412 adds impls behind this trait); do NOT stub them here as empty.
- **`crates/core/src/maestro/mod.rs` (modified — soft seam):** add `pub mod provider;` in a distinct region (additive; auto-merges on rebase per §8.1).
- **`crates/core/src/handlers/sessions.rs` — `parse_agent_kind` (`:405`):** add `"maestro" => Ok(AgentKind::Maestro)` so a wire `agent_kind="maestro"` resolves (the lifecycle spawn at boot is 414's wiring; this keeps the parser honest + round-trippable).
- **Lifecycle (this task's slice):** provide the spawn-config constructor — given `maestro_state.enabled` + a permitted model, build the `StartSessionRequest{ agent_kind: Maestro, cwd: scratch_dir, permission_mode: Some("strict"), .. }` and the scratch-dir creation (`~/concerto/maestro/`). Reuse the existing `adopt_orphans`/`cold_resume_session` host-survival paths unchanged (the maestro singleton chat row identifies it across restart). **The boot-time call site + the `enterpriseDataPrivacy`-disabled gate live in 414**; here, freeze the constructor + the scratch-cwd convention and prove the config assertion in a test. Where the boot wiring is not yet present, the constructor is a pure function returning the request — no `todo!()`.
- Tests (Tier 1): (1) `AgentKind::Maestro` round-trip — `as_db_kind() == "maestro"`, `from_db_kind("maestro") == Maestro`, `parse_agent_kind("maestro") == Maestro`; (2) the strict + `ReadOnly` matrix — every one of the 11 read tools `decide()`s to `AutoApprove` under `Strict`, each of the 5 write tools + `propose_chip` `decide()`s to `MustAsk` under `Strict`, and the existing `Safe`/`Restricted`/`Dangerous` rows still `MustAsk` under `Strict` (no regression); (3) a spawn-config assertion — `ClaudeCliProvider::resolve_launch` (and the `resolve_agent_bin` Maestro arm) produce args containing `--mcp-config`, `--strict-mcp-config`, the model, and NOT `--dangerously-skip-permissions`, with `permission_mode == "strict"` and `cwd == ~/concerto/maestro/`.

## Scope — out
- **Token accounting + daily budget + inert-on-exhaust** — **Task 412** + **Task 403** (403 freezes `maestro_state`/budget; 412 parses CLI/Direct-API token usage, the UTC-midnight/manual reset, and the stale-digest badge per `design/08 R-7`). NET-NEW; do **not** fake it here. `AgentEvent::ContextUsage{pct}` is wired-but-never-emitted and is NOT the carrier — leave it untouched.
- **Codex/Gemini LIVE providers + the `DirectApiProvider` impl** — **Task 412** (extends this task's frozen `MaestroProvider` seam; the `enterpriseDataPrivacy=true` + external-model ⇒ disabled consequence is 412/413's, D1). This task ships Claude-CLI-only LIVE.
- **The boot-time lifecycle spawn + `enterpriseDataPrivacy`-disabled gate + the `MaestroServer` handler** — **Task 414** (calls this task's spawn-config constructor at boot; publishes `maestro.disabled_by_policy`). This task freezes the constructor + scratch convention; 414 wires the call site.
- **The 16 MCP tool schemas + the `concerto-maestro-mcp` transport** — **Task 401** (frozen); this task **consumes** 401's `--mcp-config` endpoint as frozen — it does not re-shape a tool.
- **Write-tool impls + the confirmation-chip execution path** — **Task 406** (consumes the strict ⇒ `MustAsk` matrix this task freezes). This task only classifies the tools so the gate fires.
- **The routing pre-parser** (`@workarea`/`/digest`) — **Task 408** (consumes `AgentKind::Maestro` + `send_input`).
- **Real-world Tier-3:** the real interactive Maestro LLM session (Sonnet-class multi-turn orchestration quality, real `--mcp-config` round-trip against a live agent host, host-crash cold-resume of the live Maestro) is the **Phase-4 Tier-3 operator checklist line** ("drive the Maestro chat end-to-end against a live Claude CLI: strict-mode reads auto-approve, a write tool surfaces a confirmation chip, and the session survives a Core restart"). CI proves only the deterministic type/matrix/config layer.

## Public interface this task locks
- **`AgentKind::Maestro` + DB string + parser/cold-resume arms (FROZEN, design/08 §3.1 / PHASE4_PLANNING §4.8):**
  ```rust
  // crates/core/src/agent_supervisor/actor.rs
  pub enum AgentKind { Echo, Claude, Codex, Gemini, Maestro } // + Maestro

  impl AgentKind {
      pub fn as_db_kind(&self) -> &'static str {
          match self {
              AgentKind::Echo | AgentKind::Claude => "claude",
              AgentKind::Codex => "codex",
              AgentKind::Gemini => "gemini",
              AgentKind::Maestro => "maestro", // sessions.agent_kind CHECK already allows it
          }
      }
  }
  // from_db_kind (cold-resume, ~:1248):  "maestro" => AgentKind::Maestro
  // parser-pack arms (~:629 / ~:1357 / ~:2626): AgentKind::Maestro => no-op/structured pack (NOT ClaudeCodePack)
  // parse_agent_kind (handlers/sessions.rs ~:405): "maestro" => Ok(AgentKind::Maestro)
  ```
- **`ToolClass::ReadOnly` + the strict auto-approve arm (FROZEN, design/04 §3.10 + design/08 §3.2/§3.10 / PHASE4_PLANNING §4.8):**
  ```rust
  // crates/core/src/security/permission.rs
  pub enum ToolClass { Safe, Restricted, Dangerous, ReadOnly } // + ReadOnly

  // decide() — new arm ABOVE the catch-all (Strict, _) => MustAsk:
  (PermissionMode::Strict, ToolClass::ReadOnly) => Decision::AutoApprove,
  (PermissionMode::Strict, _)                   => Decision::MustAsk, // unchanged for Safe/Restricted/Dangerous
  ```
  The 11 read tools classify `ReadOnly` in `TOOL_CLASSES` (`tool_classes.rs`); the 5 write tools + `propose_chip` are left unclassified ⇒ `classify_tool` fallback `Restricted` ⇒ strict ⇒ `MustAsk` ⇒ existing `AwaitingApproval`/`ResolveApproval` chip flow. `Decision`, `auto_decision_string()`, and the non-strict modes are unchanged.
- **The `MaestroProvider` provider-selection seam (FROZEN by 402, extended by 412, design/08 §3.9 / PHASE4_PLANNING §4.3):**
  ```rust
  // crates/core/src/maestro/provider.rs
  pub struct MaestroLaunchContext { /* managed policy view + scratch dir + 401 mcp endpoint */ }
  pub struct MaestroLaunchSpec {
      pub bin: String,
      pub args: Vec<String>,           // includes --mcp-config <endpoint> --strict-mcp-config + model + preamble flag
      pub model: String,               // default "claude-4.6-sonnet" (design/08 R-1)
      pub preamble: String,            // the Maestro preamble (replaces the default agent preamble)
      pub mcp_config_path: PathBuf,    // 401's concerto-maestro-mcp stdio endpoint
      pub strict_mcp_config: bool,     // true ⇒ ONLY Maestro tools visible
      pub permission_mode: String,     // always "strict"
      pub scratch_cwd: PathBuf,        // ~/concerto/maestro/
  }
  pub trait MaestroProvider: Send + Sync {
      fn resolve_launch(&self, ctx: &MaestroLaunchContext) -> concerto_error::Result<MaestroLaunchSpec>;
  }
  pub struct ClaudeCliProvider { /* … */ } // LIVE; reads ManagedPolicy::{default_model, claude_executable_path}
  // 412 adds CodexCliProvider / GeminiCliProvider (LIVE) + DirectApiProvider (frozen-unwired ⇒ typed Err, NOT a macro)
  ```
  (Field names/types are designed minimally + append-friendly and FROZEN; `Result` is `concerto_error::Result`.) Consumes 401's `concerto-maestro-mcp` MCP transport + the 16-tool schema registry **as frozen by Task 401 (PHASE4_PLANNING §4.1)** — this task does not re-lock them.

## Implementation notes
- **The load-bearing rule: strict stays strict for everything except `ReadOnly`.** The new `(Strict, ReadOnly) => AutoApprove` arm is the *only* loosening; the catch-all `(Strict, _) => MustAsk` must remain so PTY workarea sessions (which never carry `ReadOnly` tools) see identical behavior. Order matters — the `ReadOnly` arm goes above the wildcard. Assert the no-regression case in tests.
- **Reuse, don't reinvent: the Maestro is a `start_session` caller.** Do not write a new spawn loop, host handshake, or cold-resume path — `resolve_agent_bin` + the parser-pack arms + `from_db_kind` are the only supervisor edits; host-survival (`adopt_orphans`) and `cold_resume_session` work for free once the arms exist. The maestro singleton is identified by the `chats(kind='maestro')` row (403's bootstrap), so cold-resume after a Core restart finds it.
- **Parser pack must NOT be `ClaudeCodePack`.** The Maestro's structured output is MCP tool calls over the `--mcp-config` channel, not regex-scraped from the PTY; reusing the fragile `ClaudeCodePack` tool-call scraper would double-fire / mis-parse. Use a no-op/structured `MaestroPack` (or `EchoPack` as the explicit pass-through if 400 pins it). The `adopt_orphans` site (`:2626`) already `_ =>` falls to `EchoPack`; add an explicit `"maestro"` arm anyway for honesty.
- **No `--dangerously-skip-permissions`.** The V0.1 Claude arm suppresses Claude's own gates because the workarea is user-trusted; the Maestro instead runs `permission_mode=strict` so *every* tool call is intercepted by `PermissionResolver` (reads auto-approved via `ReadOnly`, writes surfaced as chips). The provider still pre-seeds the scratch-dir trust record (reuse `ensure_claude_trusts_dir` for the Claude CLI so the TUI trust dialog never blocks).
- **The provider seam is the agent-loop shape, distinct from `OneShotLlm` (D5).** `OneShotLlm::suggest -> String` (312, FROZEN) is the one-shot summarizer/digest path (404/409); the Maestro chat agent needs a *launch spec* (binary/model/preamble/mcp-config), not a string completion — that is why this is a separate trait. Do not route the Maestro through `OneShotLlm`.
- **The frozen-unwired arms are 412's; do not pre-stub them empty.** Leave `MaestroProvider` with the LIVE `ClaudeCliProvider` only; 412 adds Codex/Gemini + a `DirectApiProvider` whose `resolve_launch` returns a **typed `Err`** (e.g. `Error::Validation("maestro.direct_api.unimplemented")`), never `unimplemented!()`/`todo!()`, never an empty-success spec. If you must reference the arm, document it as 412-owned.
- **Cross-platform:** the agent supervisor is `#[cfg(unix)]`-heavy (UDS host handshake) like the `sessions`/`streams` handlers — gate any new supervisor-adjacent surface the same way the surrounding code does; the scratch-dir + provider types are pure and build on all lanes. Use forward-slash-safe path construction for `~/concerto/maestro/` (resolve `home::home_dir()` like `ensure_claude_trusts_dir`).
- **Regen:** the `MaestroProvider`/`MaestroLaunchSpec`/`ToolClass`/`AgentKind` Rust surface updates `docs/interfaces/rust-api.md` ⇒ `./scripts/regen-interfaces.sh` regenerates it; commit it. No proto/schema change (no migration; the `sessions.agent_kind` CHECK already admits `'maestro'`).
- **Parallel build hint:** the three FROZEN surfaces are file-disjoint and can fan out, then integrate into one commit — **AgentKind+spawn-arm** (`agent_supervisor/actor.rs` + `handlers/sessions.rs`) ∥ **ToolClass::ReadOnly+strict-matrix** (`security/{permission,tool_classes}.rs`) ∥ **provider-trait-freeze** (`maestro/{mod,provider}.rs`). The spawn arm depends on the provider trait only at the `resolve_agent_bin` wiring seam (integrate last).

## Verification
**Tier 1.** The `rust` §5.3 command set.
1. `cargo check --workspace` clean (the new `Maestro` variant, `ReadOnly` class, and `provider.rs` module compile; the `maestro/mod.rs` `pub mod provider;` resolves).
2. `cargo clippy --workspace --all-targets -- -D warnings` clean (exhaustive `match` arms — the new `AgentKind`/`ToolClass` variants force every `match` to be updated, which clippy/`check` enforces; this is the safety net for the three parser sites).
3. `cargo fmt --all -- --check` clean.
4. `cargo test -p concerto-core maestro` (+ `permission` + `agent_kind`) → proves: (a) `AgentKind::Maestro` round-trip (`as_db_kind`/`from_db_kind`/`parse_agent_kind`); (b) the strict+`ReadOnly` matrix — 11 reads `AutoApprove`, 5 writes + `propose_chip` `MustAsk`, `Safe`/`Restricted`/`Dangerous` still `MustAsk` under strict; (c) the spawn-config assertion — `ClaudeCliProvider::resolve_launch` + the `resolve_agent_bin` Maestro arm emit `--mcp-config`/`--strict-mcp-config`/the model, omit `--dangerously-skip-permissions`, set `permission_mode="strict"` and `cwd=~/concerto/maestro/`.
5. `cargo test --workspace --no-fail-fast` → all pass (the existing `tool_classes`/`permission` tests stay green — no regression on `Safe`/`Restricted`/`Dangerous`).
6. `cargo deny check` → green (no new workspace pins; `rmcp` was vetted by 401).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`rust-api.md` gains `MaestroProvider`/`MaestroLaunchSpec` + the `AgentKind::Maestro`/`ToolClass::ReadOnly` variants). No `proto.md`/`schema.md` delta.
8. `scripts/smoke.sh` → **unchanged** gate (this task adds the agent-kind/permission/provider types but wires no boot-time spawn — the maestro-digest smoke capability is turned on by 414; the V0.1 session-spawn path still boots). Exits 0.

**Tier-1 scope + what it does NOT cover.** CI fully proves the deterministic layer: the `AgentKind::Maestro` round-trip, the strict+`ReadOnly` permission matrix (reads auto-approve, writes/`propose_chip` `MustAsk`, no regression), and the launch-spec assertion (flags/model/cwd/no-skip-permissions). It does **NOT** cover the **real interactive Maestro session** — a live Claude CLI dialed at 401's `--mcp-config`, multi-turn orchestration quality, a real write-tool confirmation chip round-trip, or host-crash cold-resume of the live Maestro. That is the **Phase-4 Tier-3 operator checklist line** "drive the Maestro chat end-to-end against a live Claude CLI: strict-mode reads auto-approve, a write tool surfaces a confirmation chip, the session survives a Core restart," signed off at the phase gate (Codex/Gemini/Direct-API backends are 412's).

## Definition of Done
- [x] `AgentKind::Maestro` added + all touch-sites updated: `as_db_kind => "maestro"`, `from_db_kind` cold-resume arm, parser-pack arm at all three sites (no-op/structured pack, NOT `ClaudeCodePack`), `resolve_agent_bin` Maestro arm, `parse_agent_kind` arm — FROZEN per PHASE4_PLANNING §4.8
- [x] `ToolClass::ReadOnly` added + the `(Strict, ReadOnly) => AutoApprove` arm above the unchanged catch-all; the 11 read tools classify `ReadOnly`; the 5 write tools + `propose_chip` stay `Restricted` ⇒ strict ⇒ `MustAsk` ⇒ existing chip flow — FROZEN per §4.8 / D4
- [x] `MaestroProvider` trait + `MaestroLaunchSpec` frozen with `ClaudeCliProvider` LIVE (reads `ManagedPolicy::{default_model, claude_executable_path}`, default Sonnet); Codex/Gemini/Direct-API left as the 412-owned seam — FROZEN per §4.3 / D1/D5
- [x] Maestro launches the chosen CLI with the Maestro preamble + `--mcp-config` (401) + `--strict-mcp-config` + `permission_mode=strict` + scratch `cwd=~/concerto/maestro/`; NO `--dangerously-skip-permissions`
- [x] Spawn-config constructor + scratch-dir convention frozen (boot call site + `enterpriseDataPrivacy` gate deferred to 414); no migration (author-checked highest = 0014)
- [x] Tests: `AgentKind` round-trip, strict+`ReadOnly` matrix (incl. no-regression), launch-spec assertion
- [x] All Verification commands pass on a clean checkout; smoke gate unchanged (green)
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams — the 412-owned `DirectApiProvider` arm — return a typed `Err`/`Status`, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed (`rust-api.md`; no proto/schema change)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/agent_supervisor/actor.rs` (modified — `AgentKind::Maestro` + `as_db_kind`/`from_db_kind` arms + parser-pack arms at the three sites + the `resolve_agent_bin` Maestro arm delegating to `MaestroProvider`)
- `crates/core/src/security/permission.rs` (modified — `ToolClass::ReadOnly` + the `(Strict, ReadOnly) => AutoApprove` arm)
- `crates/core/src/security/tool_classes.rs` (modified — the 11 read tools classified `ReadOnly`; round-trip tests)
- `crates/core/src/handlers/sessions.rs` (modified — `parse_agent_kind` `"maestro"` arm)
- `crates/core/src/maestro/provider.rs` (new — the FROZEN `MaestroProvider` trait + `MaestroLaunchSpec`/`MaestroLaunchContext` + LIVE `ClaudeCliProvider`)
- `crates/core/src/maestro/mod.rs` (modified — `pub mod provider;` in a distinct region; the spawn-config constructor + scratch-cwd convention)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-4: Maestro-as-agent — AgentKind::Maestro, ToolClass::ReadOnly, MaestroProvider (Claude live)

Spawns the long-lived Maestro session by reusing the Agent-Supervisor
spine (start_session / host-survival / cold-resume). Freezes
AgentKind::Maestro (+ all touch-sites, no-op parser pack), ToolClass::ReadOnly
auto-approving the 11 read tools under strict while writes/propose_chip stay
MustAsk (the confirmation-chip flow), and the MaestroProvider launch-spec
seam with Claude CLI LIVE (--mcp-config + --strict-mcp-config + strict +
scratch cwd). Codex/Gemini/Direct-API + token budget are Task 412. Tier-1:
AgentKind round-trip, strict+ReadOnly matrix, launch-spec assertion. Real
interactive Maestro is the Phase-4 Tier-3 gate.

Refs: tasks/v1.0/402-maestro-as-agent.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan** —
  - **Parser pack:** chose a NEW `MaestroPack` (`crates/core/src/agent_supervisor/parsers/maestro.rs`), NOT reused `EchoPack`. The trait's `agent_kind()` returns a single `AgentKind`, so a distinct pack keeps the pack→kind round-trip honest; behaviour matches Echo's pass-through (raw bytes + one assistant message, never `AwaitingApproval`) since the Maestro's tool calls ride MCP, not the PTY scrape. All three parser sites (`start_session` ~:629, cold-resume ~:1357, `adopt_orphans` ~:2626) use it; the `:2626` site got an explicit `"maestro"` arm above the `_ => EchoPack` fallthrough for honesty.
  - **`MaestroLaunchSpec` fields:** match the frozen sketch exactly (`bin, args, model, preamble, mcp_config_path, strict_mcp_config, permission_mode, scratch_cwd`). `MaestroLaunchContext` = `{ managed: ManagedPolicy, scratch_cwd, mcp_config_path }`. `ClaudeCliProvider` emits args `["--model", <model>, "--mcp-config", <path>, "--strict-mcp-config", "--append-system-prompt", <preamble>]` and never `--dangerously-skip-permissions`.
  - **Scratch-cwd helper location:** landed in `maestro/mod.rs` (Task 402 region), NOT `provider.rs` — `maestro_scratch_dir()` / `ensure_maestro_scratch_dir()` / `maestro_start_request()` / `ensure_maestro_scratch_trusted()`. Const `MAESTRO_SCRATCH_SUBDIR = "concerto/maestro"`. The `resolve_agent_bin` Maestro arm builds the spec inline (default `ManagedPolicy`, `mcp_config_path = req.cwd.join(".mcp.json")`).
  - **`from_db_kind`:** extracted as a `pub fn AgentKind::from_db_kind(&str) -> Option<AgentKind>` next to `as_db_kind` (the cold-resume site now calls it) so the DB round-trip is unit-testable — additive, keeps the `"maestro"` mapping identical to the inline arm it replaced.
  - **`ensure_claude_trusts_dir`:** promoted `fn → pub(crate) fn` + a `pub(crate) use` re-export from `agent_supervisor/mod.rs` so the Maestro scratch helper can reuse the trust-preseed. start_session's trust-preseed `matches!` now also fires for `AgentKind::Maestro`.
  - **Author-check (migration):** highest migration on `main` is **`0015_maestro_state.sql`** (Task 403 already merged into this base), NOT 0014 — the task doc's §3 said "still 0014" but 403 landed first. This task adds **no migration**; the `sessions.agent_kind` CHECK already admits `'maestro'`. The 0016 reservation (Task 410) is intact. No block-shift needed (we add nothing).
- **Open questions for next task** —
  - **Task 412** extends the FROZEN `MaestroProvider` trait (`crates/core/src/maestro/provider.rs`): implement `MaestroProvider for CodexCliProvider`/`GeminiCliProvider` (LIVE) building the same `MaestroLaunchSpec` shape via `ManagedPolicy::{codex,gemini}_executable_path()`; fill the body of the already-present `DirectApiProvider::resolve_launch` (currently returns `Err(Error::Validation("maestro.direct_api.unimplemented: …"))`). It also adds the daily budget + token counting (consuming 403's `maestro_state`). FROZEN surface it builds on: `trait MaestroProvider::resolve_launch`, `MaestroLaunchSpec`, `MaestroLaunchContext`.
  - **Task 406** consumes the FROZEN strict ⇒ `MustAsk` matrix: the 5 write tools + `propose_chip`/`notify_user` classify `Restricted` (via the `classify_tool` fallthrough — they are deliberately UNregistered in `TOOL_CLASSES`) ⇒ strict ⇒ `MustAsk` ⇒ the existing `AwaitingApproval`/`ResolveApproval` chip flow. FROZEN surface: `ToolClass::ReadOnly` + the `(Strict, ReadOnly) => AutoApprove` arm in `security/permission.rs::decide`.
  - **Task 414** calls the FROZEN spawn-config constructor `maestro::maestro_start_request(workarea_id, scratch_cwd)` at boot (after `ensure_maestro_scratch_dir()` + `ensure_maestro_scratch_trusted()`), gated on `maestro_state.enabled` + the `enterpriseDataPrivacy`-disabled policy, then hands the request to `AgentSupervisorHandle::start_session` (host-survival/cold-resume work for free). FROZEN surface: `AgentKind::Maestro` + the `maestro::{maestro_start_request, ensure_maestro_scratch_dir, MAESTRO_SCRATCH_SUBDIR}` helpers.
- **Deliberate debt** — No `todo!()`/`unimplemented!()`/`TODO`/`FIXME` in new code (verified). The 412-owned `DirectApiProvider::resolve_launch` is a documented seam returning a typed `Err(Error::Validation(...))`, NOT an empty stub or a macro. Codex/Gemini are NOT pre-stubbed here (412 adds them behind the trait). Token accounting is intentionally absent — `AgentEvent::ContextUsage{pct}` left untouched. The boot-time lifecycle spawn + the `enterpriseDataPrivacy` gate are intentionally deferred to 414; the constructor here is a pure function returning the request.
- **Smoke-gate state** — **unchanged.** No `scripts/smoke.d/*` / `smoke.manifest` change (no boot-time Maestro spawn; the maestro-digest capability is 414's). `scripts/smoke.sh` is not required for an `unchanged` gate but WAS run as a no-regression check: **PASS (exit 0, 157s, all checks PASSED)**.
