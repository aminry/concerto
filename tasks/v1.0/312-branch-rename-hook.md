# Task 312 — Branch-Rename Hook + the `OneShotLlm` Seam (`compose_action_prompt`, deterministic LIVE)

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 307, 310 |
| Touches subsystem(s) | 03 (Workspace/Session Mgr), 04 (Agent Supervisor — `action_prefs`/one-shot seam), 02 (gix-wrap — `git branch -m`) |
| Smoke gate | unchanged |

## Goal
Make a workarea's branch name follow the work: when the first user message arrives in any session of a workarea, propose a branch name from the prompt, and on confirm rename **every repo's worktree branch in the workarea** via `git branch -m`. Today a workarea's branch is the static `concerto/<composer>` set at create-time (`crates/core/src/workspace_manager/workarea.rs`), there is no rename path, `gix-wrap` exposes only `worktree_add` (no `git branch -m` wrapper), and — critically — **there is no one-shot LLM call path in the codebase at all** (no Maestro/provider crate until Phase 4). This task therefore does two things. First (the **owned, frozen seam** per `PHASE3_PLANNING §4.4`): it creates `crates/core/src/llm/oneshot.rs` with the `OneShotLlm` trait, a **live `DeterministicOneShot` impl** (slug-from-prompt for branch names; template title/body for PRs — the latter consumed by Task 321), and the `compose_action_prompt(action, prefs, context)` helper that reads Task 310's resolved `action_prefs`. Per **D1**, the deterministic impl is the **LIVE path in Phase 3**; the pluggable real-LLM provider is an unwired trait seam supplied in P4 (Task 412). Second (the feature): `WorkareaManager::suggest_workarea_branch_name(id)` (calls `OneShotLlm` with the `branch_rename` action + the first-message prompt) and `rename_workarea_branch(id, new)` (per repo: `git branch -m <old> <new>`, update `workareas.branch_name`, skip + warn any repo whose branch already exists on remote with different content → suffix `-N` per `design/03 §8`), plus a new `gix-wrap` `rename_branch` shell-out and a `WorkareaEvent::BranchRenamed`. After this task the branch-rename UX works end-to-end on the deterministic path, and 321 reuses the exact same `OneShotLlm` + `compose_action_prompt` for PR title/body with **no new LLM machinery**.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.6 — the branch-rename spec: first-user-message trigger → one-shot suggestion → confirm → `git branch -m <old> <new>` **per repo in the workarea** → update `workareas.branch_name`; a repo whose branch already exists on remote with a different name is **skipped + the user warned**. §7.2 — the sequence diagram (`WSM → Sup: name_suggestion_call(prompt, model=haiku)` → `branch_rename_proposed` → user accept → `rename_workarea_branch` → loop per repo `git branch -m` → `workarea.events: branch.renamed`). §8 — the remote-conflict skip + the `-N` suffix rule. Reproduce the cross-repo apply + skip-and-warn faithfully.
- `design/04_Agent_Supervisor.md` §3.13 — **the `compose_action_prompt` contract you OWN the implementation of**: the design signature is `compose_action_prompt(action: ActionKind, repo_id: RepositoryId, base_prompt: &str) -> String`; the seven `ActionKind`s (`code_review`/`pr_create`/`error_fix`/`conflict_resolve`/`branch_rename`/`commit_message`/`digest_summary`); the per-action prefs source is **`repositories.action_prefs_json` (+ checked-in `action_prefs.toml`) resolved by Task 310's resolver** — your helper reads the resolved `action_prefs.<action>` value and injects it into the prompt. The `branch_rename` action's pref ("kebab-case with the Linear ticket prefix when one exists") is the one this task exercises. §2 (V1.0 row) — "agent name-suggestion mode (one-shot, called by 03 for branch rename)": the design assumes the Agent Supervisor grows this mode; **D1 says it is NOT wired to a real LLM in P3** — the deterministic impl is the live path.
- `tasks/v1.0/PHASE3_PLANNING.md` §1 **D1** (deterministic fallback is the LIVE P3 path; the live-LLM path is wired in P4/412 and judged at that gate — state this verbatim in Verification), §2 (row 312/321: "**312 owns** `crates/core/src/llm/oneshot.rs`: the `OneShotLlm` trait + a `DeterministicOneShot` impl (LIVE) + `compose_action_prompt` (reads 310's resolved `action_prefs`). **321 reuses** it"), §4.4 (**the FROZEN seam shape** — transcribe it: `trait OneShotLlm { async fn suggest(&self, req: OneShotRequest) -> Result<String> }` + `DeterministicOneShot` (slug-from-prompt for branch names, template title/body for PRs) + `compose_action_prompt(action, prefs, context)`), §6 (312 also depends on **310**).
- `tasks/v1.0/310-settings-precedence-resolver.md` → "Handoff Notes" — the `ProjectSettingsResolver::action_pref(repo_id, action)` getter + the migration-0011 `repositories.action_prefs_json` your `compose_action_prompt` reads. **310 is a hard dependency** (the resolved-prefs source); do not start until its handoff is readable. If 310's `action_pref` getter signature differs, follow the handoff.
- `tasks/v1.0/307-parallel-workareas-fsm.md` → "Handoff Notes" — the full workarea FSM + the multi-repo workarea shape (the per-repo worktrees `rename_workarea_branch` loops over). 307 is a hard dependency.
- `crates/core/src/workspace_manager/workarea.rs` — the `WorkareaManager` handle (holds `persistence`, `repo_manager`, broadcasts `WorkareaEvent`); branch is static `concerto/<composer>` today (no rename path). `WorkareaEvent` (line ~81) is `Created`/`Archived`/`Restored` — you APPEND `BranchRenamed`. `list_workarea_repos`/the per-repo worktree paths (persist) give you the repos to loop over.
- `crates/gix-wrap/src/api.rs` + `crates/gix-wrap/src/cmd.rs` — `worktree_add` (the shell-out pattern: `cmd::run(&["worktree", "add", …], cwd)`); there is **no `git branch -m` wrapper** — you add `rename_branch(repo_dir, old, new)` shelling out `git branch -m <old> <new>` via `cmd::run` (mirror `worktree_add`'s structure). `list_branches` gives the remote-ref check for the skip-on-conflict rule.
- `crates/core/src/audit/event.rs` — `AuditKind` enum + `as_str`; you ADD `BranchRenamed` (wire `branch_renamed`) and, if you record pref injection, `ActionPrefInjected` (wire `action_pref_injected`, per `design/04 §3.13`). Mirror the `WorkareaRestored` precedent.
- `crates/core/Cargo.toml` — `async-trait` is already a workspace dep (used by 218's `CoreClient`); reuse it for `OneShotLlm`. No new heavy dep.

## Scope — in
**The owned `OneShotLlm` seam (`crates/core/src/llm/oneshot.rs`, FROZEN per `PHASE3_PLANNING §4.4`):**
- `trait OneShotLlm { async fn suggest(&self, req: OneShotRequest) -> Result<String>; }` (`#[async_trait]`).
- `OneShotRequest` — the input: the `ActionKind` (at least `BranchRename` + `PrCreate` for 321; include the full seven-action enum from `design/04 §3.13` so 321/605 reuse it), the `repo_id`, the composed prompt (after `compose_action_prompt`), and any context the deterministic impl needs (the first-message text for branch rename; the diff/commit context for PR — 321 fills that).
- `DeterministicOneShot` — the **LIVE P3 impl**: for `BranchRename`, produce a kebab-case slug from the prompt (lowercase, strip non-alphanumerics, collapse dashes, bounded length, honor the `branch_rename` pref if it asks for a ticket prefix — best-effort, deterministic); for `PrCreate`, a template title/body (so 321 has a working fallback). Pure, no I/O, no network — fully CI-provable.
- `compose_action_prompt(action: ActionKind, prefs: &ResolvedActionPrefs, context: &str) -> String` — reads Task 310's resolved `action_prefs.<action>` (passed in, or looked up via the resolver handle — decide the exact param shape to match 310's getter and FREEZE it) and prepends/injects it per the §3.13 injection table. Records `ActionPrefInjected{action, repo_id, pref_hash, tokens_added}` per call (audit) if you wire the audit; otherwise leave the audit to a follow-on and note it.
- The **real pluggable provider is UNWIRED** — leave a `OneShotLlm`-shaped seam (e.g. the manager holds `Arc<dyn OneShotLlm>` defaulting to `DeterministicOneShot`) that P4/412 swaps. No provider, no CLI shell-out, no network here.

**The branch-rename feature (`workspace_manager` + `gix-wrap`):**
- `gix-wrap::rename_branch(repo_dir, old, new) -> Result<()>` — `git branch -m <old> <new>` via `cmd::run`, mirroring `worktree_add`.
- `WorkareaManager::suggest_workarea_branch_name(&self, id) -> Result<String>` — resolve the first repo's `action_prefs` (via 310) → `compose_action_prompt(BranchRename, prefs, first_message)` → `OneShotLlm::suggest` → the proposed name. Wired to fire on the **first user message** of any session in the workarea (the trigger lives at the message ingress; if the clean trigger seam doesn't exist yet, expose `suggest_workarea_branch_name` as the callable + note the ingress-wiring seam in Handoff — mirror how design/spike tasks surface an un-wired-but-callable seam).
- `WorkareaManager::rename_workarea_branch(&self, id, new) -> Result<RenameReport>` — for **each repo** in the workarea: check whether `new` already exists on that repo's remote with different content (via `list_branches`); if so **skip that repo + warn**, renaming its target to `<new>-N` per `design/03 §8`; else `git branch -m <old> <new>`. Update `workareas.branch_name`. Emit `WorkareaEvent::BranchRenamed`. A per-repo failure must **not abort the others** (partial success is fine; the report names the skipped repos).
- `AuditKind::BranchRenamed` (+ `ActionPrefInjected` if wired) emitted appropriately.
- Tests (Tier 1): the deterministic slug is stable + kebab-case for a sample prompt; `compose_action_prompt` injects the `branch_rename` pref from a resolved `action_prefs`; `rename_workarea_branch` renames every repo's branch in a multi-repo fixture; **a repo whose remote already has `new` is skipped + suffixed `-N` while the other repos rename** (partial-success path); `workareas.branch_name` updates; `BranchRenamed` event fires.

