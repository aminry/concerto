# Task 319 — PR-Set Semantics: `merge_order` + `external_id` + `repository_full_name` (migration 0014), Ordered `GetWorkareaPrSet`, `SetMergeOrder`

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 308, 313 |
| Touches subsystem(s) | 03 (Workspace/Session Manager), 09 (Persistence), 13 (VCS Provider Integration) |
| Smoke gate | unchanged |

## Goal
Make the implicit per-workarea PR set first-class with a persisted, user-reorderable **`merge_order`**, and give octocrab (313/316/320) the two GraphQL handles it needs on each PR row. Today `pull_requests` (migration 0008) has no `merge_order`, no GraphQL node id, and no `owner/repo` string; `GetWorkareaPrSet` (Task 45) returns rows ordered by `pr_number` and its own doc-comment promises `merge_order` "is V1.0." This task lands **migration 0014** adding `pull_requests += merge_order INTEGER, external_id TEXT, repository_full_name TEXT`, threads the three columns through the persist `NewPullRequest`/`PullRequest` structs + upsert + `list_by_workarea`, **orders the PR set by `merge_order` (fallback `pr_number`)**, assigns `merge_order` by **insertion order** (`max(merge_order)+1` per workarea; D7), and adds a `SetMergeOrder` RPC on the **`Workareas`** gRPC service so the user (via task 324's drag UI) can reorder. After this task the PR set is an ordered `(repo, PR)` plan keyed by `workarea_id` with no join table — the exact data foundation task 320's coordinated merge loop iterates, and the surface task 324 binds.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.9 (Per-workarea PR set, implicit) — **the PR set is all `pull_requests` rows with `workarea_id = <this>`; no separate join table.** The merge plan is the **ordered list of (repo, PR) tuples derived from `pull_requests.merge_order`**; coordinated merge invokes each in order (via Scheduler 05 §3.9), coordinated revert in reverse `merge_order`. §5.1 — the `WorkareaManager` PR-set surface (`list_prs_in_workarea`, `get_merge_plan`, `merge_workarea_pr_set`, `revert_workarea_pr_set`) — **319 owns `list`/ordering + `SetMergeOrder`; `get_merge_plan`/`merge`/`revert` are task 320** (D7/§4.5). §3.9 R-6 (merge-anyway override) is 320's concern, not 319's.
- `design/13_VCS_Provider_Integration.md` §4 — `pull_requests` keyed `(workarea_id, repository_id)` **with `merge_order` (09 §4.5)**; the workarea's PR set is the implicit set of rows for that `workarea_id`. §3.5 (PR-set merge protocol: "fetch all pull_requests for this workarea, ordered by `merge_order`") — confirms the ordering contract 319 fulfils.
- `tasks/v1.0/PHASE3_PLANNING.md` §1 **D7** (`merge_order` default = **insertion order** = `max(merge_order)+1` per workarea; **`SetMergeOrder` RPC (319)** lets the user reorder; **324** UI drag writes it; **NO dependency-graph inference** — that's R-6/V2.0; coordinated-merge RPCs live on the **`Workareas`** gRPC service next to the existing `GetWorkareaPrSet`). §2 (319 row: "`MergeWorkareaPrSet`/`RevertWorkareaPrSet`/`GetWorkareaMergePlan` on the `Workareas` service (D7). 319 adds `GetWorkareaPrSet` (if absent) + `SetMergeOrder`."). §3 (migration reservation: **`0014` = `pull_requests += merge_order INTEGER, external_id TEXT (GraphQL node id), repository_full_name TEXT` — octocrab needs both for GraphQL thread/resolve**; 319 owns 0014). §4.5 (coordinated-merge RPC ownership split: 319 freezes `GetWorkareaPrSet`/`SetMergeOrder`; 320 freezes `GetWorkareaMergePlan`/`MergeWorkareaPrSet`/`RevertWorkareaPrSet`).
- **Author check (do FIRST):** confirm the highest `crates/persist/migrations/NNNN_*.sql` on `main`. As of writing it is `0008_pull_requests.sql`; PHASE3_PLANNING reserves 0009–0014 in task order (306→0009, 307→0010, 310→0011, 313→0012, 315→0013, **319→0014**). If those upstream Phase-3 migrations have NOT all landed when 319 runs, **0014 is still 319's reserved number** — do not renumber to fill a gap; use 0014 and note any skipped intermediate numbers in Handoff. If a Phase-2 migration landed above 0008 (it did not at planning time), shift the whole block up by the offset and note it.
- `crates/persist/migrations/0008_pull_requests.sql` — the shipped schema: `pr_number`, `base_ref`, `head_ref`, `state`, `title`, `body`, `url`, `head_sha`, `created_at`, `updated_at`, `UNIQUE(workarea_id, repository_id)`. **SCHEMA DIVERGENCE NOTE:** `design/09 §4.5`'s nominal column names (`external_id`, `repository_full_name`, `base_branch`, `head_branch`, `last_synced_at`) do NOT all match the shipped 0008 names — **do NOT rename any shipped column.** 0014 only ADDs `merge_order`, `external_id`, `repository_full_name` (the latter two named per `design/09 §4.5`); keep `base_ref`/`head_ref`/`updated_at` as shipped.
- `crates/persist/src/pull_requests.rs` — `upsert` (the `INSERT … ON CONFLICT(workarea_id, repository_id) DO UPDATE` column list to extend), `list_by_workarea` (currently `ORDER BY pr_number` — change to `ORDER BY merge_order, pr_number`), `get`, `row_to_pull_request`. The module doc-comment carries the locked 0008 schema block — re-lock it with the 0014 additions.
- `crates/persist/src/api.rs` — `NewPullRequest` + `PullRequest` structs (lines ~938 / ~959) to extend with the three fields. The persistence layer is dumb storage; the caller supplies `merge_order` (the upsert helper computes the insertion-order default — see Implementation notes).
- `crates/core/src/vcs/actor.rs` `upsert_from_detail` — the call site that builds `NewPullRequest` from a `gh_cli::PrDetail`; it must populate the three new fields (`external_id`/`repository_full_name` from the provider detail; `merge_order` via the insertion-order default). **NOTE:** 313 relocates this into `crates/vcs`; if 313 already extended `NewPullRequest`'s construction for octocrab, align with its Handoff rather than double-adding fields.
- `crates/proto/proto/concerto/v1/vcs.proto` — the `PullRequest` message: **fields 1–14 FROZEN (Task 45); field 15 is the next free number.** Add `merge_order` at 15, `external_id` at 16, `repository_full_name` at 17.
- `crates/proto/proto/concerto/v1/workareas.proto` — the existing `GetWorkareaPrSet(WorkareaId) returns (GetWorkareaPrSetResponse)` RPC (Task 45) whose doc-comment says *"V0.1 returns rows ordered by `pr_number`; PR-set merge ordering (`merge_order`) is V1.0"* — **319 fulfils that promise**: do NOT add a new fetch RPC, change the ordering + add `SetMergeOrder`.
- `crates/core/src/handlers/workareas.rs` (line ~216 `get_workarea_pr_set`, line ~336 `pull_request_to_proto`) + `crates/core/src/workspace_manager/workarea.rs` (line ~396 `list_pr_set` → delegates to `pull_requests::list_by_workarea`). These are the handler + manager call sites to wire the ordering + the new `SetMergeOrder` path through.
- `tasks/v1.0/308-multi-session-edit-mutex.md` → "Handoff Notes" (when it exists) — 308 is a declared dep for the multi-session workarea context the PR set lives under; confirm no conflicting `pull_requests` change. 313's Handoff — the octocrab provider that fills `external_id`/`repository_full_name`.

## Scope — in
- **Migration `0014_pull_requests_merge_order.sql`:** `ALTER TABLE pull_requests ADD COLUMN merge_order INTEGER NOT NULL DEFAULT 0; ALTER TABLE pull_requests ADD COLUMN external_id TEXT NOT NULL DEFAULT ''; ALTER TABLE pull_requests ADD COLUMN repository_full_name TEXT NOT NULL DEFAULT '';` — purely additive `ADD COLUMN` (no table recreation; the frozen `UNIQUE(workarea_id, repository_id)` + all 0008 columns untouched).
- `NewPullRequest` + `PullRequest` structs gain `merge_order: i64`, `external_id: String`, `repository_full_name: String`. `upsert` column lists + binds extended; `external_id`/`repository_full_name` participate in the `DO UPDATE SET` (they refresh on re-sync); **`merge_order` is preserved across upserts** (like `created_at`) so a re-sync of a PR row does NOT reset a user's reordering — only the first insert assigns it.
- A `set_merge_order(conn, pr_id, order) -> Result<()>` persist helper + a `next_merge_order(pool, workarea_id) -> Result<i64>` helper (`SELECT COALESCE(MAX(merge_order), -1) + 1 FROM pull_requests WHERE workarea_id = ?`) used by `upsert_from_detail` to assign the **insertion-order default** (D7) on first insert.
- `list_by_workarea` (and thus `list_pr_set`) ordered `ORDER BY merge_order, pr_number` — deterministic, fallback to `pr_number` when two rows share a `merge_order`.
- `WorkareaManager::set_merge_order(workarea_id, repository_id, order)` (or by `pr_id`) on the workarea manager + a `Workareas.SetMergeOrder` gRPC handler that validates the workarea + PR exist, writes the order, and returns the re-ordered `GetWorkareaPrSetResponse` (so the client gets the new ordering in one round-trip; mirrors how `Update*` RPCs return the updated entity).
- proto: `PullRequest` += `merge_order=15`, `external_id=16`, `repository_full_name=17`; new `SetMergeOrderRequest { string workarea_id = 1; string repository_id = 2; int64 merge_order = 3; }`; `rpc SetMergeOrder(SetMergeOrderRequest) returns (GetWorkareaPrSetResponse);` on the `Workareas` service; `pull_request_to_proto` (both copies — `handlers/vcs.rs` and `handlers/workareas.rs`) populates the three new fields.
- Tests (Tier 1): migration round-trips the three columns; `upsert` of a new PR assigns `merge_order = max+1` (two PRs in one workarea get 0, 1); a re-`upsert` of an existing PR preserves its `merge_order` (does not reset to a default); `list_by_workarea` returns rows in `merge_order` then `pr_number`; `set_merge_order` reorders and `GetWorkareaPrSet`/`SetMergeOrder` return the new order; `external_id`/`repository_full_name` round-trip; a multi-repo workarea (distinct `repository_id`s) yields multiple ordered rows; `UNIQUE(workarea_id, repository_id)` still enforces one PR per repo per workarea.

## Scope — out
- The **coordinated merge/revert loop** + `GetWorkareaMergePlan`/`MergeWorkareaPrSet`/`RevertWorkareaPrSet` RPCs — **task 320** (Tier-2; D7/§4.5 puts these on the `Workareas` service too, but 320 owns them). 319 is the **data model + ordering + `GetWorkareaPrSet` + `SetMergeOrder`** only.
- Creating PRs (the octocrab/gh `create_pr` that populates `external_id`/`repository_full_name`) — **task 313**; 319 only adds the columns + threads them through the existing upsert path.
- The **drag-to-reorder UI** that calls `SetMergeOrder` — **task 324** (web-ts, Desktop PR-set panel).
- Any **dependency-graph inference** of merge order (topological-by-repo-dep) — explicitly **R-6/V2.0** (D7); V1.0 is pure insertion-order + manual reorder.
- A new `streams.proto` `Event` oneof arm for PR-set changes — the `Event` oneof is FROZEN through field 16; PR-set change events ride the existing `workarea.events` broadcast (`design/03 §5.3`) / `pr_set.events` (320), not a new oneof variant.
- GraphQL review-thread fetch/resolve that *uses* `external_id`/`repository_full_name` — **task 316**; 319 only provides the columns it reads.

## Public interface this task locks
- **Migration `0014` (FROZEN):** `pull_requests += merge_order INTEGER NOT NULL DEFAULT 0`, `external_id TEXT NOT NULL DEFAULT ''`, `repository_full_name TEXT NOT NULL DEFAULT ''`. Additive `ADD COLUMN` only; the 0008 `UNIQUE(workarea_id, repository_id)` + every shipped column name unchanged.
- **Persist structs (FROZEN additions):** `NewPullRequest` + `PullRequest` gain `merge_order: i64`, `external_id: String`, `repository_full_name: String`. `merge_order` is preserved across upserts (caller-or-default assigned on first insert only); `external_id`/`repository_full_name` refresh on upsert.
- **proto `PullRequest` (FROZEN field numbers, additive):** `int64 merge_order = 15;`, `string external_id = 16;`, `string repository_full_name = 17;` (fields 1–14 stay frozen from Task 45).
- **proto `Workareas` service (FROZEN):** `message SetMergeOrderRequest { string workarea_id = 1; string repository_id = 2; int64 merge_order = 3; }`; `rpc SetMergeOrder(SetMergeOrderRequest) returns (GetWorkareaPrSetResponse);`. The existing `GetWorkareaPrSet(WorkareaId) returns (GetWorkareaPrSetResponse)` is REUSED (ordering changed to `merge_order, pr_number`), NOT re-numbered. `GetWorkareaMergePlan`/`MergeWorkareaPrSet`/`RevertWorkareaPrSet` are RESERVED for task 320 — do NOT add them here.
- **Ordering contract (FROZEN):** the PR set is the implicit set of `pull_requests` rows for a `workarea_id`, ordered `(merge_order, pr_number)`; `merge_order` default = insertion order (`max(merge_order)+1` per workarea). No join table.

## Implementation notes
- **Insertion-order default, computed at the caller, preserved on re-upsert.** The `upsert` SQL keeps `merge_order` out of the `DO UPDATE SET` clause (like `created_at`) so a PR re-sync never clobbers a user's reorder. The first insert needs a value: `upsert_from_detail` calls `next_merge_order(pool, workarea_id)` before building `NewPullRequest`. Because two PRs created back-to-back race on `MAX(merge_order)`, compute the default and insert inside the **same writer lock** the upsert already takes (the persistence writer is serialized — `self.persistence.writer().await` is a single connection), so the `MAX` and the insert are atomic.
- **Two `pull_request_to_proto` copies.** There are independent converters in `handlers/vcs.rs` (line ~201) and `handlers/workareas.rs` (line ~336). Update BOTH to populate the three new fields, or extract a shared converter — either is fine; do not let them drift.
- **`SetMergeOrder` returns the re-ordered set.** Mirror the `UpdateWorkareaPermissionMode`-style "return the updated entity" convention: after writing the order, re-read `list_pr_set` and return a `GetWorkareaPrSetResponse` so the Desktop drag UI (324) re-renders from the authoritative order in one round-trip.
- **`external_id` / `repository_full_name` population is 313's job at create time; 319 just stores them.** For rows created before 313 wires octocrab, they default to `''` (the migration default) — harmless; 316's GraphQL paths only run for octocrab-backed repos that have them. Do not block on 313 to land the columns + ordering.
- **Cross-platform.** Pure SQLite + Rust; no platform concerns. The migration runs identically on the Windows/Linux CI lanes (Task 113).
- Regen: schema + proto change ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/proto.md` (+ `rust-api.md` for the persist structs); commit both.

## Verification
**Tier 1.** Fully CI-self-verifiable (migration + structs + ordering + RPC); no network. (The real coordinated merge against GitHub is task 320 + the Phase-3 Tier-3 checklist — not this task.)
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-persist pull_request` → migration round-trips the three columns; insertion-order default (0,1) on two PRs; re-upsert preserves `merge_order`; `list_by_workarea` ordered `(merge_order, pr_number)`; `set_merge_order` reorders; `external_id`/`repository_full_name` round-trip; `UNIQUE(workarea_id, repository_id)` enforced.
4. `cargo test -p concerto-core workareas` → `GetWorkareaPrSet` returns merge-order-sorted rows; `SetMergeOrder` writes + returns the re-ordered set; a multi-repo workarea yields multiple ordered rows.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → green (no new dependency).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `PullRequest.merge_order/external_id/repository_full_name`, `SetMergeOrder`; `rust-api.md` gains the persist struct fields).
8. `scripts/smoke.sh` → **unchanged** (no smoke capability; coordinated merge is 320).

## Definition of Done
- [x] Migration `0014` ADDs `merge_order`/`external_id`/`repository_full_name` to `pull_requests` (additive; 0008 `UNIQUE` + columns untouched); reserved-number check noted in Handoff
- [x] `NewPullRequest`/`PullRequest` + `upsert` + `list_by_workarea` thread the three fields; `merge_order` preserved on re-upsert, `external_id`/`repository_full_name` refreshed
- [x] Insertion-order default (`max(merge_order)+1` per workarea) assigned atomically under the writer lock; `set_merge_order` helper
- [x] `GetWorkareaPrSet` ordered `(merge_order, pr_number)`; `SetMergeOrder` RPC on the `Workareas` service returns the re-ordered set
- [x] proto: `PullRequest` += 15/16/17, `SetMergeOrderRequest` + `SetMergeOrder` RPC — FROZEN numbers; 320's merge/revert/plan RPCs NOT added here
- [x] Both `pull_request_to_proto` copies populate the new fields
- [x] All `rust` §5.3 commands pass; interfaces regenerated (proto.md + rust-api.md)
- [x] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams in Handoff)
- [x] Single commit with the message below

