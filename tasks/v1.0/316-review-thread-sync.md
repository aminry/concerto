# Task 316 — Review-Thread Sync (GraphQL) + Check-Run/Deployment Aggregation on `checks.<wa>.<repo>`

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 313 |
| Touches subsystem(s) | 13 (VCS Provider Integration), 10 (Client API Protocol — events) |
| Smoke gate | unchanged |

## Goal
Make a workarea's PRs show their **review threads, check runs, and deployments** live, off webhooks, without persisting any of it. Today the V0.1 VCS layer fetches check runs only via `gh pr view --json statusCheckRollup` (`crates/core/src/vcs/gh_cli.rs`), has **no review-thread support** (GitHub's review threads are GraphQL-only), **no deployment aggregation**, and emits no per-(workarea,repo) events. This task implements, on the octocrab `GitHubProvider` that **Task 313** stood up: (1) **`list_review_threads(pr)`** via GitHub's GraphQL API (one query, full structure — `design/13 §3.6`), cached in-memory keyed `(pr_id, thread_id)`, **never** written to SQLite (GitHub is canonical), refreshed on workarea open; (2) **`resolve_thread(id)`** as the GraphQL mutation (update cache + emit on success); (3) **check-run + deployment aggregation** into the `VcsState` caches (`design/13 §4`: `check_cache` TTL 30s); and (4) **event emission** of `pr.thread_updated` / `pr.check_run_updated` / `pr.deployment_updated` on the **`checks.<workarea_id>.<repository_id>`** subject (`design/13 §5.3`) carrying an **opaque payload** — with **no new `streams.proto` `Event` oneof arm** (the oneof is frozen through field 16; `PHASE3_PLANNING §2`). It consumes Task 315's webhook-receipt cache-invalidation hook for the targeted, instant-update path and the §6.3 TTL/force-refresh paths. After this task the Desktop Checks panel + Diff viewer (Task 324) render threads/checks/deploys inline and update instantly off webhooks; the Tier-2 double is recorded GraphQL+REST fixtures via 313's `testkit`, and real GitHub thread sync is the Phase-3 Tier-3 checklist line.