## Scope — out
- **The real pluggable LLM provider** (Claude/Codex/Gemini CLI + Direct API) — **Task 412** (Phase 4) supplies it behind the `OneShotLlm` seam; this task ships only `DeterministicOneShot` as the live path (D1).
- **PR title/body composition** — **Task 321** reuses this task's `OneShotLlm` + `compose_action_prompt` (+ the `PrCreate` deterministic template) and adds no new LLM machinery. This task provides the seam + the `PrCreate` deterministic fallback; 321 wires it into the PR flow.
- **`suggest_cones`** — a *separate* Maestro-delegate seam (Task 305), also unwired; do not conflate it with `OneShotLlm`.
- **Per-repo branch override** (`branch_override` per repo) — V2.0 (`design/03 R-1`); in V1.0 every repo in a workarea shares one `branch_name`.
- A gRPC RPC surface for the rename — if the Desktop needs `SuggestWorkareaBranchName`/`RenameWorkareaBranch` RPCs, a later Desktop/UI task appends them; this task ships the manager methods (the design routes the trigger through the Local API message ingress, not a dedicated RPC). Decide in-task whether a thin RPC is warranted; if added, append it to `Workareas` (do not renumber).
- The Settings/Repository-Settings UI for `action_prefs` — Desktop tasks; this task only reads the resolved value.

