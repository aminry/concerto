# 08 — Maestro Agent

*Sub-system design doc. Inherits locked decisions from `00_Architecture_Overview.md`. PRD §14 defines the product. This is the LLM session behind the "Concerto chat" at the top of the app.*

---

## 1. Purpose & scope

The Maestro Agent is an **agent process supervised by 04 with a different toolset and no working directory**. Its job (PRD §14.1):

- Be the "dispatcher / historian / planner" so the user can spend attention on review + prompt-writing.
- Surface a digest after the user returns from absence.
- Route prompts to specific workareas / sessions via `@workarea` and `@workarea/session-kind` syntax.
- Spawn new workspaces + workareas from natural language.
- Answer cross-workarea / cross-workspace queries ("what touched libs/auth today?").
- Propose next steps as chips.

It owns:

- **Maestro session lifecycle** — itself a session (per 03's 3-level model) running under a special "maestro" workarea-equivalent with restricted tools.
- **Maestro tool implementations** — ~16 tools the agent can call covering workspaces, workareas, sessions.
- **Digest generation** — periodic and on-demand rolling summaries across active workareas.
- **Per-workarea summary cache** — short summaries of workarea state (plus its sessions' last turns), refreshed as workareas work.
- **Routing parser** — `@workarea` / `@workarea/session` / `@all` / `@idle` / `@blocked` / `/digest` / `/pause` / `/new`.
- **Privacy enforcement** — what the Maestro can read from each workarea (summary-only by default; full chat opt-in per workspace).
- **Maestro chat history** — persisted like any chat; with daily condense pass.
- **Cost guardrails** — per-day token cap; cheap-model default.

It does **not** own: writing code (no edit/shell tools); modifying workareas directly (it sends prompts; workarea sessions do the work); replacing per-session chats.

---

## 2. Phase scope

| Phase | What ships |
|---|---|
| **V0.1** | (not in V0.1) |
| **V1.0** | Full Concerto chat with the 16-tool set (§5). `@workarea` and `@workarea/session-kind` routing. Digest on user return. New-workspace-and-workarea-from-natural-language. Suggested next-step chips. Per-workarea summary cache. Default model: Sonnet. Daily token budget. Per-workarea `exclude_from_maestro` toggle. |
| **V2.0** | + MCP-augmented context (Linear, Slack — fetches issue text, channel summaries). + cross-workarea search over commits/diffs. + voice-first interaction on Apple Watch. + spawn parallel multi-workarea plans from one prompt. |

---

## 3. Key design decisions (sub-system-internal)

### 3.1 The Maestro is itself an agent process

**Choice:** The Maestro runs as a long-lived **PTY-CLI session** under the Agent Supervisor (04) — concretely, a session spawned through the existing `AgentSupervisorHandle::start_session(StartSessionRequest{ workarea_id, agent_kind, echo_text, cwd, permission_mode, resume_session_id }) -> SessionId` (built signature: `crates/core/src/agent_supervisor/actor.rs:368`, struct at `:94`), under a **new `AgentKind::Maestro`** variant. It reuses the same `concerto-agent-host` machinery (PTY + CBOR-over-UDS, §3.6 of `04`) as workarea sessions, but:

- The agent CLI is invoked with **our** tool set served by the in-process `concerto-maestro-mcp` server (§3.2) — dialed via the CLI's own `--mcp-config` + `--strict-mcp-config`, not the default tools.
- Working directory is `cwd = ~/concerto/maestro/` (a scratch dir created at spawn; **not** a worktree, **no** edit-mutex — the Maestro has no file-edit tools).
- The agent has no file-edit, no shell, no network — a strictly restricted toolset.
- `permission_mode = "strict"` (every Maestro write tool call is intercepted by 04's PermissionResolver; the read tools are classified `ToolClass::ReadOnly` and auto-approve under strict — see `04 §3.10`).
- **Tool calls ride MCP, not PTY-scrape.** The Maestro uses a **no-op / structured parser pack** — its tool calls arrive over the `concerto-maestro-mcp` transport, so it does **not** reuse the fragile `ClaudeCodePack` terminal-regex scraper for tool-call extraction.

**Why "an agent" rather than a custom orchestrator:**
- Reuses 04's lifecycle, host-survival, cold-resume, token tracking.
- Stays close to the underlying agent's reasoning loop — natural language in, structured tool calls out.
- Same UI patterns (chips, voice, attachments).

> **Built-state reconciliation (2026-06-09).** `AgentKind` on `main` is `{Echo, Claude, Codex, Gemini}` with **no `Maestro` arm** (`crates/core/src/agent_supervisor/actor.rs:66`); `resolve_agent_bin` (`actor.rs:1818`) hardcodes `("claude", ["--dangerously-skip-permissions"])` and passes **no** `--mcp-config`/`--strict-mcp-config`/model/preamble/permission-mode. The `sessions.agent_kind` CHECK already accepts `'maestro'` (schema pre-provisioned). **Task 402** adds the `AgentKind::Maestro` variant (`as_db_kind`/`from_db_kind`/`resolve_agent_bin` spawn arm/parser-pack match/`parse_agent_kind`) + the scratch-cwd + strict-mode spawn — **FROZEN by Task 402** (PHASE4_PLANNING §4.8). This doc only pins the shape; it writes no code, adds no proto/Rust-api/SQL, and reserves no migration — `scripts/regen-interfaces.sh` produces an **empty diff** for this commit (do not expect a `docs/interfaces/` change from a doc-only reconciliation).

### 3.2 Tool definitions: net-new in-process MCP server + Core↔CLI MCP-stdio transport

**Choice:** Concerto hosts a built-in, **in-process `rmcp` stdio MCP server** named `concerto-maestro-mcp` (module `crates/core/src/maestro/mcp.rs` — the **first** MCP *server* in the codebase) that exposes the Maestro tools (§5.1) over the MCP wire protocol. When the Maestro agent spawns, the supervisor configures the CLI to dial **only** this server via the **CLI's own `--mcp-config`** (pointing at the stdio endpoint) **+ `--strict-mcp-config`** (so ONLY the 16 Maestro tools are visible — no filesystem, no shell, no other MCP servers). The agent calls the tools via standard MCP semantics.

**Why MCP:** it's the contract the agent CLIs already speak. We don't invent a new tool-calling shape; we leverage what's there.

The `concerto-maestro-mcp` server runs **in-process inside the Core** (same binary, no separate process). It is reached by an `rmcp` **stdio MCP transport** — a **net-new** mechanism with no precedent in the codebase.

> **Built-state reconciliation (2026-06-09).** This transport **does not exist yet**, and the prior design language above ("in-process MCP transport, stdio over a pipe to the agent host") conflated two unrelated things. To be precise:
> - The existing `concerto-agent-host` is **PTY + CBOR-over-UDS *terminal* multiplexing** (`04 §3.6`/§3.9) — it is **NOT** an MCP transport. The Maestro's MCP stdio transport is a separate, net-new channel the CLI opens directly to the in-process `rmcp` server; the agent-host PTY stream and the MCP stdio stream are distinct.
> - The only MCP code on `main` today is **read-only config *discovery*** — `crates/core/src/agent_supervisor/mcp.rs:54` (`McpServer`) / `:73` (`McpScope`) / `:162` (list servers) parse `~/.claude/mcp.json`-style config files; there is **no MCP server** and `rmcp` is not in the workspace. Do **not** mistake `agent_supervisor/mcp.rs` for "the existing MCP server to wire up."
> - `rmcp` is a **net-new workspace dependency**; adding it is a `cargo deny` operator **Stop-and-ask** (the Task 313 octocrab/RUSTSEC precedent).
>
> **FROZEN by Task 401** (PHASE4_PLANNING §4.1): the `rmcp` add, the `crates/core/src/maestro/mcp.rs` module, the transport, and the 16 frozen tool schemas. The 16-tool split (11 read / 5 write / 2 side-channel, §5.1) is correct as written — this section only makes concrete **how** the tools reach the agent, not which tools exist.

### 3.3 Reading state — summaries by default, at the workarea grain

**Choice:** The Maestro's `get_workarea_summary` tool reads from a **cache** of per-workarea summaries, not from raw chat. Cache shape:

```rust
pub struct WorkareaSummary {
    pub workarea_id: WorkareaId,
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub composer_name: String,
    pub branch_name: String,
    pub status: WorkareaStatus,
    pub last_activity_at: Instant,

    // Aggregated across all sessions of this workarea (each session's
    // last turn is summarized by its agent; we keep the most recent).
    pub sessions: Vec<SessionSummary>,
    pub last_turn_summary: String,                    // ≤ 300 chars; from the most recently active session
    pub last_3_turn_summaries: Vec<String>,

    // Hard facts (per repo in the workarea — no LLM)
    pub repos: Vec<RepoSummary>,                      // commits_ahead, files_changed, lines, pr_state, ci_state per repo

    pub blocked_on: Option<BlockedReason>,            // "awaiting_approval" | "test_failure" | "merge_conflict" | ...

    pub generated_at: Instant,
    pub generation: u64,
}

pub struct SessionSummary {
    pub session_id: SessionId,
    pub agent_kind: AgentKind,
    pub model: String,
    pub status: SessionStatus,
    pub last_turn_summary: String,
}

pub struct RepoSummary {
    pub repository_id: RepositoryId,
    pub repo_name: String,
    pub commits_ahead: u32,
    pub files_changed: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
    pub pr_state: Option<PrState>,
    pub ci_state: Option<CiState>,
}
```

**Per-workarea privacy toggle:** if `workareas.settings_json.exclude_from_maestro = true`, only the hard facts (status, branch, repo names) are exposed; summaries are blanked. The workarea shows up as `[private workarea, name only]` in the Maestro's view.

**Per-workspace full-chat access:** `workspaces.settings_json.concerto_chat_full_chat_access = true` lifts the Maestro out of summary-only and grants it the raw last-3-turns of chat (per session). Off by default.

### 3.4 Summary generation

**Choice:** Two sources, in order of preference:

1. **The workspace agent's own end-of-turn summary** — emitted via the parser packs (04). Some agents (Claude Code with `--end-summary`) produce this naturally. Stored on each `chat_messages` row that closes a turn.
2. **A Concerto-side rolling summarizer** — when the agent doesn't produce its own, the Core runs a tiny Haiku-class summarization call against the last N messages (with rate-limit + token budget). Cached.

The first option is free (the agent was already going to summarize). The second costs tokens but is cheap.

Summaries refresh:
- After every workspace agent turn complete (the agent owns the cadence).
- After 10 minutes of inactivity (the Concerto summarizer ensures freshness).
- On `GetMaestroDigest` call (force-refresh if stale > 60 s).

### 3.5 Routing: `@workarea` and `@workarea/session` parser

**Choice:** The Maestro's text input goes through a **pre-parse** before the LLM sees it. The routing syntax targets workareas (by composer name) or specific sessions within a workarea (by agent kind):

```
"@bach apply the migration pattern from chopin"
  → { kind: "workarea", target: "bach", body: "..." }
  → routes to the most-recently-active session in workarea "bach"

"@bach/claude apply the migration pattern from chopin"
  → { kind: "session", workarea: "bach", agent_kind: "claude", body: "..." }
  → routes to the Claude session in workarea "bach" specifically

"@bach,@mozart apply..."
  → { kind: "fanout", targets: ["bach", "mozart"], body: "..." }

"@all status"   / "@idle ..."   / "@blocked ..."
  → resolves dynamically to a set of workareas/sessions at routing time
```

Cross-workspace disambiguation: workarea composer names are unique within a workspace but not across workspaces. When the user types `@bach` and multiple workspaces have a "bach" workarea, the Maestro picks the **currently-selected** one (the one the UI has focus on). If ambiguous, the Maestro asks: "I see two `bach` workareas — Idempotency keys / Login refactor. Which?"

When a routing directive is detected:

1. The pre-parser handles it directly — calls `route_prompt_to_session(session_id, prompt)` per resolved target — without involving the Maestro LLM (saves tokens, ensures literal routing).
2. The Maestro records the action in its chat history with a synthesized assistant message ("Routed to bach / Claude").
3. The user sees the session's response surfaced back in the Maestro chat as quoted lines.

Free-form text (no `@`, no `/`) goes to the Maestro LLM normally.

This means **routing is deterministic** (no risk of LLM mis-routing) and **the Maestro only spends tokens on the questions that actually need its reasoning**.

### 3.6 Digest generation (the killer feature)

PRD §14.4.3. When the user has been away > 30 minutes and reopens Concerto, or asks `/digest`:

```
generate_digest():
    summaries = [get_workarea_summary(wa) for wa in active_workareas]
    deltas_since_last_user_activity = compute_deltas(summaries, last_seen_at)
    prompt = build_digest_prompt(summaries, deltas)
    response = LLM.complete(prompt, model=sonnet, max_tokens=600)
    chips = suggestion_engine.next_step_chips(workareas=active_workareas)
    return Digest { text: response, chips: chips }
```

The digest prompt is templated:

```
You are Concerto's maestro. The user just returned after being away N minutes.
Here is the state of their {n} active workareas (grouped by workspace):

{per_workarea_block}

Write a concise (3-5 sentence) digest. Group by:
- Finished (and ready for action)
- Blocked (and needing user input)
- Still working (with current focus)

End with a one-line proposed next step.
```

The output is rendered above the standard chat composer. Chips come from 07.

Target: < 5s p50 (PRD §22.3) — measured from `GetDigest` RPC to UI render.

> **Built-state reconciliation (2026-06-09).** The `generate_digest()` block above is **pseudo-code over not-yet-built seams**, not a built API. `next_step_chips`, `compute_deltas`, and `LLM.complete(..., model=sonnet)` have no implementation on `main`. The **live V1.0** summarizer/digest path is the deterministic fallback `DeterministicOneShot` via the FROZEN `OneShotLlm` seam (Task 312; `ActionKind::DigestSummary`); the real-LLM digest is **Task 412**'s separate provider seam, judged for quality + latency at the Phase-4 Tier-3 gate. The Maestro's digest **chips** are persisted on the digest's `chat_messages` row (Task 409), **not** left in 07's volatile ~60s suggestion buffer. A future reader should not mistake this pseudo-code for a built signature.

### 3.7 Maestro chat history — daily condensation

**Choice:** The Maestro's chat grows continuously. To keep token cost bounded:

- Recent messages (last 24h) are kept verbatim.
- Each day, an offline summarizer pass condenses 24-48h-old messages into a one-paragraph daily summary.
- The agent's input is `daily_summaries[:weekly] + verbatim[last 24h] + user's latest`.
- The user sees the full unabridged history in the UI; only the agent's input is summarized.

This keeps per-day cost roughly flat regardless of session length.

### 3.8 Spawn new workspace + workarea from natural language

**Choice:** The Maestro's `create_workspace_from_description` tool wraps multiple sub-calls:

1. Parse the description for an issue reference (Linear, GitHub) → if found, fetch via 13.
2. Detect multi-repo intent (e.g., "across both the API and the iOS app") → propose the repo set.
3. Plan-mode cone suggestion via Repo Mgr (02) → propose per-(workarea, repo) cones.
4. Compose a confirmation chip slate ("Create workspace + first workarea (bach)" / "Just create the workspace, no workarea yet" / "Edit repo set / cones").
5. The user confirms; create flows through Workspace/Workarea/Session Mgr (03) as normal — first the workspace, then the first workarea, then an initial session (Claude in plan mode by default).

The Maestro never creates a workspace, workarea, or session silently. The user always confirms.

### 3.9 Cost: provider, model, and budget

**Pluggable LLM provider.** The Maestro's LLM backend is **user-configurable** in Settings → Concerto Chat → Backend. Same pattern as the Suggestion Engine's ranking backend (`07 §3.11`):

| Backend | How it routes | Auth source |
|---|---|---|
| **Claude CLI** | Long-lived `claude` session spawned by 04 via `concerto-agent-host`; the Concerto preamble is replaced with the Maestro preamble; tool surface is `concerto-maestro-mcp`. | User's Claude Code auth (Pro/Max session or API key). |
| **Codex CLI** | Same shape via `codex`. | User's Codex auth. |
| **Gemini CLI** | Same shape via `gemini`. | User's Gemini auth. |
| **Direct API** | The Maestro session runs against a direct provider API (Anthropic / OpenAI / Bedrock / Vertex / OpenRouter / Vercel AI Gateway / Azure AI Foundry), bypassing CLI subprocesses. Tool-calling implemented via the provider's native function-call API. Foundry uses the deployment-name → model-name mapping configured in Settings → Providers. | API token in `09 §3.7` keychain. |

**Defaults:**

- **Backend:** Auto-pick the first available from `Claude CLI → Codex CLI → Gemini CLI → Direct API (Anthropic)`. User can change anytime.
- **Model:** `claude-4.6-sonnet` (cheaper than Opus, plenty smart for orchestration). For Codex / Gemini / Direct API, the equivalent tier ("Sonnet-class").
- **Daily input budget:** 200K tokens.
- **Daily output budget:** 50K tokens.

All user-configurable in Settings → Concerto Chat.

When the budget is exceeded:
- The Maestro goes inert (no LLM calls); routing and tool calls (deterministic) still work.
- The UI shows a yellow banner: "Maestro budget exhausted; routing still works."
- Resets at UTC midnight or on user-clicked "Reset budget."

**Provider switching mid-day:** the user can change backend at any time; in-flight calls finish on the old backend, new calls go to the new. Daily budget is **per backend** (changing backend doesn't reset the day's total — the cumulative count is across backends). Audit log records backend switches.

For enterprise with on-prem LLMs (Bedrock with VPC, Vertex, Azure AI Foundry, local): use the **Direct API** backend with the appropriate base URL.

**V1.0 backend scope (PHASE4_PLANNING D1).** Of the four backends above, the **three CLI backends — Claude / Codex / Gemini — ship LIVE** in V1.0, reusing the existing `concerto-agent-host` PTY machinery + the `AgentKind::Maestro` spawn (§3.1). The **Direct API backend ships as a FROZEN, unwired Tier-1 seam** — a native function-call loop with no precedent in the codebase — that returns a typed `unimplemented` (never `todo!()`/`unimplemented!()`, never empty-success) until a fast-follow wires it. Consequence (recorded in §3.10): with `enterpriseDataPrivacy=true` and an external model, Maestro is **disabled**, so for V1.0 such orgs get **routing only** (deterministic, zero-token) and **never an external-model digest**; on-prem Direct-API Maestro (Bedrock-VPC / Vertex / Azure AI Foundry) is a **Tier-3 gate / fast-follow**, outside V1.0's automatable bar. The daily budget is **cumulative across backends** but is owned/wired by Tasks 403 (the `maestro_state` counters, migration 0015) and 412 (the counting + inert-on-exhaust), not by this design edit.

> **Built-state reconciliation (2026-06-09).** There is **zero** Maestro provider abstraction and **zero** token accounting on `main` (`AgentEvent::ContextUsage{pct}` is wired-but-never-emitted and is **not** the budget carrier). The interactive-agent provider-selection seam (which CLI + model + Maestro preamble + `--mcp-config` + strict mode + scratch cwd to launch) is **FROZEN by Task 402 with Claude CLI live**, and **extended by Task 412** (Codex/Gemini live + the `DirectApiProvider` frozen-unwired arm + the daily budget) — PHASE4_PLANNING §4.3. This seam is distinct from the one-shot `OneShotLlm` (§3.6) used for summaries/digests.

### 3.10 `enterpriseDataPrivacy` interaction

When `managed.json.enterpriseDataPrivacy = true`:

- If the Maestro's model is **external** (Anthropic API, etc.): Maestro is disabled. Tray + UI shows it's off due to policy.
- If the Maestro's model is **on-prem** (Bedrock with VPC, Vertex, Azure AI Foundry, local LLM): Maestro works normally.

Routing still works in all modes (deterministic, no LLM).

> **Built-state reconciliation (2026-06-09).** Because the on-prem path requires the **Direct API backend**, and Direct-API is a **frozen-unwired Tier-1 seam in V1.0** (§3.9, PHASE4_PLANNING D1), the *only* live consequence in V1.0 is the disabled branch: `enterpriseDataPrivacy=true` + an external CLI model ⇒ **Maestro disabled ⇒ routing-only** (deterministic, no external-model digest). The "works normally on-prem" branch is a **fast-follow + Phase-4 Tier-3 gate** item once Direct-API is wired (the operator confirms at the gate that `enterpriseDataPrivacy=true` truly yields routing-only / no external-model digest). The disabled-by-policy behavior is enforced by **Task 413** (the `WorkspaceSettingsResolver::enterprise_data_privacy()` gate before any external summary/digest).

---

## 4. Data model

### 4.1 Persistent

A dedicated `chats` row with `kind = 'maestro'`. Its `chat_messages` are the Maestro chat history.

```sql
-- A single maestro chat per user (one row in chats)
-- Reuses chat_messages

CREATE TABLE maestro_state (
    id              INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
    daily_in_today  INTEGER NOT NULL DEFAULT 0,
    daily_out_today INTEGER NOT NULL DEFAULT 0,
    budget_resets_at INTEGER NOT NULL,
    last_digest_at  INTEGER,
    enabled         INTEGER NOT NULL DEFAULT 1
);
```

Daily summaries are stored as `chat_messages` with a `metadata.role_extra = 'daily_summary'` tag.

### 4.2 In-memory

```rust
pub struct MaestroState {
    session: Option<SessionId>,                  // the running Maestro session
    summary_cache: HashMap<WorkareaId, WorkareaSummary>,
    daily_budget: TokenBudget,
    pending_routings: HashMap<RoutingId, RoutingHandle>,
}
```

---

## 5. Interfaces

### 5.1 Maestro tools (exposed to the Maestro agent via MCP)

The tool set is split across the 3 levels:

```
# Read-only — workspace/workarea/session hierarchy
list_workspaces()                                      → [{id, name, archived, n_workareas, n_repos}]
list_workareas(workspace_id?)                          → [{id, workspace_id, composer, branch, status, last_activity}]
list_sessions(workarea_id?)                            → [{id, workarea_id, agent_kind, status, last_activity}]
get_workspace_summary(workspace_id)                    → { workspace, n_active_workareas, ... }
get_workarea_summary(workarea_id)                      → WorkareaSummary

# Read-only — adjacent state
list_recent_activity(since)                            → [Event]
list_active_schedules()                                → [Schedule]
read_inbox_summary()                                   → InboxSummary
read_pr_set_for_workarea(workarea_id)                  → PrSetStatus
get_workarea_recent_commits(workarea_id, repo_id?)     → [Commit]
cross_workarea_search(query)                           → [Hit]      (commits, diffs, todos across all workareas)

# Write — all require user confirmation chip in the UI before executing
route_prompt_to_session(session_id, prompt)
fanout_to_sessions([session_ids], prompt)
create_workspace(spec)                                 → workspace_id  (user confirms)
create_workarea(workspace_id, spec)                    → workarea_id   (user confirms)
set_workarea_paused(workarea_id, paused: bool)

# Side-channels
notify_user(text, severity)                            → goes through 14
propose_chip(chip)                                     → adds to current slate
```

**16 tools total**, classified for 04's strict-mode permission matrix (`04 §3.10`):

- **11 read tools → `ToolClass::ReadOnly`** (auto-approve under strict): `list_workspaces`, `list_workareas`, `list_sessions`, `get_workspace_summary`, `get_workarea_summary`, `list_recent_activity`, `list_active_schedules`, `read_inbox_summary`, `read_pr_set_for_workarea`, `get_workarea_recent_commits`, `cross_workarea_search`.
- **5 write tools + `propose_chip` → `MustAsk` under strict** (surfaced as the existing `AwaitingApproval`/`ResolveApproval` confirmation chip, carrying `urgent`/`destructive_label`): `route_prompt_to_session`, `fanout_to_sessions`, `create_workspace`, `create_workarea`, `set_workarea_paused`, and `propose_chip`. **No bypass** of the write-tool confirmation (R-2).
- `notify_user` is a side-channel that **routes to 14** (not a confirmation chip).

(The matching `ToolClass::ReadOnly` rule + the 11-read/5-write classification live in `04 §3.10`; keep the tool names identical across the two docs. The new permission class is **FROZEN by Task 402** — PHASE4_PLANNING §4.8.)

The `concerto-maestro-mcp` server runs **in-process** (same Core binary) over the net-new `rmcp` **stdio MCP transport** (§3.2 — distinct from the agent-host PTY/CBOR-over-UDS stream). It is **not** the same as `concerto-mcp` — that server is for **workarea sessions** and exposes workarea-introspection tools (see `04 §3.6`/§3.11). The Maestro never sees `concerto-mcp`; workarea sessions never see `concerto-maestro-mcp`. Two distinct tool surfaces, two distinct roles — but the **same net-new CLI-dialed `--mcp-config` + `--strict-mcp-config` stdio transport** mechanism (neither server exists on `main` today; `concerto-maestro-mcp` is the **first** instance, FROZEN by Task 401).

### 5.2 Public Rust API (Core-side)

```rust
pub struct MaestroHandle { /* opaque */ }

impl MaestroHandle {
    pub async fn send_to_maestro(&self, text: String, attachments: Vec<Attachment>) -> Result<()>;
    pub async fn get_digest(&self) -> Result<Digest>;
    pub async fn set_workarea_visibility(&self, wa: WorkareaId, vis: MaestroVisibility) -> Result<()>;
    pub async fn set_enabled(&self, on: bool) -> Result<()>;
    pub async fn get_state(&self) -> Result<MaestroState>;
}
```

### 5.3 gRPC surface

```proto
service Maestro {
  rpc SendToMaestro(MaestroMessageRequest) returns (google.protobuf.Empty);
  rpc GetDigest(GetDigestRequest) returns (Digest);
  rpc SetWorkareaVisibility(VisibilityRequest) returns (google.protobuf.Empty);
}
```

### 5.4 Emitted events

| Event | Stream | When |
|---|---|---|
| `maestro.message` | `maestro.events` | Maestro emitted a chat message |
| `maestro.routing_executed` | `maestro.events` | A `@workspace` routing fired |
| `maestro.digest_generated` | `maestro.events` | Digest produced |
| `maestro.budget_exhausted` | `maestro.events` | Daily budget tripped |
| `maestro.disabled_by_policy` | `maestro.events` | enterpriseDataPrivacy turned it off |

---

## 6. Internal architecture

```mermaid
flowchart TB
    subgraph Coord["MaestroAgentActor"]
        Lifecycle["Agent lifecycle<br/>(supervises via 04)"]
        PreParse["@workarea / @workarea/session<br/>pre-parser"]
        Summaries["WorkareaSummary cache<br/>+ refresher"]
        DigestGen["Digest generator"]
        Tools["concerto-maestro-mcp<br/>(in-process)"]
        Budget["TokenBudget tracker"]
        History["History condenser<br/>(daily)"]
    end
    PreParse --> WSM["03 Workspace/Workarea/Session Mgr<br/>(via route_prompt_to_session)"]
    Tools --> WSM
    Tools --> Sched["05 Sched"]
    Tools --> VCS["13 VCS"]
    Tools --> Notif["14 Notif"]
    Summaries -.subscribes.- Sup["04 Agent.events"]
    Summaries -.subscribes.- WSEvents["03 workarea.events + session.events"]
    Lifecycle --> Sup
    Budget --> DB["09 Persist"]
    History --> DB
```

### 6.1 Maestro lifecycle

- On Core start, if `maestro_state.enabled = 1` and the model is permitted by managed settings: spawn the Maestro agent via 04 with the special toolset config and the daily-summary preamble.
- The agent host is just another `concerto-agent-host` instance — survives Core restart.
- On user message via `SendToMaestro`:
   - Pre-parse for `@workarea` / `@workarea/session` / `/cmd` directives.
   - If directive: handle deterministically, append synthesized assistant message.
   - Else: forward to the agent's stdin; stream response.
- On budget exhaustion: stop the agent (clean), surface inert state. Re-spawn at midnight reset.

### 6.2 Summary cache maintenance

The Maestro subscribes to `session.events.*` for every active session. On `TurnComplete` for a session:

1. Extract the end-of-turn summary (parser pack provides it).
2. Update the owning `WorkareaSummary.last_turn_summary` (and the per-session entry inside `sessions: Vec<SessionSummary>`).
3. Increment `generation`; emit a lightweight "summary updated" hint.

Hard facts (commits ahead, PR state, CI) are pulled from 03 + 13 caches on demand.

If `concerto_chat_full_chat_access = true` for a workspace: the summary cache also pulls last-3-turn raw text and stores it.

### 6.3 Routing pre-parse implementation

```rust
fn pre_parse(input: &str) -> ParseOutcome {
    let lower = input.to_ascii_lowercase();
    if let Some((directive, body)) = parse_slash(&input) { return Slash { directive, body }; }
    if let Some((targets, body)) = parse_at(&input) { return Routing { targets, body }; }
    Freeform(input.to_owned())
}
```

Targets like `@all` / `@idle` / `@blocked` resolve to dynamic workarea (or session) sets at routing time, scoped to the currently-active workspace.

### 6.4 Digest flow

```mermaid
sequenceDiagram
    actor User
    participant DT as Desktop
    participant Coord as Maestro
    participant Summ as SummaryCache
    participant Sug as Suggestion Engine
    participant LLM as Coord agent (LLM)
    User->>DT: open app (78 min absence)
    DT->>Coord: GetDigest
    Coord->>Summ: list_active_summaries
    Summ-->>Coord: 6 summaries
    Coord->>LLM: digest prompt (with summaries)
    LLM-->>Coord: 4-sentence digest
    Coord->>Sug: next_step_chips
    Sug-->>Coord: top 4 chips
    Coord-->>DT: Digest { text, chips }
```

Target latency: < 5s p50.

---

## 7. Sequence diagrams — hot paths

### 7.1 `@bach run the e2e suite`

```mermaid
sequenceDiagram
    actor User
    participant DT as Desktop
    participant Coord as Maestro
    participant WSM as Workspace/Workarea/Session Mgr (03)
    participant Sup as Agent Sup (04)
    User->>DT: type "@bach run the e2e suite"
    DT->>Coord: SendToMaestro
    Coord->>Coord: pre-parse → Routing(workarea="bach", "run the e2e suite")
    Coord->>WSM: resolve bach within active workspace → session_id
    Coord->>Sup: send_input(session_id, ...) via WSM.route_prompt_to_session
    Sup-->>Coord: ok
    Coord->>Coord: synthesize assistant message<br/>"Routing to bach's Claude session..."
    Coord-->>DT: maestro.message
    Sup-->>Coord: session.events bach's session: "starting e2e suite..."
    Coord->>Coord: surface session's first response as quoted line
    Coord-->>DT: maestro.message updated
```

### 7.2 "What touched libs/auth today?"

```mermaid
sequenceDiagram
    actor User
    participant DT as Desktop
    participant Coord as Maestro
    participant LLM as Coord agent
    participant Tools as Coord MCP tools
    participant VCS as VCS
    participant WSM as Workspace Mgr
    User->>DT: type free-form q
    DT->>Coord: SendToMaestro
    Coord->>LLM: forward
    LLM->>Tools: cross_workarea_search("libs/auth", since=24h)
    Tools->>WSM: per-workarea commit search
    WSM-->>Tools: matches (2 workareas)
    Tools-->>LLM: results
    LLM-->>Coord: "Two workareas edited libs/auth in the last 24h: chopin (3 files, committed 1h ago), mozart (1 file, uncommitted). Overlap on TokenStore.ts."
    LLM->>Tools: propose_chip("Compare TokenStore.ts")
    Coord-->>DT: maestro.message + chip
```

### 7.3 New workspace + workarea from natural language

```mermaid
sequenceDiagram
    actor User
    participant DT as Desktop
    participant Coord
    participant Tools
    participant VCS
    participant Repo as Repo Mgr (02)
    User->>Coord: "open a workspace + workarea for ENG-4827"
    Coord->>LLM: ...
    LLM->>Tools: fetch_linear_issue("ENG-4827") (via 13)
    Tools->>VCS: fetch_linear_issue
    VCS-->>Tools: issue body
    LLM->>Tools: list cones for affected repos (api, android, ios) via 02
    Tools->>Repo: cone size probes
    Repo-->>Tools: stats
    LLM-->>Coord: proposal text + propose_chip(Create session) / propose_chip(Just create marketplace-api) / propose_chip(Edit cones)
    Coord-->>DT: render
    User->>DT: tap Create session
    DT->>WSM: create_workspace + create_workarea (normal flow)
```

---

## 8. Error handling & failure modes

| Failure | Detection | Response |
|---|---|---|
| LLM unreachable | API error | Routing + tools still work; LLM-required questions surface "model unreachable" |
| Budget exhausted | counter | Maestro goes inert (LLM-side); routing/tools work; UI banner |
| Workarea mid-restart while Maestro queries | summary cache miss | Maestro says "<workarea> is currently restarting; one moment" |
| User asks for content from `exclude_from_maestro` workarea | privacy check | Decline politely; surface that the workarea is privacy-restricted |
| Bad routing target (`@nonexistent`) | pre-parser | Synthesize assistant message: "I don't see a workarea named X in the active workspace — did you mean bach / mozart?" |
| Ambiguous `@composer` (multiple workspaces have it) | resolver | Ask user to disambiguate; offer matching workareas as chips |
| Routing target with no active agent | check | Synthesize: "<target> has no active agent. Start one?" with a chip |
| Maestro agent host crashes | 04 cold-resume flow | Auto-restart since maestro history persists |
| `enterpriseDataPrivacy = true` + external LLM | managed check at start | Maestro disabled with a clear banner; routing still works |
| Maestro's daily summary task fails | log + retry | Skip a day if necessary; user not blocked |
| Tools call returns excessive data (e.g., 10MB cross_workarea_search result) | tool guardrail | Truncate + summarize before LLM sees it; warn in tool result |

---

## 9. Dependencies on other sub-systems

| Sub-system | How |
|---|---|
| **04 Agent Supervisor** | Hosts the Maestro agent process; same host machinery as workarea sessions |
| **03 Workspace/Workarea/Session Mgr** | Per-workarea state, routing target, summary source |
| **05 Scheduler** | Listing schedules |
| **13 VCS Provider** | Issue fetches, PR / check status for digest |
| **07 Suggestion Engine** | Next-step chips for digest |
| **09 Persistence** | Maestro chat history, budget, daily summaries |
| **14 Notifications** | `notify_user` tool flows here |
| **12 Security** | enterpriseDataPrivacy + managed.maestro settings |

The Maestro depends on a lot — by design. It's the "outer agent" that needs visibility into everything.

---

## 10. Testing strategy

| Layer | What | How |
|---|---|---|
| Unit | Pre-parser routing — every directive shape | Table-driven |
| Unit | Budget tracking | Synthetic clock |
| Unit | Summary cache update on TurnComplete | Synthetic event |
| Integration | Real Maestro agent + stubbed workarea sessions → routing fires | E2E |
| Integration | Digest generation against 6-workarea fixture | E2E |
| Privacy | `exclude_from_maestro` excludes from summaries | Behavioral |
| Privacy | `enterpriseDataPrivacy` disables LLM but not routing | Behavioral |
| Cost | Daily condensation keeps token cost roughly flat over 30 days | Bench |
| Resilience | Maestro host crash mid-routing | Inject SIGKILL; assert recovery |
| Latency | Digest < 5s on a 6-workarea state | Bench |

---

## 11. Open questions / deferred

*All items resolved. See **§12 Resolved decisions log** below.*

## 12. Resolved decisions log

| # | Question | Decision | Where in doc |
|---|---|---|---|
| R-1 | Default model: Sonnet vs Haiku | **Sonnet** — better orchestration. User downgrades to Haiku to save tokens. | §3.9 |
| R-2 | Write tools that bypass user confirmation | **No bypass.** Every user-visible side effect surfaces as a chip. Maintains the "every action is user-confirmed" invariant. | §3.1 |
| R-3 | Maestro + external MCP (Linear, Slack) | **V2.0.** External context expands attack surface; defer. | (V2.0) |
| R-4 | Multiple Maestros per user | **No — singleton.** One chat at the top of the app. | §3.1 |
| R-5 | Maestro follows PRs across machines | **V2.0.** Cross-Core federation is a bigger design problem. | (V2.0) |
| R-6 | Cross-workarea search indexing | **V1.0 live grep; V2.0 Tantivy** if performance demands. | §3.7 |
| R-7 | Maestro inert — show last digest? | **Yes — show last good digest with a "stale" badge.** Continuity matters. | §3.9 |
| R-8 | Voice on Apple Watch | **V2.0 via iPhone bridge.** Tools produce voice-rendered choices. | (V2.0) |
| R-9 | Chat attachments (file uploads) | **V1.0 text-only; V1.5 drag-drop of a diff or commit URL.** | (phased) |
| R-10 | Token-budget alert thresholds | **80% amber, 100% red; user-configurable.** | §3.9 |
| R-11 | LLM provider for Maestro | **Pluggable** — Claude CLI / Codex CLI / Gemini CLI / Direct API (Anthropic, OpenAI, Bedrock, Vertex, OpenRouter, etc.). User picks in Settings → Concerto Chat → Backend. Auto-picks first available on first run. Daily budget cumulative across backends. (Parallel to `07 §3.11`.) | §3.9 (revised) |

---

*End of `08_Maestro_Agent.md`. The Maestro agent is hosted by `04_Agent_Supervisor.md` like any other agent, but with a restricted toolset implemented as an in-process MCP server. Digest chips come from `07_Suggestion_Engine.md`.*