## Inputs to read before starting
- `design/13_VCS_Provider_Integration.md` §3.6 (review-thread sync: **GraphQL preferred** — one query, full structure; on update → cache `(pr_id, thread_id)` + emit `checks.<workarea_id>.<repository_id>`; "Send to agent" composes a message with thread context routed to a user-picked session; "Mark resolved" = GraphQL mutation → update cache + emit; **NOT persisted to SQLite** — GitHub is canonical; refresh from origin on workarea open). This is the spine of the task.
- `design/13_VCS_Provider_Integration.md` §5.3 (the **exact events + subjects**: `pr.check_run_updated` / `pr.deployment_updated` / `pr.thread_updated` all on `checks.<workarea_id>.<repository_id>`). **Trust §5.3's per-(workarea,repo) subject** — note §6.2's diagram says `checks.<workspace_id>` but that is the diagram's shorthand; §5.3 is authoritative and matches the existing `GetWorkareaRepoDiff` per-(workarea,repo) scoping.
- `design/13_VCS_Provider_Integration.md` §4 (`VcsState`: `check_cache: HashMap<(RepoId, ShaString), Vec<CheckRun>>` TTL 30s; `threads_cache: HashMap<PullRequestId, Vec<ReviewThread>>` refreshed-on-open — the two caches this task fills), §6.3 (cache invalidation: **webhook-targeted** → just the affected PR/check/thread; **TTL expiry** → lazy refresh on next read; **user force-refresh** → fetch everything for the open workarea — implement all three), §3.8 (the `VcsProvider` trait whose `list_review_threads`/`resolve_thread`/`list_deployments` methods 313 FROZE and this task implements on `GitHubProvider`).
- `design/03_Workspace_Session_Manager.md` §5.3 — the `checks.<workarea>.<repo>` stream definition (the per-(workarea,repo) granularity 316 emits on, matching the existing diff scoping).
- `crates/proto/proto/concerto/v1/streams.proto` — the **FROZEN `Event` oneof** (`session = 10` … `transport = 16`; field 16 is the ceiling). `PHASE3_PLANNING §2` mandates **no new oneof arm**: the `checks.<wa>.<repo>` payload must ride a **new `Subject` variant + an opaque carrier that is NOT a oneof arm** (see Implementation notes for the field-17-non-oneof-`bytes` decision). Read how `transport = 16` was added (Task 216) to match the additive discipline — but the new field here is a **sibling of the oneof**, not inside it.
- `crates/core/src/handlers/streams.rs` — the `Subject` enum (`SessionEvents`/`SessionIo`/`WorkspaceEvents`/`WorkareaEvents`/`SuggestionEvents(Option<String>)`/`TransportEvents`) + `parse_subject` (the string → typed-`Subject` parser at ~line 836) + the per-subject ring-buffer pump. Add `Subject::Checks { workarea_id, repository_id }` parsed from `checks.<workarea_id>.<repository_id>`, mirroring how `suggestion.events.<workarea_id>` is parsed. The ring-bound default (`RING_EVENT_CAP`) applies (these are count-bounded events, not byte-bounded like `session.io`).
- `crates/core/src/vcs/actor.rs` + `gh_cli.rs` — the V0.1 `get_check_runs` (the `statusCheckRollup` path) + `repository_id → owner/repo` resolution; after **Task 313** the octocrab `GitHubProvider` is in `crates/vcs`. 316 adds the GraphQL thread query/mutation + the deployments REST call + the check-run aggregation on top of 313's client. **313 is the hard dependency** (the `VcsProvider` trait, `ReviewThread`/`ThreadId`/`Deployment`/`CheckRun` value types, the octocrab client, and the `testkit`).
- `crates/proto/proto/concerto/v1/vcs.proto` — the FROZEN V0.1 `Vcs` service (5 RPCs, field numbers frozen at Task 45; `CheckRun`/`Issue` shapes). 316 **appends** `ListReviewThreads` / `ResolveThread` / `ListDeployments` RPCs + their new messages (`ReviewThread`/`ReviewThreadComment`/`Deployment`/the request/response types) — never renumber the frozen 5. (The `VcsHandle::list_review_threads`/`resolve_thread`/`list_deployments` are already in `design/13 §5.1`.)
- `crates/core/src/handlers/` — where the `Vcs` service handler lives (mirror `handlers/streams.rs` event emission). The new RPCs + the emission of the three `checks.*` events live here / in the VCS actor.
- `tasks/v1.0/313-vcs-provider-github.md` → "Handoff Notes" — the `crates/vcs` crate name, the FROZEN `VcsProvider` method set + `ReviewThread`/`ThreadId`/`Deployment` value types (the author transcribed `design/13 §3.8`), octocrab's `graphql()` capability (or whether 313 pulled `graphql_client`), and the `#[cfg(feature = "testkit")]` `FakeGitHub` GraphQL-fixture builders + `crates/vcs/tests/fixtures/`. **316 enables `concerto-vcs/testkit` as a dev-dep** (`PHASE3_PLANNING §4.3`).
- `tasks/v1.0/315-webhook-receiver.md` → "Handoff Notes" — the **cache-invalidation hook** webhook receipt leaves (the §6.3 "targeted invalidation" seam); 316 consumes it to drive the instant `checks.*` update path. If 315 has not merged, 316's TTL + force-refresh paths still work poll-only; the webhook-triggered path is wired when 315 lands (state this in Scope/Handoff).