## Public interface this task locks
- **Rust `OneShotLlm` seam (FROZEN, `crates/core/src/llm/oneshot.rs`, per `PHASE3_PLANNING §4.4`):** `trait OneShotLlm { async fn suggest(&self, req: OneShotRequest) -> Result<String>; }` + `OneShotRequest` (carrying `ActionKind` + `repo_id` + composed prompt/context) + `ActionKind` (the seven `design/04 §3.13` actions) + `DeterministicOneShot` (the LIVE impl) + `compose_action_prompt(action, prefs, context) -> String`. **321 consumes these names; the real provider (412) implements the trait.** Freeze the trait method set + `ActionKind` variants exactly.
- **Rust (FROZEN):** `WorkareaManager::suggest_workarea_branch_name(id) -> String` and `rename_workarea_branch(id, new) -> RenameReport` (the report names renamed + skipped repos). `gix-wrap::rename_branch(repo_dir, old, new)`.
- **`WorkareaEvent::BranchRenamed`** (append-only on the event enum) + **`AuditKind::BranchRenamed`** (wire `branch_renamed`).

## Implementation notes
- **D1 is the contract: deterministic is LIVE, not a stub.** `DeterministicOneShot` must produce a genuinely useful kebab-case branch name (and a usable PR title/body for 321), not `todo!()`. The "LLM" seam being unwired means the *real provider* is absent — the deterministic path ships and works. State the D1 sentence in Verification verbatim.
- **Own the seam minimally and freeze it precisely.** 321 and 412 both build on this; getting `OneShotLlm`/`OneShotRequest`/`ActionKind`/`compose_action_prompt` shapes right *now* avoids a "Revise" task later. Match `compose_action_prompt`'s param shape to Task 310's `action_pref(repo_id, action)` getter — read 310's handoff and align (pass the resolved pref string/struct, don't re-resolve inside the helper).
- **Cross-repo apply, partial success.** The rename loops over every repo's worktree in the workarea; one repo's remote-conflict skip must not abort the others. The `RenameReport` is the contract the UI/warning surface reads. Use `list_branches` to detect the remote conflict; the `-N` suffix follows `design/03 §8`.
- **`git branch -m` via shell-out, not gix.** Branch ops are git-authoritative (`design/02 §3.1`); add the `rename_branch` wrapper to `gix-wrap` using the existing `cmd::run` pattern (`worktree_add` is the template). Cross-platform: `git branch -m` works identically on the win/linux CI lanes (Task 113) — no special handling.
- **First-message trigger seam.** The design fires the suggestion on the first user message via the Local API message ingress. If that ingress doesn't expose a clean hook yet, make `suggest_workarea_branch_name` a callable manager method + document the ingress-wiring seam (a one-line follow-on) in Handoff — do NOT block the task on building message-ingress plumbing that belongs to another subsystem. The rename itself (`rename_workarea_branch`) is fully testable without the trigger.
- **No new heavy deps.** `async-trait` is already pinned. The slug logic is hand-rolled (no slug crate needed). `cargo deny` stays green.
- Regen: new `AuditKind` (+ any proto if you add the optional RPC) ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/rust-api.md` (+ `proto.md` if a proto changed); commit.

## Verification
Tier 1. **Tier-1 covers the deterministic path; the live-LLM path is wired in P4 (412) and judged at that phase gate** (D1, mirroring the README's `notify_user`-stubbed-until-P5 precedent).
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core llm` → `DeterministicOneShot` produces a stable kebab-case branch slug for a sample prompt; `compose_action_prompt` injects the resolved `branch_rename` pref; `PrCreate` template title/body is non-empty (the 321 fallback).
4. `cargo test -p concerto-core branch_rename` (+ `cargo test -p concerto-gix-wrap rename_branch`) → cross-repo rename in a multi-repo fixture; remote-conflict repo skipped + `-N` suffixed while siblings rename (partial success); `workareas.branch_name` updated; `BranchRenamed` event + audit emitted.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → green (no new deps; `async-trait` already pinned).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`rust-api.md` gains the `llm` module + `BranchRenamed`/`ActionPrefInjected` audit kinds + `gix-wrap::rename_branch`).
8. `scripts/smoke.sh` → **unchanged** (no new capability; co-located happy path stays green).

