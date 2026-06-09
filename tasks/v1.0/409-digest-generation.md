# Task 409 — `generate_digest()` (return-from-absence digest, <5 s p50) + Maestro-side chip persistence (consumes 404's summary cache + 408's `/digest` route; persists chips to the digest `chat_messages` row per D11)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 404, 408 |
| Touches subsystem(s) | 08 (Maestro), 07 (Suggestion Engine — chips) |
| Smoke gate | unchanged |

## Goal
Build the **return-from-absence digest** — the Maestro's killer feature (`design/08 §3.6`, PRD §14.4.3): when the user reopens Concerto after an absence (or types `/digest`), the Maestro gathers its active-workarea summaries, computes what changed since they last looked, asks the one-shot LLM for a 3–5-sentence grouped summary, and renders it above the composer with next-step chips. **Today there is no digest code at all** — no `crates/core/src/maestro/digest.rs`, no `generate_digest()`, no `Digest` assembly; the LLM seam `OneShotLlm::suggest` (`crates/core/src/llm/oneshot.rs:126`) exists with `ActionKind::DigestSummary` already reserved (`oneshot.rs:50`, wire string `"digest_summary"`) and its **LIVE** impl `DeterministicOneShot` (`oneshot.rs:141`), but nothing calls it for a digest; the summary cache (`WorkareaSummary`, frozen by 404 per PHASE4_PLANNING §4.4) and the routing pre-parser (`pre_parse(&str) -> ParseOutcome` with the `Slash{directive: "digest", ..}` arm, frozen by 408 per §4.7) are the two inputs this task stitches together; `chats(kind='maestro')` singleton + `maestro_state.last_digest_at` (frozen by 403 per §4.6) hold the persistence anchor; and `chat_messages` chips have **nowhere to live** (the V0.1 Suggestion Engine's `Chip` buffer evaporates after ~60 s `DEDUP_TTL`, `crates/core/src/suggestions/chip.rs:49`). This task adds `crates/core/src/maestro/digest.rs`: a `generate_digest(workspace_id, last_seen_at) -> Result<Digest>` free async fn that (1) collects the active-workarea `WorkareaSummary`s from 404's cache, (2) computes a typed `WorkareaDelta` per workarea since `last_seen_at`, (3) builds the **templated digest prompt** (`design/08 §3.6`), (4) calls `OneShotLlm::suggest(OneShotRequest{action: ActionKind::DigestSummary, ..})` — **`DeterministicOneShot` is the LIVE P4 path; the real Sonnet provider is 412, judged at the gate** (§4.5) — (5) assembles a `Digest{ text, groups: {finished, blocked, working}, next_step, chips }`, and (6) **persists the digest's chips on the Maestro side** (**FROZEN: `Digest` + `WorkareaDelta` per design/08 §3.6 / PHASE4_PLANNING §4.4/§4.7**) — D11: the chips are written into the digest's `chat_messages` row (the `kind='maestro'` chat) so they survive the suggestion buffer's TTL, **not** left in the volatile suggestion engine. After this task, `/digest` (408's slash route) and the future `GetDigest` RPC have a real digest producer; **Task 414** consumes `generate_digest()` to serve `GetDigest` and publish the `maestro.digest_generated` event over `maestro.events`. Real-LLM digest *quality* and real-LLM *latency* stay Tier-3 (the phase-gate "leave >30 min, judge digest quality + latency" line); this task proves the deterministic assembly + the **<5 s p50** budget against the deterministic summarizer + a 6-workarea fixture (`design/08 §3.6/§10`, PHASE4_PLANNING §2 "409 digest latency proof").

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §1 (**D5**, **D11**) — **AUTHORITATIVE.** D5: `OneShotLlm`/`DeterministicOneShot` is the digest's one-shot seam (reused, not modified); the interactive agent is a *separate* 402 seam — do **not** route the digest through the agent loop. D11: the digest's chips are **Maestro-persisted** (attached to the digest `chat_messages` row), never left in the ~60 s suggestion buffer.
- `tasks/v1.0/PHASE4_PLANNING.md` §4.4 (**consumes**) — `WorkareaSummary`/`SessionSummary`/`RepoSummary` + the cache refresh contract are **FROZEN by 404**; this task reads them, never re-derives a different shape. Note the `i64` unix-ms `last_activity_at`/`generated_at` (not `Instant`) and the force-on-`GetDigest`-if-stale-60 s refresh trigger.
- `tasks/v1.0/PHASE4_PLANNING.md` §4.7 (**consumes**) — the routing grammar + `ParseOutcome` (`Freeform` | `Routing{targets, body}` | `Slash{directive, body}`) **FROZEN by 408**; `/digest` is the `Slash{directive: "digest", ..}` arm. 409 is the handler that arm dispatches to.
- `tasks/v1.0/PHASE4_PLANNING.md` §4.5 (**consumes**) — `OneShotLlm` reuse FROZEN by 312; `ActionKind::DigestSummary` is already reserved; `DeterministicOneShot` is the LIVE fallback. The real provider is 412's seam (§4.3).
- `tasks/v1.0/PHASE4_PLANNING.md` §4.6 (**consumes**) — `maestro_state.last_digest_at` + the `chats(kind='maestro')` singleton are **FROZEN by 403**; 409 sets `last_digest_at` after a successful digest and writes the digest message into the maestro chat. **No migration in 409** (the column + singleton already exist); migrations are not 409's — confirm the highest `crates/persist/migrations/NNNN_*.sql` on main is still **0014** (0015 is 403's, 0016 is 410's); if a migration landed above 0014, note the drift in Handoff but 409 adds none regardless.
- `design/08_Maestro_Agent.md` §3.6 — the `generate_digest()` pseudocode + the templated digest prompt (grouped Finished / Blocked / Still-working + a one-line next step) + the **<5 s p50** target; §6.4 — the digest sequence (cache → LLM → suggestion chips → `Digest`); §10 — "Latency: Digest < 5s on a 6-workarea state | Bench" + "Integration: Digest generation against 6-workarea fixture | E2E".
- `crates/core/src/llm/oneshot.rs` — `OneShotLlm::suggest(OneShotRequest{action, repo_id, prompt, context}) -> Result<String>` (line 126), `ActionKind::DigestSummary` (line 50), `compose_action_prompt` (line 182), `DeterministicOneShot` (line 141; the `_ =>` arm echoes `context` trimmed — the LIVE digest text source until 412). The digest reuses this verbatim; do **not** add an `ActionKind` variant or touch this file.
- `crates/core/src/suggestions/chip.rs` — the in-process `Chip{rule_id, workarea_id, title, priority: i32, created_at, action: ChipAction}` (line 49) + `ChipAction{Compress, NewSession, OpenTestFailure, CommitAndPush, ReviewTool, Resume}` (line 16). The digest's chips reuse this shape (D11 says `propose_chip` "mirrors the `Chip` shape"); 409 serializes them onto the digest message — it does **not** call `next_step_chips` (that method does not exist; see Implementation notes).
- `crates/persist/src/chat_messages.rs` — `insert(conn: &mut SqliteConnection, NewChatMessage{id, chat_id, role, content_json, created_at, parent_id, superseded_by}) -> Result<String>` (line 44). **There is NO `metadata` column** (410 adds it via 0016); the digest's chips ride inside `content_json` (a JSON envelope) on a `role='assistant'` row. The free-fn-over-`&mut SqliteConnection` write pattern is the persist convention.
- `tasks/v1.0/404-*.md` → "Handoff Notes" — the **exact** built `WorkareaSummary` field names/types + the cache accessor 409 reads (`get_workarea_summary` / a list-active-summaries entrypoint) + the `status: WorkareaStatus` / `blocked_on` shape that drives the Finished/Blocked/Working grouping. **Read this first at impl time** — it pins the consumed surface 409 must not re-shape.
- `tasks/v1.0/408-*.md` → "Handoff Notes" — the built `ParseOutcome::Slash` shape + how `/digest` dispatches (whether the slash handler calls `generate_digest` directly or returns a marker 414 acts on); align 409's entrypoint signature to what 408 froze.
- `tasks/v1.0/305-cone-stats-suggest-seam.md` — the dense, citation-heavy register + the seam discipline (typed `Err`/`Status`, never `todo!()`/`unimplemented!()`, never empty-success) this file mirrors.

## Scope — in
- **`crates/core/src/maestro/digest.rs` (new) — delta compute + prompt + assembly:**
  - `WorkareaDelta` — a typed per-workarea diff computed from a `WorkareaSummary` against `last_seen_at: i64` (unix-ms): which fields advanced since the user was last seen (e.g. `commits_ahead` increase, status transition into `Finished`/blocked, new PR/CI state, last-turn changed). Pure function `compute_delta(&WorkareaSummary, last_seen_at: i64) -> WorkareaDelta` — **no LLM, no I/O**.
  - `DigestGroup` classification: each active workarea sorts into **Finished** (ready for action), **Blocked** (needs user input — driven by 404's `blocked_on`/status), or **Still working** (current focus), per `design/08 §3.6`.
  - `build_digest_prompt(summaries: &[WorkareaSummary], deltas: &[WorkareaDelta], away_minutes: u64) -> String` — the templated prompt verbatim from `design/08 §3.6` (the "You are Concerto's maestro… grouped by Finished / Blocked / Still working… End with a one-line proposed next step" template, with the per-workarea block). Deterministic; the `context` field passed to `OneShotRequest`.
  - `generate_digest(...)` (the entrypoint 408's `/digest` and 414's `GetDigest` call): gather active-workarea `WorkareaSummary`s from 404's cache (force-refresh-if-stale-60 s is 404's contract — call its refresh entrypoint), `compute_delta` per workarea, `build_digest_prompt`, `OneShotLlm::suggest(OneShotRequest::new(ActionKind::DigestSummary, repo_id, prompt, prompt))` (the deterministic impl echoes `context` ⇒ pass the built prompt as both `prompt` and `context` so the LIVE path returns the grouped scaffold; 412's provider sends `prompt` to the model), assemble `Digest`, derive next-step chips, persist, set `last_digest_at`, return.
  - **Take the `OneShotLlm` as an injected `Arc<dyn OneShotLlm>`** (default `DeterministicOneShot`) — the same dependency-injection seam 312 froze, so 412 swaps the provider with zero change here.
- **Chip derivation + Maestro-side persistence (D11):**
  - Derive the digest's chips from the grouped state deterministically (e.g. a `CommitAndPush`/`ReviewTool`/`Resume` `ChipAction` for the Finished/Blocked/crashed groups) — reusing the `Chip`/`ChipAction` shape from `suggestions/chip.rs`. **Do not** depend on a `ChipRanker`/`next_step_chips`/`propose_chip` (none exist in V0.1; 407 owns `propose_chip`, 620 owns `ChipRanker`).
  - **Persist the chips on the Maestro side**: write the digest as a `chat_messages` row (`role='assistant'`, `chat_id` = the `kind='maestro'` singleton chat id from 403) whose `content_json` is a JSON envelope carrying `{text, groups, next_step, chips:[…]}`. Because `chat_messages` has **no `metadata` column** until 410's 0016, the chips live inside `content_json` — they survive precisely because they are a persisted row, **not** a suggestion-buffer entry that evaporates at `DEDUP_TTL`. Use `chat_messages::insert` over `&mut SqliteConnection` (the persist convention); caller-supplied UUIDv7 id for chronological ordering.
  - Update `maestro_state.last_digest_at` via 403's frozen accessor after a successful persist (so "away since last digest" deltas are anchored). On LLM error, follow `design/08 §8`/R-7: do not crash — return a `Digest` whose `text` is a typed degraded message ("model unreachable; routing still works") and still surface the deterministic groups + chips (Tier-3 412 owns the stale-badge UI).
- **`crates/core/src/maestro/mod.rs` (modified):** add `pub mod digest;` in the additive module region (the soft seam — distinct region, auto-merges on rebase per PHASE4_PLANNING §8.1) and re-export `Digest`/`WorkareaDelta`/`generate_digest` so 408/414 reach them.
- **Tests (Tier 2):** (1) **6-workarea fixture** (`design/08 §10`): build six `WorkareaSummary`s spanning Finished/Blocked/Working, run `generate_digest` against `DeterministicOneShot`, assert the `Digest` groups each workarea correctly, the `next_step` is non-empty, and ≥1 chip is produced. (2) **Chip persistence (D11):** assert the digest writes one `chat_messages` row in the maestro chat whose `content_json` round-trips the chips, and that the row survives an interval > `DEDUP_TTL` (a persisted row has no TTL — assert by re-reading after a simulated tick). (3) **`compute_delta`** table cases: `commits_ahead` advance, status→Finished, status→blocked, no-change-since-`last_seen_at`. (4) **`last_digest_at` set** after success; **degraded path** on an injected always-`Err` `OneShotLlm` returns the typed degraded `Digest` (not a panic, not empty). (5) **Latency: <5 s p50** — a Criterion bench (or a timed `#[test]` averaging N runs) over `generate_digest` on the 6-workarea fixture with `DeterministicOneShot`, asserting the p50 is well under 5 s (deterministic path is sub-millisecond; the assertion guards against an accidental O(n²)/blocking regression and documents the budget).

## Scope — out
- **The real-LLM digest provider (Sonnet/Haiku quality + real latency)** — owned by **Task 412** (the `MaestroProvider`/`DirectApiProvider` seam, §4.3); 409 injects `Arc<dyn OneShotLlm>` so 412 swaps it. This task leaves the LIVE deterministic path + the injection seam; real-LLM quality/latency is the Tier-3 gate line.
- **The `Maestro.GetDigest` gRPC RPC + `maestro.digest_generated` event publishing** — owned by **Task 414** (fills 401.5's `MaestroServer` skeleton, publishes over `maestro.events` per §4.2). 409 provides `generate_digest()`; 414 calls it from the handler and emits the event. 409 does **not** touch `maestro.proto`, `handlers/maestro.rs`, `streams.rs`, `api_server.rs`, or `boot.rs`.
- **The `WorkareaSummary` cache + refresh triggers + `commits_ahead` helper** — owned by **Task 404** (§4.4); 409 reads the cache and calls 404's force-refresh entrypoint, never re-derives summaries or hard facts.
- **The `/digest` slash parsing** — owned by **Task 408** (§4.7); 409 is the handler the `Slash{directive: "digest"}` arm dispatches to, not the parser.
- **`chat_messages.metadata` + daily-summary tagging** — owned by **Task 410** (migration 0016, §D12); 409 carries chips in `content_json`, **not** in a `metadata` column (which does not exist yet). The leaves-a-seam note: when 410 lands `metadata`, a digest row is still a plain `assistant` message (not a `daily_summary`), so no rework.
- **`propose_chip` (Maestro slate) / `ChipRanker` / next-step ranking** — owned by **Task 407** (`propose_chip`) and **Task 620** (`ChipRanker`). 409 derives its chips deterministically from the grouped state; it does not call the suggestion engine.
- **The real-world Tier-3 line:** leave Concerto for >30 min across several active workareas, return, and **judge the digest's quality and measure its real-LLM latency** against the <5 s p50 budget (Phase-4 manual checklist).

## Public interface this task locks
- **`crates/core/src/maestro/digest.rs` — `Digest` + `WorkareaDelta` + `generate_digest` (FROZEN, design/08 §3.6 / PHASE4_PLANNING §4.4/§4.7/D11).** The digest producer's surface; 408 (`/digest`) and 414 (`GetDigest`) consume it. Field names/types align to 404's frozen `WorkareaSummary` (consumed, not re-locked):

```rust
/// The return-from-absence digest (`design/08 §3.6`). Produced by
/// [`generate_digest`]; consumed by 408's `/digest` slash route and 414's
/// `GetDigest` RPC (which maps it onto the `Digest` proto frozen by 401.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    /// The LLM-written 3–5-sentence summary (LIVE path = `DeterministicOneShot`
    /// echo of the grouped scaffold; 412 swaps the real provider). Never empty:
    /// a degraded/unreachable LLM yields a typed fallback line, not "".
    pub text: String,
    /// Workareas grouped per `design/08 §3.6`.
    pub finished: Vec<DigestEntry>,
    pub blocked: Vec<DigestEntry>,
    pub working: Vec<DigestEntry>,
    /// The one-line proposed next step (`design/08 §3.6` template tail).
    pub next_step: String,
    /// Next-step chips, derived deterministically from the grouped state and
    /// **persisted on the digest's `chat_messages` row** (D11) — NOT left in
    /// the ~60 s suggestion-engine buffer. Mirrors `suggestions::Chip`.
    pub chips: Vec<Chip>,
    /// Unix-ms the digest was generated (set into `maestro_state.last_digest_at`).
    pub generated_at: i64,
    /// Whether the LLM path degraded (model unreachable / budget inert) — the
    /// groups+chips are still valid; 412 renders the "stale" badge (R-7).
    pub degraded: bool,
}

/// One workarea's line in a digest group (a projection of 404's
/// `WorkareaSummary` + its computed [`WorkareaDelta`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestEntry {
    pub workarea_id: WorkareaId,
    pub composer_name: String,
    pub one_line: String,
    pub delta: WorkareaDelta,
}

/// What advanced for a workarea since `last_seen_at`. Pure-computed from a
/// `WorkareaSummary`; no LLM, no I/O. Drives the digest's "what changed" prose
/// and the Finished/Blocked/Working classification.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkareaDelta {
    pub commits_ahead_added: u32,
    pub files_changed_delta: i64,
    pub became_finished: bool,
    pub became_blocked: bool,
    pub pr_state_changed: bool,
    pub ci_state_changed: bool,
    pub last_turn_changed: bool,
}

/// Generate the return-from-absence digest for one workspace's active
/// workareas. Gathers 404's summaries (force-refresh-if-stale-60s per §4.4),
/// computes deltas since `last_seen_at`, builds the templated prompt
/// (`design/08 §3.6`), runs it through the injected [`OneShotLlm`]
/// (`DeterministicOneShot` is the LIVE P4 path; 412 swaps the real provider),
/// persists the digest + its chips to the `kind='maestro'` chat (D11), sets
/// `maestro_state.last_digest_at`, and returns the assembled [`Digest`].
pub async fn generate_digest(
    workspace_id: WorkspaceId,
    last_seen_at: i64,
    summaries: &SummaryCache,          // 404's frozen cache handle (§4.4)
    llm: &Arc<dyn OneShotLlm>,         // injected; DeterministicOneShot default (§4.5)
    pool: &SqlitePool,                 // persist the digest row + chips (D11)
) -> Result<Digest>;
```

- **Consumed as frozen (NOT re-locked here):** `WorkareaSummary`/`SessionSummary`/`RepoSummary` + the cache handle and refresh contract — **frozen by Task 404 (PHASE4_PLANNING §4.4)**; `ParseOutcome`/`Slash{directive:"digest"}` — **frozen by Task 408 (§4.7)**; `OneShotLlm`/`OneShotRequest`/`ActionKind::DigestSummary`/`DeterministicOneShot` — **frozen by Task 312 (§4.5)**; `maestro_state.last_digest_at` accessor + the `chats(kind='maestro')` singleton id — **frozen by Task 403 (§4.6)**; `Chip`/`ChipAction` — defined by the V0.1 Suggestion Engine (`suggestions/chip.rs`). 409 imports these; the exact field names are read from each owner's Handoff Notes at impl time and must match verbatim.

## Implementation notes
- **The digest reuses `OneShotLlm`, NOT the interactive agent loop (D5).** The digest is a one-shot, string-out, no-stream call — exactly `OneShotLlm`'s shape. The interactive Maestro chat agent (402's provider seam) is the *wrong* shape (it streams, has a budget, holds a session). Route the digest through `OneShotLlm::suggest` with `ActionKind::DigestSummary`; pass the built prompt as **both** `prompt` and `context` so `DeterministicOneShot`'s `_ =>` echo arm (`oneshot.rs:158`) returns the grouped scaffold as the LIVE digest text, while 412's real provider reads `prompt`.
- **Chips persist because the row persists (D11) — the load-bearing rule.** The V0.1 suggestion buffer drops chips at `DEDUP_TTL` (~60 s, `suggestions/state.rs`). A returning user may take minutes to act on a digest. So the digest's chips are serialized into the persisted `chat_messages.content_json` envelope, never handed to the suggestion engine. This is the whole point of D11 — do not "helpfully" also push them to `SuggestionEngineHandle` (that would re-introduce the TTL race and double-surface the chips).
- **Reuse, don't reinvent:** the `Chip`/`ChipAction` types (`suggestions/chip.rs`), the `chat_messages::insert` free-fn write pattern, the `Arc<dyn OneShotLlm>` injection seam (mirror how 312/404 inject it), and the `Resolved`/JSON-envelope conventions already in the codebase. Do **not** add a new `ActionKind`, a new chips table, or a `metadata` column (410's job).
- **Cross-platform / cfg gating:** `digest.rs` is pure Rust + sqlx + the in-process LLM seam — it does **not** touch the agent supervisor (that's 402/406), so it needs **no `#[cfg(unix)]` gate**. The digest does not spawn or talk to the Maestro PTY session; it reads the cache + persists. Keep it host-agnostic so the Windows/Linux Core lanes (Task 113) compile it.
- **Degraded path is a typed value, never a macro.** On `OneShotLlm::suggest` error, set `degraded: true` and `text` to a typed degraded line; still return the deterministic groups + chips. No `todo!()`/`unimplemented!()`/`unwrap()` on the LLM path — `design/08 §8` (LLM unreachable ⇒ "routing + tools still work") and R-7 (show last good digest with a stale badge — 412 wires the "last good" cache; 409 returns the freshly-degraded one).
- **No proto/two-site registration in 409.** This task adds **no** gRPC surface and **no** event — those are 414's. So **no** `regen-interfaces.sh` proto delta is expected from 409; only the `rust-api.md` may gain the new `Digest`/`WorkareaDelta` structs if they live in a `crates/*/src/api.rs`-scanned path (they live in `crates/core/src/maestro/digest.rs`, which `regen-interfaces.sh` does **not** scan — confirm `git diff docs/interfaces/` is clean and note it, matching 305's Handoff finding that core `src/maestro`/`repo_manager` modules are not captured). Regen: run `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` and **commit any delta** (expected: none).
- **Parallel build hint:** three disjoint fan-out sub-parts (DAG `fanout`): **(a) digest-prompt + delta-compute** — `compute_delta` + `DigestGroup` classification + `build_digest_prompt` + `Digest` assembly (pure, no I/O); **(b) chip-persistence** — the `content_json` envelope + `chat_messages::insert` + `last_digest_at` set + the chips-survive-TTL test; **(c) latency-bench-fixture** — the 6-workarea `WorkareaSummary` fixture builder + the Criterion/timed <5 s p50 bench. (a)/(b)/(c) build independently against 404/408/403's frozen surfaces and integrate in the one commit.

## Verification
**Tier 2.** The `rust` §5.3 command set; the test double is `DeterministicOneShot` + the 6-workarea fixture.
1. `cargo check --workspace` → clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` → clean.
3. `cargo fmt --all -- --check` → clean.
4. `cargo test -p concerto-core digest` (+ `maestro::digest`) → proves: the 6-workarea fixture groups each workarea into Finished/Blocked/Working correctly with a non-empty `next_step` and ≥1 chip; the digest persists exactly one `chat_messages` row in the `kind='maestro'` chat whose `content_json` round-trips the chips and survives a simulated interval > `DEDUP_TTL`; `compute_delta` table cases (commits-ahead advance, status→Finished, status→blocked, no-change); `maestro_state.last_digest_at` is set after success; the always-`Err` `OneShotLlm` yields a typed degraded `Digest` (`degraded == true`, `text` non-empty, groups+chips still present) — not a panic, not empty.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo bench -p concerto-core digest_latency` (or the timed `#[test]` if a Criterion bench is overkill) → **p50 < 5 s** on the 6-workarea fixture with `DeterministicOneShot` (deterministic path is sub-ms; the assertion guards the budget and any future blocking regression).
7. `cargo deny check` → green (no new crate; `criterion` is dev-only if added — confirm it is already a workspace dev-dep, else this is a Stop-and-ask per the 401 `rmcp`/313 octocrab precedent).
8. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → expected **clean** (409 adds no proto/`api.rs` surface; `crates/core/src/maestro/*` is not scanned — note in Handoff if a delta appears, then commit it).
9. `scripts/smoke.sh` → **unchanged** (409 touches no smoke capability; the digest is CI-provable in-process via the fixture + the deterministic summarizer; the live `maestro-digest` smoke capability is 414's wire + the Tier-3 gate).

**Tier-2 double + what it does NOT cover.** The double is `DeterministicOneShot` (the LIVE one-shot path) + a synthetic 6-workarea `WorkareaSummary` fixture: it proves the delta compute, the Finished/Blocked/Working grouping, the templated-prompt assembly, the **<5 s p50** budget on the deterministic path, and the **D11 chip-persistence** (chips survive past the suggestion buffer's ~60 s TTL because they live on a persisted row). It does **NOT** cover **real-LLM digest quality** (whether a real Sonnet call writes a *good* 3–5-sentence summary — that's 412's provider) or **real-LLM latency** (whether the real call meets <5 s p50 over the network). Those defer to the **Phase-4 Tier-3 checklist** line: *"leave for >30 min across active workareas, return, judge digest quality + measure latency."*

## Definition of Done
- [x] `crates/core/src/maestro/digest.rs` (new): `generate_digest(workspace_id, last_seen_at, summaries, llm, pool) -> Result<Digest>` + `Digest`/`DigestEntry`/`WorkareaDelta` + `compute_delta` + `build_digest_prompt`, consuming 404's `WorkareaSummary` (§4.4), 408's `/digest` route (§4.7), 312's `OneShotLlm`/`ActionKind::DigestSummary` (§4.5), and 403's `last_digest_at`/maestro-chat singleton (§4.6) — none re-locked
- [x] Digest routes through `OneShotLlm::suggest(ActionKind::DigestSummary)` with `DeterministicOneShot` as the LIVE path, injected as `Arc<dyn OneShotLlm>` so 412 swaps the real provider with zero change here (D5)
- [x] **D11:** the digest's chips are persisted on the Maestro side — written into the `kind='maestro'` chat's `chat_messages.content_json` envelope (`chat_messages::insert`), surviving past the suggestion buffer's ~60 s `DEDUP_TTL`; `maestro_state.last_digest_at` set after success
- [x] `crates/core/src/maestro/mod.rs` (modified): `pub mod digest;` added in the additive region + `Digest`/`WorkareaDelta`/`generate_digest` re-exported for 408/414
- [x] **<5 s p50** proven by a Criterion bench (or timed test) over `generate_digest` on the 6-workarea fixture with the deterministic summarizer; degraded-LLM path returns a typed `Digest` (R-7), never a macro
- [x] Tests (Tier 2): 6-workarea grouping, chip-persistence-survives-TTL, `compute_delta` table cases, `last_digest_at` set, degraded path — all green; `cargo test --workspace --no-fail-fast` passes
- [x] No new migration (highest on main is 0014; 0015/0016 are 403/410); no proto/event surface (414 owns those); `cargo deny` green (no new runtime crate)
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams return a typed `Err`/degraded `Digest`, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any schema/contract changed (expected: no `docs/interfaces/` delta — `crates/core/src/maestro/*` is not scanned)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/digest.rs` (new — `generate_digest`, `Digest`/`DigestEntry`/`WorkareaDelta`, `compute_delta`, `build_digest_prompt`, chip-persistence into the maestro chat, `last_digest_at` set)
- `crates/core/src/maestro/mod.rs` (modified — `pub mod digest;` in the additive region + re-exports of `Digest`/`WorkareaDelta`/`generate_digest`)
- `crates/core/benches/digest_latency.rs` (new — the <5 s p50 Criterion bench on the 6-workarea fixture; OR an in-module timed `#[test]` if a bench harness is not warranted — decide minimally and note)
- `crates/core/tests/maestro_digest.rs` (new — the 6-workarea grouping + chip-persistence-survives-TTL + degraded-path integration tests, if not co-located as `#[cfg(test)]` in `digest.rs`)
- `docs/interfaces/*` (regenerated — expected no delta; commit only if `regen-interfaces.sh` produces one)

## Commit message
```
phase-4: Maestro digest generation (<5 s p50) + chip persistence

Add generate_digest(): gather 404's active-workarea summaries, compute
deltas since last_seen, build the design/08 §3.6 templated prompt, run it
through OneShotLlm::suggest(DigestSummary) (DeterministicOneShot LIVE; the
real provider is 412), group Finished/Blocked/Working + a one-line next
step, and persist the digest's chips on the Maestro side (D11 — into the
kind='maestro' chat_messages row, surviving the ~60s suggestion buffer).
<5 s p50 proven by a Criterion bench on a 6-workarea fixture against the
deterministic summarizer. Real-LLM digest quality + latency stay Tier-3.

Refs: tasks/v1.0/409-digest-generation.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** — (record any divergence: e.g. the exact `WorkareaSummary` field names 404 froze vs. what §4.4 sketched; whether `generate_digest`'s signature took 404's cache as `&SummaryCache` vs an `Arc<...>` handle; whether the <5 s proof landed as a Criterion bench or a timed `#[test]`; whether `criterion` was already a dev-dep or needed adding (Stop-and-ask if a new crate); whether `chats(kind='maestro')` singleton id is looked up vs passed in; whether `docs/interfaces/` produced a delta).
- **Open questions for next task:** — **Task 414** consumes `generate_digest()` (the FROZEN `Digest`/`WorkareaDelta` surface above) to serve `Maestro.GetDigest` and publish `maestro.digest_generated` over `maestro.events`; confirm the `Digest` struct maps cleanly onto 401.5's `Digest` proto (text + chips + groups), and whether 414 wants `generate_digest` to also emit the event or leave that to the handler. **Task 412** swaps the injected `Arc<dyn OneShotLlm>` for the real provider — confirm the injection point is the only change needed (it should be). **Task 410** later adds `chat_messages.metadata`; note that 409's digest row stays a plain `assistant` message (not `daily_summary`), so 410 needs no rework here.
- **Deliberate debt:** — (e.g. chips carried in `content_json` rather than a `metadata` column because 410 has not landed 0016 yet — when it does, no migration of existing digest rows is needed; the degraded-LLM `Digest` returns a typed fallback line, not the macro — the explicit R-7/§8 contract).
- **Smoke-gate state:** — **Unchanged** (409 touches no `scripts/smoke.d/*` or `scripts/smoke.manifest`; the digest is CI-provable in-process via the deterministic summarizer + the 6-workarea fixture; the live `maestro-digest` smoke capability + real-LLM latency are 414's wire and the Phase-4 Tier-3 gate). Note whether `cargo deny` stayed green (it should — no new runtime crate; `criterion` is dev-only).
```
