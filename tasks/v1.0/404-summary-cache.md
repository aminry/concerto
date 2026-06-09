# Task 404 — Per-workarea summary cache: `WorkareaSummary`/`SessionSummary`/`RepoSummary` FROZEN + `commits_ahead` helper + Haiku-fallback summarizer (agent-independent, refresh on `TurnComplete`)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 401 |
| Touches subsystem(s) | 08 (Maestro), 03 (Workspace/Workarea/Session Mgr), 02 (Repo Mgr — facts), 13 (VCS — PR/CI facts) |
| Smoke gate | unchanged |

## Goal
Build the **agent-independent per-workarea summary cache** that the Maestro reads from instead of raw chat (`design/08 §3.3`/`§6.2`), so it parallelizes the agent spine (`PHASE4_PLANNING D9`). **Today** there is no Maestro code at all — no `crates/core/src/maestro/summary.rs`, no cache, no summarizer wiring; 401 created only `maestro/{mod,mcp}.rs` + the tool-schema registry. The hard facts the design wants are **not precomputed anywhere**: `commits_ahead` has **NO implementation** (the `gix-wrap` surface in `crates/gix-wrap/src/lib.rs:74` has `diff_head`/`diff_to_main`/`cone_index_stats` but no ahead-count); `files_changed`/`lines_*` must be **counted from** `diff_to_main` (`crates/gix-wrap/src/diff.rs:79`) / `diff_head` (`diff.rs:70`) `DiffPayload` output; `ci_state` comes from the opaque `checks.<wa>.<repo>` stream frames; `pr_state` is the `pull_requests.state` string column (`crates/persist/src/api.rs:956`, `one of open|closed|merged|draft`). This task **LOCKS** (PHASE4_PLANNING §4.4): the `WorkareaSummary`/`SessionSummary`/`RepoSummary` structs (the `design/08 §3.3` shape but with **`i64` unix-ms** timestamps, NOT `Instant`, per D9/§2-404), the in-memory `HashMap<WorkareaId, WorkareaSummary>` cache, the **refresh contract** (refresh on `AgentEvent::TurnComplete` from `session.events.<sid>`, on a `workarea.events` `status:<to>` transition, after **10-min idle**, and **force-on-`GetDigest`-if-stale-60s**), and a **NEW `gix-wrap` `commits_ahead` helper** (rev-list `branch..base`). The fallback summarizer **REUSES** `OneShotLlm::suggest` with `ActionKind::DigestSummary` (`crates/core/src/llm/oneshot.rs:51`, **FROZEN** by Task 312 — consumed here, NOT re-locked); `DeterministicOneShot` is the **LIVE** P4 path. **After this task** the read-tool `get_workarea_summary` (Task 405), the digest generator (Task 409), the privacy-blanking pass (Task 413), and the Desktop rendering (Task 415) all consume these frozen shapes — never re-deriving a different one. The real-LLM summary quality stays Tier-3 (the phase-gate digest-quality line); 412's real provider is judged there.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md §4.4` — **AUTHORITATIVE**: this task OWNS the `WorkareaSummary`/`SessionSummary`/`RepoSummary` structs + the refresh-trigger contract + the `commits_ahead` helper + the hard-fact derivation. Also **§1 D5** (the `OneShotLlm` reuse vs the separate 402 agent seam — do not conflate), **§1 D9** (agent-independence; hard facts not precomputed), **§2 (404 rows)** (`i64` unix-ms not `Instant`; `commits_ahead: u32` via a new gix-wrap helper; in-memory HashMap, **no migration**; summarizer = `DeterministicOneShot` live, real Haiku is 412 judged at the gate), and **§8.1 (404 write-set)** (the maestro `mod.rs` soft seam).
- `design/08_Maestro_Agent.md §3.3` — the `WorkareaSummary`/`SessionSummary`/`RepoSummary` struct shapes (transcribe the field set; swap `Instant` → `i64` ms per D9). `§3.4` — the two summary sources (agent end-of-turn summary preferred; Concerto-side Haiku-class summarizer fallback) + the three refresh cadences. `§6.2` — "subscribe to `session.events.*`; on `TurnComplete` update the owning `WorkareaSummary`; hard facts pulled on demand." `§3.6`/`§6.4` — the digest's `list_active_summaries` consumer (Task 409) so the cache shape fits.
- `crates/core/src/agent_supervisor/events.rs:101` — `AgentEvent::TurnComplete { session_id }` (the refresh trigger). Note `ContextUsage{pct}` (line 122) is **wired-but-never-emitted** — it is NOT the carrier; do not depend on it.
- `crates/core/src/agent_supervisor/actor.rs:317` — `AgentSupervisorHandle::subscribe_events(&SessionId) -> Option<broadcast::Receiver<AgentEvent>>` (and `subscribe_events_with_replay` at :333) — the broadcast the cache subscribes to per active session.
- `crates/core/src/llm/oneshot.rs:35`/`:84`/`:121` — `ActionKind::DigestSummary`, `OneShotRequest{action, repo_id, prompt, context}`, `OneShotLlm::suggest(req)->Result<String>`, `compose_action_prompt`, and `DeterministicOneShot` (the LIVE fallback). **Consume FROZEN** (Task 312 / PHASE4_PLANNING §4.5); do NOT add an `ActionKind` variant or change the trait.
- `crates/gix-wrap/src/diff.rs:70`/`:79` — `diff_head(worktree)`/`diff_to_main(worktree, branch) -> DiffPayload{files: Vec<FileDiff{path, kind, hunks}}>`; count `files.len()` for `files_changed` and sum hunk `+`/`-` lines for `lines_added`/`lines_removed`. `crates/gix-wrap/src/lib.rs:68-88` — where `pub mod ahead;` + the `commits_ahead` re-export go (the existing frozen-surface doc-comment block is the pattern to extend).
- `crates/persist/src/api.rs:956`/`:988` — `pull_requests.state` (`open|closed|merged|draft`) → `pr_state`. `crates/core/src/workspace_manager/workarea.rs:216` — the workarea `state` string (the `status:<to>` source for the `workarea.events` refresh trigger).
- `tasks/v1.0/305-cone-stats-suggest-seam.md` → "Handoff Notes" — the **seam-discipline + gix-wrap-not-core placement** precedent (the index probe landed in `gix-wrap`, not core, to avoid a new `gix` dep in core); `commits_ahead` follows the same placement rule. Also the typed-`Err`-not-`unimplemented!()` discipline.