**Tier-1 scope note (for the phase checklist):** Tier-1 proves the cross-repo `git branch -m` logic + the deterministic suggestion + the seam shape. What it does NOT cover: the **quality of a real-LLM branch suggestion** — that is judged at the **Phase-4** gate once Task 412 wires the live provider behind the `OneShotLlm` seam (D1). No Phase-3 Tier-3 line is needed; this is the documented P4 hand-off, not un-automatable physical reality.

## Definition of Done
- [x] `crates/core/src/llm/oneshot.rs` ships the FROZEN `OneShotLlm` trait + `OneShotRequest` + `ActionKind` (seven actions) + **live** `DeterministicOneShot` + `compose_action_prompt` reading 310's resolved `action_prefs`
- [x] The real-LLM provider is an **unwired** seam (manager holds `Arc<dyn OneShotLlm>` defaulting to `DeterministicOneShot`); no provider/CLI/network in this task (D1)
- [x] `gix-wrap::rename_branch` (`git branch -m` shell-out) added; `worktree_add` pattern mirrored
- [x] `WorkareaManager::{suggest_workarea_branch_name, rename_workarea_branch}` with **cross-repo apply + per-repo remote-conflict skip + `-N` suffix** (partial success, no abort)
- [x] `workareas.branch_name` updated; `WorkareaEvent::BranchRenamed` + `AuditKind::BranchRenamed` emitted
- [x] First-message trigger wired OR the ingress-wiring seam documented in Handoff (rename itself fully testable)
- [x] Verification commands pass; interfaces regenerated; smoke gate unchanged + green
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (the unwired-provider seam + any trigger seam documented in Handoff)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/llm/mod.rs` (new — `pub mod oneshot`)
- `crates/core/src/llm/oneshot.rs` (new — `OneShotLlm` + `OneShotRequest` + `ActionKind` + `DeterministicOneShot` + `compose_action_prompt`)
- `crates/core/src/lib.rs` (modified — `pub mod llm`)
- `crates/gix-wrap/src/api.rs` (modified — `rename_branch`)
- `crates/core/src/workspace_manager/workarea.rs` (modified — `suggest_workarea_branch_name`, `rename_workarea_branch`, `WorkareaEvent::BranchRenamed`)
- `crates/core/src/audit/event.rs` (modified — `BranchRenamed` (+ `ActionPrefInjected`) variant + `as_str` arm)
- `crates/core/tests/branch_rename.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated; `proto.md` if an optional RPC was added)

