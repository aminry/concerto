# Task 413 — Maestro privacy enforcement: `exclude_from_maestro` blanking + `concerto_chat_full_chat_access` + `enterpriseDataPrivacy`-disables-if-external (the privacy gate over 404's summary cache)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 404 |
| Touches subsystem(s) | 08 (Maestro), 12 (Security), 03 (Workspace Mgr) |
| Smoke gate | unchanged |

## Goal
Enforce the three Maestro privacy rules (`design/08 §3.3` / `§3.10`) over **404's `WorkareaSummary` cache** so that no privacy-restricted workarea content and no external-model egress can leave Concerto. Today the schema + settings substrate exist but **nothing reads them in a Maestro context**: `WorkareaManager::set_exclude_from_maestro` (`crates/core/src/workspace_manager/workarea.rs:3010`) persists the per-workarea toggle into `workareas.settings_json` via the `set_settings_json_key` RMW helper (`crates/persist/src/workareas.rs:547`) and the `Workarea` proto carries `exclude_from_maestro` field 11 (Task 311) — **but 311's own doc-comment says "the actual privacy *enforcement* … is Maestro Task 413"**; `WorkspaceSettingsResolver::enterprise_data_privacy() -> Resolved<bool>` is live (`crates/core/src/settings/resolver.rs:290`) but **read only by `handlers/vcs.rs` issue-fetch (and even there 411 fixes a hardcoded `false`)**, never by the Maestro; and **`concerto_chat_full_chat_access` does not exist at all** — there is no such key, no reader, no writer (`workspaces.rs` exposes only whole-blob `get_settings_json`/`set_settings_json` at `crates/persist/src/workspaces.rs:236/254`, with NO keyed RMW mirror of `workareas::set_settings_json_key`). This task adds `crates/core/src/maestro/privacy.rs` (new) defining a **FROZEN `PrivacyPolicy`** that drives three behaviors: **(a)** a `blank_excluded(summary, excluded) -> WorkareaSummary` hook that, for an `exclude_from_maestro` workarea, strips every LLM-derived field to name-only (`last_turn_summary = "[private workarea, name only]"`, empty `sessions`/`last_3_turn_summaries`) while leaving the **hard facts** (status, branch, repo names, `commits_ahead`/`files_changed`/`lines_*`/`pr_state`/`ci_state`) intact; **(b)** a new `concerto_chat_full_chat_access` **`workspaces.settings_json` bool key** (default `false`; **no migration, no proto field**) that, when `true`, lifts the cache's source from summary-only to the raw last-3-turns — added via a **net-new keyed RMW accessor `workspaces::set_settings_json_key`/`get_settings_json_bool`** mirroring `workareas::set_settings_json_key` (`design/08 §3.3`'s per-workspace full-chat opt-in); **(c)** a `MaestroExternalDisabled` gate — when `WorkspaceSettingsResolver::enterprise_data_privacy()` resolves `true` **and** the configured Maestro model is **external** (CLI/Direct-API to a non-on-prem provider), the Maestro LLM is **disabled** (`design/08 §3.10`) and the gate is checked **before any external summary/digest call** — while deterministic routing still works. The hook is invoked by 404's `summary.rs` (a privacy filter on the cache read path, owned here as `crates/core/src/maestro/summary.rs` is co-listed in this task's write-set). After this task, **405's `get_workarea_summary`**, **409's digest**, and **412's provider gate** all read privacy-filtered summaries and a single `PrivacyPolicy::maestro_disabled_by_policy()` decision; the `MaestroExternalModel` classification consumes **412's provider seam** (FROZEN by 402, extended by 412) and **404's `WorkareaSummary`** (FROZEN by 404, PHASE4_PLANNING §4.4) — neither is re-locked here. What stays Tier-3/out: the **real external-LLM data-egress check** (confirming a live Maestro talking to a real provider leaks only hard facts for an excluded workarea) is the Phase-4 Tier-3 gate line; this task proves the *policy logic* is correct, which is fully CI-provable.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md §4.4` — **AUTHORITATIVE**: `WorkareaSummary`/`SessionSummary`/`RepoSummary` + the cache shape are **FROZEN by 404 (D9)**; 413's privacy-blanking **consumes** these, never re-derives a different shape. The hard-fact set (status / branch / repo names / `commits_ahead` / `files_changed` / `lines_*` / `pr_state` / `ci_state`) is what survives blanking.
- `tasks/v1.0/PHASE4_PLANNING.md §1 D1 + D10` — **AUTHORITATIVE**: D1 = CLI-first; Direct-API is a FROZEN unwired seam ⇒ `enterpriseDataPrivacy=true` + an **external** model ⇒ **Maestro disabled** (`design/08 §3.10`); on-prem Direct-API is a Tier-3 + follow-on. D10 = 413 enforces the resolver **before ANY external summary/digest** and skips `exclude_from_maestro` workareas (blank to name-only); `concerto_chat_full_chat_access` is a **net-new `workspaces.settings_json` key** (no proto/migration) added by 413 using the `exclude_from_maestro` RMW-key precedent.
- `tasks/v1.0/PHASE4_PLANNING.md §2 (413 row)` + `§4.3` — **AUTHORITATIVE**: 413's three surfaces ((a) resolver gate before external summary/digest; (b) per-workarea `exclude_from_maestro` skip; (c) `concerto_chat_full_chat_access` new bool key, no migration, read via the settings resolver). §4.3 = the Maestro provider-selection seam is **FROZEN by 402, extended by 412**; 413 only *queries* whether the chosen model is external — it does not own the seam.
- `tasks/v1.0/PHASE4_PLANNING.md §3` — **AUTHORITATIVE**: 413 adds **no migration** (`concerto_chat_full_chat_access` is a JSON key). **Author check (do this first):** confirm the highest `crates/persist/migrations/NNNN_*.sql` on `main` is still `0014` (`0014_pull_requests_merge_order.sql`); 413 adds none, but if a Phase-4 migration (403's 0015 / 410's 0016) has already landed, that does not affect 413 — note it in Handoff anyway. The **CHECK-widen ban** is irrelevant here (no schema change).
- `tasks/v1.0/PHASE4_PLANNING.md §8.1 (413 write-set)` — 413 writes `crates/core/src/maestro/{privacy,summary}.rs`, `crates/core/src/settings/resolver.rs` (read), `crates/persist/src/workspaces.rs` (settings key). Hard seam shared with **404** (`summary.rs`) ⇒ serialize-after-404 (413 depends on 404).
- `design/08_Maestro_Agent.md §3.3` — the `WorkareaSummary` shape + the two privacy paragraphs verbatim: **"if `workareas.settings_json.exclude_from_maestro = true`, only the hard facts (status, branch, repo names) are exposed; summaries are blanked. The workarea shows up as `[private workarea, name only]`"** and **"`workspaces.settings_json.concerto_chat_full_chat_access = true` lifts the Maestro out of summary-only and grants it the raw last-3-turns of chat (per session). Off by default."** Transcribe the blank string + the default exactly.
- `design/08_Maestro_Agent.md §3.10` — **AUTHORITATIVE product rule**: `enterpriseDataPrivacy=true` + **external** model ⇒ Maestro disabled (tray/UI shows off-due-to-policy); + **on-prem** model (Bedrock-VPC / Vertex / Azure-Foundry / local) ⇒ works normally; **routing still works in all modes (deterministic, no LLM)**.
- `design/08_Maestro_Agent.md §10 (Testing strategy)` — the two **Privacy** behavioral rows are this task's Tier-1 spec: "`exclude_from_maestro` excludes from summaries" + "`enterpriseDataPrivacy` disables LLM but not routing" — both "Behavioral".
- `crates/core/src/settings/resolver.rs:290` — `pub fn enterprise_data_privacy(&self) -> Resolved<bool>` (managed `Some(b)` wins → checked-in → local DB → default `false`). This is the gate input; consume it as-is (you may add a *read-only* convenience getter for `concerto_chat_full_chat_access` here if the resolver is the natural home — see Implementation notes; do NOT change `enterprise_data_privacy`'s logic).
- `crates/persist/src/workareas.rs:547` — `pub async fn set_settings_json_key(conn: &mut SqliteConnection, id: &WorkareaId, key: &str, value: serde_json::Value) -> Result<()>` — the **non-clobbering read-modify-write precedent** (SELECT settings_json → parse object → set key → persist). Mirror its exact shape into `workspaces.rs` for the workspace-id keyed accessor.
- `crates/persist/src/workspaces.rs:236/254` — the existing whole-blob `get_settings_json(pool, id) -> Option<String>` / `set_settings_json(conn, id, payload)` (Task 302). Note there is **no keyed-RMW accessor here yet** — 413 adds `set_settings_json_key` + a typed bool reader, both `WorkspaceId`-keyed, mirroring `workareas.rs`.
- `crates/core/src/workspace_manager/workarea.rs:3010` — `set_exclude_from_maestro` (Task 311): the precedent that *writes* the per-workarea toggle. 413 is its consumer (reads the toggle from the summary's source workarea to decide blanking). Do NOT modify 311's writer.

## Scope — in
- **`crates/core/src/maestro/privacy.rs` (new):**
  - Define the FROZEN `PrivacyPolicy` struct + its three pure decision methods (see Public interface). It is a **pure policy object** constructed from the resolved inputs (the `enterprise_data_privacy` bool, the chosen model's externality, the per-workarea `exclude_from_maestro` flag, the per-workspace `concerto_chat_full_chat_access` flag) — **no I/O inside `privacy.rs`** (callers resolve the inputs; the policy decides). This keeps it table-test-driven (`design/08 §10`).
  - `blank_excluded(summary: WorkareaSummary, excluded: bool) -> WorkareaSummary`: if `!excluded`, return unchanged; if `excluded`, return a copy with every **LLM-derived / chat-derived** field stripped — `last_turn_summary = "[private workarea, name only]"` (the exact string from `design/08 §3.3`), `last_3_turn_summaries = vec![]`, and each `SessionSummary.last_turn_summary` cleared (or `sessions` emptied — **decide and document; emptying `sessions` is the stronger guarantee since a `SessionSummary` carries no hard facts the UI needs for a private workarea**) — while **preserving every hard fact**: `workarea_id`, `workspace_id`, `workspace_name`, `composer_name`, `branch_name`, `status`, `last_activity_at`, `repos` (the whole `Vec<RepoSummary>`), `blocked_on`, `generated_at`, `generation`. The invariant: **after blanking, the summary carries zero LLM/chat-derived text and every git/PR/CI hard fact.**
  - `SummarySource` enum `{ SummaryOnly, FullLast3Turns }` + `summary_source(full_chat_access: bool) -> SummarySource` (`true` ⇒ `FullLast3Turns`, default `false` ⇒ `SummaryOnly`). This is the flip 404's refresher/reader honors when deciding whether to populate raw last-3-turns vs. summary text.
  - `MaestroExternalModel` classification: a small enum or bool input `is_external_model` the gate consumes. **413 does not invent the provider seam** — it takes "is the configured Maestro model external?" as a constructed input (see Implementation notes for who computes it: the boot/provider wiring derived from 412's `MaestroProvider`; for 413 itself, take a `bool`/typed enum). `maestro_disabled_by_policy(enterprise_data_privacy: bool, is_external_model: bool) -> bool` returns `true` **iff** both are `true` (`design/08 §3.10`). Routing is unaffected by this method (it gates only the LLM/external paths).
  - A single combined `enum MaestroLlmGate { Allowed, DisabledExternalPolicy }` (or equivalent) the digest/summarizer call sites check **before** issuing any external summary or digest LLM call; deterministic routing/tool paths NEVER consult it.
- **`crates/core/src/maestro/summary.rs` (modified — privacy hook, co-owned with 404):**
  - On the cache **read path** (the path 405's `get_workarea_summary` and 409's digest pull from), apply `PrivacyPolicy::blank_excluded` using the source workarea's `exclude_from_maestro` flag (read from `workareas.settings_json` via the existing 311 storage — the manager-side getter, NOT a new SQL read here if one already exists; resolve the flag from the same place 404 already knows the workarea). Blanking is applied at **read/serve time**, not at refresh time, so toggling `exclude_from_maestro` takes effect on the next read without a cache rebuild — **document this choice** (alternative: blank at refresh; read-time blanking is safer because it cannot be defeated by a stale cache entry written before the toggle flipped).
  - On the **refresh path**, consult `summary_source(full_chat_access)` to decide whether the cache entry populates raw last-3-turns (`FullLast3Turns`) or summary text only (`SummaryOnly`). Default `SummaryOnly`.
  - **Gate external summarization:** before the summarizer issues any external `OneShotLlm`/provider call (the real-LLM path; the `DeterministicOneShot` local path is NOT external and is NOT gated), check `maestro_disabled_by_policy`; if disabled, **do not call** — fall back to the deterministic/hard-fact-only summary (the workarea still shows hard facts + a "[maestro disabled by policy]"-class marker, exact wording your call, documented).
- **`crates/persist/src/workspaces.rs` (modified — `concerto_chat_full_chat_access` settings key):**
  - Add `pub async fn set_settings_json_key(conn: &mut SqliteConnection, id: &WorkspaceId, key: &str, value: serde_json::Value) -> Result<()>` — a **verbatim mirror of `workareas::set_settings_json_key`** (SELECT `settings_json` → parse object (or `{}`) → set key → re-serialize → `UPDATE`), so the write is non-clobbering of `permission_mode` and other keys.
  - Add `pub async fn get_settings_json_bool(pool: &SqlitePool, id: &WorkspaceId, key: &str) -> Result<Option<bool>>` (or a `concerto_chat_full_chat_access`-specific typed reader) returning `None` when the row/key is absent (caller defaults to `false`). Keep this layer dumb storage (the doc-comment convention in `workspaces.rs`).
  - **Do NOT** add a column, a migration, or a proto field. **Do NOT** widen any CHECK.
- **Settings resolver read (`crates/core/src/settings/resolver.rs`):** optionally add a *read-only* convenience getter `concerto_chat_full_chat_access(&self) -> Resolved<bool>` mirroring `enterprise_data_privacy()`'s shape **if** the resolver already loads the workspace `settings_json` blob — otherwise read the bool via the persist accessor at the call site and keep the resolver untouched. **Do not alter `enterprise_data_privacy()`.** Decide minimally and document which path you took.
- **`crates/core/src/maestro/mod.rs` (modified — soft seam):** add `pub mod privacy;` in the distinct region 401 reserved for later-task module declarations (additive, auto-merges on rebase).
- Tests (Tier 1): table-driven behavioral property tests in `privacy.rs` (and a summary-hook integration test in `summary.rs`): **(1)** excluded workarea — `blank_excluded(summary, true)` leaks ONLY hard facts: assert `last_turn_summary == "[private workarea, name only]"`, `last_3_turn_summaries.is_empty()`, `sessions` emptied/cleared, AND `repos`/`status`/`branch_name`/`commits_ahead`/`files_changed`/`pr_state`/`ci_state` all unchanged (a positive assertion on each hard fact, not just the negative). **(2)** `blank_excluded(summary, false)` is the identity (round-trips unchanged). **(3)** `maestro_disabled_by_policy`: truth table — `(priv=true, external=true) ⇒ true`; the other three combos ⇒ `false`; and a property assert that **routing/tool decisions never consult the gate** (assert the routing/tool path compiles & runs with `MaestroLlmGate::DisabledExternalPolicy` set — i.e. the gate is only on the LLM call site). **(4)** `summary_source`: `full_chat_access=true ⇒ FullLast3Turns`, `false/absent ⇒ SummaryOnly`. **(5)** persist round-trip: `workspaces::set_settings_json_key(conn, ws, "concerto_chat_full_chat_access", true)` then `get_settings_json_bool` returns `Some(true)`; setting an unrelated key (`permission_mode`) first and then the access key preserves both (non-clobber assertion); absent key ⇒ `None`.

## Scope — out
- **The `WorkareaSummary`/`SessionSummary`/`RepoSummary` shapes + the cache refresh triggers + `commits_ahead` helper** — owned by **Task 404** (PHASE4_PLANNING §4.4); 413 consumes them frozen and adds only a read-time filter + a refresh-source toggle. This leaves the cache structure entirely 404's.
- **The Maestro provider-selection seam (which CLI/model, on-prem vs external classification source)** — FROZEN by **402**, extended by **412** (PHASE4_PLANNING §4.3). 413 takes "is the chosen model external?" as a constructed input; **412** is who derives that bool from the live `MaestroProvider` + `ManagedPolicy::default_model()`. This leaves a `bool`/enum seam 412 fills.
- **Wiring the disabled-by-policy state onto the wire / events** — `maestro.disabled_by_policy` event is **Task 414** (publishes `maestro.events`); 413 only computes the `bool`. This leaves the event publication to 414.
- **The desktop banner ("Maestro budget exhausted / disabled by policy")** — **Task 415** renders it against 401.5's frozen proto. 413 provides the policy decision, not the UI.
- **The `handlers/vcs.rs` `enterprise_data_privacy=false` hardcode fix** — **Task 411** (D10) replaces the hardcoded `false` in `FetchIssueByUrl` with the resolved value. 413 enforces the resolver in the *Maestro summary/digest* path only; it does NOT touch `handlers/vcs.rs`.
- **The per-workarea write API (`set_exclude_from_maestro`) + the proto field 11** — **Task 311** (live); 413 reads the toggle, never re-writes it or re-locks the field.
- **Daily token budget / inert-on-exhaust** — **Task 412** (budget) / **403** (`maestro_state`). The `enterpriseDataPrivacy` disable is a *distinct* inert path from budget-exhaust; do not conflate. 413 owns only the privacy disable.
- **Real-world Tier-3:** with a live Maestro pointed at a real external provider, leave an `exclude_from_maestro` workarea active, ask the Maestro about it, and **confirm the response leaks only hard facts (no chat/summary text)** — and confirm `enterpriseDataPrivacy=true` + an external model disables the LLM while `@workarea` routing still fires. This is the Phase-4 Tier-3 checklist line ("confirm an excluded workarea leaks only hard facts"); CI cannot exercise a real provider, so it is deferred to the operator at the phase gate.

## Public interface this task locks
**Consumes `WorkareaSummary`/`SessionSummary`/`RepoSummary` as frozen by Task 404 (PHASE4_PLANNING §4.4)** — not re-locked here. **Consumes the Maestro provider model-externality classification as frozen by Task 402 / extended by Task 412 (PHASE4_PLANNING §4.3)** — 413 takes it as a `bool`/typed input. **Consumes `WorkspaceSettingsResolver::enterprise_data_privacy()` as-is (`crates/core/src/settings/resolver.rs:290`)** — read-only, logic unchanged.

- **Rust (FROZEN, design/08 §3.3 / §3.10 / PHASE4_PLANNING §4.4 + D10), `crates/core/src/maestro/privacy.rs`:**

```rust
/// Pure Maestro privacy policy (no I/O). Callers resolve the inputs; this
/// object decides. design/08 §3.3 + §3.10; PHASE4_PLANNING §2 (413) / D10.
pub struct PrivacyPolicy;

/// Which raw-content source the summary cache serves to the Maestro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummarySource {
    /// Default: summaries only (no raw chat). design/08 §3.3.
    SummaryOnly,
    /// `concerto_chat_full_chat_access = true`: raw last-3-turns per session.
    FullLast3Turns,
}

/// Whether the Maestro LLM may run, given the enterprise-privacy gate.
/// Deterministic routing/tools NEVER consult this. design/08 §3.10.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaestroLlmGate {
    Allowed,
    /// enterpriseDataPrivacy=true AND the chosen Maestro model is external.
    DisabledExternalPolicy,
}

impl PrivacyPolicy {
    /// Blank an `exclude_from_maestro` workarea's summary to name-only:
    /// strips every LLM/chat-derived field, preserves every hard fact.
    /// `excluded == false` ⇒ identity. The blanked `last_turn_summary` is
    /// exactly `"[private workarea, name only]"` (design/08 §3.3). FROZEN.
    pub fn blank_excluded(summary: WorkareaSummary, excluded: bool) -> WorkareaSummary;

    /// The cache source the Maestro is granted for a workspace.
    /// `full_chat_access` defaults to `false` ⇒ `SummaryOnly`. FROZEN.
    pub fn summary_source(full_chat_access: bool) -> SummarySource;

    /// `true` iff `enterprise_data_privacy && is_external_model` (design/08
    /// §3.10). The single disable decision; routing is unaffected. FROZEN.
    pub fn maestro_disabled_by_policy(enterprise_data_privacy: bool, is_external_model: bool) -> bool;

    /// The LLM gate the digest/summarizer checks BEFORE any external call.
    pub fn llm_gate(enterprise_data_privacy: bool, is_external_model: bool) -> MaestroLlmGate;
}

/// The exact name-only blank string (design/08 §3.3) — FROZEN.
pub const PRIVATE_WORKAREA_BLANK: &str = "[private workarea, name only]";
```

- **Rust (FROZEN, dumb-storage accessors), `crates/persist/src/workspaces.rs`** — mirrors `workareas::set_settings_json_key` (`crates/persist/src/workareas.rs:547`), keyed by `WorkspaceId`:

```rust
/// Non-clobbering read-modify-write of one `workspaces.settings_json` key
/// (mirror of `workareas::set_settings_json_key`). 413 uses it for the new
/// `concerto_chat_full_chat_access` bool (no column, no migration). FROZEN.
pub async fn set_settings_json_key(
    conn: &mut SqliteConnection,
    id: &WorkspaceId,
    key: &str,
    value: serde_json::Value,
) -> Result<()>;

/// Read one `workspaces.settings_json` bool key. `None` ⇒ row/key absent
/// (caller defaults to `false`, e.g. `concerto_chat_full_chat_access`). FROZEN.
pub async fn get_settings_json_bool(
    pool: &SqlitePool,
    id: &WorkspaceId,
    key: &str,
) -> Result<Option<bool>>;
```

- **The `workspaces.settings_json` key (FROZEN, no proto/migration), `design/08 §3.3`:** `concerto_chat_full_chat_access` — a JSON bool, **default `false`**, written/read via the two accessors above. It is **not** a proto field and **not** a column; it never appears in a migration. This is the `exclude_from_maestro` derived-key precedent (`workareas.settings_json`) applied at the workspace grain.

## Implementation notes
- **The load-bearing rule: blanking happens at READ/SERVE time, gated external calls happen at CALL time, and routing never touches either.** Three separate enforcement points, one shared pure `PrivacyPolicy`. `blank_excluded` runs on every summary the cache *serves* (so a freshly-flipped `exclude_from_maestro` is honored without a cache rebuild — a stale pre-toggle cache entry can NOT leak). `maestro_disabled_by_policy` is checked at the *external LLM call site* in `summary.rs` (and consumed identically by 409's digest + 412's provider) **before** the call — never after. The deterministic routing/tool path (408) must compile and run with the gate `DisabledExternalPolicy` — assert this in tests so a future refactor can't accidentally gate routing.
- **Reuse, don't reinvent the RMW.** `workspaces::set_settings_json_key` is a line-for-line mirror of `workareas::set_settings_json_key` (`workareas.rs:547`): SELECT the blob → `serde_json::from_str::<Value>` (or `{}` on absent/parse-fail) → set the key → re-serialize → `UPDATE`. Do NOT introduce SQLite `json_set()` (the codebase parses-in-Rust; stay consistent). Add the table to nothing new — there is no schema change, so `crates/persist/tests/initial_schema.rs` is **untouched**.
- **`is_external_model` is an input, not a computation 413 owns.** 413 must NOT reach into 412's `MaestroProvider` or re-derive provider externality — that is 402/412's seam (PHASE4_PLANNING §4.3). Define a tiny typed input (a `bool`, or a `MaestroModelLocality { External, OnPrem }` enum if clearer) and let the boot/provider wiring (412) supply it. For 413's own tests, pass the input directly. **On-prem** = Bedrock-VPC / Vertex / Azure-Foundry / local (`design/08 §3.10`); **external** = Anthropic/OpenAI public API or a CLI dialing a public provider. Document that in V1.0 (D1: Direct-API unwired) the *live* backends are the CLIs, so the practical external case is "a CLI configured against a public provider" — the on-prem-Direct-API path that would re-enable Maestro under `enterpriseDataPrivacy` is the Tier-3 + follow-on.
- **The deterministic summarizer is NOT external.** `DeterministicOneShot` (the live P4 fallback, PHASE4_PLANNING §4.5) runs in-process and egresses nothing — it must remain available when `maestro_disabled_by_policy` is `true` so hard-fact summaries still render. Only the *external* `OneShotLlm`/provider path is gated. Make the gate guard exactly the external call, not the whole summarizer.
- **Cross-platform:** this task is pure policy + a SQLite accessor — **no `#[cfg(unix)]` gate needed** (no agent-supervisor / PTY / stream surface touched). `privacy.rs` has no platform-specific code; it compiles identically on the Windows/Linux CI lanes (Task 113).
- **No gRPC surface added** ⇒ no two-site registration, no proto edit. The `disabled_by_policy` *event* is 414; the *bool* is 413.
- **Regen:** no `*.proto`, no SQL migration, and the new `pub` Rust API is `pub fn`/`pub async fn` (free fns + impl methods + a const) — `regen-interfaces.sh` captures `struct`/`enum`/`type` defs from `crates/*/src/api.rs`, not free fns and not `src/maestro/*` / `src/settings/*` modules (the established behavior Task 305 documented). The `SummarySource`/`MaestroLlmGate` enums live in `maestro/privacy.rs`, not `api.rs`, so `docs/interfaces/rust-api.md` is **unchanged**. Still run `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` to prove zero drift; commit only if it does change.
- **Parallel build hint:** the three sub-parts are file-disjoint and can fan out (per the DAG `fanout`): **(1)** `maestro/privacy.rs` `blank_excluded` + `summary_source` + the persist `concerto_chat_full_chat_access` accessors in `workspaces.rs` + their round-trip tests; **(2)** the `enterpriseDataPrivacy`-disables-if-external gate (`maestro_disabled_by_policy`/`llm_gate` + the `summary.rs` external-call guard + the resolver read); **(3)** the privacy property tests (the `design/08 §10` behavioral table). Integrate into one commit; the only shared file is `maestro/summary.rs` (the read-hook + the call-gate touch it — keep those two edits in disjoint fns to merge cleanly).

## Verification
**Tier 1.** The `rust` §5.3 command set; this task adds **no** smoke capability and **no** proto/migration.
1. `cargo check --workspace` → clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` → clean.
3. `cargo fmt --all -- --check` → clean.
4. `cargo test -p concerto-core maestro::privacy` (+ `privacy`/`summary` filters) → proves: (1) excluded-workarea blanking leaks ONLY hard facts (positive assert on each hard field; `last_turn_summary == "[private workarea, name only]"`; `last_3_turn_summaries` + `sessions` empty); (2) `blank_excluded(_, false)` identity; (3) `maestro_disabled_by_policy` truth table `(true,true)⇒true` else `false`; (4) routing/tool path runs with the gate `DisabledExternalPolicy` set (routing NOT gated); (5) `summary_source` flip.
5. `cargo test -p concerto-persist workspaces` (settings-key filter) → `set_settings_json_key`/`get_settings_json_bool` round-trip `Some(true)`, non-clobber of `permission_mode`, absent-key ⇒ `None`.
6. `cargo test --workspace --no-fail-fast` → all pass.
7. `cargo deny check` → green (no new crates; `serde_json` already a dep).
8. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → **no diff** (no proto/migration; the new Rust API is free fns + non-`api.rs` enums, which the regen does not capture — per Task 305's documented behavior). Commit nothing under `docs/interfaces/` unless a diff appears.
9. `scripts/smoke.sh` → **unchanged** (413 touches no smoke capability).

**Tier-1 scope + what it does NOT cover.** The three privacy behaviors are **deterministic policy logic** and fully CI-provable: blanking is a pure transform asserted field-by-field; the disable decision is a 4-row truth table; the `full_chat_access` flip + the persist round-trip are exact. **It does NOT cover** the real-world data-egress check — a live Maestro talking to a real external provider must be observed to leak only hard facts for an `exclude_from_maestro` workarea, and `enterpriseDataPrivacy=true`+external must be observed to disable the LLM while `@workarea` routing still fires. That is the **Phase-4 Tier-3 checklist line: "confirm an excluded workarea leaks only hard facts"** (and the budget/disable-inert companion line), verified by the operator at the phase gate. No new Tier-3 line is *added* by this task beyond the one already in the Phase-4 manual checklist.

## Definition of Done
- [x] `crates/core/src/maestro/privacy.rs` (new): `PrivacyPolicy` + `blank_excluded` (name-only blanking preserving all hard facts; exact `"[private workarea, name only]"`), `summary_source`/`SummarySource`, `maestro_disabled_by_policy`/`llm_gate`/`MaestroLlmGate`, `PRIVATE_WORKAREA_BLANK` const — all FROZEN per design/08 §3.3/§3.10
- [x] `crates/core/src/maestro/summary.rs` (modified): read-time blanking hook + refresh-source toggle + external-LLM-call gate (deterministic fallback when disabled)
- [x] `crates/persist/src/workspaces.rs` (modified): `set_settings_json_key` + `get_settings_json_bool` (`WorkspaceId`-keyed, mirroring `workareas::set_settings_json_key`); `concerto_chat_full_chat_access` is a JSON bool key, default `false`, **no column/migration/proto field/CHECK-widen**
- [x] `crates/core/src/maestro/mod.rs` (modified): `pub mod privacy;` in the later-task region
- [x] Tests (Tier 1): excluded-leaks-only-hard-facts, blank identity, disable truth-table + routing-not-gated, `summary_source` flip, persist round-trip + non-clobber + absent-key
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (there are no signature-frozen unwired seams in 413 — the policy is fully implemented; any deliberate debt is documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any schema/contract changed (expected: **no** `docs/interfaces/` diff — no proto/migration; verify with `git diff --exit-code`)
- [x] All Verification commands pass on a clean checkout; smoke unchanged
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/privacy.rs` (new — `PrivacyPolicy`, `blank_excluded`, `SummarySource`/`summary_source`, `MaestroLlmGate`/`maestro_disabled_by_policy`/`llm_gate`, `PRIVATE_WORKAREA_BLANK`, the property tests)
- `crates/core/src/maestro/summary.rs` (modified — read-time `blank_excluded` hook, refresh-source toggle, external-LLM-call gate with deterministic fallback)
- `crates/core/src/maestro/mod.rs` (modified — `pub mod privacy;`)
- `crates/persist/src/workspaces.rs` (modified — `set_settings_json_key` + `get_settings_json_bool` for the `concerto_chat_full_chat_access` key + round-trip tests)
- `crates/core/src/settings/resolver.rs` (modified — optional read-only `concerto_chat_full_chat_access` getter, ONLY if the resolver already loads the workspace blob; otherwise untouched)

## Commit message
```
phase-4: Maestro privacy enforcement over the summary cache

Adds maestro/privacy.rs (PrivacyPolicy): blank exclude_from_maestro
workareas to name-only ("[private workarea, name only]") preserving all
hard facts, the concerto_chat_full_chat_access workspaces.settings_json
bool (default false, no migration; RMW key mirroring workareas), and the
enterpriseDataPrivacy+external => Maestro-disabled LLM gate (routing
unaffected). Gates external summary/digest before the call; deterministic
fallback stays live. Real external-LLM egress is the Phase-4 Tier-3 gate.

Refs: tasks/v1.0/413-privacy-enforcement.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** — (e.g. whether `is_external_model` landed as a `bool` or a `MaestroModelLocality` enum; whether `blank_excluded` empties `sessions` vs. clears each `SessionSummary.last_turn_summary`; whether the `concerto_chat_full_chat_access` reader lives in the resolver or only in the persist accessor; whether `summary.rs` already had a read/serve seam to hook or one had to be introduced; confirm the migration high-water on `main` was 0014 — note any shift if 403/410 landed first).
- **Open questions for next task:** — **Task 405** (`get_workarea_summary`) and **Task 409** (digest) consume the read-time-blanked `WorkareaSummary` (FROZEN by 404, §4.4) and must call the privacy hook on the serve path; **Task 412** consumes `maestro_disabled_by_policy`/`llm_gate` to gate its external provider + supplies the `is_external_model` input from the live `MaestroProvider`; **Task 414** publishes the `maestro.disabled_by_policy` event from the `MaestroLlmGate::DisabledExternalPolicy` decision; **Task 415** renders the disabled banner. The FROZEN surfaces they build on: `PrivacyPolicy::{blank_excluded,summary_source,maestro_disabled_by_policy,llm_gate}`, `PRIVATE_WORKAREA_BLANK`, and `workspaces::{set_settings_json_key,get_settings_json_bool}` + the `concerto_chat_full_chat_access` key.
- **Deliberate debt:** — (expected NONE; the policy is fully implemented. If the `is_external_model` input is hardwired to a constant pending 412's provider wiring, document it here as a typed seam, NOT a `todo!()`.)
- **Smoke-gate state:** — **Unchanged** (no `scripts/smoke.d/*` / `scripts/smoke.manifest` change; the privacy policy is CI-provable in-process; real external-LLM egress is the Phase-4 Tier-3 gate line). Confirm `cargo deny check` stayed green (no new crates).