## Scope — in
- **`crates/gix-wrap/src/ahead.rs` (new):**
  - `pub async fn commits_ahead(worktree_path: &Path, base: &str) -> Result<u32>` — shell-out `git rev-list --count <base>..HEAD` (the ahead count of the workarea branch over its base), mirroring the `diff::diff_against` shell-out idiom (`cmd::run`). Forward-slash refs; pass `base` through verbatim (caller validates). Return `0` (not an error) when the count is empty/zero. **FREEZE** this signature in the `lib.rs` doc-comment block + re-export.
- **`crates/gix-wrap/src/lib.rs` (modified):** add `pub mod ahead;`, the `pub use ahead::commits_ahead;` re-export, and a frozen-surface doc-comment paragraph (Task 404) matching the existing per-task blocks.
- **`crates/core/src/maestro/summary.rs` (new):**
  - The three **FROZEN** structs (`WorkareaSummary`/`SessionSummary`/`RepoSummary`) per the §"Public interface" block — `design/08 §3.3` shape, `i64` unix-ms timestamps, `commits_ahead: u32`.
  - `pub struct SummaryCache { /* HashMap<WorkareaId, WorkareaSummary> + generation counter + a clock seam */ }` with: `get(&self, wa: &WorkareaId) -> Option<WorkareaSummary>` (cheap clone for the read tool), `upsert/refresh_workarea(...)` (rebuild one entry; bump `generation`), and `is_stale(&self, wa, max_age_ms: i64) -> bool` (the GetDigest force-refresh predicate at **60 s**).
  - **The refresh contract (the load-bearing deliverable):** an event-driven refresher that (a) on `AgentEvent::TurnComplete` for a tracked session, updates that session's `SessionSummary.last_turn_summary` + the owning `WorkareaSummary.last_turn_summary`/`last_3_turn_summaries`, bumps `generation`; (b) on a `workarea.events` `status:<to>` transition rebuilds the entry's `status`/`blocked_on`; (c) a **10-min idle** timer forces a refresh via the summarizer; (d) `force_refresh_if_stale(wa, 60_000)` for the on-`GetDigest` path. Take the `AgentSupervisorHandle` event broadcast (`subscribe_events`) + the `workarea.events` feed as injected inputs.
  - **Hard-fact derivation (no LLM):** per repo in the workarea, fill `RepoSummary` — `commits_ahead` via `gix_wrap::commits_ahead(worktree, base)`; `files_changed`/`lines_added`/`lines_removed` counted from `diff_to_main(worktree, base)` (`DiffPayload`); `pr_state` from `pull_requests.state`; `ci_state` parsed from the opaque `checks.<wa>.<repo>` frames (define a small `parse_ci_state(&[u8]) -> Option<CiState>` over the opaque frame; if the frame shape is not yet stable, keep the parser total + default to `None` and FREEZE the entry point — do not panic).
  - **The fallback summarizer:** `pub async fn summarize_turn(llm: &dyn OneShotLlm, repo_id: &str, recent: &str) -> Result<String>` building an `OneShotRequest{action: ActionKind::DigestSummary, repo_id, prompt: compose_action_prompt(...), context: recent}` and calling `suggest`. **Prefer the agent's own end-of-turn summary** (`design/08 §3.4`: if the `TurnComplete`-bearing chat row already carries a summary, use it free); fall back to this call only when absent. `DeterministicOneShot` is the injected LIVE impl.