## Outputs
- `crates/persist/migrations/0014_pull_requests_merge_order.sql` (new)
- `crates/persist/src/pull_requests.rs` (modified — schema doc-block re-lock; `upsert` columns; `list_by_workarea` ordering; `set_merge_order` + `next_merge_order` helpers; `row_to_pull_request`)
- `crates/persist/src/api.rs` (modified — `NewPullRequest`/`PullRequest` += three fields)
- `crates/core/src/vcs/actor.rs` (modified — `upsert_from_detail` populates the three fields via the insertion-order default; **align with 313 if it already extended this**)
- `crates/core/src/workspace_manager/workarea.rs` (modified — `set_merge_order` on `WorkareaManager`; `list_pr_set` ordering flows from persist)
- `crates/core/src/handlers/workareas.rs` (modified — `SetMergeOrder` handler; ordered `get_workarea_pr_set`; `pull_request_to_proto` += fields)
- `crates/core/src/handlers/vcs.rs` (modified — `pull_request_to_proto` += fields)
- `crates/proto/proto/concerto/v1/vcs.proto` (modified — `PullRequest` 15/16/17)
- `crates/proto/proto/concerto/v1/workareas.proto` (modified — `SetMergeOrderRequest` + `SetMergeOrder` RPC)
- `docs/interfaces/proto.md` + `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: PR-set semantics — merge_order + external_id + repository_full_name

Migration 0014 adds merge_order/external_id/repository_full_name to
pull_requests (additive). The implicit per-workarea PR set is now ordered
by merge_order (default = insertion order, max+1 per workarea), preserved
across re-syncs. Adds SetMergeOrder on the Workareas service; GetWorkareaPrSet
now sorts by merge_order. Coordinated merge is task 320.

Refs: tasks/v1.0/319-pr-set-semantics.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan: (1) `upsert_from_detail` lives in `crates/vcs/src/actor.rs`, NOT `crates/core/src/vcs/actor.rs` — 313 relocated it into the `concerto-vcs` crate; this task edited the real location. (2) `crates/core/tests/webhook_ingest.rs` (one `NewPullRequest` literal) was beyond the listed `Outputs` but had to gain the three new fields to compile — a mechanical add, no behavior change. (3) Added a `pull_requests::id_by_workarea_repo(pool, workarea_id, repository_id)` persist helper (not named in Outputs) so `SetMergeOrder` can turn the wire's `(workarea, repository)` key into the row primary key — the `SetMergeOrderRequest` keys on `(workarea_id, repository_id)`, and the seam needed a read path. (4) Migration reserved-number check: highest on `main` is `0013_webhook_deliveries.sql`; all upstream Phase-3 migrations (0009–0013) landed as planned, so `0014` is the natural next number — no gap, not renumbered. (5) Fixed a real seeded-draft bug: `crates/persist/tests/pull_requests_merge_order.rs` used `uuid::Uuid::now_v7()` but `concerto-persist` has no `uuid` dev-dep (would not compile under clippy `--all-targets`); switched its `new_pr` helper to a deterministic `pr-{workarea}-{repo}` id (the upsert keys on `(workarea_id, repository_id)`, so a stable id is correct and avoids a new dependency). The core test's `uuid` use is fine — `concerto-core` already depends on `uuid`.
- Open questions for next task (320): `GetWorkareaPrSet` returns the set ordered `(merge_order, pr_number)` — iterate it directly for coordinated merge; reverse it for coordinated revert (`design/13 §3.5`). `merge_order` is dense-on-first-insert (0,1,2,…) but `SetMergeOrder` writes arbitrary i64 (the tests use `-1`/`-5` to move-to-front), so 320 must NOT assume contiguous/non-negative orders — sort, don't index. `external_id`/`repository_full_name` are `''` on `gh`-CLI-created rows (only octocrab populates `external_id`); 316's GraphQL paths must tolerate empty values. `WorkareaManager::set_merge_order(workarea_id, repository_id, order)` + `list_pr_set(workarea_id)` are the manager-level handles; `GetWorkareaMergePlan`/`MergeWorkareaPrSet`/`RevertWorkareaPrSet` proto RPCs are RESERVED and NOT added (320 owns them on the `Workareas` service).
- Deliberate debt: none. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code. `external_id` is never populated by the `gh`-CLI upsert path (only `repository_full_name` is, via `resolve_repo_full_name`); this is by design (313/316's octocrab create populates `external_id`), defaulting to `''` per the migration.
- Smoke-gate state: unchanged (no smoke capability added; coordinated merge is task 320). Full `rust` gate green: `cargo check`/`clippy -D warnings`/`test --workspace --no-fail-fast` (853 passed, 0 failed, incl. 6 new persist + 4 new core tests) / `cargo deny check` (RUSTSEC-2023-0071 ratified, no new advisory) / `regen-interfaces.sh` + `git diff --exit-code docs/interfaces/` clean / `cargo fmt --all -- --check` clean.
