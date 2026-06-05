# Task 317 — Native Linear + Jira Issue-Fetch Clients (Desktop-Mediated OAuth + Linear API-Key) + No-Op `write_back` Seam

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 313 |
| Touches subsystem(s) | 13 (VCS Provider Integration), 12 (Security & Identity — keychain) |
| Smoke gate | unchanged |

## Goal
Replace the V0.1 "fetch Linear/Jira issues out-of-band via the agent" path with **two native issue-fetch clients in `crates/vcs`** — a **Linear GraphQL** client and a **Jira REST** client — so the Core can resolve a `linear.app`/`atlassian.net` issue URL into `{title, description, labels, status}` for the workspace/workarea-creation-from-issue flow (`PRD §6.7`, `design/13 §3.7`) **without ever persisting issue-body content**. Credentials are stored in the OS keychain via 313's FROZEN `VcsSecretSlot` accessor (D4) using **Desktop-mediated OAuth** (the Desktop runs the 3LO dance in its webview and ships the token to the Core over the paired transport — no redirect URI is invented for a tray-less Core; D6), with **Linear additionally accepting a personal API key** (the simple path). The `VcsHandle::fetch_issue(url)` router (313, D-313-routing) dispatches GitHub-vs-Linear-vs-Jira by URL host; this task supplies the Linear/Jira halves. This task also lands the **no-op `write_back` trait seam** (D5): a `IssueWriteBack` trait + a `NoopWriteBack` LIVE impl, so the real status-transition-on-merge (task 320.5) hangs off it without re-touching 317. The new gRPC surface (`FetchIssue` extended to route by URL, plus credential-set RPCs) lets the Desktop Settings → Linear / Settings → Jira panels store tokens and lets task 411 (Maestro, P4) call issue fetch.