- **`crates/core/src/maestro/mod.rs` (modified):** add `pub mod summary;` in the additive module region (the **soft seam** — own only this one line per §8.1).
- Tests (Tier 2): (1) `commits_ahead` on a `file://` fixture — N commits past `base` → `N`; zero-ahead → `0`; (2) synthetic `AgentEvent::TurnComplete` injection bumps `generation` and updates `last_turn_summary` for the right workarea; (3) a synthetic `workarea.events` `status:blocked` updates `status`/`blocked_on`; (4) `is_stale`/`force_refresh_if_stale(60_000)` against a controlled clock; (5) hard-fact derivation from a fixture `DiffPayload` (exact `files_changed`/`lines_*`) + a fixture `pull_requests.state` → `pr_state`; (6) the summarizer routes through `DeterministicOneShot` (deterministic string), proving the seam without a real LLM.

## Scope — out
- **The live Maestro agent / provider selection** (which CLI + model + preamble to spawn) — **Task 402** (+ 412 for Codex/Gemini/Direct-API). The cache is agent-independent (D9): it reads existing events and does not require a running Maestro session.
- **The real Haiku/Sonnet summarizer call** — **Task 412**'s provider; here the LIVE path is `DeterministicOneShot`. Real summary quality is judged at the **Phase-4 Tier-3 gate**.
- **`get_workarea_summary` MCP tool + its `cone_suggest_error_to_status`-style mapping** — **Task 405** (consumes this cache as frozen by 404).
- **Digest generation / `list_active_summaries` / `/digest` route** — **Task 409** (reads the cache).
- **Privacy blanking** (`exclude_from_maestro` → name-only; `concerto_chat_full_chat_access` → last-3-turns raw) — **Task 413** (gates this cache; this task stores the fields, does not redact them).
- **`maestro.events` publishing** of "summary updated" hints on the wire — **Task 414**; here the `generation` bump is in-process only.
- **No migration** — the cache is in-memory (`HashMap`), per PHASE4_PLANNING §3/§2-404. The `ci_state`/`pr_state` reads consume existing columns/frames; no schema change.
- **Tier-3 (phase checklist):** "leave for >30 min across active workareas, return, judge **digest quality** + measure latency" — the real-LLM summary content is not CI-provable here.

## Public interface this task locks
**Rust `commits_ahead` helper (FROZEN, design/08 §3.3 / PHASE4_PLANNING §4.4), `crates/gix-wrap/src/ahead.rs`:**
```rust
/// Count of commits on the worktree's `HEAD` that are NOT on `base`
/// (i.e. `git rev-list --count <base>..HEAD`). `base` is passed through
/// verbatim — callers building it from user input validate first.
/// Returns `0` (not an error) for a zero/empty count.
pub async fn commits_ahead(worktree_path: &std::path::Path, base: &str) -> concerto_error::Result<u32>;
```