## Commit message
```
phase-3: branch-rename hook + OneShotLlm seam (deterministic LIVE)

Creates crates/core/src/llm/oneshot.rs (OneShotLlm trait +
DeterministicOneShot live impl + compose_action_prompt reading 310's
resolved action_prefs) per PHASE3_PLANNING §4.4. Per D1 the deterministic
path is live; the real provider is an unwired seam (P4/412). Adds
gix-wrap::rename_branch and WorkareaManager::{suggest_workarea_branch_name,
rename_workarea_branch} with cross-repo git branch -m + per-repo
remote-conflict skip (-N suffix). 321 reuses the seam for PR title/body.

Refs: tasks/v1.0/312-branch-rename-hook.md
```

## Handoff Notes (filled in when finishing)
- **The seam 321 reuses (FROZEN, do NOT re-lock):** `crates/core/src/llm/oneshot.rs` exports `trait OneShotLlm { async fn suggest(&self, req: OneShotRequest) -> Result<String> }` (`#[async_trait]`), `OneShotRequest { action: ActionKind, repo_id: String, prompt: String, context: String }` (+ `OneShotRequest::new(action, repo_id, prompt, context)`), the seven-variant `ActionKind` (`code_review/pr_create/error_fix/conflict_resolve/branch_rename/commit_message/digest_summary`, with `as_str()`), the LIVE `DeterministicOneShot`, and `compose_action_prompt(action: ActionKind, pref: &Resolved<Option<String>>, context: &str) -> String`. **For PR title/body (321): use `ActionKind::PrCreate`; `DeterministicOneShot` already returns a `"<title>\n\n<body>"` template — split on the FIRST `"\n\n"` (title = before, body = after). Build the request via `OneShotRequest::new`, get the pref from `resolver.action_pref(repo_id, ActionKind::PrCreate.as_str())`, compose with `compose_action_prompt`, then `manager.one_shot.suggest(req).await` (the manager already holds `Arc<dyn OneShotLlm>` defaulting to `DeterministicOneShot`).** Add NO new LLM machinery — 412 swaps the provider via `WorkareaManager::with_one_shot(Arc<dyn OneShotLlm>)`. `compose_action_prompt`'s pref param is exactly 310's `action_pref` return type (`Resolved<Option<String>>`); pass it through, never re-resolve inside the helper.
- **First-message trigger is an UNWIRED ingress seam (deliberate, one-line follow-on).** `suggest_workarea_branch_name(id, first_message)` is a directly-callable manager method but is NOT yet fired from the Local API message ingress — the ingress (`design/03 §7.2`) does not expose a clean first-user-message hook today, and building that plumbing belongs to the message-ingress subsystem, not this task. To wire it: at the message ingress, on the first user message of any session in a workarea, call `suggest_workarea_branch_name` → surface the proposed name to the user → on confirm call `rename_workarea_branch(id, confirmed)`. The rename apply is fully tested + usable standalone without the trigger.
- **No gRPC RPC added (decided in-task).** The design routes the rename trigger through the Local API message ingress, not a dedicated RPC, so `SuggestWorkareaBranchName`/`RenameWorkareaBranch` RPCs are intentionally NOT on the `Workareas` service. The Desktop branch-rename UX task can append thin RPCs over `WorkareaManager::{suggest_workarea_branch_name, rename_workarea_branch}` later (do not renumber). The `WorkareaEvent::BranchRenamed` event already rides `workarea.events` with the opaque `branch_renamed` kind (no `streams.proto` change — `handlers/streams.rs` maps it), so the Desktop can observe renames today.
- **Drift from plan / debt:** (1) The `Outputs` row "`docs/interfaces/rust-api.md` (regenerated; gains the `llm` module + `BranchRenamed`/`ActionPrefInjected` audit kinds + `gix-wrap::rename_branch`)" produced **no diff** — `scripts/regen-interfaces.sh` only scrapes `pub trait/struct/enum` brace-blocks from `crates/*/src/api.rs`. This task adds free `fn`s (`rename_branch`), enum *variants* (`AuditKind`/`WorkareaEvent`), and a module outside `api.rs` (`crates/core/src/llm/`, and `concerto-core` has no `src/api.rs` at all) — none of which the generator emits. So `docs/interfaces/` is byte-identical and the CI "interface summaries" check passes; the doc simply does not enumerate these (matches existing behaviour for other core methods/audit kinds). No action needed; flagged so 321/412 don't expect the `llm` seam to show up in `rust-api.md`. (2) `AuditKind::ActionPrefInjected` is emitted via `tracing::info!(audit.kind=…)` (the established audit precedent), not a structured audit-store write — same pattern as every other `AuditKind` today. (3) No migration (uses the existing `workareas.branch_name` column + 310's `action_prefs_json`). All within `Outputs`.
- **Smoke-gate state — unchanged + green.** No new capability, no `scripts/smoke.d/*` entry, `scripts/` untouched. Full gate clean: `cargo check`/`clippy -D warnings`/`fmt --check` (only the expected `imports_granularity` nightly noise) green; `cargo test -p concerto-core --lib llm` 7/7, `--test branch_rename` 5/5, `concerto-gix-wrap` rename 2/2, `cargo test --workspace --no-fail-fast` all pass; `cargo deny check` advisories/bans/licenses/sources ok; `regen-interfaces.sh` → no diff.