## Inputs to read before starting
- `design/13_VCS_Provider_Integration.md` §3.7 — the authoritative Linear (GraphQL) + Jira (REST + Atlassian OAuth) fetch contract: each returns **title, description, labels, status**; issue body is shown to the user, fed into workspace creation, and read by Maestro plan-mode for cone suggestions, but **never persisted** — fetched on demand, **cached 1 h in memory** (also §4 "Issue fetches are not persisted; held in a small TTL cache (1h)").
- `design/13` §6.1 (`choose_backend`: `if op == fetch_issue: use the specific Linear/Jira client` — these are NOT `VcsProvider` methods; the per-host dispatch is the `VcsHandle::fetch_issue(url)` router 313 owns), §5.1 (the `VcsHandle::fetch_issue(url)` + `fetch_linear_issue(id)` surface this extends), §12 R-9 (**Linear/Jira write-back on PR merge is V1.0, configurable per project** — but the write itself is task 320.5; 317 only lands the no-op seam per D5).
- `tasks/v1.0/PHASE3_PLANNING.md` §1 D5 (317 = fetch + a no-op `write_back` trait seam; the real transition is insert 320.5), **D6 (Desktop-mediated OAuth; Linear also accepts a personal API key; no redirect URI for a tray-less Core)**, §1 D4 + §4.1 (the FROZEN `VcsSecretSlot` keychain accessor 313 owns — 317 CONSUMES `VcsSecretSlot::{LinearAccessToken, LinearRefreshToken, JiraAccessToken, JiraRefreshToken}` keyed by the provider account id; **never** add closed `SecretKind` variants; **never** put VCS secrets in `vcs_credentials` or SQLite), §3 (migration table — 317 has **NO row**: tokens → keychain, non-secret metadata → 313's `vcs_credentials` table; 317 adds no migration), §4.3 (`crates/vcs` + `testkit` is FROZEN by 313 — 317 enables `concerto-vcs/testkit` as a dev-dep and reuses its `wiremock` `FakeLinear`/`FakeJira` builders + fixtures).
- `crates/keychain/src/api.rs` — the EXACT pattern 313's `VcsSecretSlot` mirrors: the existing `CoreSecretSlot` enum + `Secrets::{get,set,delete}_core_secret(core_id, slot)` (account `cores.<core_id>.<slot>`) added by Task 218 as a **parameterized** accessor (NOT a new `SecretKind` variant). 313's `VcsSecretSlot` + `get/set/delete_vcs_secret(scope_id, slot)` (account `vcs.<scope_id>.<slot_slug>`) follow this verbatim — read it so you call 313's accessor with the right `scope_id` (the provider account id for Linear/Jira) and `slot`.
- `crates/core/src/vcs/actor.rs` (current `VcsHandle`) + `crates/core/src/handlers/vcs.rs` (the gRPC `Vcs` handler + the `Issue` proto conversion) — the `fetch_issue` surface to mirror. **NOTE:** 313 relocates `VcsProvider` + the clients into a new `crates/vcs` crate (D2/§4.3); read 313's Handoff (when it exists) for the final module paths. If 317 runs before 313 has merged, this is a Stop-and-ask — 317 **depends on** 313 and must build against its `crates/vcs`.
- `Cargo.toml` `[workspace.dependencies]` — `reqwest = { version = "0.13", default-features = false, features = ["rustls", "json", "http2"] }` is ALREADY pinned (Task 112; rustls, no openssl — Windows-lane clean). Reuse it for both clients (Linear GraphQL POST + Jira REST). `wiremock` is introduced by 313 (§4.3); reuse it as a dev-dep. Do not add `graphql_client` for Linear unless 313 already pinned it (a hand-rolled GraphQL query string over `reqwest` is sufficient and keeps the dep tree bounded — see Implementation notes).
- `crates/proto/proto/concerto/v1/vcs.proto` — the existing `Issue` message (`number=int64`, `title`, `body`, `state`, `url`, `labels`) + the `FetchIssue` RPC. `Issue.number` is **GitHub-shaped (int64)** but Linear/Jira ids are **strings** (`ENG-123`, `PROJ-45`) — resolve this in the proto (see Public interface).
- `crates/core/src/security/managed.rs` — the `enterpriseDataPrivacy` / external-tracker policy hook (the issue body must not leak to an external tracker call when a project has `enterprise_data_privacy` set; consult the resolved setting before an outbound fetch, surfacing a typed refusal).

## Scope — in
- A `crates/vcs` `linear` module: a `LinearClient` that, given an issue identifier (`ENG-123` or a `linear.app/.../issue/ENG-123/...` URL), POSTs a single GraphQL `issue(id:)` query to `https://api.linear.app/graphql` with the stored bearer token, and maps the response to the shared `Issue` value type (title, description→body, labels[].name, state.name). Linear auth accepts **either** an OAuth access token (`VcsSecretSlot::LinearAccessToken`) **or** a personal API key (same slot — the header form is identical: `Authorization: <token>`).
- A `crates/vcs` `jira` module: a `JiraClient` that, given a Jira issue key (`PROJ-45` or an `*.atlassian.net/browse/PROJ-45` URL), GETs `/rest/api/3/issue/{key}` on the project's Atlassian cloud base URL with the stored OAuth bearer token, and maps fields → `Issue` (summary→title, description→body [ADF flattened to text], labels, status.name). On 401, attempt one transparent OAuth refresh via `VcsSecretSlot::JiraRefreshToken` before failing.
- Wire both into the `VcsHandle::fetch_issue(url)` **router** (313 owns the router; 317 registers the Linear/Jira host arms): `linear.app`/`*.linear.app` → `LinearClient`; `*.atlassian.net`/`*.jira.com` → `JiraClient`; everything else → GitHub (313). Add `fetch_linear_issue(id)`/`fetch_jira_issue(key)` direct helpers per `design/13 §5.1`.
- **1 h in-memory TTL cache** of fetched issues keyed by canonicalized URL; issue body **never** written to SQLite (privacy floor).
- Credential storage via 313's keychain accessor: `set_vcs_secret(account_id, VcsSecretSlot::LinearAccessToken|JiraAccessToken|...)`. Non-secret metadata (provider, scope, external account id, token expiry) → 313's `vcs_credentials` table (migration 0012, 313). The **token-receiving** side: a gRPC method the Desktop calls to ship the OAuth token (obtained in its webview, D6) or the Linear API key to the Core → keychain.
- The **no-op `write_back` seam** (D5): a `pub trait IssueWriteBack { async fn transition_on_merge(&self, issue_ref: &IssueRef, transition: IssueTransition) -> Result<()>; }` + `pub struct NoopWriteBack` (LIVE: returns `Ok(())`, logs at debug). 317 wires `NoopWriteBack` as the default; 320.5 supplies the real Linear/Jira-transition impl behind the same trait.
- gRPC: extend `FetchIssue` to accept a full issue **URL** (route by host) in addition to the existing `(repository_id, issue_number)` GitHub form, returning the shared `Issue`; add `SetVcsCredential` (Desktop ships the OAuth token / Linear API key → keychain) reading the new `VcsSecretSlot`s.
- Tests (Tier 2, against 313's `wiremock` `FakeLinear`/`FakeJira` doubles): Linear GraphQL fetch maps title/body/labels/status; Jira REST fetch maps the same; a 401 → one refresh → retry (Jira); URL-host routing picks the right client; the 1 h cache returns the cached issue without a second HTTP call; an `enterprise_data_privacy` project refuses an external-tracker fetch with a typed error; `NoopWriteBack::transition_on_merge` returns `Ok(())`; **no issue body reaches SQLite** (assert against a spy persistence / no write path exists).

## Scope — out
- The **real status-transition-on-merge** (Linear `issueUpdate` / Jira transition) — **task 320.5** (rust), hung off coordinated-merge completion, per-project opt-in via `projects.settings_json`, reusing the `IssueWriteBack` trait + 313's `testkit` doubles. 317 ships only the **no-op** impl + the trait.
- The `VcsSecretSlot` keychain accessor + `vcs_credentials` table + the `crates/vcs` crate + `wiremock` `testkit` — **all owned/FROZEN by 313** (D2/D4/§4.1/§4.3). 317 CONSUMES them; it does not re-lock them.
- The **Desktop OAuth webview UX** (Settings → Linear / Settings → Jira; the 3LO browser dance) — a Desktop task (`design/15` Settings; not a numbered P3 task — the panel rides on 322/Settings work). 317 ships the **Core-side** `SetVcsCredential` RPC the Desktop calls after it completes the dance; it does not build the webview.
- GitHub issue fetch — already shipped (Task 45) and owned by 313's router GitHub arm.
- Maestro reading the issue body for cone suggestion — **task 411 (P4)** consumes `fetch_issue`; the seam is published here, wired live in P4.
- Persisting issue bodies / a local issue cache that survives restart (forbidden by `design/13 §3.7`).

## Public interface this task locks
- **Rust trait (FROZEN):** `pub trait IssueWriteBack: Send + Sync { async fn transition_on_merge(&self, issue_ref: &IssueRef, transition: IssueTransition) -> Result<()>; }` + `pub struct NoopWriteBack` (LIVE impl, `Ok(())`). `IssueRef { provider: IssueProvider, external_id: String, project_url: String }` where `IssueProvider ∈ { Linear, Jira }`. Lives in `crates/vcs/src/write_back.rs`. Also FROZEN here: `#[non_exhaustive] pub enum IssueTransition { MergedDone }` — the transition vocabulary (V1.0 ships only the merge-completion forward transition). 320.5's `LinearJiraWriteBack` implements the SAME trait for `MergedDone` and adds no variant — do not change the trait signature or the enum there.
- **Rust client surface (FROZEN):** `LinearClient::fetch(&self, id_or_url: &str) -> Result<Issue>` and `JiraClient::fetch(&self, key_or_url: &str) -> Result<Issue>`; the shared `Issue` value type the `crates/vcs` `Issue` (313's value type, mirroring the proto) — gains a **string** external id (see proto below).
- **proto (vcs.proto, FROZEN field numbers):** add to the existing `Issue` message `string external_id = 7;` (the provider-native string id — `ENG-123`/`PROJ-45`; the existing `int64 number = 1` stays GitHub-only and is `0` for Linear/Jira). Add `FetchIssueByUrlRequest { string url = 1; }` and route `FetchIssue` to accept it via a new RPC `rpc FetchIssueByUrl(FetchIssueByUrlRequest) returns (Issue);` (additive — the existing `FetchIssue(FetchIssueRequest) returns (Issue)` is FROZEN, untouched). Add `SetVcsCredentialRequest { VcsCredentialProvider provider = 1; string account_id = 2; string access_token = 3; optional string refresh_token = 4; optional int64 expires_at = 5; }` with `enum VcsCredentialProvider { VCS_CREDENTIAL_PROVIDER_UNSPECIFIED = 0; VCS_CREDENTIAL_PROVIDER_LINEAR = 1; VCS_CREDENTIAL_PROVIDER_JIRA = 2; }` and `rpc SetVcsCredential(SetVcsCredentialRequest) returns (google.protobuf.Empty);` on the `Vcs` service. FREEZE these numbers.
- **Keychain (CONSUMED, not locked here):** 313's `VcsSecretSlot::{LinearAccessToken, LinearRefreshToken, JiraAccessToken, JiraRefreshToken}` via `Secrets::{get,set,delete}_vcs_secret(scope_id, slot)`, account `vcs.<scope_id>.<slot_slug>` where `scope_id` = the provider account id. 317 adds NO `SecretKind` variant and NO migration.

## Implementation notes
- **Token hygiene is the security floor.** OAuth tokens + the Linear API key go to the keychain ONLY — never SQLite, never logs, never the `vcs_credentials` table (which holds only non-secret metadata). Mirror `gh_cli.rs`'s never-log-subprocess-output discipline: wrap tokens in `SecretValue` end-to-end; the `SetVcsCredential` handler is the only place a token is in cleartext, and it goes straight to `set_vcs_secret`. Issue body never persisted (privacy floor; aligns with `design/08`'s `exclude_from_maestro` spirit).
- **Linear GraphQL without `graphql_client`.** A single hand-rolled query string (`query($id:String!){ issue(id:$id){ identifier title description labels{nodes{name}} state{name} url } }`) POSTed via `reqwest` with a typed `serde` response struct is enough and avoids a new codegen dep + its license review. Only pull `graphql_client` if 313 already pinned it for review-thread work (316) and the workspace dep is ratified.
- **Jira ADF → text.** Jira Cloud returns the description as **Atlassian Document Format** (a JSON node tree), not plain text. Flatten it to text (walk `content[].content[].text`) for the `Issue.body`; do not attempt to render it. Keep the flattener small + total (unknown node types → skip).
- **Desktop-mediated OAuth (D6).** There is **no loopback/redirect server on the Core**. The Desktop completes the 3LO dance in its webview and calls `SetVcsCredential` with the resulting access/refresh token; the Core stores it and never sees the browser. Document this clearly in the RPC doc-comment so the Desktop task knows the contract. Linear's personal API key takes the same `SetVcsCredential` path with `refresh_token` empty.
- **Reuse 313's `testkit` double — do not stand up a real Linear/Jira.** The Tier-2 double is 313's `wiremock`-backed `FakeLinear` (GraphQL fixture) + `FakeJira` (REST fixture) under `crates/vcs/tests/fixtures/`. Add Linear/Jira fixture JSON if 313 left them as builders only; coordinate the fixture shape in Handoff.
- **Cross-platform.** Both clients are pure `reqwest` (rustls) — Windows-lane clean. The keychain stays mac-only in V1.0 behind 313's accessor (the Windows seam is 608); no raw Security-framework calls.
- **`enterprise_data_privacy` gate.** Before an outbound Linear/Jira fetch, consult the resolved project setting (task 310's resolver is the source of truth; if 310 hasn't landed in this workarea's path, read `projects.settings_json.enterprise_data_privacy` directly and note the seam). A privacy-locked project → refuse with a typed `vcs.external_tracker_blocked` error, do not call out.
- Regen: proto change ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/proto.md`; commit it.

## Verification
**Tier 2.** The double is **313's `wiremock`-backed `FakeLinear` + `FakeJira`** (`concerto-vcs/testkit` dev-dep) serving recorded GraphQL/REST fixtures with a synthetic clock for the TTL test.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-vcs linear jira write_back` → Linear fetch maps title/body/labels/status; Jira fetch maps the same; Jira 401→refresh→retry; URL-host routing; 1 h-cache hit (no second HTTP call under synthetic clock); `enterprise_data_privacy` refusal; `NoopWriteBack` returns `Ok(())`; the no-persist assertion.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (reuses the ratified `reqwest`/rustls + 313's `wiremock`; **no new workspace pin** unless `graphql_client` is reused from 313 — if introduced here, an advisory-ignore is a Stop-and-ask).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `Issue.external_id`, `FetchIssueByUrl`, `SetVcsCredential`).
7. `scripts/smoke.sh` → **unchanged** (no smoke capability added; Linear/Jira fetch has no co-located smoke surface — real fetch is the Tier-3 line below).

**Tier-2 double + what it does NOT cover.** The `wiremock` `FakeLinear`/`FakeJira` doubles prove: query/response mapping, host routing, OAuth-refresh retry, the TTL cache, the privacy gate, and the no-op write-back seam. They do **NOT** cover: a **real Linear OAuth/API-key fetch** or a **real Jira/Atlassian 3LO + REST fetch** against live APIs — those are the Phase-3 Tier-3 checklist line **"fetch a real Linear and Jira issue"** (and the Desktop-mediated OAuth round-trip end-to-end). The real write-back (320.5) is its own task + checklist confirmation.

## Definition of Done
- [ ] `crates/vcs` `LinearClient` (GraphQL) + `JiraClient` (REST, ADF-flatten, one OAuth refresh) map to the shared `Issue` value type
- [ ] Both wired into the `VcsHandle::fetch_issue(url)` host router (Linear/Jira arms) + `fetch_linear_issue`/`fetch_jira_issue` helpers; 1 h in-memory TTL cache; issue body never persisted
- [ ] Credentials stored via 313's `VcsSecretSlot::{Linear,Jira}*` keychain accessor (NO new `SecretKind`, NO migration); non-secret metadata via 313's `vcs_credentials`
- [ ] `IssueWriteBack` trait + LIVE `NoopWriteBack` impl FROZEN; 320.5 reuses the trait unchanged
- [ ] proto: `Issue.external_id = 7`, `FetchIssueByUrl` RPC, `SetVcsCredential` RPC + `VcsCredentialProvider` enum — FROZEN field numbers; Desktop-mediated OAuth documented in the RPC comment
- [ ] `enterprise_data_privacy` projects refuse an external-tracker fetch with a typed error
- [ ] Tier-2 tests pass against 313's `wiremock` `FakeLinear`/`FakeJira`; all `rust` §5.3 commands green; interfaces regenerated
- [ ] Builds on the Windows CI lane (pure `reqwest`/rustls; keychain behind 313's accessor)
- [ ] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/vcs/src/linear.rs` (new — `LinearClient` GraphQL fetch)
- `crates/vcs/src/jira.rs` (new — `JiraClient` REST fetch + ADF flatten)
- `crates/vcs/src/write_back.rs` (new — `IssueWriteBack` trait + `NoopWriteBack` + `IssueRef`)
- `crates/vcs/src/lib.rs` or `crates/vcs/src/mod.rs` (modified — register `linear`/`jira`/`write_back`; register the host-router arms)
- `crates/vcs/Cargo.toml` (modified — `concerto-vcs/testkit` dev-dep; `reqwest` reuse; `graphql_client` only if reused from 313)
- `crates/vcs/tests/fixtures/linear_issue.json` + `jira_issue.json` (new — fixtures, if 313 left builders only)
- `crates/core/src/handlers/vcs.rs` (modified — `FetchIssueByUrl` + `SetVcsCredential` handlers; route to the new clients)
- `crates/proto/proto/concerto/v1/vcs.proto` (modified — `Issue.external_id`, `FetchIssueByUrl`, `SetVcsCredential`, `VcsCredentialProvider`)
- `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-3: native Linear + Jira issue-fetch clients + no-op write-back seam

Linear (GraphQL) + Jira (REST, ADF-flatten) clients in crates/vcs wired
into the fetch_issue host router, credentials via 313's VcsSecretSlot
keychain accessor (Desktop-mediated OAuth + Linear API key), 1h in-memory
cache, issue body never persisted. Adds the IssueWriteBack trait + LIVE
NoopWriteBack; the real status-transition-on-merge lands in 320.5.

Refs: tasks/v1.0/317-linear-jira-clients.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan: —
- Open questions for next task: —
- Deliberate debt: —
- Smoke-gate state: —
