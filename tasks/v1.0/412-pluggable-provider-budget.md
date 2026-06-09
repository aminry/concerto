# Task 412 — Pluggable LLM provider: `CodexCliProvider`/`GeminiCliProvider` live + daily `TokenBudget` (200K in / 50K out, inert-on-exhaust) + `DirectApiProvider` frozen seam (extends 402's `MaestroProvider`, consumes 403's budget accessor)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 402, 403 |
| Touches subsystem(s) | 08 (Maestro), 04 (Agent Sup), 12 (Security), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Make the Maestro's LLM backend a real, pluggable, budget-bounded thing: ship the **three CLI backends LIVE** (Claude from 402, plus **Codex + Gemini** here), freeze the **Direct-API arm as an unwired Tier-1 seam**, and wire the **net-new daily token budget** that goes inert-on-exhaust. Today there is **zero** token accounting in the codebase — `AgentEvent::ContextUsage{pct}` is wired-but-never-emitted and is explicitly **NOT** the carrier (PHASE4_PLANNING D6); the only live LLM-backend selection is 402's `MaestroProvider` trait + `ClaudeCliProvider` (Claude CLI launched through `AgentSupervisorHandle::start_session`), and `crates/core/src/agent_supervisor/actor.rs:1841` still has `AgentKind::Codex | AgentKind::Gemini => Err(Error::Validation("agent.not_implemented: codex/gemini deferred to Phase 3"))` — Codex/Gemini have never been spawnable; `ManagedPolicy::{codex_executable_path,gemini_executable_path,default_model}` (`security/managed.rs:343–363`) are parsed-but-unread; `SecretKind::ProviderToken(Provider{Anthropic,OpenAI,Gemini,Bedrock,Vertex})` (`crates/keychain/src/api.rs:22/41`) is defined+tested but unread. This task ADDS, behind 402's **FROZEN `MaestroProvider` seam (consumed as frozen, PHASE4_PLANNING §4.3 — never re-locked here)**: `CodexCliProvider`/`GeminiCliProvider` (same `start_session` spawn shape as Claude, different binary + flags, resolved from `ManagedPolicy::{codex,gemini}_executable_path()`); a **FROZEN `DirectApiProvider` (PHASE4_PLANNING D1, `design/08 §3.9`)** that returns a **typed `unimplemented`** (NEVER the `unimplemented!()` macro, NEVER empty-success) but already reads `SecretKind::ProviderToken(..)` + `ManagedPolicy::default_model()` so the fast-follow only fills bodies; a net-new **`TokenBudget`** (`daily_in_today`/`daily_out_today` bumped via 403's accessor **consumed as frozen, PHASE4_PLANNING §4.6**, from the provider's parsed token usage, **cumulative across backends**, `design/08 §3.9`); an **inert-on-exhaust** mode (LLM calls stop; routing + deterministic tools keep working; UI banner; **last-good digest with a stale badge**, `design/08 R-7`); a **UTC-midnight + manual reset**; and an **auto-pick first-available** selector (`Claude → Codex → Gemini → Direct`, `design/08 §3.9` defaults). After this task, the Maestro can run on any of the three installed CLIs under a hard daily cap, 414's `maestro.budget_exhausted` event has a real trip-point to publish, and 415's banner has real state to render; the real on-prem Direct-API loop and real CLI/API token-accuracy stay Tier-3 (the phase-gate "confirm budget-exhaust goes inert while routing works" line) and the Direct-API body is the documented fast-follow.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §1 D1/D5/D6 + §2 row "412 provider trait shape" + §4.3/§4.6 — **AUTHORITATIVE** decisions this obeys: D1 (3 CLI backends LIVE, Direct-API a FROZEN unwired Tier-1 seam; `enterpriseDataPrivacy=true`+external ⇒ Maestro disabled), D5 (this is the *interactive-agent* seam, distinct from `OneShotLlm`), D6 (token accounting is net-new; `ContextUsage{pct}` is NOT the carrier; budget cumulative across backends); §4.3 names 402 the owner of `MaestroProvider` (this task **extends, never re-locks** it); §4.6 names 403 the owner of the `maestro_state` budget accessor (consumed as frozen).
- `tasks/v1.0/402-maestro-agent-spine.md` → "Public interface this task locks" + "Handoff Notes" — the **FROZEN `MaestroProvider` trait** + `ClaudeCliProvider` + the spawn-launch shape (`AgentKind::Maestro`, scratch cwd `~/concerto/maestro/`, `--mcp-config`/`--strict-mcp-config`, `strict` permission mode) this task mirrors for Codex/Gemini; the parser-pack story for the Maestro session; any drift 402 recorded on `resolve_agent_bin`'s Maestro arm.
- `tasks/v1.0/403-maestro-state-budget.md` → "Public interface this task locks" + "Handoff Notes" — the **FROZEN `maestro_state` accessors** (singleton get, bump-daily-counters, reset-budget, set-last-digest, set-enabled) + `budget_resets_at` semantics this task calls; confirm the bump-counter accessor signature + units (tokens) before wiring.
- `design/08_Maestro_Agent.md` §3.9 (provider table: Claude/Codex/Gemini CLI + Direct API rows; defaults — auto-pick `Claude→Codex→Gemini→Direct(Anthropic)`, model `claude-4.6-sonnet`/"Sonnet-class", **daily input 200K / output 50K**; inert-on-exhaust behavior + UTC-midnight/manual reset; budget per-backend-cumulative), §3.10 (`enterpriseDataPrivacy` + external ⇒ disabled; routing always works), §8 (failure table: LLM-unreachable + budget-exhausted rows — routing/tools survive), R-7 (show last good digest with a stale badge), R-1/R-10 (Sonnet default; 80% amber / 100% red thresholds, user-configurable).
- `design/04_Agent_Supervisor.md` §3 (the agent-host spawn machinery + permission interception the CLI providers reuse verbatim) — for the Codex/Gemini binary-resolution + flag shapes and the host-survival/cold-resume guarantees the providers inherit unchanged.
- `crates/core/src/agent_supervisor/actor.rs:368` (`AgentSupervisorHandle::start_session(StartSessionRequest{ workarea_id, agent_kind, echo_text, cwd, permission_mode, resume_session_id }) -> SessionId`), `:1818` (`resolve_agent_bin(&req) -> (String, Vec<String>)`; the `AgentKind::Codex|AgentKind::Gemini => Err("deferred to Phase 3")` arm at `:1841` this task makes live for the Maestro path), `:66` (`AgentKind = {Echo, Claude, Codex, Gemini}` — NO Maestro variant; 402 adds it) — the spawn surface; `send_input(&SessionId, Vec<u8>)` is the only send-prompt path; **no model/provider arg is threaded today**.
- `crates/core/src/security/managed.rs:343–363` — `ManagedPolicy::{default_model() -> Option<&str>, claude_executable_path()/codex_executable_path()/gemini_executable_path() -> Option<&Path>}` (parsed-but-unread getters this task **reads** for binary resolution + model selection; do NOT add fields).
- `crates/keychain/src/api.rs:22/41` + `crates/keychain/src/lib.rs:39` — `SecretKind::ProviderToken(Provider{Anthropic,OpenAI,Gemini,Bedrock,Vertex})`, account slug `provider_token.<slug>`. **`SecretKind` is a CLOSED, `Copy`, frozen enum — read existing variants via the parameterized accessor; NEVER add a variant** (313's discipline). The Direct-API seam reads these for the future wiring; it does not mint new slots.
- `crates/core/src/llm/oneshot.rs` — the `OneShotLlm`/`DeterministicOneShot`/`ActionKind::DigestSummary` seam (FROZEN by 312, PHASE4_PLANNING §4.5). **Do not conflate** (D5): `OneShotLlm` is String-only/no-stream/no-budget and is the summarizer/digest path; `MaestroProvider` is the interactive-agent path. This task's budget counts the *Maestro agent's* tokens; whether `OneShotLlm` calls also count toward the budget is a documented decision (default: out of scope here — the summarizer's tokens are 404/409's accounting, this task counts only the interactive agent's usage; record in Handoff).

## Scope — in
- **`crates/core/src/maestro/provider.rs` (modified — the live providers + the frozen Direct-API arm + selector):**
  - Add `CodexCliProvider` and `GeminiCliProvider` implementing 402's **frozen** `MaestroProvider` trait, each launching its CLI through `AgentSupervisorHandle::start_session` with the Maestro's `AgentKind::Maestro` spawn (scratch cwd, `--mcp-config`/`--strict-mcp-config`, `strict`), differing only in the resolved binary + flags. Binary resolution: `ManagedPolicy::{codex,gemini}_executable_path()` if set, else the bare name (`codex`/`gemini`) on `$PATH` (mirror `ClaudeCliProvider`'s 402 resolution; if 402 routed binary choice through `resolve_agent_bin`'s Maestro arm, extend that arm — record which site in Handoff).
  - Add `DirectApiProvider` as a **FROZEN, unwired** `MaestroProvider` impl: every trait method returns a **typed `Err`/`Status`** carrying a stable `maestro.direct_api_unimplemented` marker (helper `direct_api_unimplemented()` / `is_direct_api_unimplemented()`, mirroring 313's `unimplemented_err`/`is_unimplemented` prefix pattern — **never** `unimplemented!()`/`todo!()`). Its constructor already reads `SecretKind::ProviderToken(provider)` (via the keychain parameterized accessor) + `ManagedPolicy::default_model()` so the fast-follow that fills the native function-call loop changes only bodies, not the seam.
  - Add the **auto-pick selector** `select_provider(policy, available) -> MaestroBackend` implementing the `design/08 §3.9` order `Claude → Codex → Gemini → Direct`: pick the first CLI whose binary resolves on `$PATH`/managed-path; fall through to `Direct` only when a `ProviderToken` exists; if none, the Maestro is unconfigured (typed error, surfaced as disabled — NOT a panic). User override (explicit `MaestroBackend`) takes precedence over auto-pick.
- **`crates/core/src/llm/{mod.rs,provider.rs}` (modified/new — the budget + inert state):**
  - Add `TokenBudget { daily_in_today: u64, daily_out_today: u64, in_cap: u64, out_cap: u64, resets_at_unix_ms: i64 }` (caps default 200_000 / 50_000 per `design/08 §3.9`; **user/managed-overridable** but the cap source is out of this task — default constants here). A `record_usage(in_tokens, out_tokens)` that bumps the in-memory counters AND persists via 403's bump-daily-counters accessor (single source of truth = `maestro_state`; the in-memory copy is a cache hydrated from `maestro_state` at boot). `is_exhausted()` = either counter ≥ its cap. `reset()` (manual + UTC-midnight) zeroes both counters + advances `budget_resets_at` to the next UTC midnight via 403's reset accessor.
  - Add `parse_token_usage(backend, raw) -> Option<(u64, u64)>` — extracts `(in, out)` token counts from each CLI's end-of-turn usage report (per-backend; the exact scrape source is the CLI's usage line / a structured event — implement what the live CLIs emit and keep a typed seam for the ones that don't yet, returning `None` ⇒ "couldn't account this turn", logged, never panicking). **NET-NEW**: there is no prior token-parsing anywhere; this is the carrier, NOT `ContextUsage{pct}`.
  - Add an **inert-on-exhaust** state on the Maestro: when `is_exhausted()`, the interactive LLM path is skipped (no `start_session`/`send_input` of free-form prompts to the agent), routing + deterministic tools still execute, and `get_digest()` returns the **last good digest with a `stale: true` badge** (`design/08 R-7`) rather than a fresh LLM call. A `BudgetExhausted` typed state the handle exposes for 414 to publish `maestro.budget_exhausted` and 415 to render the banner.
- **`crates/core/src/security/managed.rs` (read-only use — no new fields):** read `default_model()` (model selection passed to the provider where the CLI accepts a `--model`/equivalent flag; Direct-API reads it for the future request) + `{codex,gemini}_executable_path()` for binary resolution. Add NO managed fields (310 froze them); if a getter you need is missing that is a Stop-and-ask, not a new field.
- **Privacy interlock (consume 413's gate, do not re-own it):** the `enterpriseDataPrivacy=true` + external-model ⇒ Maestro-disabled consequence (D1, `design/08 §3.10`) is enforced by the resolver path 413 owns; here the selector MUST NOT auto-pick `Direct` (an external API) when `enterprise_data_privacy()` resolves true unless the configured base URL is on-prem — encode the "external Direct under privacy ⇒ not selectable" rule as a typed `disabled_by_policy` outcome the selector returns (414 publishes `maestro.disabled_by_policy`). The CLI backends are unaffected (they use the user's own CLI auth).
- Tests (Tier 2): a **mock `MaestroProvider`** returning scripted `(in, out)` token counts drives: (1) budget bump accumulates across *different* backends (cumulative-across-backends, D6); (2) crossing 200K-in OR 50K-out flips `is_exhausted()`; (3) inert-on-exhaust — routing/deterministic-tool calls still succeed while a free-form LLM turn is skipped and `get_digest()` returns the last-good digest with `stale=true`; (4) UTC-midnight + manual `reset()` zero both counters and advance `budget_resets_at`; (5) `select_provider` auto-pick order `Claude→Codex→Gemini→Direct` + user override + the no-backend typed-error + the `enterpriseDataPrivacy`+external-Direct `disabled_by_policy` outcome; (6) `DirectApiProvider` methods return the typed `direct_api_unimplemented` marker (a positive assertion on the seam, never a panic); (7) `parse_token_usage` extracts counts from a recorded Codex + Gemini usage sample and returns `None` (logged, no panic) on an unparseable sample.

## Scope — out
- **The real Direct-API native function-call loop** (Anthropic/OpenAI/Bedrock/Vertex/Azure-Foundry/OpenRouter request+tool-call+stream) — the **fast-follow** behind this task's FROZEN `DirectApiProvider` seam; this task ships the seam returning the typed `direct_api_unimplemented`, reading the keychain + model getters for that future wiring. On-prem Direct-API (Bedrock-VPC/Vertex/Foundry/local) is a Tier-3 gate item.
- **`maestro_state` schema + the budget accessors themselves** — **Task 403** (migration 0015; PHASE4_PLANNING §4.6). This task **consumes** them; it adds no migration and no persist accessor.
- **The `maestro.budget_exhausted` / `maestro.disabled_by_policy` event publishing + the `Maestro` gRPC impl** — **Task 414** (PHASE4_PLANNING §4.2/D7). This task exposes the typed `BudgetExhausted`/`disabled_by_policy` *state*; 414 reads it and publishes on `maestro.events` (`Event.checks_opaque=17`). This task wires no proto.
- **The Settings → Concerto Chat → Backend UI + the budget banner / 80%-amber-100%-red thresholds rendering** — **Task 415** (desktop, against 401.5's frozen surface). This task exposes the state (`is_exhausted`, `stale` flag, current backend); 415 renders it. R-10's user-configurable threshold *values* are 415's; this task ships the defaults.
- **The rolling-summarizer / digest LLM call** — routes through `OneShotLlm`/`DeterministicOneShot` (D5, §4.5), owned by 404/409, NOT this seam. This task counts only the interactive Maestro agent's tokens (record the boundary in Handoff).
- **Per-workarea privacy blanking + `enterpriseDataPrivacy` resolver + `concerto_chat_full_chat_access`** — **Task 413**. This task consumes 413's resolved `enterprise_data_privacy()` decision only at the selector boundary (external-Direct-not-selectable); it does not own the gate.
- The real-world Tier-3 line: leave the Maestro running across active workareas, exhaust the daily budget, and confirm it goes inert (no LLM calls) while `@workarea` routing + deterministic tools still work and the last digest shows a stale badge — the Phase-4 manual-checklist line "confirm budget-exhaust goes inert while routing still works."

## Public interface this task locks
- **Consumes `MaestroProvider` as frozen by Task 402 (PHASE4_PLANNING §4.3).** This task adds impls of it (`CodexCliProvider`, `GeminiCliProvider`, `DirectApiProvider`) and the selector — it does NOT re-lock the trait. The new impl types + selector + budget are what this task freezes:
  ```rust
  /// The three CLI backends ship LIVE; Direct-API is a FROZEN unwired seam (PHASE4_PLANNING D1).
  /// NET-NEW here — 402 froze the `MaestroProvider` trait + a `ClaudeCliProvider` STRUCT (no
  /// backend enum); this enum is introduced by 412, its `Claude` variant maps to 402's
  /// `ClaudeCliProvider`. The variant set is FROZEN here.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum MaestroBackend { Claude, Codex, Gemini, Direct }

  /// LIVE: same `start_session` spawn shape as 402's ClaudeCliProvider, different binary + flags.
  pub struct CodexCliProvider { /* supervisor handle, resolved bin, model */ }
  pub struct GeminiCliProvider { /* supervisor handle, resolved bin, model */ }

  /// FROZEN, UNWIRED (PHASE4_PLANNING D1, design/08 §3.9). Reads the keychain + model
  /// getters for the fast-follow; every method returns the typed `direct_api_unimplemented`
  /// marker — NEVER unimplemented!()/todo!(), NEVER empty-success.
  pub struct DirectApiProvider { provider: keychain::Provider, model: Option<String> }

  /// Auto-pick `Claude → Codex → Gemini → Direct` (design/08 §3.9). User override wins.
  /// Returns a typed `disabled_by_policy` outcome when enterpriseDataPrivacy=true selects
  /// an external Direct backend; a typed error when no backend is configured.
  pub fn select_provider(
      policy: &ManagedPolicy,
      enterprise_data_privacy: bool,
      override_backend: Option<MaestroBackend>,
  ) -> Result<MaestroBackend>;
  ```
- **`TokenBudget` + inert-on-exhaust (FROZEN, design/08 §3.9 / PHASE4_PLANNING §4.6):**
  ```rust
  /// Net-new daily token accounting (D6). Cumulative ACROSS backends. The in-memory
  /// copy is a cache; `maestro_state` (403) is the source of truth.
  pub struct TokenBudget {
      pub daily_in_today: u64,
      pub daily_out_today: u64,
      pub in_cap: u64,           // default 200_000 (design/08 §3.9)
      pub out_cap: u64,          // default 50_000  (design/08 §3.9)
      pub resets_at_unix_ms: i64,
  }
  pub const DEFAULT_DAILY_IN_CAP: u64 = 200_000;
  pub const DEFAULT_DAILY_OUT_CAP: u64 = 50_000;

  impl TokenBudget {
      /// Bump both counters; persists via 403's maestro_state bump accessor (consumed as frozen).
      pub async fn record_usage(&mut self, in_tokens: u64, out_tokens: u64) -> Result<()>;
      /// True when either counter has reached its cap (LLM goes inert; routing/tools survive).
      pub fn is_exhausted(&self) -> bool;
      /// Manual + UTC-midnight reset: zero both counters, advance resets_at to next UTC midnight.
      pub async fn reset(&mut self) -> Result<()>;
  }

  /// Per-backend usage parse (NET-NEW carrier — NOT ContextUsage{pct}). None = unaccountable turn.
  pub fn parse_token_usage(backend: MaestroBackend, raw: &str) -> Option<(u64, u64)>;
  ```
- consumes the `maestro_state` budget accessor as frozen by **Task 403** (PHASE4_PLANNING §4.6) — `record_usage`/`reset` call 403's bump-daily-counters / reset-budget / get-singleton accessors verbatim; this task does not redeclare them.
- consumes `SecretKind::ProviderToken(Provider)` + the parameterized keychain accessor as frozen by prior tasks — read-only, via the existing accessor; the closed `SecretKind` enum is untouched.
- consumes `ManagedPolicy::{default_model,codex_executable_path,gemini_executable_path}` as frozen by **Task 310** — read-only getters; no new managed fields.

## Implementation notes
- **The load-bearing rule: extend, never re-lock.** 402 owns `MaestroProvider`; 403 owns `maestro_state`. This task adds impls + budget logic on top. If the design contradicts 402's frozen trait shape (e.g. you need a method the trait lacks to thread the model), that is a **Stop-and-ask**, not a silent re-lock — capture it and surface it rather than widening 402's surface.
- **Reuse 402's spawn verbatim — don't reinvent the host machinery.** Codex/Gemini providers differ from `ClaudeCliProvider` only in the resolved binary + flags; everything else (scratch cwd `~/concerto/maestro/`, `--mcp-config`/`--strict-mcp-config`, `strict` permission mode, host-survival, cold-resume) flows through the **same** `AgentSupervisorHandle::start_session` path. Do not duplicate the spawn; parameterize it. The `resolve_agent_bin` arm at `actor.rs:1841` currently errors for Codex/Gemini — the Maestro spawn path (whatever site 402 chose) must resolve the right binary; record whether you extended `resolve_agent_bin` or routed binary choice through the provider in Handoff.
- **`maestro_state` is the single source of truth for the budget.** The in-memory `TokenBudget` is a cache hydrated at boot from 403's singleton-get; every `record_usage`/`reset` writes through to `maestro_state` so a Core restart mid-day resumes the same cumulative count (the budget must not reset on restart — only at UTC midnight or manual reset). Cumulative-across-backends (D6) falls out for free because the counter lives in one row regardless of which provider produced the tokens.
- **Inert-on-exhaust is a behavior gate, not a teardown.** When `is_exhausted()`, skip the interactive LLM call but keep routing + deterministic tools live and serve the last-good digest with `stale=true` (R-7) — do NOT kill the agent session in this task (whether to clean-stop + re-spawn at midnight is 414's lifecycle concern; here, just gate the LLM-call path). The typed `BudgetExhausted` / `disabled_by_policy` state is read by 414 to publish; do not publish events here (no proto/stream wiring in this task).
- **The Direct-API seam returns a typed marker, never the macro.** `DirectApiProvider`'s methods return `Err` carrying a stable `maestro.direct_api_unimplemented` string (helper + `is_*` predicate, 313's pattern) so a caller can distinguish "Direct-API not wired yet" from a real provider error, and so the fast-follow is a body-only change. Document it in Handoff under Deliberate debt (signature-frozen seam).
- **`parse_token_usage` is the net-new carrier.** There is zero precedent; `ContextUsage{pct}` is explicitly not it (D6). Implement against what the live Codex/Gemini CLIs actually emit at end-of-turn (a usage line / structured event); keep `None` as the honest "couldn't account this turn" answer (logged at `debug`, never a panic, never a silent 0 that under-counts) — record any CLI whose usage you could not parse as a Tier-3 accuracy caveat.
- **Cross-platform:** the CLI providers spawn through the agent supervisor, which is the `#[cfg(unix)]`-gated subsystem — keep any new spawn-adjacent code behind the same gate the 402 Maestro spawn path uses (mirror the sessions/streams handler gating); the budget/`TokenBudget`/`parse_token_usage` logic is pure and OS-agnostic (no `std::os::unix` in it) so it compiles + tests on the Windows/Linux lanes.
- **No proto, no two-site registration in this task.** This is provider + budget logic inside `crates/core`; there is no new gRPC service here (414 owns the `Maestro` service impl). If a regen is triggered only by a touched `rust-api.md` surface (the new public `provider.rs`/`llm` types), run the regen and commit the doc.
- Regen: new public Rust API (the `MaestroBackend`/`TokenBudget`/`DirectApiProvider` exports) ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/rust-api.md`; commit it. No proto/schema change in this task (403 owns the migration).
- **Parallel build hint:** the three FROZEN surfaces are file-disjoint and can be built by helper sub-agents in parallel, then integrated into the one commit (DAG `fanout`): (a) **Codex + Gemini CLI provider impls** (`provider.rs`, reusing 402's spawn) ∥ (b) **daily `TokenBudget` + inert-on-exhaust + last-good-digest/stale** (`llm/{mod,provider}.rs`, writing through 403's accessor) ∥ (c) **`DirectApiProvider` frozen seam + `select_provider` model/keychain selection** (`provider.rs` + `managed.rs` read getters). (a) and (c) share `provider.rs` (serialize the final merge of that file); (b) is independent. All three integrate behind 402's frozen `MaestroProvider`.

## Verification
**Tier 2.** The double is a **mock `MaestroProvider` returning scripted `(in, out)` token counts** + recorded Codex/Gemini usage samples for `parse_token_usage`; it does **NOT** cover real CLI/API token-count accuracy or the real on-prem Direct-API loop — that uncovered part is the Phase-4 Tier-3 checklist line "confirm budget-exhaust goes inert while routing still works" (and on-prem Direct-API as a gate/fast-follow item).

1. `cargo check --workspace` clean (the new providers + budget compile; Core's maestro module builds).
2. `cargo clippy --workspace --all-targets -- -D warnings` clean; then `cargo fmt --all -- --check` clean (CI `format.yml` parity — `--all` covers every workspace member).
3. `cargo test -p concerto-core maestro::provider` (and `llm::` budget tests) → proves: budget accumulates cumulatively across *different* backends; crossing 200K-in OR 50K-out flips `is_exhausted()`; inert-on-exhaust skips the free-form LLM turn while routing/deterministic-tool calls still succeed and `get_digest()` returns the last-good digest with `stale=true`; UTC-midnight + manual `reset()` zero both counters and advance `budget_resets_at`; `select_provider` auto-pick order + user override + no-backend typed error + `enterpriseDataPrivacy`+external-Direct `disabled_by_policy`; `DirectApiProvider` returns the typed `direct_api_unimplemented` marker (positive assertion, no panic); `parse_token_usage` extracts counts from recorded Codex/Gemini samples and returns `None` (logged) on garbage.
4. `cargo test --workspace --no-fail-fast` → all pass (no regression to 402's `ClaudeCliProvider` or 403's accessor tests).
5. `cargo deny check` → green (this task adds no new workspace pin — the Direct-API HTTP client is the fast-follow's pin, not this task's; if a CLI-usage-parse helper crate is genuinely needed, vet it cargo-deny-clean and any advisory-ignore is a Stop-and-ask, 313's discipline).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`rust-api.md` gains `MaestroBackend`/`TokenBudget`/`DirectApiProvider`/`select_provider`); no `schema.md`/proto diff.
7. `scripts/smoke.sh` → unchanged gate (this task adds no smoke capability; the Maestro spine boots through the extended provider set with identical behavior when no budget is exhausted). Exits 0.

**Tier-2 double + what it does NOT cover.** The mock-provider scripted-token-count double proves the budget math, cumulative-across-backends, inert-on-exhaust + last-good-digest/stale, reset, the selector, and the typed Direct-API seam. It does **NOT** prove real Codex/Gemini CLI token-report accuracy, real Direct-API token accounting, or the real on-prem Direct-API loop — those defer to the Phase-4 Tier-3 checklist line "confirm budget-exhaust goes inert while routing still works" (signed off at the phase gate; on-prem Direct-API is also a documented fast-follow).

## Definition of Done
- [x] `CodexCliProvider` + `GeminiCliProvider` implement 402's FROZEN `MaestroProvider` LIVE, launching their CLI through the same `start_session` spawn shape (scratch cwd, `--mcp-config`/`--strict-mcp-config`, `strict`), binary resolved from `ManagedPolicy::{codex,gemini}_executable_path()` else `$PATH`
- [x] `DirectApiProvider` frozen UNWIRED seam: reads `SecretKind::ProviderToken(..)` + `ManagedPolicy::default_model()`, every method returns the typed `direct_api_unimplemented` marker (not the macro, not empty-success)
- [x] `select_provider` auto-picks `Claude→Codex→Gemini→Direct` (design/08 §3.9), honors a user override, returns a typed error when no backend is configured, and a typed `disabled_by_policy` when `enterpriseDataPrivacy`+external-Direct
- [x] Net-new `TokenBudget` (200K in / 50K out defaults) bumped from `parse_token_usage`, cumulative across backends, persisted through 403's `maestro_state` accessor (source of truth survives restart)
- [x] Inert-on-exhaust: LLM calls stop, routing + deterministic tools still work, `get_digest()` serves the last-good digest with a `stale` badge (R-7); typed `BudgetExhausted`/`disabled_by_policy` state exposed for 414
- [x] UTC-midnight + manual `reset()` zero both counters and advance `budget_resets_at`
- [x] No new managed fields (310 froze them) and no new `SecretKind` variant (closed enum) — read existing getters/accessors only
- [x] All Verification commands pass on a clean checkout; interfaces regenerated + committed; smoke gate unchanged (green)
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (the Direct-API signature-frozen seam returns a typed `Err` with the `maestro.direct_api_unimplemented` marker, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any schema/contract changed (rust-api.md only; no proto/schema)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/provider.rs` (modified — `CodexCliProvider`/`GeminiCliProvider` LIVE impls, `DirectApiProvider` frozen-unwired seam + `direct_api_unimplemented` helper, `MaestroBackend` enum, `select_provider` + the `disabled_by_policy` outcome)
- `crates/core/src/llm/mod.rs` (modified — re-export the budget types; declare the budget module if new)
- `crates/core/src/llm/provider.rs` (new — `TokenBudget` + `DEFAULT_DAILY_{IN,OUT}_CAP` + `record_usage`/`is_exhausted`/`reset` over 403's accessor + `parse_token_usage` + the inert-on-exhaust state + last-good-digest/stale plumbing)
- `crates/core/src/security/managed.rs` (modified — read-only use of `default_model`/`{codex,gemini}_executable_path`; NO new fields — if no code change beyond a doc-comment cross-reference is needed, omit this from the commit)
- `docs/interfaces/rust-api.md` (regenerated — `MaestroBackend`/`TokenBudget`/`DirectApiProvider`/`select_provider`)

## Commit message
```
phase-4: pluggable Maestro provider — Codex/Gemini CLI live + daily TokenBudget + Direct-API frozen seam

Extends 402's frozen MaestroProvider with live CodexCliProvider/GeminiCliProvider
(same start_session spawn, resolved bin/flags) and a FROZEN unwired DirectApiProvider
(typed maestro.direct_api_unimplemented, reads ProviderToken keychain + default_model
for the fast-follow). Wires the net-new daily TokenBudget (200K in / 50K out,
cumulative across backends) through 403's maestro_state accessor: inert-on-exhaust
keeps routing/tools live + serves the last-good digest with a stale badge (R-7);
UTC-midnight + manual reset. Auto-picks Claude→Codex→Gemini→Direct.

Test double: a mock provider with scripted token counts (does NOT cover real
CLI/API token accuracy or the on-prem Direct-API loop — Tier-3 "budget-exhaust
goes inert while routing works").

Refs: tasks/v1.0/412-pluggable-provider-budget.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan** — <e.g. whether binary resolution extended `resolve_agent_bin`'s Maestro arm or routed through the provider; the exact `parse_token_usage` source each CLI emits; whether `OneShotLlm`/summarizer tokens were folded into the budget or kept out (default: out, counted by 404/409); whether `select_provider`'s `disabled_by_policy` distinguishes on-prem Direct base URLs; any 402/403 accessor-signature surprises.>
- **Open questions for next task** — <Task 414 consumes the FROZEN `BudgetExhausted`/`disabled_by_policy` state exposed here to publish `maestro.budget_exhausted`/`maestro.disabled_by_policy` on `maestro.events` (Event.checks_opaque=17); Task 415 consumes `is_exhausted`/`stale`/current-backend for the banner + the 80%-amber/100%-red thresholds (R-10); the Direct-API fast-follow fills `DirectApiProvider`'s bodies behind the unchanged frozen seam. Note the exact field/method names 414/415 must read.>
- **Deliberate debt** — <the `DirectApiProvider` is a signature-frozen seam returning the typed `maestro.direct_api_unimplemented` marker (no macro); the real native function-call loop + on-prem base-URL handling is the fast-follow. Any CLI whose end-of-turn token usage could not be parsed (`parse_token_usage` → `None`) is a Tier-3 accuracy caveat noted here.>
- **Smoke-gate state** — <`unchanged`; not re-run in-worktree (CI/operator gate for an `unchanged` task). The Maestro spine boots through the extended provider set with identical behavior absent an exhausted budget; `cargo check --workspace` green.>