## Scope — in
- **`list_review_threads(pr)`** on `GitHubProvider` (313's octocrab client): one GraphQL query returning the full thread structure (`design/13 §3.6`) → `Vec<ReviewThread>` (the 313-frozen type: thread id, path, line, resolved flag, comments). Cache in `VcsState.threads_cache` keyed by `PullRequestId`; **refresh-on-workarea-open** (the open trigger calls this); **never** persist to SQLite (`design/13 §3.6` / R-3 "no local PR-diff cache"). Use octocrab's `graphql()` (or 313's `graphql_client`) — confirm 313's choice; do not add a new GraphQL dep without checking 313's tree.
- **`resolve_thread(id)`** as the GraphQL `resolveReviewThread` mutation → on success, update the cached thread's resolved flag + emit `pr.thread_updated` on `checks.<wa>.<repo>`.
- **Check-run aggregation:** fetch check runs (octocrab Checks API for the PR head SHA) into `VcsState.check_cache` keyed `(RepoId, Sha)` with **TTL 30s** (`design/13 §4`). Emit `pr.check_run_updated` on cache change.
- **Deployment aggregation:** `list_deployments(repo, ref)` (octocrab Deployments API) → emit `pr.deployment_updated` on change. (No new cache column required beyond `design/13 §4`'s in-memory shape.)
- **Event emission on `checks.<workarea_id>.<repository_id>`** (`design/13 §5.3`) carrying an **opaque payload** — `pr.thread_updated` / `pr.check_run_updated` / `pr.deployment_updated`. The payload is a small self-describing frame (a deterministic-CBOR or JSON map with a `kind` discriminator + the changed entity) that **324 parses opaquely**; the wire `Event` carries it in the **new non-oneof `bytes` carrier** (Implementation notes), **not** a new oneof arm. Add `Subject::Checks { workarea_id, repository_id }` + its `parse_subject` arm + ring-buffer registration.
- **Cache invalidation (all three paths, `design/13 §6.3`):** (a) **webhook-targeted** — consume 315's invalidation hook to refresh just the affected PR/check/thread + emit; (b) **TTL expiry** — lazy refresh on next read when the 30s `check_cache` entry is stale; (c) **user force-refresh** — a "refresh everything for the open workarea" path (re-fetch threads + checks + deploys for every PR in the workarea + emit).
- **The three new RPCs** appended to the `Vcs` service: `ListReviewThreads(pr) → ReviewThreadsResponse`, `ResolveThread(thread_id) → Empty`, `ListDeployments(workarea, repo) → DeploymentsResponse` (+ the `GetChecks` path may stay or be supplemented for the workarea-scoped aggregate — keep the frozen `GetChecks` intact). FROZEN at the next free field/method numbers above the Task-45 five.
- **"Send to agent" context-attach** (`design/13 §3.6`): a path that composes a message with the thread context attached, routed to a **user-picked session** of the workarea (the composer-attach). This is the Core-side message-compose + route; the UI picker is **324**. Implement the Core side (a `VcsHandle`/RPC that takes `(thread_id, session_id)` and posts the composed message to that session via the existing session-message path); keep it minimal.
- Tests (Tier 2): `list_review_threads` against a recorded GraphQL fixture (`testkit` `FakeGitHub`); `resolve_thread` mutation against a fixture → cache updated + event emitted; check-run aggregation TTL behavior (synthetic clock — stale entry triggers refetch); deployment aggregation; `parse_subject("checks.<wa>.<repo>")` round-trips to `Subject::Checks`; an emitted `checks.*` event carries the opaque frame and a subscriber on that subject receives it; the webhook-triggered targeted-invalidation path (a fake invalidation hook fires → just the affected thread refetched + emitted).

## Scope — out
- **The `VcsProvider` trait + `ReviewThread`/`ThreadId`/`Deployment`/`CheckRun` value types + the octocrab client + `testkit`** — **Task 313** (`PHASE3_PLANNING §4.3`). 316 implements the three GraphQL/REST methods on the trait 313 froze; it never re-locks the trait or the value types.
- **The webhook receiver / relay route / HMAC / migration 0013** — **Task 315**. 316 consumes 315's cache-invalidation hook; it does not receive webhooks.
- **`wait_for_check_runs`** (the scheduler primitive that polls/subscribes to check runs for the merge gate) — **Task 318**. 316 makes the check-run cache + events; 318 consumes them.
- **Any new migration.** Review threads + checks + deployments are **in-memory only** (`design/13 §3.6`/§4 — GitHub is canonical). `PHASE3_PLANNING §3` assigns 316 **no migration** (0014 is owned by **Task 319** for `merge_order`/`external_id`/`repository_full_name`). If GraphQL thread/resolve needs the PR's GraphQL node id (`external_id`) and the `repository_full_name`, those columns are **319's** — 316 reads them when present, or derives `owner/repo` from the existing `repositories` row (state the dependency in Handoff if 319 hasn't landed).
- **A new `streams.proto` `Event` oneof arm** — forbidden (`PHASE3_PLANNING §2`; the oneof is frozen through 16). The opaque carrier is a non-oneof field; see Implementation notes.
- **The Desktop Checks/PR/Diff UI** (rendering threads, the "Send to agent" picker, force-refresh button, status dots) — **Task 324** (`web-ts`). 316 ships the Core data + events + the opaque frame 324 parses.
- **Linear/Jira** review-thread equivalents — out (GitHub-only here; 317 owns Linear/Jira fetch, no thread concept).

## Public interface this task locks
- **`Subject::Checks { workarea_id, repository_id }`** in `crates/core/src/handlers/streams.rs` + its `parse_subject` arm parsing `checks.<workarea_id>.<repository_id>` (mirroring `suggestion.events.<workarea_id>`). FROZEN subject string format.
- **The opaque `checks.*` event carrier:** a new **non-oneof** `optional bytes checks_opaque = 17` on the `Event` message in `streams.proto` (sibling of — **not inside** — the frozen `body` oneof, honoring `PHASE3_PLANNING §2`), set only for `checks.<wa>.<repo>` events. FROZEN field number 17 (the first free number on `Event`, additive above the oneof's 16). The **frame format** inside it (a deterministic-CBOR/JSON map with `{ kind: "thread_updated"|"check_run_updated"|"deployment_updated", … }`) is FROZEN here so 324 parses it. *(If 313 or an earlier Phase-3 task already introduced a generic opaque event carrier, REUSE it and note in Handoff — do not add a second.)*
- **`Vcs` service additions** (appended above the Task-45-frozen 5 RPCs): `ListReviewThreads` / `ResolveThread` / `ListDeployments` + the `ReviewThread` / `ReviewThreadComment` / `Deployment` / request/response proto messages, at the next free method/field numbers. FROZEN.
- **The three event kinds** (`pr.thread_updated`, `pr.check_run_updated`, `pr.deployment_updated`) on `checks.<workarea_id>.<repository_id>` (`design/13 §5.3`). FROZEN subjects + kind discriminators.
- **`GitHubProvider::{list_review_threads, resolve_thread, list_deployments}`** — the implementations of 313's FROZEN trait methods (caches in-memory, never SQLite).

## Implementation notes
- **The "no new oneof arm" constraint is the load-bearing design call.** `PHASE3_PLANNING §2` says the `checks.<wa>.<repo>` payload routes on the subject with an **opaque** payload and **no** new `streams.proto` `Event` oneof arm (the oneof is frozen through field 16). The resolution: add `optional bytes checks_opaque = 17` to the `Event` message **outside** the `oneof body` block (a oneof arm would be inside it). Field 17 is the first free number on `Event` (the oneof tops out at 16; field 17 is unused at the message level). This keeps every existing client wire-compatible (they ignore an unknown field) while letting 324 parse the frame. **Verify field 17 is free on `Event`** before claiming it (the `session_event` Kind oneof uses 17 internally — that is a different message; `Event` itself stops at the oneof's 16). If a generic opaque carrier already exists, reuse it.
- **Freeze the opaque frame format, don't leave it to 324.** 324 is a separate (web-ts) task; it must parse what 316 emits. Pin the frame: a small map `{ kind, workarea_id, repository_id, <entity> }` where `<entity>` is the changed thread/check/deploy in a minimal shape (deterministic-CBOR or JSON — pick one, match the relay envelope encoding 315.0 chose if that simplifies the codebase). Document it in the proto comment on `checks_opaque`.
- **Threads/checks/deploys are NEVER persisted.** `design/13 §3.6`/R-3: GitHub is canonical. The caches are `VcsState`'s in-memory maps; refresh-on-open + TTL + webhook-targeted are the only freshness mechanisms. Do not add a SQLite table (that is why 316 has no migration).
- **octocrab GraphQL is thinner than its REST.** Review threads + `resolveReviewThread` are GraphQL-only. Confirm 313's choice: octocrab's `graphql()` raw-query path vs a typed `graphql_client` query. If 313 pinned `graphql_client`, write the `.graphql` query/mutation files + the generated structs; if not, hand-roll the query string + deserialize. Either way the **wire fixtures** (`testkit`) are recorded GraphQL responses — the test never hits real GitHub.
- **Subject parsing mirrors `suggestion.events.<workarea_id>`.** `checks.<workarea_id>.<repository_id>` has two trailing segments (workarea + repo); `parse_subject` splits on `.` after the `checks.` prefix. Reject a missing repo segment with the existing `streams.unknown_subject` `INVALID_ARGUMENT` discipline. The ring buffer registers per concrete subject string (one ring per `(workarea, repo)` pair) — `RingBound::Count(RING_EVENT_CAP)` (not byte-bounded).
- **Event emission stays additive to the streams machinery.** Emit through the same broadcast → per-subject pump the existing events use (the pump assigns offsets + re-broadcasts). The `checks_opaque` Event sets field 17 + leaves the oneof `body` empty (or a sentinel) — confirm the pump/subscriber path tolerates a `body`-less Event carrying only `checks_opaque` (it should, since the offset/at fields are separate; add a test).
- **Tier-2 double = recorded GraphQL + REST fixtures via 313's `testkit`.** No real GitHub token in CI. `FakeGitHub` serves the recorded `list_review_threads` GraphQL response, the `resolveReviewThread` mutation response, the check-runs REST response, and the deployments REST response. **What it does NOT cover:** real GitHub GraphQL thread structure drift, real resolve round-trip, real deployment statuses — the Phase-3 Tier-3 "confirm review threads sync" checklist line.
- **Cross-platform:** pure octocrab/reqwest (rustls) + `sqlx` reads + in-memory caches — nothing `#[cfg(unix)]`. Builds on the Windows CI lane (Task 113).
- **Parallel build hint:** two independent Outputs a lead can fan out — (a) the `GitHubProvider` GraphQL/REST method impls + caches (`crates/vcs`), (b) the `Subject::Checks` + `checks_opaque` proto/streams wiring + the three RPCs + emission (`crates/core`/`crates/proto`). They meet at the FROZEN opaque-frame format + the `checks.<wa>.<repo>` subject.

## Verification
**Tier 2.** The `rust` §5.3 set + the recorded-fixture `testkit` double.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-vcs -p concerto-core threads checks deployments` → `list_review_threads`/`resolve_thread` against GraphQL fixtures, check-run TTL refetch (synthetic clock), deployment aggregation, `parse_subject("checks.wa.repo") → Subject::Checks`, a `checks.*` subscriber receiving the opaque frame, and the webhook-targeted invalidation path all pass.
4. `cargo test --workspace --no-fail-fast` → all pass (the new proto field + subject don't break existing stream tests; `body`-less Events tolerated).
5. `cargo deny check` → green (no new pins beyond 313's `octocrab`/`graphql_client`/`wiremock`; if 316 adds the CBOR/JSON frame crate confirm it's already in-tree or MIT/Apache-clean).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen: `proto.md` gains the three `Vcs` RPCs + the new messages + `Event.checks_opaque = 17`; `rust-api.md` gains the `VcsHandle` thread/deploy methods.
7. `scripts/smoke.sh` → **unchanged** gate (real thread sync needs real GitHub — Tier-3). Confirm existing smoke stays green.

**Tier-2 double + what it does NOT cover.** The double is **313's `testkit` `FakeGitHub` serving recorded GraphQL + REST fixtures** (no real GitHub token) + an in-process `checks.<wa>.<repo>` subscriber. It proves: the GraphQL thread query + resolve mutation deserialize + drive the cache; the check-run/deploy aggregation + TTL; the `Subject::Checks` parse + ring + the opaque-frame emission + a subscriber receiving it; the webhook-targeted invalidation hook. It does **NOT** cover: real GitHub GraphQL responses, a real resolve round-trip, or real deployment data — the **Phase-3 Tier-3 checklist** line ("confirm review threads sync" against a real GitHub repo with a live webhook).

## Definition of Done
- [ ] `GitHubProvider::list_review_threads` (GraphQL, one query) + `resolve_thread` (GraphQL mutation) implemented on 313's trait; threads cached `(pr_id, thread_id)` in-memory, refresh-on-open, **never** SQLite
- [ ] Check-run aggregation into `check_cache` (TTL 30s) + deployment aggregation; `pr.check_run_updated` / `pr.deployment_updated` / `pr.thread_updated` emitted on `checks.<workarea_id>.<repository_id>`
- [ ] `Subject::Checks { workarea_id, repository_id }` + `parse_subject` arm + ring registration; the opaque payload rides the **non-oneof** `Event.checks_opaque = 17` (NO new oneof arm); the frame format FROZEN + documented for 324
- [ ] `Vcs` service appended with `ListReviewThreads` / `ResolveThread` / `ListDeployments` + their messages, above the Task-45-frozen 5 RPCs; "Send to agent" Core-side compose+route implemented
- [ ] All three §6.3 invalidation paths (webhook-targeted via 315's hook, TTL-lazy, user-force-refresh) implemented
- [ ] `concerto-vcs/testkit` enabled as a dev-dep; Tier-2 tests (GraphQL fixtures, TTL, subject parse, opaque-frame emit/receive, invalidation) pass
- [ ] No migration added (in-memory only); builds on the Windows CI lane; polling/existing paths unaffected
- [ ] All §5.3 `rust` commands pass; interfaces regenerated + committed (proto + rust-api); smoke unchanged + green
- [ ] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams for 324/315 documented in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/vcs/src/github/` (or `crates/core/src/vcs/`, per 313's crate layout) — `list_review_threads` / `resolve_thread` (GraphQL) + `list_deployments` + check-run aggregation + the in-memory caches (new/modified)
- `crates/vcs/src/github/graphql/` — the review-thread query + resolve mutation (`.graphql` files if 313 uses `graphql_client`, else inline query strings) (new)
- `crates/proto/proto/concerto/v1/streams.proto` (modified — `optional bytes checks_opaque = 17` on `Event`, outside the frozen oneof, + its frame-format doc comment)
- `crates/proto/proto/concerto/v1/vcs.proto` (modified — `ListReviewThreads`/`ResolveThread`/`ListDeployments` RPCs + `ReviewThread`/`ReviewThreadComment`/`Deployment`/request/response messages, appended above the frozen 5)
- `crates/core/src/handlers/streams.rs` (modified — `Subject::Checks` + `parse_subject` arm + ring registration)
- `crates/core/src/handlers/vcs.rs` (modified/new — the three new RPC handlers + the `checks.*` event emission + the "Send to agent" compose-route)
- `crates/core/src/vcs/` (modified — `VcsState` threads/check/deploy cache fills + the webhook-invalidation hook consumption)
- `crates/vcs/tests/` + `crates/core/tests/` (new — Tier-2 tests using `concerto-vcs/testkit` GraphQL/REST fixtures + a `checks.*` subscriber)
- `Cargo.toml` (touched crates — `concerto-vcs` dev-dep with `testkit`; the CBOR/JSON frame crate only if newly introduced)
- `docs/interfaces/proto.md` + `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: review-thread sync (GraphQL) + check-run/deploy aggregation

Implements GitHubProvider::list_review_threads/resolve_thread (GitHub
GraphQL) + check-run/deployment aggregation on Task 313's octocrab
client; all in-memory (never SQLite — GitHub is canonical). Emits
pr.thread_updated/check_run_updated/deployment_updated on the new
checks.<workarea>.<repo> subject carrying an opaque frame via a non-oneof
Event.checks_opaque field (no new streams.proto oneof arm). Consumes
Task 315's webhook invalidation hook for instant updates. Real thread
sync is the Phase-3 Tier-3 line; proven against recorded GraphQL fixtures.

Refs: tasks/v1.0/316-review-thread-sync.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan — —
- Open questions for next task — —
- Deliberate debt — —
- Smoke-gate state — —
