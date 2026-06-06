# Phase 3 (Multi-X, Monorepo & VCS) — Planning Addendum

*Read this AFTER `README.md` §4–§6 and BEFORE any Phase-3 task file. It records the
decisions the Phase-3 planning conversation (2026-06-05) locked on top of the README
inventory, the cross-task **frozen contracts** the 24+2 task files must agree on, and the
**migration-number reservation** that keeps parallel-conceptual tasks from racing.*

| Field | Value |
|---|---|
| Status | Approved (2026-06-05) |
| Scope | Phase 3 only (tasks 301–324 + inserts 315.0, 320.5) |
| Supersedes | Nothing. Amends `README.md §6` Phase-3 inventory (the 2 insert rows). |
| Authority | These decisions are FIXED for the Phase-3 task files exactly as `README.md §4` decisions are fixed; revising one is a new planning conversation. |

The single most load-bearing rule: **every interface in §4 below is FROZEN by the task named
as its owner; later tasks CONSUME it, never re-lock it.** If a task author finds the design
contradicts a §4 contract, that's a Stop-and-ask, not a silent re-lock.

---

## 1. The nine locked decisions

| # | Decision | Choice (locked) | Consumed by |
|---|---|---|---|
| **D1** | The LLM seam (no Maestro/provider until P4) | **Deterministic fallback is the LIVE path in P3.** The Maestro/one-shot-LLM delegate is an *unwired trait seam*. Tasks stay Tier-1 but each states: "Tier-1 covers the deterministic path; the live-LLM path is wired in P4 (412) and judged at that phase gate." Mirrors the README's `notify_user`-stubbed-until-P5 precedent. | 305, 312, 321 |
| **D2** | VCS test double | **One shared `wiremock`-backed fixture harness, built in 313** (`crates/vcs` `testkit` feature: recorded REST+GraphQL responses, synthetic rate-limit headers + synthetic clock). 314/315/316/320 reuse it as a dev-dep. Mirrors how P2 built the loopback-Iroh double once. (`design/13 §10` names wiremock.) | 313 → 314, 315, 316, 320, 320.5 |
| **D3** | Inbound webhook → Core routing | **Relay opens an ephemeral Iroh bidi to the Core's `endpoint_id` on a NEW reserved channel tag `0x04` (Webhook)** and pumps the webhook as a small envelope; Core verifies HMAC. Offline Core → relay **drops + logs** (no buffering; relay stays near-stateless per `design/11`). Because this refines the P2 relay contract, **insert 315.0 (doc)** amends `design/11`/`design/13` before 315 implements. | 315.0 → 315 |
| **D4** | New VCS secret classes + keychain namespace | **313 freezes** a *parameterized* keychain accessor (mirroring Task 218's `CoreSecretSlot`, NOT new closed `SecretKind` variants): `VcsSecretSlot ∈ {GithubAppPrivateKey, WebhookSecret, LinearAccessToken, LinearRefreshToken, JiraAccessToken, JiraRefreshToken}` keyed by a scope id, account string `vcs.<scope_id>.<slot_slug>`. Non-secret metadata (app/installation id, token expiry) → new `vcs_credentials` table (migration **0012**, see §3). Keychain stays mac-only in V1.0 with a documented Windows seam (→608). | 313 → 314, 315, 317 |
| **D5** | Linear/Jira scope | **317 = fetch + a no-op `write_back` trait seam.** The real status-transition-on-merge lands as **insert 320.5 (rust)** hung off coordinated-merge completion (honors `design/13 R-9` without bloating 317). | 317 → 320.5 |
| **D6** | OAuth flow on a headless/remote Core (317) | **Desktop-mediated OAuth**: the Desktop runs the 3LO dance in its webview and ships the resulting token to the Core over the paired transport → keychain. **Linear also accepts a personal API key** (simple path). No redirect URI invented for a tray-less Core. | 317 |
| **D7** | `merge_order` + PR-set merge ownership | **Default = insertion order** (`max(merge_order)+1` per workarea); **`SetMergeOrder` RPC (319)** lets the user reorder; **324** UI drag writes it. No dependency-graph inference (that's `R-6`/V2.0). Coordinated-merge RPCs live on the **`Workareas`** gRPC service next to the existing `GetWorkareaPrSet` (03 owns the merge loop). | 319, 320, 324 |
| **D8** | Desktop "Code & PRs" IA | **Follow `design/15 §3.4`** (center-bottom region, Level-1 repo selector + Level-2 Diff/Checks/PR tabs). V0.1 shipped these as flat right-rail tabs with no repo dimension; 322 *needs* the repo dimension. Record the right-rail→center move as **drift in the 322 Handoff**. | 322, 324 |
| **D9** | Migration & settings-schema coordination | (a) **Migration-number reservation table** (§3) — each task owns a fixed number. (b) **Canonicalize `managed.json` on camelCase** per `design/12 §3.8`, with **serde `alias`** for the already-shipped snake_case keys (back-compat), noted as a one-line design-amendment in 310. (c) The published `project_settings.json` schema artifact (editor autocomplete) → folded into **310**. | 307, 310, 313, 315, 319 |

---

## 2. Resolved sub-decisions (smaller forks — locked so the 8 authors stay consistent)

| Area | Question | Locked answer |
|---|---|---|
| 301 | size→strategy recommendation surface | A **separate `EstimateRepoSize(url) → SizeReport` RPC** called *before* `AddRepository` (matches `design/02 §7.1` pre-clone sequence). `AddRepository` gains a real `strategy` arg; stops hardcoding `"full"`. |
| 302 | where the workspace-level cone-defaults layer lives | Inside **`workspaces.settings_json`** as a `{ repository_id: [cone_paths] }` map (NO new column). Freeze that nested JSON shape. Inheritance: `repositories.cone_defaults_json` → `workspaces.settings_json.cone_defaults` → `workarea_repos.sparse_cones_json`. |
| 303 | gix status approach | **Keep the V0.1 shell-out `status()` seam** (spike-104 GO: 25 ms p50, under the 100 ms bar). 303 only wires it through a per-workarea sparse cone + adds a Criterion bench gate. **No gix-native rewrite.** |
| 304 | idle-prefetch "idle" signal | **Injected as a testable closure/trait** (like fsmonitor's `is_alive`) so the scheduler is CI-provable; real client-heartbeat wiring is a small documented follow-on. Eager triggers (worktree-create + HEAD-update) ship fully. |
| 305 | `suggest_cones` | **Rust trait seam only**, unwired (delegates to Maestro 08, P4). The telemetry (`ConeStats` / `EstimateConeSize`) IS implemented in P3 (reads the git index). |
| 307 | `finished` / `partial` workarea status | **Both added** as persisted statuses (migration **0009**, recreate-table to widen the `workareas.status` CHECK) + FSM states + the proto status-comment. `partial` = a multi-repo workarea where ≥1 repo's `git worktree add` failed (`design/03 §8`). |
| 308 | per-workarea edit mutex placement | A **shared `EditMutexRegistry`** (`HashMap<WorkareaId, Arc<Mutex<()>>>`) in a neutral module both the workarea owner (03) and the supervisor (04) hold an `Arc` to. |
| 309 | reference worktree for files-to-copy | **First repo by `workspace_repos.position`** (the ordering column 306 adds — see §3). No per-project designation field in V1.0. |
| 311 | `exclude_from_maestro` surface | **Typed proto `bool` on the `Workarea` message** (next free field number), derived from `workareas.settings_json.exclude_from_maestro`. Sets the precedent for future derived settings keys. |
| 312 / 321 | one-shot LLM ownership | **312 owns** `crates/core/src/llm/oneshot.rs`: the `OneShotLlm` trait + a `DeterministicOneShot` impl (LIVE) + `compose_action_prompt` (reads 310's resolved `action_prefs`). **321 reuses** it for PR title/body — adds no new LLM machinery. |
| 313 | `fetch_issue` routing | GitHub `fetch_issue(repo, number)` stays on the `VcsProvider` trait; a top-level `VcsHandle::fetch_issue(url)` **router** dispatches GitHub-vs-Linear-vs-Jira by URL host (`design/13 §6.1`). |
| 313 | crate placement | A dedicated **`crates/vcs`** crate (mirrors `crates/relay`/`crates/transport`) houses `VcsProvider` + `GitHubProvider`(octocrab) + `GitHubProviderViaCli` + Linear/Jira + the `testkit` feature — contains the octocrab/graphql_client/reqwest tree so the Core's dep graph + cargo-deny surface stay bounded. **313 freezes the crate name.** Vet octocrab's tree with `cargo deny` first; an advisory-ignore is a Stop-and-ask (operator decision). |
| 316 / 324 | VCS events on the wire | **Route on existing broadcast + a `checks.<wa>.<repo>` subject with an opaque payload — NO new `streams.proto` `Event` oneof arm** (the oneof is frozen through field 16). Keeps every client wire-compatible; 324 parses the opaque frame. |
| 318 | required-checks set | The required set is a **caller parameter** (`320` supplies it), **defaulting to "all check-runs for the SHA reach a terminal conclusion."** No branch-protection API read in V1.0. |
| 319 | merge ownership home | `MergeWorkareaPrSet` / `RevertWorkareaPrSet` / `GetWorkareaMergePlan` on the **`Workareas`** service (see D7). 319 adds `GetWorkareaPrSet` (if absent) + `SetMergeOrder`. |
| 322–324 | verification dir | **`apps/desktop`** (NOT `apps/web` — that crate doesn't exist until P5/519). Each desktop task's `Verification` **overrides** the orchestrator default to `pnpm -C apps/desktop typecheck|lint|test|build` (Task 218 already added those scripts). |
| 305/302 | smoke gate | **302 adds a new `sparse-cone-clone` capability** (`scripts/smoke.d/<NN>-sparse-cone-clone.sh`) — a blobless+sparse clone of a small CI fixture + a cone-set + a `status` assertion. Other Phase-3 rust tasks: `unchanged` unless they say otherwise. |

---

## 3. Migration-number reservation

Current last shipped migration is **`0008_pull_requests.sql`**. Phase-3 migrations are reserved
**in task order** below. A task with NO row here adds **no** migration (it uses an existing
column, a `settings_json`/JSON key, the keychain, or repo-local state).

> **Author check (do this first):** confirm the actual highest `crates/persist/migrations/NNNN_*.sql`
> on `main` before writing. If a Phase-2 task landed a migration above 0008, **shift this whole
> block up by the same offset, preserving order** — and note it in your Handoff.

| Migration | Owner task | Adds |
|---|---|---|
| `0009` | **306** | `workspace_repos.position INTEGER` — deterministic repo order (drives "first-listed reference repo" for 309 + stable multi-repo UI). |
| `0010` | **307** | Recreate `workareas` to widen the `status` CHECK with `finished` + `partial`. |
| `0011` | **310** | `repositories.action_prefs_json TEXT` — the local-DB layer of the settings precedence chain (per-repo `pr_create`/`branch_rename` prefs, `design/04 §3.13`). |
| `0012` | **313** | `vcs_credentials` table — non-secret VCS metadata (provider, scope, external account/app/installation id, token_expires_at). **Secrets stay in the keychain (D4).** |
| `0013` | **315** | `webhook_deliveries` table — delivery-id idempotency that survives restart (id, repo_id, received_at; TTL cleanup). |
| `0014` | **319** | `pull_requests` += `merge_order INTEGER`, `external_id TEXT` (GraphQL node id), `repository_full_name TEXT` (octocrab needs both for GraphQL thread/resolve). |

311 (`exclude_from_maestro`) = `workareas.settings_json` JSON key, **no migration**.
302/305 = existing columns / git index, **no migration**.
320.5 (write-back enable) = `projects.settings_json` JSON key, **no migration**.

---

## 4. Cross-cutting FROZEN contracts (owner → consumers)

**4.1 Keychain VCS secrets — FROZEN by 313 (D4).** Parameterized accessor mirroring Task 218's
`CoreSecretSlot`: `Secrets::{get,set,delete}_vcs_secret(scope_id: &str, slot: VcsSecretSlot)`,
account string `vcs.<scope_id>.<slot_slug>`. `scope_id` = `app_id` (GitHub App), `repo_id`
(webhook secret), or the provider account id (Linear/Jira). **Never** add closed `SecretKind`
variants; **never** put VCS secrets in `vcs_credentials` or `cores.json`.

**4.2 Iroh channel tag `0x04` (Webhook) — RESERVED by 315.0, implemented by 315 (D3).** Joins
`0x01 Api`, `0x02 PushHint`, `0x03 Pairing`. Relay→Core inbound webhook pump; Core verifies HMAC
with the per-repo `VcsSecretSlot::WebhookSecret`. The exact relay→Core envelope (delivery-id,
endpoint targeting, signature header passthrough, body) is pinned by the 315.0 doc amendment.

**4.3 `crates/vcs` + `testkit` — FROZEN by 313 (D2).** The `VcsProvider` trait (method set per
`design/13 §3.8` — the author transcribes it faithfully and freezes it), `GitHubProvider`
(octocrab, default) + `GitHubProviderViaCli` (fallback), and a `#[cfg(feature = "testkit")]`
module exposing `wiremock`-backed `FakeGitHub`/`FakeLinear`/`FakeJira` builders + recorded
fixtures under `crates/vcs/tests/fixtures/`. 314/315/316/317/320/320.5 enable
`concerto-vcs/testkit` as a dev-dep. New workspace pins introduced here: `octocrab`,
`graphql_client`, `wiremock` (cargo-deny-clean — verify).

**4.4 `OneShotLlm` seam — FROZEN by 312, reused by 321 (D1).** `crates/core/src/llm/oneshot.rs`:
`trait OneShotLlm { async fn suggest(&self, req: OneShotRequest) -> Result<String> }` +
`DeterministicOneShot` (the LIVE P3 impl: slug-from-prompt for branch names, template title/body
for PRs) + `compose_action_prompt(action, prefs, context)` reading 310's resolved `action_prefs`.
The real pluggable provider is **unwired in P3** (P4/412 supplies it). 305's `suggest_cones` is a
*separate* Maestro-delegate seam, also unwired.

**4.5 Coordinated-merge RPCs — FROZEN by 319/320 on the `Workareas` service (D7).** 319:
`GetWorkareaPrSet(WorkareaId) → PrSet`, `SetMergeOrder(SetMergeOrderRequest) → PrSet`. 320:
`GetWorkareaMergePlan(WorkareaId) → MergePlan`, `MergeWorkareaPrSet(WorkareaId) → stream MergeProgress`,
`RevertWorkareaPrSet(WorkareaId) → RevertReport`. Pause-on-fail surfaces via the `MergeProgress`
stream (`Step N of M failed`). 324 binds these + the opaque `checks.<wa>.<repo>` frames.

**4.6 Repo-manager proto shapes — FROZEN by their owners.** `ConeStats { uint64 file_count = 1;
uint64 disk_size_bytes = 2; }` and `EstimateConeSizeRequest { string repository_id = 1; repeated
string cone_paths = 2; }` (305). `PrewarmProgress { uint64 blobs_fetched = 1; uint64 blobs_total = 2;
bool done = 3; }` (304). `CloneStrategy` enum serializing to the existing
`repositories.clone_strategy` TEXT values `full|blobless|treeless` + a `sparse` flag (301).
`SizeReport` for `EstimateRepoSize` (301).

---

## 5. The two inserts (amend `README.md §6` Phase-3 inventory)

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| **315.0** | Amend `design/11`/`design/13`: relay→Core inbound-webhook framing on the new `0x04` Webhook channel (envelope, endpoint targeting, offline-Core drop). Runs before 315. (doc, like Task 200.) | 215 | 3 | doc |
| **320.5** | Linear/Jira issue **write-back** on coordinated-merge completion (transition issue status; per-project opt-in via `projects.settings_json`; reuses 317's `write_back` seam + the `testkit` doubles). | 317, 320 | 2 | rust |

---

## 6. Refined dependencies (beyond the README inventory rows)

These deps are implied by the decisions above and MUST appear in the task files' `Depends on`:

- **312** also depends on **310** (consumes the resolved `action_prefs` + defines `OneShotLlm`).
- **321** also depends on **312** (reuses `OneShotLlm` + `compose_action_prompt`) and **310**.
- **314, 315, 316, 317, 320, 320.5** also depend on **313** (the `crates/vcs` trait + `testkit`).
- **315** also depends on **315.0** (the design amendment) and **315.0** on **215** (the WSS/relay).
- **320.5** depends on **317** + **320**.
- **309** depends on **306** (reads `workspace_repos.position`).
- **322** depends on **302, 306** (multi-repo + cones) and **324** on **319, 320**.

---

## 7. Verification note for the desktop tasks (322–324)

The orchestrator's `web-ts` command set (`README §5.3`) targets `apps/web`, which **does not exist
until Phase 5 (task 519)**. Each of 322/323/324 MUST put an explicit `Verification` override:
`pnpm -C apps/desktop typecheck && pnpm -C apps/desktop lint && pnpm -C apps/desktop test && pnpm -C apps/desktop build`
(Task 218 added those scripts + `vitest`). The Tier-2 double is the existing mocked-`invoke`
+ React-Query/Zustand component tests; real cross-machine remote-mode rendering is the Phase-3
Tier-3 checklist's job, not these tasks'.

---

## 8. Concurrency / wave map (pipelined + bounded-parallel execution)

The orchestrator runs Phase 3 **pipelined and up to K = 3 file-disjoint tasks in flight** per
`AUTO_EXECUTE_PROMPT.md` → *Concurrency model* (+ tiered validation, `README.md §5.3/§5.4`).
This section is the phase-specific input to that model: the clusters, their shared seams, and
which tasks are safe to overlap. **The merge invariant is unchanged: dependency-ordered,
serialized merges; `main` always green; in-flight branches rebase onto each new `main`; a
substantive rebase conflict → re-dispatch the later task fresh.**

**Completion state (update as you go):** 301 ✅ merged · 302 ✅ merged · 303–324 + 315.0/320.5 pending.

**Clusters (tasks inside a cluster share hot files → keep *intra*-cluster work sequential; parallelize *across* clusters):**

| Cluster | Tasks | Hot shared files (intra-cluster collision) |
|---|---|---|
| **A — Repo-manager** | 303, 304, 305 | `crates/core/src/repo_manager/actor.rs`, `…/mod.rs`, `crates/core/src/handlers/repositories.rs`, `crates/proto/proto/concerto/v1/repositories.proto` (304 = `PrewarmBlobs`, 305 = `EstimateConeSize`/`ConeStats` — both append → collide). 303 is light (gix-wrap benches/tests + a small caller touch). |
| **B — Workspace/Workarea/Session** | 306→307→308, 309, 311, 312 | `crates/core/src/workspace_manager/*`, `crates/persist/src/workareas.rs`, workareas/sessions proto, migrations 0009 (306) / 0010 (307). Largely a dependency chain already. |
| **C — VCS** | 313→{314,316,317,320.5}, 315.0, 315, 318, 319, 320, 321 | `crates/vcs/*` (new, 313 freezes it — **must land first**), migrations 0012 (313)/0013 (315)/0014 (319), `Workareas` service proto (319/320), `crates/core/src/llm/oneshot.rs` (312 owns, 321 reuses). |
| **D — Settings** | 310 | `crates/persist/src/repositories.rs` + `…/api.rs` (migration 0011 `action_prefs_json`) — **note this also collides with 305**, which reads `repositories.cone_defaults_json`; don't run 305 ∥ 310. |
| **E — Desktop (web-ts)** | 322, 323, 324 | `apps/desktop/*` only — **file-disjoint from every Rust crate**, so a desktop task can overlap any Rust task once its Rust deps have merged. 322/323/324 collide with *each other* (same tree). |

**Soft seam to watch:** `crates/core/src/boot.rs` (handle wiring) is touched by many tasks; additive wiring in different regions usually auto-merges on rebase, but if two in-flight tasks edit the same region it conflicts → fallback. Treat as watch-on-rebase, not a hard block.
**Trivially-mergeable (never blocks concurrency):** `Cargo.lock`, workspace `Cargo.toml` member list, `docs/interfaces/*`, `scripts/smoke.manifest`, distinct `scripts/smoke.d/*`, distinct test files.

**Eligibility each tick** = dependency-ready (per the README inventory + §6 refined deps) **AND** file-disjoint on a hard seam from every in-flight task. Refined deps that matter for ordering: 313 gates 314/315/316/317/320/320.5; 315.0 gates 315; 310 gates 312/321; 312 gates 321; 319 gates 320; 320 gates 320.5/324.

**Suggested opening waves (illustrative — recompute eligibility each tick, prefer lowest-numbered + most-unblocking):**
- **Wave 1 (ready now, disjoint):** `303` (cluster A, light) ∥ `313` (cluster C root — start it early, it unblocks the whole VCS cluster) ∥ `315.0` (doc, `design/` only — zero code collision, ideal filler).
- **Wave 2:** as 313 merges → `317`/`316`/`314` become eligible (intra-C: serialize on `crates/vcs` source, or split by disjoint files); `306` (cluster B root) ∥ a cluster-A task (`304` or `305`, not both) ∥ `310` (settings — but **not** alongside `305`).
- **Desktop tasks (322/323/324)** slot in as concurrency fillers against Rust work once their Rust deps (302✓/306, 308, 320) have merged — they never collide with Rust files.

**If unsure whether two tasks are disjoint → serialize them.** A green `main` and correct interfaces outrank the speedup.

*End of Phase-3 planning addendum. The 24 inventory task files (301–324) + the 2 inserts
(315.0, 320.5) are written against this document, `README.md`, and the `design/` sections each
cites.*
