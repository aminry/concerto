# Phase 4 (Maestro Agent) — Planning Addendum

*Read this AFTER `README.md` §4–§6 and BEFORE any Phase-4 task file. It records the
decisions the Phase-4 planning conversation (2026-06-09) locked on top of the README
inventory, the cross-task **frozen contracts** the 17 task files must agree on, the
**migration-number reservation**, and the **machine-consumable task graph** (§8) the
auto-execute loop reads to run independent tasks in parallel.*

| Field | Value |
|---|---|
| Status | Approved (2026-06-09) |
| Scope | Phase 4 only (tasks 401–415 + inserts 400, 401.5) |
| Supersedes | Nothing. Amends `README.md §6` Phase-4 inventory (the 2 insert rows + refined deps). |
| Authority | These decisions are FIXED for the Phase-4 task files exactly as `README.md §4` decisions are fixed; revising one is a new planning conversation. |

The single most load-bearing rule: **every interface in §4 below is FROZEN by the task named
as its owner; later tasks CONSUME it, never re-lock it.** If a task author finds the design
contradicts a §4 contract, that's a Stop-and-ask, not a silent re-lock.

**Phase 4 is greenfield over clean seams.** There is no Maestro code today — no
`crates/core/src/maestro/`, no `maestro.proto`, no in-process MCP server, no summary cache, no
`@workarea` router, no LLM-provider abstraction, no token accounting. The schema was
pre-provisioned (`chats.kind='maestro'` singleton; `sessions.agent_kind`/`schedules` already
allow `'maestro'`). The biggest divergences from `design/08` are reconciled by **insert 400**
(read it first). The canonical product spec is `design/08_Maestro_Agent.md`; where the built
code diverges, 400's amendment + this addendum govern, and the task author transcribes the
**built** signatures (not the design doc's idealized ones).

---

## 1. The twelve locked decisions

| # | Decision | Choice (locked) | Consumed by |
|---|---|---|---|
| **D1** | Maestro LLM-backend scope for V1.0 | **CLI-first.** The three **CLI backends (Claude / Codex / Gemini)** ship LIVE in V1.0, reusing the existing `concerto-agent-host` PTY machinery. The **Direct-API backend** (Anthropic/OpenAI/Bedrock/Vertex/Azure-Foundry/OpenRouter — a native function-call loop with **no precedent** in the codebase) ships as a **FROZEN, unwired Tier-1 seam** wired in a fast-follow. Consequence: `enterpriseDataPrivacy=true` + an external model ⇒ **Maestro disabled** (`design/08 §3.10`'s defined behavior); on-prem Direct-API is a Tier-3 gate item + follow-on. Mirrors the README's deterministic-live / real-LLM-seam Phase-3 precedent. | 402, 412, 413 |
| **D2** | The Maestro agent **is a PTY-CLI session** | The Maestro runs as a long-lived agent **session** under the Agent Supervisor (`AgentKind::Maestro`), reusing `start_session`/host-survival/cold-resume verbatim — **not** a bespoke orchestrator and **not** the Direct-API loop in V1.0. Its tools are served by the in-process `concerto-maestro-mcp` server, dialed by the CLI via the CLI's own `--mcp-config` + `--strict-mcp-config` (so ONLY Maestro tools are visible). | 401, 402 |
| **D3** | MCP transport (Core ↔ agent CLI) | **Net-new, no precedent.** The Core hosts an **in-process `rmcp` stdio MCP server** (`concerto-maestro-mcp`); the spawned CLI connects to it via the CLI's `--mcp-config` pointing at that stdio endpoint. The existing agent-host is PTY + CBOR-over-UDS *terminal* multiplexing — it is **not** an MCP transport; do not conflate it. `crates/core/src/agent_supervisor/mcp.rs` is read-only config **discovery**, NOT a server. **400** pins the framing; **401** implements it; adding `rmcp` is an operator cargo-deny decision (Stop-and-ask on any advisory-ignore, mirroring 313's octocrab vetting). | 401, 402, 405–407 |
| **D4** | Strict-mode **ReadOnly auto-approve** | The built `PermissionResolver` matrix maps `(Strict, _) ⇒ MustAsk` for **every** tool (even `Safe` reads). `design/08 §3.2/§3.10` wants strict-but-reads-auto-approved. **Add a `ToolClass::ReadOnly`** bucket that auto-approves under strict; the **11 read tools** classify ReadOnly; the **5 write tools + `propose_chip`** classify so strict forces `MustAsk` ⇒ surfaced as the existing `AwaitingApproval`/`ResolveApproval` **confirmation chip**. **400** amends the design permission matrix; **402** implements `ToolClass::ReadOnly` + the strict-matrix arm. | 402, 405, 406, 407 |
| **D5** | Two distinct LLM seams — do not conflate | `OneShotLlm` (Task 312, FROZEN, `suggest(req)->String`) is **REUSED** for one-shot work only: the per-workarea **rolling summarizer** and the **digest** (`ActionKind::DigestSummary` is already reserved). Its live impl `DeterministicOneShot` is the LIVE P4 fallback path. The **interactive Maestro chat agent** uses a **SEPARATE provider-selection seam** frozen by **402** (which CLI + model + preamble + `--mcp-config` to launch); `OneShotLlm`'s String-only/no-stream/no-budget shape is the WRONG shape for the agent loop. Direct-API (D1) is a frozen-unwired arm of the 402 seam. | 402, 404, 409, 412 |
| **D6** | Token accounting is net-new | There is **zero** token accounting in the codebase; `AgentEvent::ContextUsage{pct}` is wired-but-never-emitted and is **NOT** the carrier. **403** freezes `maestro_state` (migration **0015**: `daily_in_today`/`daily_out_today`/`budget_resets_at`/`last_digest_at`/`enabled`, singleton `id=1`) + the budget accessor. **412** wires the counting (parsed from the CLI/Direct-API token usage), the inert-on-exhaust behavior, and the UTC-midnight/manual reset. Budget is **cumulative across backends** (`design/08 §3.9`). | 403, 412 |
| **D7** | `maestro.events` wire shape | Maestro events ride a **NEW subject `maestro.events`** carrying an **opaque payload on the existing non-oneof `Event.checks_opaque = 17` carrier** — **NOT** a new `Event.body` oneof arm (the oneof is FROZEN through field 16). **401.5** reserves `Subject::MaestroEvents` + the `parse_subject` branch + a `StreamsHandler::with_maestro_events(sender)` setter (mirroring `with_transport_events`/`with_vcs_events`); **414** publishes `maestro.message`/`routing_executed`/`digest_generated`/`budget_exhausted`/`disabled_by_policy`. | 401.5, 409, 414 |
| **D8** | Two-site service registration | A new gRPC service registers in **BOTH** `add_core_services` (`crates/core/src/api_server.rs` — serves UDS **and** Iroh via `CoreServiceSet`) **AND** `connect_bridge.rs` `serve` (the Connect-Web front door). **401.5** adds an **initially-unimplemented `MaestroServer`** at both sites (handler returns `Status::unimplemented`, surface FROZEN — the `UpsertProjectMcp` precedent); **414** fills the impl. Missing the second site is the single easiest Phase-4 bug. `#[cfg(unix)]`-gate the handler (it depends on the agent supervisor). | 401.5, 414 |
| **D9** | Summary cache is **agent-independent** | **404** builds the `WorkareaSummary` cache from EXISTING `session.events.<sid>` (`TurnComplete`) + `workarea.events` (`status:<to>`) + `checks.<wa>.<repo>` + `pull_requests.state` — it does **not** require 402's live Maestro agent, so it parallelizes the agent spine. **Hard facts are not precomputed:** `commits_ahead` has NO implementation (404 adds a `gix-wrap` ahead-count helper); `files_changed`/`lines_*` are counted from `diff_to_main`/`diff_head` output; `ci_state` is parsed from the opaque `checks.<wa>.<repo>` frames; `pr_state` is `pull_requests.state`. | 404, 405, 409, 413, 415 |
| **D10** | Privacy enforcement + the `enterprise_data_privacy=false` debt | The LIVE `handlers/vcs.rs::FetchIssueByUrl` **hardcodes `enterprise_data_privacy=false`** (a Phase-3 deliberate-debt; the resolver now exists). **411** (the consumer of `fetch_issue_url`) replaces it with the resolved `WorkspaceSettingsResolver::enterprise_data_privacy()` value; **413** enforces the resolver before ANY external summary/digest and skips `exclude_from_maestro` workareas (blank to name-only). `concerto_chat_full_chat_access` is a **net-new `workspaces.settings_json` key** (no proto/migration) added by **413** using the `exclude_from_maestro` RMW-key precedent. | 411, 413 |
| **D11** | Digest chip persistence | The V0.1 Suggestion Engine has **no `ChipRanker`/`propose_chip`/`next_step_chips`** and its chips **evaporate after ~60s** (`DEDUP_TTL`). So **407**'s `propose_chip` adds to a **Maestro-owned current slate** (not the volatile suggestion buffer), and **409**'s digest chips are **persisted by the Maestro** (attached to the digest's `chat_messages` row), not left in the suggestion engine's buffer. `propose_chip` mirrors the `Chip` shape; it does not extend the suggestion engine. | 407, 409 |
| **D12** | `chat_messages` daily-summary tagging | `chat_messages` has **no `metadata` column** today. **410** adds `metadata TEXT` via migration **0016** (additive `ALTER TABLE ADD COLUMN` — **no** CHECK-widen) and tags daily summaries `metadata.role_extra='daily_summary'` (`design/08 §3.7/§4.1`); it is **not** folded into `content_json`. | 410 |

---

## 2. Resolved sub-decisions (smaller forks — locked so the 17 authors stay consistent)

| Area | Question | Locked answer |
|---|---|---|
| 401 | crate vs module placement | A **new core module `crates/core/src/maestro/`** (with `mcp.rs`, `summary.rs`, `routing.rs`, `tools/{read,write,side}.rs`, `provider.rs`), **not** a new workspace crate. The server must reach the 03/05/07/13/14 handles in-process; this matches every other subsystem (`agent_supervisor`, `suggestions`, …) and lets `CoreServiceSet` wire it. **401 freezes the module path + `mod.rs` skeleton.** |
| 401 | MCP SDK | **`rmcp`** (the de-facto Rust MCP SDK). Add to the workspace + `crates/core/Cargo.toml`; **vet with `cargo deny` first** — an advisory-ignore or a new disallowed SPDX is a **Stop-and-ask** (operator decision), mirroring 313's octocrab/RUSTSEC handling. |
| 401 | tool-schema return before impl | Every one of the 16 tools is **registered with its FROZEN input/output schema** in 401 but returns a typed `unimplemented` MCP error until its impl task (405/406/407) lands — **never** `todo!()`/`unimplemented!()` and **never** empty-success (the 305 seam discipline). |
| 402 | `AgentKind::Maestro` touch-sites | Adding the variant touches `as_db_kind` (`"maestro"`), `resolve_agent_bin` (the Maestro spawn arm), `from_db_kind` (cold-resume, actor.rs ~1248), the parser-pack selection match (actor.rs ~1357), and `parse_agent_kind` (handlers/sessions.rs ~405). The Maestro uses a **no-op/structured parser pack** (its tool calls ride MCP, not PTY-scrape) — 400 pins the parser story; do not reuse the fragile `ClaudeCodePack` regex scraper for tool calls. |
| 402 | scratch cwd | `~/concerto/maestro/` (a scratch dir created at spawn; **not** a worktree). No edit-mutex (Maestro has no file-edit tools). |
| 404 | `WorkareaSummary` field types | Follow `design/08 §3.3` shape but use **`i64` unix-ms** for `last_activity_at`/`generated_at` (persistence/wire-friendly), not `Instant`. `commits_ahead: u32` via a **new `gix-wrap` ahead-count helper** (rev-list `branch..base` or symmetric). The cache is in-memory (`HashMap<WorkareaId, WorkareaSummary>`); no migration. |
| 404 | summarizer model | Reuse `OneShotLlm` + `ActionKind::DigestSummary`; the LIVE path is `DeterministicOneShot` (truncate/collapse). The real Haiku/Sonnet call is 412's provider, **judged at the phase gate** (Tier-2: "the double does NOT cover real-LLM summary quality"). |
| 405/406/407 | tool-file split | Read tools → `maestro/tools/read.rs`; write tools → `maestro/tools/write.rs`; side-channels → `maestro/tools/side.rs`. **Disjoint files** → parallel-safe consumers of 401's frozen schema registry. The lead of each owns only its file + a one-line registration in `maestro/tools/mod.rs` (lead-owned seam). |
| 406 | write-tool confirmation | The 5 write tools (`route_prompt_to_session`, `fanout_to_sessions`, `create_workspace`, `create_workarea`, `set_workarea_paused`) classify so strict ⇒ `MustAsk` ⇒ the existing `AwaitingApproval`/`ResolveApproval` chip flow (carries `urgent`/`destructive_label`) confirms before execution. **No bypass** (`design/08 R-2`). |
| 408 | routing home + grammar | `maestro/routing.rs`: a **pure deterministic pre-parser** (`pre_parse(&str) -> ParseOutcome`) + a composer→workarea→session resolver over `workareas::list_by_workspace` (composer-sorted) + `sessions::list_by_workarea`. **No server-side active-workspace exists** — the Maestro takes an explicit `workspace_id`; cross-workspace `@composer` disambiguation is the Maestro's job (ask-with-chips). Routing spends **zero** LLM tokens. |
| 409 | digest latency proof | `<5 s p50` measured (Criterion or a timed test) against the **deterministic** summarizer + a 6-workarea fixture (`design/08 §10`). The real-LLM latency is a Tier-3 gate line. |
| 411 | `SuggestCones` RPC + create flow | Add **`Repositories.SuggestCones`** RPC (reuse the pre-written `cone_suggest_error_to_status` mapping) + inject a **Maestro-backed `ConeSuggester`** via `RepoManager::with_cone_suggester` at boot. `create_workspace`/`create_workarea` tools wrap `WorkspaceManager::create_workspace` + `WorkareaManager::create_workarea` (or their gRPC handlers). 411 also **fixes D10's `enterprise_data_privacy=false` debt** in `handlers/vcs.rs`. |
| 412 | provider trait shape | A `MaestroProvider` seam: `CliProvider` (Claude/Codex/Gemini — launch the chosen CLI through 402's spawn) LIVE + `DirectApiProvider` returning a typed `unimplemented` (FROZEN seam, D1). Reads `ManagedPolicy::default_model()` + the `{claude,codex,gemini}_executable_path()` getters + `SecretKind::ProviderToken(..)` (for the future Direct-API key). Inert-on-exhaust shows the **last good digest with a stale badge** (`design/08 R-7`). |
| 413 | privacy surfaces | (a) `WorkspaceSettingsResolver::enterprise_data_privacy()` gate before any external summary/digest; (b) per-workarea `exclude_from_maestro` skip (blank to name-only); (c) **`concerto_chat_full_chat_access`** = new `workspaces.settings_json` bool (default false), lifts summary-only → last-3-turns raw. No migration; no new proto field (read via the settings resolver). |
| 415 | verification dir | **`apps/desktop`** (NOT `apps/web` — it doesn't exist until P5/519). 415's `Verification` **overrides** the orchestrator's `web-ts` default to `pnpm -C apps/desktop typecheck|lint|test|build` (322–324 precedent; Task 218 added the scripts + `vitest`). The Tier-2 double is mocked `@tauri-apps/api` invoke + React-Query/Zustand component tests; real live-data rendering is the Phase-4 Tier-3 checklist's job. |

---

## 3. Migration-number reservation

Current last shipped migration is **`0014_pull_requests_merge_order.sql`**. Phase-4 migrations are
reserved **in task order** below. A task with NO row here adds **no** migration (it uses an
existing column, a `settings_json` JSON key, an in-memory cache, or the keychain).

> **Author check (do this first):** confirm the actual highest `crates/persist/migrations/NNNN_*.sql`
> on `main` before writing. If a task landed a migration above 0014, **shift this whole block up by
> the same offset, preserving order** — and note it in your Handoff.

| Migration | Owner task | Adds |
|---|---|---|
| `0015` | **403** | `maestro_state` table — singleton (`id INTEGER PRIMARY KEY CHECK (id = 1)`), `daily_in_today`/`daily_out_today`/`budget_resets_at`/`last_digest_at`/`enabled` (`design/08 §4.1`). The first daily-counter/budget table (schedules deliberately deferred its budget columns). |
| `0016` | **410** | `chat_messages` += `metadata TEXT` — **additive `ALTER TABLE ADD COLUMN`** (no CHECK-widen). Carries `role_extra='daily_summary'` for the condensation pass. |

- 411 (`SuggestCones`) = new RPC, **no migration**.
- 413 (`concerto_chat_full_chat_access`) = `workspaces.settings_json` JSON key, **no migration**.
- 404 (summary cache) = in-memory `HashMap`, **no migration**.
- The `chats(kind='maestro')` singleton (403 bootstrap) needs **no schema change** — the row already validates against the `0001` CHECK; 403 only inserts it if absent.

**CHECK-widening is BANNED** in this codebase (`foreign_keys=ON` + per-migration transactions ⇒ `DROP` cascade-deletes children). If any task must widen a CHECK, copy migration **0010**'s in-place `PRAGMA writable_schema = ON; UPDATE sqlite_master SET sql=...; PRAGMA writable_schema = RESET;` rewrite. Neither 0015 nor 0016 needs it (one new table; one additive column).

---

## 4. Cross-cutting FROZEN contracts (owner → consumers)

**4.1 The 16 Maestro MCP tool schemas + the MCP transport — FROZEN by 401 (D2/D3).** The
`concerto-maestro-mcp` in-process `rmcp` server, the module path `crates/core/src/maestro/mcp.rs`,
and the **input/output JSON schema of all 16 tools** (`design/08 §5.1`): 11 read
(`list_workspaces`, `list_workareas`, `list_sessions`, `get_workspace_summary`,
`get_workarea_summary`, `list_recent_activity`, `list_active_schedules`, `read_inbox_summary`,
`read_pr_set_for_workarea`, `get_workarea_recent_commits`, `cross_workarea_search`), 5 write
(`route_prompt_to_session`, `fanout_to_sessions`, `create_workspace`, `create_workarea`,
`set_workarea_paused`), 2 side-channel (`notify_user`, `propose_chip`). The CLI dial mechanism
(`--mcp-config` + `--strict-mcp-config`) is FROZEN by 400/401. Tool args/names are transcribed
from `design/08 §5.1`. 405/406/407 fill impls behind these frozen schemas; **never** re-shape a
tool's schema.

**4.2 `maestro.proto` + `MaestroHandle` + `maestro.events` — FROZEN by 401.5 (D7/D8).**
`service Maestro { rpc SendToMaestro(MaestroMessageRequest) returns (Empty); rpc GetDigest(GetDigestRequest) returns (Digest); rpc SetWorkareaVisibility(VisibilityRequest) returns (Empty); }` + the `Digest`/`Chip`-bearing messages (`design/08 §5.3`), the Rust `MaestroHandle` API (`send_to_maestro`/`get_digest`/`set_workarea_visibility`/`set_enabled`/`get_state`, `design/08 §5.2`), the `Subject::MaestroEvents` arm + `parse_subject("maestro.events")` + `StreamsHandler::with_maestro_events`, and the **two-site `MaestroServer` registration** (initially `Status::unimplemented`). Payloads ride `Event.checks_opaque=17`; **no new `Event.body` oneof arm**. 414 fills the impl; 415 consumes the proto/TS surface; 409 publishes the digest event.

**4.3 The Maestro provider-selection seam — FROZEN by 402, extended by 412 (D1/D5).** The
interactive-agent backend seam (which CLI binary + model + Maestro preamble + `--mcp-config` +
`strict` mode + scratch cwd to launch). 402 freezes the trait with **Claude CLI LIVE**; 412 adds
**Codex + Gemini LIVE** + the daily budget + a **`DirectApiProvider` frozen-unwired arm** (typed
`unimplemented`, not the macro). Distinct from `OneShotLlm` (4.5).

**4.4 `WorkareaSummary`/`SessionSummary`/`RepoSummary` + the cache refresh contract — FROZEN by
404 (D9).** The `design/08 §3.3` structs (with `i64` ms timestamps), the refresh triggers
(`TurnComplete`, 10-min idle, force-on-`GetDigest`-if-stale-60s), the `commits_ahead` helper, and
the hard-fact derivation (diff counts / `pull_requests.state` / opaque `checks` frames). 405's
`get_workarea_summary`, 409's digest, 413's privacy-blanking, and 415's rendering all consume
these — never re-derive a different shape.

**4.5 `OneShotLlm` reuse — FROZEN by 312, reused (not modified) by 404/409 (D5).** The summarizer
+ digest route through `OneShotLlm::suggest` with `ActionKind::DigestSummary`; `DeterministicOneShot`
is the LIVE fallback. The trait is a V1.0 stability contract — Maestro **consumes** it; the real
provider is 412's separate seam (4.3).

**4.6 `maestro_state` + budget accessor + `chats(kind='maestro')` singleton — FROZEN by 403
(D6).** The 0015 schema, the typed accessors (`get`-singleton, bump-daily-counters, reset-budget,
set-last-digest, set-enabled), and the singleton-bootstrap. 412 consumes the budget; 410 consumes
the maestro chat id; 414 reads `enabled`/digest state.

**4.7 The routing grammar + `ParseOutcome` — FROZEN by 408 (D2).** `pre_parse(&str) -> ParseOutcome`
(`Freeform` | `Routing{targets, body}` | `Slash{directive, body}`) covering `@workarea`,
`@a,@b` fanout, `@all`/`@idle`/`@blocked`, `/digest`/`/pause`/`/new`, and the composer→session
resolver. 409 (`/digest`) and 414 (`SendToMaestro` pre-parse) consume it.

**4.8 `AgentKind::Maestro` + `ToolClass::ReadOnly` + scratch-cwd — FROZEN by 402 per 400 (D4).**
The new agent kind (+ its DB string `"maestro"`, parser-pack arm, cold-resume arm), the new
permission class that auto-approves the 11 read tools under strict, and the `~/concerto/maestro/`
scratch convention. Everything in cluster M consumes these.

---

## 5. The two inserts (amend `README.md §6` Phase-4 inventory)

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| **400** | **Maestro architecture reconciliation** — amend `design/08`/`04`: (a) the net-new Core↔CLI MCP-stdio transport (`--mcp-config`/`--strict-mcp-config`), (b) Maestro-as-PTY-CLI-session + `AgentKind::Maestro` (vs the design's vaguer "agent process"), (c) the strict-mode **ReadOnly-auto-approve** permission rule (`design/04 §3.2/§3.10`), (d) Direct-API-deferred-as-Tier-1-seam (D1) + the `enterpriseDataPrivacy`-disabled consequence. Runs **first** (doc, `design/` only — zero code collision), like Task 200 / 315.0. | — | 3 | doc |
| **401.5** | **Maestro wire-contract freeze** — `maestro.proto` + `MaestroHandle` Rust API + `maestro.events` subject (`Subject::MaestroEvents` + `parse_subject` + `with_maestro_events`) + an **unimplemented `MaestroServer` registered in BOTH sites** (`add_core_services` + `connect_bridge.rs`). Proto/wire only; handler returns `Status::unimplemented`. **Unblocks 415 (Desktop UI) to start against frozen types in parallel with the entire Rust spine.** | 400 | 1 | rust |

---

## 6. Refined dependencies (the task-graph edge-list)

These deps refine the README inventory rows; they (and the README rows) MUST appear in each task
file's `Depends on`. The machine-consumable form is `PHASE4_DAG.json` + §8.

| Task | Depends on | Why (beyond the README row) |
|---|---|---|
| 400 | — | doc root |
| 401 | 400 | implements 400's transport/permission amendment |
| 401.5 | 400 | proto/wire freeze; parallel to 401 (different files; soft `api_server.rs`/`boot.rs` seam) |
| 402 | 401 | spawns the Maestro using 401's MCP server config; adds `AgentKind::Maestro`+`ToolClass::ReadOnly` |
| 403 | — | independent persistence root (migration 0015) |
| 404 | 401 | lives in the `maestro` module 401 creates; cache is otherwise agent-independent (D9) |
| 405 | 401, 404 | read-tool impls behind 401's frozen schemas; `get_workarea_summary` returns 404's type |
| 406 | 401, 402 | write-tool impls; needs the strict/chip-gate (402) |
| 407 | 401 | `notify_user` stub (against 14) + `propose_chip`; Maestro-owned slate (D11) |
| 408 | 402 | pre-parses `SendToMaestro` input; resolver over existing list APIs |
| 409 | 404, 408 | digest over summaries (404) + `/digest` route (408); persists its chips (D11) |
| 410 | 403 | condensation over the maestro chat (403); adds `metadata` (0016) |
| 411 | 406, 403, 305, 313 | `create_workspace_from_description` write tool (406) + issue-fetch (313) + `ConeSuggester`/`SuggestCones` (305); fixes the privacy debt (D10) |
| 412 | 402, 403 | extends 402's provider seam (Codex/Gemini live + Direct-API seam) + 403's budget (D6) |
| 413 | 404 | privacy gate over the summary cache (404); `concerto_chat_full_chat_access` key |
| 414 | 401.5, 409 | fills 401.5's handler skeleton; publishes `maestro.events`; surfaces digests (409) |
| 415 | 401.5, 218 | Desktop UI against 401.5's frozen proto + 218's `CoreClient`; mocked invoke (NOT 414) |

> **The 415 unlock is the headline:** because 401.5 freezes the wire surface and 415 is a Tier-2
> mocked-invoke `web-ts` task, **415 does not wait for 414** — it builds against frozen types and
> overlaps the whole Rust spine. 414 then "lights up" live data with zero UI rework.

---

## 7. Verification note for the desktop task (415)

The orchestrator's `web-ts` command set (`README §5.3`) targets `apps/web`, which **does not exist
until Phase 5 (task 519)**. **415 MUST** put an explicit `Verification` override:
`pnpm -C apps/desktop typecheck && pnpm -C apps/desktop lint && pnpm -C apps/desktop test && pnpm -C apps/desktop build`
(Task 218 added those scripts + `vitest`; `lint` aliases `tsc --noEmit`; `test` is vitest+jsdom+
`@testing-library/react` mocking `@tauri-apps/api` invoke; there is **no** Playwright in
`apps/desktop`). The Tier-2 double is the mocked-`invoke` + component tests; real live-Maestro
rendering is the Phase-4 Tier-3 checklist's job, not 415's.

---

## 8. Concurrency / wave map (pipelined + bounded-parallel, K = 4)

The orchestrator runs Phase 4 **pipelined and up to K = 4 file-disjoint tasks in flight** per
`AUTO_EXECUTE_PROMPT.md` → *Concurrency model* (Phase 4 raises K from the default 3; see that
doc). **The merge invariant is unchanged: dependency-ordered, serialized merges; `main` always
green; in-flight branches rebase onto each new `main`; a substantive rebase conflict → re-dispatch
the later task fresh.** Eligibility = **dependency-ready (per §6 / `PHASE4_DAG.json`) AND
write-set-disjoint on a hard seam from every in-flight task.** `PHASE4_DAG.json` carries the same
data machine-readably — the orchestrator computes the eligible set from it each tick.

**Completion state (update as you go):** 400–415 + 401.5 all pending.

### 8.1 Per-task write-sets (the disjointness oracle)

A task is **file-disjoint** from another if their write-sets share no **hard-to-merge** path. Hard
seams: any `*.proto`, a shared `mod.rs`/`lib.rs`, `crates/core/src/boot.rs`, `api_server.rs`,
`connect_bridge.rs`, a migration, the same source module. Trivially-mergeable (never blocks):
`Cargo.lock`, `docs/interfaces/*`, `scripts/smoke.manifest`, distinct test files, distinct
`apps/*` vs `crates/*` trees.

| Task | Write-set (globs) | Hard seams shared with |
|---|---|---|
| 400 | `design/08_*.md`, `design/04_*.md` | — (doc only) |
| 401 | `crates/core/src/maestro/{mod,mcp}.rs`, `crates/core/src/maestro/tools/mod.rs`, `Cargo.toml`, `crates/core/Cargo.toml`, `deny.toml` | 402/404/405/406/407 (maestro `mod.rs`); soft: `boot.rs` |
| 401.5 | `crates/proto/proto/concerto/v1/maestro.proto`, `crates/core/src/handlers/{mod,maestro}.rs`, `api_server.rs`, `connect_bridge.rs`, `crates/core/src/handlers/streams.rs` | 414 (`handlers/maestro.rs`, `streams.rs`); soft: `api_server.rs`/`boot.rs` |
| 402 | `crates/core/src/agent_supervisor/actor.rs`, `crates/core/src/security/{permission,tool_classes}.rs`, `crates/core/src/maestro/{mod,provider}.rs` | 401/404 (maestro `mod.rs`); 412 (`provider.rs`); 406 (tool_classes) |
| 403 | `crates/persist/migrations/0015_*.sql`, `crates/persist/src/{maestro_state,api,lib}.rs`, `crates/persist/tests/initial_schema.rs` | 410 (migrations dir, `api.rs`/`lib.rs`) |
| 404 | `crates/core/src/maestro/summary.rs`, `crates/gix-wrap/src/{diff,ahead}.rs`, `crates/core/src/maestro/mod.rs` | 401/402 (maestro `mod.rs`) |
| 405 | `crates/core/src/maestro/tools/read.rs`, `tools/mod.rs` | 406/407 (`tools/mod.rs` registration line — lead-owned) |
| 406 | `crates/core/src/maestro/tools/write.rs`, `tools/mod.rs` | 405/407 (`tools/mod.rs`) |
| 407 | `crates/core/src/maestro/tools/side.rs`, `tools/mod.rs` | 405/406 (`tools/mod.rs`) |
| 408 | `crates/core/src/maestro/routing.rs`, `crates/core/src/maestro/mod.rs` | 401/402/404 (maestro `mod.rs`) |
| 409 | `crates/core/src/maestro/digest.rs`, `crates/core/src/maestro/mod.rs` | 401/… (maestro `mod.rs`) |
| 410 | `crates/persist/migrations/0016_*.sql`, `crates/persist/src/chat_messages.rs`, `crates/core/src/maestro/condense.rs` | 403 (migrations dir) |
| 411 | `crates/core/src/maestro/tools/write.rs` (create flow), `crates/proto/proto/concerto/v1/repositories.proto`, `crates/proto/proto/concerto/v1/vcs.proto` (adds `FetchIssueByUrlRequest.workspace_id`, D10 fix), `crates/core/src/handlers/{repositories,vcs}.rs`, `crates/core/src/repo_manager/actor.rs` | 406 (`tools/write.rs`); `repositories.proto`, `vcs.proto` (no other P4 task writes either) |
| 412 | `crates/core/src/maestro/provider.rs`, `crates/core/src/llm/*`, `crates/core/src/security/managed.rs` (read) | 402 (`provider.rs`) |
| 413 | `crates/core/src/maestro/{privacy,summary}.rs`, `crates/core/src/settings/resolver.rs` (read), `crates/persist/src/workspaces.rs` (settings key) | 404 (`summary.rs`) |
| 414 | `crates/core/src/handlers/{maestro,streams}.rs`, `crates/core/src/maestro/events.rs`, `boot.rs`, `api_server.rs`, `connect_bridge.rs` (the D8 second registration site) | 401.5 (`handlers/maestro.rs`, `streams.rs`, `connect_bridge.rs`) |
| 415 | `apps/desktop/src/**` | — (disjoint from ALL crates) |

> **`crates/core/src/maestro/mod.rs` is the soft seam of Phase 4** (most maestro tasks add a
> `pub mod X;` line + a field). **401 owns the initial `mod.rs`**; later tasks add their module
> line in a distinct region → additive, auto-merges on rebase. Treat it as watch-on-rebase, not a
> hard block. If two in-flight tasks edit the **same** `mod.rs` region → fallback to serialize.

### 8.2 Suggested waves (illustrative — recompute eligibility each tick from `PHASE4_DAG.json`)

- **Wave 1 (ready now, disjoint):** `400` (doc, `design/` only) ∥ `403` (persist, migration 0015 — fully independent root).
- **Wave 2 (400 merged):** `401` (mcp+transport) ∥ `401.5` (proto freeze — different files; soft `api_server.rs` seam) ∥ `403` still in flight ⇒ up to 4 in flight.
- **Wave 3 (401 merged → maestro module exists; 401.5 merged → wire frozen):** `402` (agent spine) ∥ `404` (summary cache) ∥ `415` (**Desktop UI starts here**, mocked invoke, parallel to ALL Rust) ∥ `410` (condensation, after 403).
- **Wave 4:** `405` (read tools, ←404) ∥ `408` (routing, ←402) ∥ `412` (provider, ←402/403) ∥ `413` (privacy, ←404). Then `406`/`407` (←405/402) as the `tools/*` files free up.
- **Wave 5:** `409` (digest, ←404/408) ∥ `411` (create-from-description, ←406/305/313).
- **Wave 6:** `414` (gRPC impl + events, ←401.5/409) — `415` is already done; 414 lights up live data.

**Cluster summary (parallelize across, serialize within):**

| Cluster | Tasks | Shared hot files |
|---|---|---|
| **M — agent spine** | 400→401→402→412 | maestro `mod.rs`/`provider.rs`, agent_supervisor `actor.rs`, spawn path |
| **W — wire/surface** | 401.5→414, 415 | `maestro.proto`, `handlers/maestro.rs`, `streams.rs`, `api_server.rs` (415 = `apps/desktop` only, fully disjoint) |
| **S — summary/read/privacy** | 404→405, 413 | maestro `summary.rs`, `tools/read.rs` |
| **WT — write/side/create** | 406, 407, 411 | maestro `tools/{write,side}.rs` |
| **D — digest/routing/persist** | 408, 409, 403, 410 | maestro `routing.rs`/`digest.rs`, persist migrations |

**If unsure whether two tasks are disjoint → serialize them.** A green `main` and correct
interfaces outrank the speedup.

*End of Phase-4 planning addendum. The 15 inventory task files (401–415) + the 2 inserts
(400, 401.5) are written against this document, `README.md`, the `PHASE4_DAG.json` graph, and the
`design/08`/`04` sections each cites.*