**Rust summary-cache shapes (FROZEN, design/08 §3.3 / PHASE4_PLANNING §4.4), `crates/core/src/maestro/summary.rs`:**
```rust
/// Per-workarea rolling summary the Maestro reads instead of raw chat
/// (design/08 §3.3). Timestamps are `i64` unix-ms (D9), NOT `Instant`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkareaSummary {
    pub workarea_id: WorkareaId,
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub composer_name: String,
    pub branch_name: String,
    pub status: String,                       // the workarea FSM state string (workarea.rs)
    pub last_activity_at: i64,                // unix-ms

    pub sessions: Vec<SessionSummary>,
    pub last_turn_summary: String,            // <= 300 chars; most-recently-active session
    pub last_3_turn_summaries: Vec<String>,

    pub repos: Vec<RepoSummary>,              // hard facts, no LLM

    pub blocked_on: Option<String>,           // "awaiting_approval" | "test_failure" | "merge_conflict" | ...

    pub generated_at: i64,                    // unix-ms
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub agent_kind: AgentKind,
    pub model: String,
    pub status: String,                       // the session status string
    pub last_turn_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    pub repository_id: RepositoryId,
    pub repo_name: String,
    pub commits_ahead: u32,                   // via gix_wrap::commits_ahead
    pub files_changed: u32,                   // diff_to_main DiffPayload.files.len()
    pub lines_added: u32,
    pub lines_removed: u32,
    pub pr_state: Option<String>,             // pull_requests.state: open|closed|merged|draft
    pub ci_state: Option<String>,             // parsed from opaque checks.<wa>.<repo> frames
}
```
The **refresh contract** is FROZEN as part of §4.4: refresh on `AgentEvent::TurnComplete` (per active session), on a `workarea.events` `status:<to>` transition, after 10-min idle, and force-on-`GetDigest`-if-stale-60s. Consumers (405/409/413/415) read this shape; they never re-derive a different one.

This task **consumes** `OneShotLlm`/`ActionKind::DigestSummary`/`OneShotRequest`/`DeterministicOneShot` **as frozen by Task 312** (PHASE4_PLANNING §4.5) — it does NOT re-lock them, add an `ActionKind` variant, or change the trait. It **consumes** `AgentEvent::TurnComplete` + `AgentSupervisorHandle::subscribe_events` as frozen by the Phase-2/04 work, and `pull_requests.state` + `diff_to_main`/`diff_head` as frozen by Phase-3.

