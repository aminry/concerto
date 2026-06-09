# Task 411 — `create_workspace_from_description` (issue parse → multi-repo detect → cone suggest → confirm chips) + `Repositories.SuggestCones` RPC + the D10 privacy-debt fix

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 2 |
| Depends on | 406, 403, 305, 313 |
| Touches subsystem(s) | 08 (Maestro), 02 (Repo Mgr), 13 (VCS), 03 (Workspace Mgr) |
| Size | medium |
| Smoke gate | unchanged |

## Goal
Build the high-level Maestro **create** tool (`design/08 §3.8` "Spawn new workspace + workarea from natural language") that turns a plain-English description into a confirmed workspace + first workarea. Today the create-flow `create_workspace`/`create_workarea` MCP tools exist only as **406's** write-tool impls wrapping `WorkspaceManager::create_workspace` (`crates/core/src/workspace_manager/actor.rs:185`) + `WorkareaManager::create_workarea` (`crates/core/src/workspace_manager/workarea.rs:695`); there is **no** description-parsing planner, **no** `Repositories.SuggestCones` RPC (305 froze the `ConeSuggester` trait + `RepoManager::with_cone_suggester` injector + `RepoManager::suggest_cones` returning `ConeSuggestError::Unwired` at `crates/core/src/repo_manager/cone_stats.rs`, and pre-wrote `cone_suggest_error_to_status` at `handlers/repositories.rs:381`, but **no RPC and no injected suggester**), and the live `handlers/vcs.rs::fetch_issue_by_url` **hardcodes `enterprise_data_privacy: false`** at `crates/core/src/handlers/vcs.rs:283` (the **D10 deliberate debt** — the resolver `WorkspaceSettingsResolver::enterprise_data_privacy()` at `crates/core/src/settings/resolver.rs:290` now exists but is unread on this path). This task adds the **`create_from_description`** orchestration into `maestro/tools/write.rs` (description → issue-ref parse → `VcsHandle::fetch_issue_url` (313's live URL router, `crates/vcs/src/actor.rs:258`) → multi-repo intent detect → `RepoManager::suggest_cones` wired through a new **Maestro-backed `ConeSuggester`** injected via `with_cone_suggester` → a **confirmation chip slate** → on confirm, `create_workspace` + `create_workarea`), adds the **`Repositories.SuggestCones` RPC** (`SuggestConesRequest`/`SuggestConesResponse`) reusing the **FROZEN** `cone_suggest_error_to_status` mapping, freezes the new `maestro/cone_suggester.rs` `ConeSuggester` impl, and **fixes the D10 debt**: `fetch_issue_by_url` resolves the real `enterprise_data_privacy()` (additive `FetchIssueByUrlRequest.workspace_id = 2`; `design/08 §3.10`). After this task the Maestro can plan a workspace from an issue link end-to-end behind a user-confirm chip (the **never-create-silently** invariant, `design/08 §3.8`/R-2), and the `SuggestCones` wire surface is available to 415's Desktop create UX. Real issue fetch + real cone-suggestion quality stay Tier-3.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §2 (row 411 — **AUTHORITATIVE**: "Add `Repositories.SuggestCones` RPC (reuse the pre-written `cone_suggest_error_to_status` mapping) + inject a Maestro-backed `ConeSuggester` via `RepoManager::with_cone_suggester` at boot. `create_workspace`/`create_workarea` tools wrap `WorkspaceManager::create_workspace` + `WorkareaManager::create_workarea` … 411 also fixes D10's `enterprise_data_privacy=false` debt in `handlers/vcs.rs`."), §1 **D10** (the privacy debt + the `fetch_issue_url` consumer is 411), §6 (411 deps `406, 403, 305, 313`), §8.1 (411 write-set: `tools/write.rs` (create flow) ∥ `repositories.proto` ∥ `handlers/{repositories,vcs}.rs` ∥ `repo_manager/actor.rs`; **hard seam: 406 on `tools/write.rs` ⇒ serialize after 406**; `repositories.proto`).
- `design/08_Maestro_Agent.md` §3.8 — **AUTHORITATIVE** the 5-step create flow (parse issue ref → multi-repo detect → cone suggest → confirmation chip slate → user confirms → create through 03). §3.8 R-2 / line 332 ("mutations surface as confirmation chips before executing") + line 221 ("**The Maestro never creates a workspace, workarea, or session silently. The user always confirms.**"). §5.1 lines 323–324 (`create_workspace(spec) → workspace_id`, `create_workarea(workspace_id, spec) → workarea_id` — both `(user confirms)`). §3.10 (enterpriseDataPrivacy behavior).
- `tasks/v1.0/305-cone-stats-suggest-seam.md` — the FROZEN `ConeSuggester` trait + `with_cone_suggester` injector + the `ConeSuggestError::{Unwired, Delegate}` contract + the pre-written `cone_suggest_error_to_status` this task **consumes** (do NOT re-shape). Its Handoff Note (2): "411 is the P4 owner that injects the Maestro-backed `ConeSuggester` … a pure addition … so 411 wires the LLM with zero proto/trait change."
- `tasks/v1.0/313-vcs-provider-github.md` — the FROZEN `VcsHandle::fetch_issue_url(url, &IssueFetchCreds)` URL router + the `testkit` `FakeGitHub`/`FakeLinear`/`FakeJira` wiremock harness (the Tier-2 double) + `IssueFetchCreds.enterprise_data_privacy`.
- `tasks/v1.0/406-write-tool-set.md` — **AUTHORITATIVE for the FROZEN `create_workspace`/`create_workarea` MCP tool schemas + the strict→`MustAsk`→`AwaitingApproval`/`ResolveApproval` confirmation-chip flow this task EXTENDS** (411 adds the higher-level `create_from_description` orchestration tool to the same `tools/write.rs`; it reuses 406's chip-gate, never bypasses it). **Read 406's Handoff before writing — `tools/write.rs` is the hard seam; 411 serializes after 406 and rebases onto its `create_*` impls.**
- `crates/core/src/handlers/vcs.rs` (lines 238–293, `fetch_issue_by_url`) — the **D10 debt site**: `enterprise_data_privacy: false` hardcoded at line 283, with the explicit "without task 310's resolver … allowed by default … enforced once the resolver lands — see Handoff" comment. This task replaces it.
- `crates/core/src/handlers/repositories.rs` (lines 369–388) — the pre-written FROZEN `cone_suggest_error_to_status` (`Unwired → Status::unimplemented`, `Delegate(e) → error_to_status`) the new `SuggestCones` handler reuses **verbatim**; mirror the `estimate_cone_size` unary-handler shape (lines 145–167).
- `crates/core/src/repo_manager/cone_stats.rs` — the `ConeSuggester` trait (`async fn suggest_cones(&self, repo: &RepositoryId, issue_text: &str) -> Result<Vec<ConePath>>`), `ConeSuggesterSeam`, `ConeSuggestError`. `crates/core/src/repo_manager/actor.rs` (`cone_suggester` field ~:106, `with_cone_suggester` ~:133, `suggest_cones` ~:1086, `list_paths_in_cone` ~:923).
- `crates/core/src/settings/resolver.rs:290` (`WorkspaceSettingsResolver::enterprise_data_privacy() -> Resolved<bool>`) + `crates/core/src/settings/boot.rs:55` (`build_resolver_for_workspace(persistence, &workspace_id, managed, &opt_out)`) — the per-workspace resolver factory the D10 fix calls. `ManagedPolicy::enterprise_data_privacy()` is the Core-wide floor used when no workspace scope is present.
- `crates/proto/proto/concerto/v1/repositories.proto` (lines 162–237) — append `SuggestCones` after `SetRepoConeDefaults` (last RPC, line 236), mirroring the `EstimateConeSizeRequest`/`ConeStats` message style; `crates/proto/proto/concerto/v1/vcs.proto:146` (`FetchIssueByUrlRequest { string url = 1; }`) — add `workspace_id = 2` additively.
- **Author check (do this first):** this task adds **no** migration (PHASE4_PLANNING §3: "411 (`SuggestCones`) = new RPC, **no migration**"). Confirm regardless that the highest `crates/persist/migrations/NNNN_*.sql` on `main` is still **`0014`** (verified at authoring: `0014_pull_requests_merge_order.sql`); if 0015/0016 (403/410) landed first that is fine — 411 touches no migration — but note any drift in Handoff.

## Scope — in
- **`crates/core/src/maestro/tools/write.rs` (modified — `create_from_description`):**
  - Add the `create_from_description(description: &str, workspace_id_hint: Option<&str>)` orchestration tool body behind 401's FROZEN tool schema (the create-flow tool surface). **Reuse 406's `create_workspace`/`create_workarea` impls + the strict→`MustAsk`→confirmation-chip gate verbatim** — `create_from_description` is a planner that ends in 406's chip slate, never a silent create (`design/08 §3.8`/R-2).
  - **Step 1 — issue-ref parse:** scan `description` for a Linear/GitHub/Jira issue URL (deterministic, zero LLM tokens); if found, call `VcsHandle::fetch_issue_url(url, &creds)` (313) to pull `Issue{number,title,body,labels,…}` as planning context. No URL → skip silently (freeform planning).
  - **Step 2 — multi-repo intent detect:** a deterministic detector over the description + fetched issue text (e.g. "across the API and the iOS app") proposing a repo subset of the global registry (`design/08 §3.8` step 2); ambiguous → carry all candidate repos into the chip slate for the user to edit (never auto-pick silently).
  - **Step 3 — cone suggest:** for each proposed repo call `RepoManager::suggest_cones(repo, issue_text)` (the seam 305 froze) — now LIVE via the injected Maestro `ConeSuggester`; pair each suggested cone with `RepoManager::list_paths_in_cone` (305) `ConeStats` so the chip slate shows file-count/size.
  - **Step 4 — confirmation chip slate:** compose the `design/08 §3.8` step-4 slate ("Create workspace + first workarea" / "Just create the workspace, no workarea yet" / "Edit repo set / cones") via 407's `propose_chip` onto the Maestro-owned slate (D11) — NOT the volatile suggestion buffer.
  - **Step 5 — on confirm:** the confirmed chip's `AwaitingApproval`/`ResolveApproval` resolution drives `WorkspaceManager::create_workspace(name, &repos, permission_mode, description, icon)` then `WorkareaManager::create_workarea(workspace_id, permission_mode)` (the existing 03 signatures) — first workspace, then first workarea, then 406's session-create default (Claude in plan mode). Return the new `workspace_id`/`workarea_id`.
- **`crates/core/src/maestro/cone_suggester.rs` (new — the Maestro-backed `ConeSuggester`):**
  - A `MaestroConeSuggester` struct `impl ConeSuggester` (305's FROZEN trait) that turns `issue_text` + the repo tree into a `Vec<ConePath>`. **The LIVE P4 path routes through `OneShotLlm::suggest` with `ActionKind` cone-suggestion intent** (reuse 312's seam, §4.5 — `DeterministicOneShot` is the live fallback that returns a deterministic cone guess, e.g. top-level dirs matching issue keywords); the real-LLM cone quality is Tier-3. Construct + inject via `RepoManager::with_cone_suggester(Arc::new(MaestroConeSuggester::new(..)))` at boot.
- **`crates/proto/proto/concerto/v1/repositories.proto` (modified — `SuggestCones`):**
  - Add `message SuggestConesRequest { string repository_id = 1; string issue_text = 2; }`, `message SuggestConesResponse { repeated string cone_paths = 1; }`, and `rpc SuggestCones(SuggestConesRequest) returns (SuggestConesResponse);` appended to `service Repositories` after `SetRepoConeDefaults`. Field numbers start at 1 (new messages). **Add only `google.protobuf.Timestamp` fields to the proto build.rs timestamp_fields list — none here.**
- **`crates/core/src/handlers/repositories.rs` (modified — `suggest_cones` handler):**
  - Add the unary `suggest_cones` handler (mirror `estimate_cone_size`): validate `repository_id` non-empty, call `RepoManager::suggest_cones`, map `Result<Vec<ConePath>, ConeSuggestError>` through the **pre-written FROZEN `cone_suggest_error_to_status`** (unwired → `UNIMPLEMENTED`, delegate-err → `error_to_status`), project into `SuggestConesResponse`.
- **`crates/core/src/handlers/vcs.rs` (modified — D10 fix):**
  - Replace the hardcoded `enterprise_data_privacy: false` (line 283) with the resolved value: build a `WorkspaceSettingsResolver` via `crate::settings::build_resolver_for_workspace(persistence, &workspace_id, managed, &opt_out)` when `req.workspace_id` is set, read `enterprise_data_privacy().into_value()`; when no workspace scope is supplied, fall back to the Core-wide `ManagedPolicy::enterprise_data_privacy()` floor (default `false`) — never silently allow when managed forces privacy. Pass the resolved bool into `IssueFetchCreds`.
- **`crates/core/src/repo_manager/actor.rs` (modified — boot injection seam):** ensure the boot path constructs `RepoManager` with `.with_cone_suggester(..)` (the existing FROZEN injector); add only the construction wiring, not a new field/trait.
- **`crates/proto/proto/concerto/v1/vcs.proto` (modified):** add `string workspace_id = 2;` to `FetchIssueByUrlRequest` (additive, optional — existing callers send empty ⇒ the Core-wide floor path).
- Tests (Tier 2): (1) `create_from_description` with a GitHub issue URL against the **313 `testkit` `FakeGitHub` wiremock** + a **stub `ConeSuggester`** → asserts a confirmation chip slate is produced (NOT a silent create) and that confirming the "create + workarea" chip calls `create_workspace` then `create_workarea`; (2) `create_from_description` with no URL → freeform planning path still ends in a chip slate; (3) multi-repo detect picks the named repo subset and carries ambiguity into the slate; (4) `SuggestCones` handler: unwired seam → `Status::unimplemented` (via `cone_suggest_error_to_status`), injected `MaestroConeSuggester`/stub → returns the cone set; (5) **D10 regression:** `fetch_issue_by_url` with `workspace_id` of an `enterprise_data_privacy=true` workspace → a Linear/Jira URL fetch returns the typed `vcs.external_tracker_blocked` error (privacy floor enforced), GitHub URL still allowed; with no `workspace_id` + managed-privacy → blocked.

## Scope — out
- **The 401-frozen MCP tool schema / the in-process MCP server / `tools/mod.rs` registration line** (owned by 401/406) — 411 fills the `create_from_description` body behind the FROZEN schema; it does not re-shape the tool schema. The 406 `create_workspace`/`create_workarea` impls + the chip-gate are **consumed as frozen by Task 406 (PHASE4_PLANNING §4.1/§2 row 406)**.
- **The real-LLM cone suggestion + the real provider behind `OneShotLlm`** — the LIVE path here is `DeterministicOneShot` (§4.5); **Task 412** supplies the real provider. Real cone-suggestion quality is the Tier-3 gate, not 411's.
- **The `notify_user`/`propose_chip` side-channel tool impls** — owned by **Task 407** (D11). 411 consumes `propose_chip` to post its chip slate to the Maestro-owned slate; it does not build the slate machinery.
- **Privacy gate over the per-workarea summary cache + `exclude_from_maestro` skip + `concerto_chat_full_chat_access`** — owned by **Task 413** (D10's other half). 411 fixes ONLY the `fetch_issue_url` `enterprise_data_privacy` hardcode; 413 enforces the resolver before any external summary/digest.
- **The routing pre-parser / `@workarea` grammar** — owned by **Task 408**; create-from-description is a write tool, not a routing directive.
- **Desktop create-from-description UX** (rendering the chip slate + cone-picker) — owned by **Task 415** (consumes the `SuggestCones` wire surface). 411 ships no `apps/desktop` code.
- **The real-world Tier-3 line:** "create a workspace from a real issue link" (the Phase-4 manual checklist) — judged at the phase gate against a live GitHub/Linear issue + real cone suggestion, not 411's CI.

## Public interface this task locks
- **proto `Repositories.SuggestCones` (FROZEN, design/08 §3.8 / PHASE4_PLANNING §2 row 411), `repositories.proto`:**
  ```proto
  // Inputs for `Repositories.SuggestCones` (Task 411, design/08 §3.8). The
  // plan-mode cone suggestion the Maestro `create_workspace_from_description`
  // flow calls: given an added repository and the parsed issue/description
  // text, the Repo Mgr delegates to the injected Maestro-backed ConeSuggester
  // (Task 305's FROZEN trait seam). With no suggester injected the RPC returns
  // UNIMPLEMENTED (via cone_suggest_error_to_status). FROZEN by PHASE4_PLANNING.
  message SuggestConesRequest {
    string repository_id = 1;
    string issue_text = 2;
  }
  // Suggested cone set (forward-slash, repo-root-relative directory prefixes),
  // the same shape `SetCones`/the cone-picker consume.
  message SuggestConesResponse {
    repeated string cone_paths = 1;
  }
  // appended to `service Repositories`, after SetRepoConeDefaults:
  //   rpc SuggestCones(SuggestConesRequest) returns (SuggestConesResponse);
  ```
- **proto `FetchIssueByUrlRequest.workspace_id` (FROZEN additive field, PHASE4_PLANNING §4 D10), `vcs.proto`:**
  ```proto
  message FetchIssueByUrlRequest {
    string url = 1;
    // Workspace scope for the enterprise_data_privacy resolver (Task 411, D10).
    // Empty ⇒ the Core-wide ManagedPolicy floor. Additive; existing callers
    // (which sent only `url`) keep working — they get the managed floor.
    string workspace_id = 2;
  }
  ```
- **Rust `MaestroConeSuggester` (FROZEN, design/08 §3.8), `crates/core/src/maestro/cone_suggester.rs`:**
  ```rust
  /// The Maestro-backed plan-mode cone suggester (Task 411). Injected into the
  /// RepoManager via `with_cone_suggester` at boot, this is the LIVE wiring of
  /// the seam Task 305 froze. The live path routes `issue_text` + the repo tree
  /// through `OneShotLlm` (DeterministicOneShot fallback); the real-LLM cone
  /// quality is the Phase-4 Tier-3 gate.
  pub struct MaestroConeSuggester { /* OneShotLlm handle + RepoManager tree access */ }

  #[async_trait::async_trait]
  impl crate::repo_manager::ConeSuggester for MaestroConeSuggester {
      async fn suggest_cones(
          &self,
          repo: &concerto_persist::RepositoryId,
          issue_text: &str,
      ) -> concerto_error::Result<Vec<concerto_gix_wrap::ConePath>>;
  }
  ```
- **Consumed as frozen, NOT re-locked here:** the `ConeSuggester` trait + `RepoManager::with_cone_suggester` + `RepoManager::suggest_cones` + `ConeSuggestError` + `cone_suggest_error_to_status` (frozen by **Task 305**, PHASE3_PLANNING §4.6 / PHASE4_PLANNING §2 row 411); `VcsHandle::fetch_issue_url` + `IssueFetchCreds` + the `testkit` harness (frozen by **Task 313**); `WorkspaceManager::create_workspace` / `WorkareaManager::create_workarea` (frozen by 03/306/307); the `create_workspace`/`create_workarea` MCP tool schemas + the confirmation-chip gate (frozen by **Task 406**, PHASE4_PLANNING §4.1); `propose_chip` (frozen by **Task 407**, D11); `OneShotLlm` (frozen by **Task 312**, §4.5); `WorkspaceSettingsResolver::enterprise_data_privacy()` (frozen by **Task 310/413**).

## Implementation notes
- **The load-bearing rule: the Maestro never creates silently — always a confirm chip (`design/08 §3.8` line 221 / R-2).** `create_from_description` is a *planner* that terminates in 406's confirmation chip slate. Steps 1–4 spend zero side effects; the workspace/workarea creation happens ONLY on the user's chip resolution (the `AwaitingApproval`/`ResolveApproval` flow 406 wired). Do NOT add a "skip confirmation" fast path.
- **Reuse, don't reinvent the create primitives.** Call 406's `create_workspace`/`create_workarea` tool impls (which wrap `WorkspaceManager::create_workspace`/`WorkareaManager::create_workarea`); do not call the gRPC handlers or the managers a second, parallel way. The cone-suggest seam, the issue router, and the chip-gate are all FROZEN by earlier tasks — 411 is glue.
- **The `SuggestCones` handler reuses the pre-written mapper verbatim.** `cone_suggest_error_to_status` already maps `Unwired → Status::unimplemented("suggest_cones is wired in P4 (Maestro, Task 411)")`. Once 411 injects the suggester at boot the live RPC returns the cone set; if a Core is constructed without the injector (or a unit test omits it) the RPC honestly returns `UNIMPLEMENTED` — never an empty success. The seam stays the typed `ConeSuggestError`, never `todo!()`/`unimplemented!()`.
- **The D10 fix is the honest resolution of a documented debt.** `handlers/vcs.rs:283`'s comment explicitly says the floor is "enforced once the resolver lands." The resolver lands (310) and now binds to the create flow's workspace. Because `FetchIssueByUrlRequest` had no workspace scope, add `workspace_id = 2` **additively** (no CHECK-widen, no migration — proto field append only); when empty, fall back to the Core-wide `ManagedPolicy::enterprise_data_privacy()` floor so a managed-privacy Core still blocks external-tracker fetches. The GitHub arm is never privacy-gated (it is the user's own repo host, per `IssueFetchCreds.github_token` doc); only Linear/Jira (external trackers) are blocked. **Stop-and-ask if the resolver cannot be built in the `Vcs` handler without a wider boot change** — wiring the managed policy / persistence into `VcsHandler` may touch `api_server.rs`/`boot.rs` construction; keep it minimal and note any boot drift in Handoff.
- **Two-site registration reminder (gRPC):** `SuggestCones` is a new RPC on the **existing** `Repositories` service — it does NOT need a new service registration, so the two-site `add_core_services` + `connect_bridge.rs` dance does not apply (the `Repositories` service is already registered at both sites). Confirm the regenerated `repositories_server` trait compiles at both call sites; do not add a new `CoreServiceSet` field.
- **Cross-platform:** the create flow + `SuggestCones` are pure async over handles (no `#[cfg(unix)]` gate needed here — unlike the agent-supervisor-bound handlers). `ConePath` is a forward-slash git path everywhere; the 313 issue router is rustls/pure-Rust. The `OneShotLlm` deterministic fallback is platform-agnostic.
- **Regen:** proto changed (`SuggestCones` + `FetchIssueByUrlRequest.workspace_id`) ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/proto.md` (and `rust-api.md` for the new `MaestroConeSuggester` struct, if captured); commit it.
- **Parallel build hint:** the three FROZEN surfaces are file-disjoint and can be built by helper sub-agents in parallel, then integrated into the one commit — **(a) `SuggestCones` RPC + the `MaestroConeSuggester` `ConeSuggester` impl + the boot injection** (`repositories.proto`, `handlers/repositories.rs`, `maestro/cone_suggester.rs`, `repo_manager/actor.rs`) ∥ **(b) the issue-parse + multi-repo-detect + create orchestration** (`maestro/tools/write.rs`) ∥ **(c) the confirm-chip slate composition + the D10 privacy-debt fix** (`maestro/tools/write.rs` chip path + `handlers/vcs.rs` + `vcs.proto`). (b) and (c) both touch `tools/write.rs` so they integrate last; (a) is fully disjoint. (Matches the `PHASE4_DAG.json` 411 fanout = SuggestCones-RPC+ConeSuggester-impl ∥ issue-parse+multi-repo-detect ∥ confirm-chip-slate+privacy-debt-fix.)

## Verification
**Tier 2.** The `rust` §5.3 set; the double is the **313 `testkit` `FakeGitHub`/`FakeLinear` wiremock harness + a stub `ConeSuggester`**.
1. `cargo check --workspace` clean (the regenerated `repositories`/`vcs` proto compiles; `Repositories` server trait gains `suggest_cones`).
2. `cargo clippy --workspace --all-targets -- -D warnings` clean; then `cargo fmt --all -- --check` clean (CI `format.yml` parity — `--all` covers every workspace member).
3. `cargo test -p concerto-core create_from_description` (+ `suggest_cones`, + `fetch_issue_by_url`) → the create-flow chip-slate tests (GitHub-URL + freeform + multi-repo-detect, each ending in a confirmation chip, NOT a silent create); the confirm-chip path calls `create_workspace` then `create_workarea`; the `SuggestCones` handler unwired→`UNIMPLEMENTED` + injected-suggester→cone-set; the **D10 regression** (enterprise-privacy workspace blocks a Linear/Jira fetch, GitHub still allowed; no-workspace + managed-privacy blocks).
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new crates — reuses 313's `octocrab`/`wiremock` + 312's `OneShotLlm`; the prior operator-ratified `RUSTSEC-2023-0071` scoped ignore is unchanged).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `SuggestCones`/`SuggestConesRequest`/`SuggestConesResponse` + `FetchIssueByUrlRequest.workspace_id`).
7. `scripts/smoke.sh` → **unchanged** (411 adds no smoke capability; the create flow + `SuggestCones` are CI-provable via the in-process harness + the 313 wiremock double).

**Tier-2 double + what it does NOT cover.** The 313 `testkit` wiremock + the stub `ConeSuggester` prove the planner's wiring: issue-URL parse → fetch projection → multi-repo detect → cone-suggest delegation → chip-slate composition → confirm-drives-create, the `SuggestCones` unimplemented/injected paths, and the D10 privacy-floor enforcement. It does **NOT** cover the real GitHub/Linear issue round-trip or real-LLM cone-suggestion quality — those are the **Phase-4 Tier-3 manual-checklist line "create a workspace from a real issue link"** (judged at the phase gate against a live issue + real cone suggestion + the real provider 412 supplies).

## Definition of Done
- [x] `create_from_description` orchestration added to `maestro/tools/write.rs`: issue-ref parse → `fetch_issue_url` → multi-repo detect → `RepoManager::suggest_cones` → confirmation chip slate → on-confirm `create_workspace` + `create_workarea` (never a silent create — `design/08 §3.8`/R-2)
- [x] `Repositories.SuggestCones` RPC (`SuggestConesRequest{repository_id=1, issue_text=2}` / `SuggestConesResponse{cone_paths=1}`) appended; handler reuses the pre-written FROZEN `cone_suggest_error_to_status` (unwired → `UNIMPLEMENTED`, not empty success)
- [x] `MaestroConeSuggester` `impl ConeSuggester` (Task 305's FROZEN trait) injected via `RepoManager::with_cone_suggester` at boot; live path routes through `OneShotLlm` (`DeterministicOneShot` fallback)
- [x] D10 fix: `handlers/vcs.rs::fetch_issue_by_url` resolves `WorkspaceSettingsResolver::enterprise_data_privacy()` (additive `FetchIssueByUrlRequest.workspace_id=2`; Core-wide `ManagedPolicy` floor when absent) — the hardcoded `false` is gone
- [x] Tests (Tier 2): create-flow chip-slate (GitHub-URL/freeform/multi-repo), confirm→create, `SuggestCones` unwired/injected, D10 privacy regression — all against the 313 wiremock + stub suggester
- [x] All Verification commands pass on a clean checkout; smoke unchanged; interfaces regenerated + committed
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams return a typed Err/Status — the `SuggestCones` unwired path returns `tonic::Status::unimplemented`, a runtime status, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed (proto changed)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/tools/write.rs` (modified — the `create_from_description` orchestration: issue parse + multi-repo detect + cone-suggest call + confirm-chip slate + on-confirm create; serialized after 406's `create_*` impls)
- `crates/core/src/maestro/cone_suggester.rs` (new — `MaestroConeSuggester` `impl ConeSuggester`, the LIVE wiring of 305's seam through `OneShotLlm`) + `crates/core/src/maestro/mod.rs` (modified — `pub mod cone_suggester;` in the additive region)
- `crates/proto/proto/concerto/v1/repositories.proto` (modified — `SuggestCones` RPC + `SuggestConesRequest`/`SuggestConesResponse`)
- `crates/proto/proto/concerto/v1/vcs.proto` (modified — `FetchIssueByUrlRequest.workspace_id = 2`)
- `crates/core/src/handlers/repositories.rs` (modified — `suggest_cones` unary handler reusing `cone_suggest_error_to_status`)
- `crates/core/src/handlers/vcs.rs` (modified — D10 fix: resolved `enterprise_data_privacy` replaces the hardcoded `false`)
- `crates/core/src/repo_manager/actor.rs` (modified — boot wires `.with_cone_suggester(MaestroConeSuggester)`)
- `docs/interfaces/proto.md` (regenerated — `SuggestCones` + `FetchIssueByUrlRequest.workspace_id`)

## Commit message
```
phase-4: create_workspace_from_description + SuggestCones RPC + D10 privacy fix

Adds the Maestro create_from_description planner (issue-ref parse via 313's
fetch_issue_url → multi-repo detect → RepoManager::suggest_cones via a wired
MaestroConeSuggester → confirmation chip slate → on-confirm create_workspace +
create_workarea; never silent, design/08 §3.8). Adds the Repositories.SuggestCones
RPC reusing the pre-written cone_suggest_error_to_status mapping (305's seam, now
LIVE). Fixes the D10 debt: fetch_issue_by_url resolves enterprise_data_privacy
(additive FetchIssueByUrlRequest.workspace_id) instead of the hardcoded false.
Tier-2 double = 313's wiremock + a stub ConeSuggester; real issue fetch + real
cone quality stay Tier-3 ("create a workspace from a real issue link").

Refs: tasks/v1.0/411-create-workspace-from-description.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** — (name the consuming **Task 415** + the FROZEN `SuggestCones` wire surface + the create-flow chip slate it renders; note 413 owns the rest of D10's privacy enforcement and consumes the same resolver)
- **Deliberate debt:** — (the `SuggestCones` RPC returns `Status::unimplemented` only if a Core is built without the injector — a runtime status, not the `unimplemented!()` macro; the live cone-suggestion quality remains the deterministic fallback until 412's provider)
- **Smoke-gate state:** —