## Implementation notes
- **The cache is built from EXISTING signals — that is the whole point of D9.** Do NOT spawn or require a Maestro agent (402). Subscribe to the agent supervisor's per-session `AgentEvent` broadcast (`subscribe_events`, actor.rs:317) and the `workarea.events` feed; rebuild entries from `gix-wrap` diffs + `pull_requests` rows + opaque `checks` frames. This is what lets 404 run in the same wave as 402.
- **`commits_ahead` is genuinely new — there is no impl today.** Place it in **`gix-wrap`, not core** (the 305 precedent: `gix`/git tooling lives in `gix-wrap` so core gains no new git dep / deny surface). Shell out `git rev-list --count <base>..HEAD` via the existing `cmd::run` helper, exactly like `diff::diff_against`. Symmetric (`...`) is wrong here — we want strictly-ahead.
- **Reuse, don't reinvent, the summarizer.** The summary call is `OneShotLlm::suggest` with `ActionKind::DigestSummary` — already reserved (`oneshot.rs:51`). `DeterministicOneShot` is the live fallback. **Prefer the agent's own end-of-turn summary** first (`design/08 §3.4` — it's free); only call the summarizer when the closing turn carries no summary. The interactive-agent provider seam (402/412) is a **different** LLM seam — do not couple to it (D5).
- **Timestamps are `i64` unix-ms, not `Instant`** (D9) — wire/persistence-friendly and matches the rest of the codebase's `created_at`/`updated_at` columns. Inject a clock seam (a `now_ms: impl Fn() -> i64` or a small `Clock` trait) so the 10-min-idle / 60-s-stale tests use a synthetic clock (`design/08 §10` "synthetic clock" testing strategy).
- **Cross-platform / no `#[cfg(unix)]` here.** This task does not own a gRPC handler or the agent supervisor's `#[cfg(unix)]` host path — it only *subscribes* to the broadcast, which is platform-neutral; the `gix-wrap` shell-out runs on the Win/Linux CI lanes (Task 113). (Tasks that wire the supervisor handler itself — 402/414 — carry the `#[cfg(unix)]` gate, not 404.)
- **Seams return typed values, never the macro.** A not-yet-stable `checks.<wa>.<repo>` opaque-frame parser returns `Ok(None)`/a typed default, never `todo!()`/`unimplemented!()` and never a fake-success — document the `ci_state = None` default basis in the fn doc-comment and FREEZE the entry point (the 305 seam discipline).
- **`maestro/mod.rs` is the Phase-4 soft seam** (§8.1): add only `pub mod summary;` in the additive region 401 left; do not touch other tasks' module lines. On rebase this auto-merges.
- **Regen:** the new `gix-wrap` `commits_ahead` + the `maestro::summary` Rust API are picked up by `./scripts/regen-interfaces.sh` into `docs/interfaces/rust-api.md` (note the 305 caveat: regen captures struct/enum/type defs from `crates/*/src/api.rs`, not free `pub fn`s nor `src/maestro/*` modules — so expect `commits_ahead` to NOT appear, like `diff_to_main` doesn't; still run + commit any diff). No proto/SQL change ⇒ `proto.md`/no migration unchanged.
- **Parallel build hint:** three disjoint sub-parts (DAG `fanout`): (a) **`WorkareaSummary` structs + the `SummaryCache` HashMap + `is_stale`/`get`/`refresh` API** in `summary.rs`; ∥ (b) **the event-subscription refresher** (TurnComplete + workarea status + idle + force-stale) ; ∥ (c) **the `commits_ahead` gix-wrap helper + the hard-fact derivation + the `DeterministicOneShot` summarizer wrapper**. Integrate into one commit.

## Verification
**Tier 2.** The `rust` §5.3 set; the Tier-2 double is synthetic `AgentEvent`/`workarea-event` injection + the deterministic summarizer (no real LLM, no live Maestro agent).
1. `cargo check --workspace` — clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean; then `cargo fmt --all -- --check` clean (CI `format.yml` parity — `--all` covers every workspace member).
3. `cargo test -p concerto-gix-wrap ahead` — proves `commits_ahead` returns the exact ahead-count on a `file://` fixture (N commits past `base` → `N`; zero-ahead → `0`).
4. `cargo test -p concerto-core maestro::summary` (or the `summary` filter) — proves: synthetic `TurnComplete` injection bumps `generation` + updates the right workarea's `last_turn_summary`; a `status:<to>` event updates `status`/`blocked_on`; `is_stale`/`force_refresh_if_stale(60_000)` against a synthetic clock; hard-fact derivation from a fixture `DiffPayload` gives exact `files_changed`/`lines_*`; `pull_requests.state` maps to `pr_state`; the summarizer routes through `DeterministicOneShot`.
5. `cargo test --workspace --no-fail-fast` — all pass.
6. `cargo deny check` — green (no new crates; `commits_ahead` is a `git` shell-out via the existing `cmd` helper — no new `gix` features).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` — commit any regen (the `maestro::summary` struct defs may surface in `rust-api.md`; free fns like `commits_ahead` will not, per the 305 caveat — that is expected, not drift).
8. `scripts/smoke.sh` — **unchanged** (404 touches no smoke capability; the cache is CI-provable via in-process injection).

**Tier-2 double + what it does NOT cover.** The double is synthetic `AgentEvent::TurnComplete`/`workarea.events` injection + a synthetic clock + the deterministic `DeterministicOneShot` summarizer. It proves the cache shape, every refresh trigger, the `commits_ahead`/diff-count/`pr_state` hard-fact derivation, and the staleness logic — fully in CI with no live Maestro agent and no real LLM. It does **NOT** cover **real-LLM summary quality** (whether a Haiku/Sonnet rolling summary is actually useful), which is 412's provider and is judged at the **Phase-4 Tier-3 checklist line**: "leave for >30 min across active workareas, return, judge digest quality + measure latency."

## Definition of Done
- [x] `crates/gix-wrap/src/ahead.rs` adds `commits_ahead(worktree, base) -> Result<u32>` (`git rev-list --count <base>..HEAD`, `0` on empty) + `lib.rs` re-export + frozen doc-comment
- [x] `WorkareaSummary`/`SessionSummary`/`RepoSummary` FROZEN in `maestro/summary.rs` with `i64` unix-ms timestamps (NOT `Instant`) per design/08 §3.3 + PHASE4_PLANNING §4.4
- [x] In-memory `SummaryCache` (`HashMap<WorkareaId, WorkareaSummary>`) with `get`/`refresh`/`is_stale`/`force_refresh_if_stale(60_000)`; **no migration**
- [x] Refresh contract implemented: `AgentEvent::TurnComplete`, `workarea.events` `status:<to>`, 10-min idle, force-on-GetDigest-if-stale-60s
- [x] Hard facts derived, not precomputed: `commits_ahead` via the new helper; `files_changed`/`lines_*` counted from `diff_to_main`/`diff_head`; `pr_state` from `pull_requests.state`; `ci_state` parsed from opaque `checks.<wa>.<repo>` frames (total parser, `None` default)
- [x] Summarizer REUSES `OneShotLlm` + `ActionKind::DigestSummary` (`DeterministicOneShot` live); prefers the agent's own end-of-turn summary; consumes the Task-312 seam, does not re-lock it or add an `ActionKind`
- [x] `maestro/mod.rs` gains only `pub mod summary;` in the additive region (soft seam)
- [x] Tests (Tier 2): `commits_ahead` exact/zero, synthetic `TurnComplete` refresh, `status:<to>` refresh, staleness on a synthetic clock, diff-count + `pr_state` derivation, deterministic-summarizer routing
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams — the `ci_state` opaque-frame parser — return a typed `None`, not the macro; documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any rust-api surface changed
- [x] Single commit with the message below

## Outputs
- `crates/gix-wrap/src/ahead.rs` (new — `commits_ahead(worktree, base) -> Result<u32>` via `git rev-list --count`)
- `crates/gix-wrap/src/lib.rs` (modified — `pub mod ahead;` + `pub use ahead::commits_ahead;` + frozen-surface doc paragraph)
- `crates/core/src/maestro/summary.rs` (new — `WorkareaSummary`/`SessionSummary`/`RepoSummary` FROZEN + `SummaryCache` + the refresh contract + hard-fact derivation + the `DeterministicOneShot` summarizer wrapper)
- `crates/core/src/maestro/mod.rs` (modified — `pub mod summary;` in the additive region)
- `docs/interfaces/rust-api.md` (regenerated — if the `maestro::summary` struct defs surface; commit any diff)

## Commit message
```
phase-4: per-workarea summary cache + commits_ahead helper

Adds the agent-independent WorkareaSummary/SessionSummary/RepoSummary
cache (FROZEN, design/08 §3.3, i64 ms timestamps) built from existing
TurnComplete + workarea.events + diff + pull_requests.state signals, the
new gix-wrap commits_ahead helper (git rev-list --count base..HEAD), and
the DeterministicOneShot fallback summarizer (real Haiku is 412, judged
at the gate). Refresh on TurnComplete / status change / 10-min idle /
force-on-GetDigest-if-stale-60s. In-memory HashMap, no migration.

Refs: tasks/v1.0/404-summary-cache.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** — (e.g. did `ci_state` end up `String` vs a typed enum? did the `checks.<wa>.<repo>` opaque frame shape force a `None`-default parser? did the clock seam land as a `Fn` or a trait? did `rust-api.md` actually change, given the 305 free-fn-omission caveat?)
- **Open questions for next task:** — **Task 405** (`get_workarea_summary` read tool) consumes the FROZEN `WorkareaSummary` from `SummaryCache::get`; **Task 409** (digest) consumes `list_active_summaries` over the cache + the `force_refresh_if_stale(60_000)` path; **Task 413** (privacy) blanks summaries on this same shape (`exclude_from_maestro` → name-only) — confirm the field set is sufficient for all three and note any blanking-affordance they will need (e.g. whether to keep `repos`/hard-facts visible while blanking `last_turn_summary`).
- **Deliberate debt:** — (e.g. `ci_state` defaulting to `None` until the `checks` opaque-frame parser is stabilized; the summarizer using `DeterministicOneShot` only — real Haiku/Sonnet quality deferred to 412 + the Tier-3 gate). State that no `todo!()`/`unimplemented!()` macro is used.
- **Smoke-gate state:** — Expected **unchanged** (404 is CI-provable via in-process event injection; `commits_ahead` is a plain `git` shell-out within existing deps; no `scripts/smoke.d/*` or manifest change). Note the migration high-water mark observed on `main` at impl time (was `0014` at authoring; if a Phase-4 migration landed first, no effect on 404 — it adds none).
