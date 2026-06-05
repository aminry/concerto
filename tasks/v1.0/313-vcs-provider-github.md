# Task 313 — `VcsProvider` trait + octocrab `GitHubProvider` (default) + `GitHubProviderViaCli` (fallback)

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | — |
| Touches subsystem(s) | 13 (VCS Provider Integration), 12 (Security — keychain), 09 (Persistence), 18 (trait-seam registry) |
| Smoke gate | unchanged |

## Goal
Stand up the V1.0 VCS foundation every other VCS task builds on. Today GitHub is reachable only through `gh` CLI shell-out (`crates/core/src/vcs/gh_cli.rs` + `actor.rs`, Task 45) and the `provider` is a hard-coded `"github"` string. This task creates a **dedicated `crates/vcs` crate** that houses the **FROZEN `VcsProvider` trait** (transcribed faithfully from `design/13 §3.8`), a default **`GitHubProvider`** on **`octocrab`** (REST for PR CRUD / merge / checks / deployments; GraphQL stubs handed to 316), a fallback **`GitHubProviderViaCli`** (the existing `gh` shell-out, reused verbatim — not rewritten), a per-call **`choose_backend(repo, op)`** dispatcher per `design/13 §6.1`, and a top-level **`VcsHandle::fetch_issue(url)` router** that dispatches GitHub-vs-Linear-vs-Jira by URL host (the Linear/Jira arms are 317's; this task wires GitHub + leaves the others as a routing seam). It also freezes (a) a **parameterized keychain `VcsSecretSlot` accessor** (mirroring Task 218's `CoreSecretSlot`, NOT new closed `SecretKind` variants) for the new VCS secret classes, (b) migration **0012** `vcs_credentials` (non-secret metadata only — secrets stay in the keychain), and (c) a **`testkit` feature** exposing a shared `wiremock`-backed fake-GitHub/Linear/Jira harness + recorded fixtures that 314/315/316/317/320/320.5 all reuse. New workspace pins (`octocrab`, `graphql_client`, `wiremock`) must clear `cargo deny` before this lands.

## Inputs to read before starting
- `design/13_VCS_Provider_Integration.md` §3.8 — the **`VcsProvider` trait is spelled out here**; transcribe its method set + value types faithfully and FREEZE (this is a V2.0 stability contract — GitLab/Bitbucket plug in behind it). §3.1 (two backends: octocrab default, gh fallback; switching is per-call, the user does not pick; GitHub Enterprise = configurable base URL, R-10), §5.1 (the `VcsHandle` public-API surface this crate exposes — note `fetch_issue(&str)` takes a URL), §6.1 (the `choose_backend(repo, op)` dispatch pseudocode: `fetch_issue → Linear/Jira client; has_octocrab_token → octocrab; gh available → gh; else NoVcsCredentials`), §4 (the `VcsState` in-memory caches + `webhook_secrets`/`rate_limits` maps that 314/315/316 populate — define the struct skeleton, leave the cache contents to the consumers), §10 (testing strategy — **`wiremock`** is named for per-backend unit tests; the Tier-2 double).
- `design/18_Distribution_and_Operations.md` §3.7 (the enterprise-module trait-seam registry — `VcsProvider` is row "`VcsProvider | 13 §3.8 | GitHub (octocrab + gh) | GitLab, Bitbucket, Gerrit, GitHub Enterprise variants`"; this task lands the OSS impl + a swap test fixture the registry contract requires, §3.7 bullet "at least one OSS impl and a test fixture for swap").
- `tasks/v1.0/PHASE3_PLANNING.md` §4.3 (this task FREEZES `crates/vcs` + the `testkit` harness + the new pins), §4.1 (the keychain `VcsSecretSlot` contract — verbatim below), §3 (migration **0012** = `vcs_credentials`; **confirm the highest `crates/persist/migrations/NNNN_*.sql` on `main` is still `0008` before writing — it is, as of this authoring; if a higher one landed, shift 0012 up by the same offset and note it in Handoff**), §1 D1 (the LLM seam stays deterministic-only in P3 — not consumed here, but PR title/body composition is 321's job, NOT this task's), §1 D2/D4, §2 row "313 `fetch_issue` routing" + "313 crate placement".
- `crates/core/src/vcs/gh_cli.rs` — the V0.1 `gh` shell-out to **reuse verbatim** as `GitHubProviderViaCli`: `resolve_gh_path`, `check_auth`, the `PrSummary`/`PrDetail`/`CheckRun`/`IssueDetail`/`IssueLabel` `serde` structs, `list_prs`/`view_pr`/`create_pr`/`merge_pr`/`get_check_runs`/`view_issue`, and the **token-hygiene discipline** (the module never logs subprocess stdout/stderr — only the command name + arg count; preserve this). `repo_full_name_from_url` (in `actor.rs`) parses `owner/repo` from a GitHub URL — reuse it.
- `crates/core/src/vcs/actor.rs` — the V0.1 `VcsHandle`/`VcsProviderActor`/`VcsConfig` actor pattern (`run` parks on shutdown; cheap-`Arc`-clone handle; `upsert_from_detail` persists the `pull_requests` cache row). The Task-45 `VcsHandle` method signatures (`create_pr`/`get_pr`/`list_prs`/`merge_pr`/`get_check_runs`/`fetch_issue`) are **FROZEN** — extend, never break (a breaking change is a "Revise" task per README §9). Keep the actor + handle pattern when the logic moves into `crates/vcs`.
- `crates/keychain/src/api.rs` + `crates/keychain/src/lib.rs` — the **`CoreSecretSlot` precedent** (Task 218): a parameterized accessor `Secrets::{get,set,delete}_core_secret(core_id, slot)` with account string `cores.<core_id>.<slot_slug>`, `slot.slug()` as public protocol, account-string round-trip tests. **Mirror this exactly** for `VcsSecretSlot`. The closed `SecretKind` enum is `Copy` + frozen (`GithubPat = "vcs.github.pat"` already exists); do NOT add variants to it.
- `crates/persist/migrations/0008_pull_requests.sql` — the migration header/comment style + the forward-only `sqlx::migrate!("./migrations")` convention (`crates/persist/src/api.rs` ~line 169). `crates/persist/src/pull_requests.rs` — the accessor-module pattern (typed `New*` struct + `upsert`/`get`) to mirror for `vcs_credentials`.
- `Cargo.toml` `[workspace.dependencies]` — the **rustls-only** posture (`reqwest = { … features = ["rustls", …] }`, NO native-tls/openssl, Task 112 comment) that `octocrab` MUST follow for the Windows lane + `cargo deny`; the per-dep license-justification comment style (each pin documents its SPDX + why). `deny.toml` `[licenses]` allow-list — the new pins must resolve to an already-allowed SPDX or you add + justify it (an advisory-ignore is a **Stop-and-ask**, operator decision, per `PHASE3_PLANNING §2` row "313 crate placement").
- `crates/relay/Cargo.toml` / `crates/transport/Cargo.toml` — the sibling dedicated-crate layout (`version.workspace`, `[lib] name = "concerto-<x>"`) to mirror for `crates/vcs`.
- `tasks/45-vcs-gh-cli.md` → "Handoff Notes" — what V0.1 deliberately deferred (the octocrab client, webhooks, threads, PR-set merge) and the frozen-surface notes.

## Scope — in
- **New crate `crates/vcs`** (`[lib] name = "concerto-vcs"`, added to the root `[workspace] members`). Houses everything below. The Core depends on it (`concerto-vcs = { path = "../vcs" }`); `crates/core/src/vcs/` becomes a thin re-export/actor-wiring shim or moves wholesale — decide in-task and record in Handoff (either keeps the `VcsProviderActor`/`VcsHandle` boot wiring in `crates/core/src/boot.rs` working unchanged).
- **The FROZEN `VcsProvider` trait** (transcribed from `design/13 §3.8`) + its value types: `CreatePrRequest`, `PullRequest`, `ProviderPrId`, `CheckRun`, `MergeMethod` (enum `Merge|Squash|Rebase`), `MergeReport`, `RevertReport`, `ReviewThread`, `ThreadId`, `Deployment`, `Issue`. Method set exactly per §3.8 (see Public interface). Mark the GraphQL methods (`list_review_threads`/`resolve_thread`) as **implemented-stub** on `GitHubProvider` (return `Err(Unimplemented)` or empty) — 316 fills them; freeze their signatures now.
- **`GitHubProvider`** on `octocrab`: REST for `create_pr`/`get_pr`/`list_check_runs`/`merge_pr`/`revert_pr`/`list_deployments`/`fetch_issue`; configurable base URL for GitHub Enterprise (R-10); reads the PAT from the keychain (`SecretKind::GithubPat`, existing). GraphQL methods are signature-frozen stubs (316).
- **`GitHubProviderViaCli`**: wrap the existing `gh_cli.rs` verbatim behind the trait (map its `PrDetail`/`CheckRun`/`IssueDetail` into the trait's value types). Preserve token hygiene + the `which`-style `gh` resolution + the `--title-file`/`--body-file` temp-file path.
- **`choose_backend(repo, op)`** per `design/13 §6.1`: `op == fetch_issue` routes to the URL-host router (GitHub arm here, Linear/Jira seam → 317); else `has_octocrab_token(repo) → GitHubProvider`; else `gh_available() → GitHubProviderViaCli`; else `Error::NoVcsCredentials`. Per-call, never user-chosen.
- **`VcsHandle::fetch_issue(url: &str)` router** (`design/13 §6.1`/§2 row "313 fetch_issue routing"): parse the URL host; `github.com`/Enterprise → GitHub issue fetch; `linear.app`/`*.atlassian.net` → a routing seam returning `Err(Unimplemented)` until 317 supplies the clients. The per-provider `fetch_issue(&Url)` stays on the `VcsProvider` trait for GitHub; the router is the top-level dispatch.
- **Keychain `VcsSecretSlot`** (the §4.1 contract, FROZEN here): a parameterized accessor `Secrets::{get,set,delete}_vcs_secret(scope_id, slot)`, account `vcs.<scope_id>.<slot_slug>`, mirroring `CoreSecretSlot`. `VcsSecretSlot ∈ { GithubAppPrivateKey, WebhookSecret, LinearAccessToken, LinearRefreshToken, JiraAccessToken, JiraRefreshToken }`. `scope_id` = `app_id` (App), `repo_id` (webhook), or provider account id (Linear/Jira). Account-string round-trip tests for every slot.
- **Migration `0012` `vcs_credentials`** (non-secret metadata only) + a `crates/persist/src/vcs_credentials.rs` accessor module (typed struct + upsert/get/list).
- **`#[cfg(feature = "testkit")]` module** in `crates/vcs`: `wiremock`-backed `FakeGitHub`/`FakeLinear`/`FakeJira` builders + recorded fixtures under `crates/vcs/tests/fixtures/` (a create-PR response, a get-PR response, a check-runs list, an issue, plus synthetic `X-RateLimit-*` headers + a synthetic-clock seam 314 needs). The harness lets a test construct a `GitHubProvider` pointed at the wiremock base URL.
- **Vet `octocrab` + `graphql_client` + `wiremock`** with `cargo deny check` BEFORE committing the pins; resolve every SPDX to the allow-list (add + justify in `deny.toml` only if posture-equivalent; an advisory is a Stop-and-ask).
- **Register the seam** in the `design/18 §3.7` trait-seam registry (the row already exists; this task satisfies its "≥1 OSS impl + swap test fixture" contract — add a `crates/vcs` trait-level swap test that exercises the trait against two impls / a fake).
- Tests (Tier 2): `GitHubProvider` REST methods against the `testkit` wiremock fixtures (create/get/merge/checks/deployments/issue); `choose_backend` dispatch table (token → octocrab, no-token+gh → cli, neither → `NoVcsCredentials`); the `fetch_issue` URL router (github host → GitHub, linear/jira host → `Unimplemented` seam); `VcsSecretSlot` account-string round-trips; `vcs_credentials` persist round-trip; the trait swap-fixture test.

## Scope — out
- **GitHub App auth + dual rate-limit pools + degraded cadence** — Task 314 (reuses this crate's `testkit` + the `VcsSecretSlot::GithubAppPrivateKey` slot + the `rate_limits` map skeleton). This task ships PAT-only octocrab.
- **Webhook receiver / `ingest_webhook` / the `0x04` channel** — Task 315 (+ design amendment 315.0). This task may declare the `ingest_webhook(repo, WebhookPayload)` signature on `VcsHandle` per `design/13 §5.1` but leaves it `Unimplemented`; 315 implements it + adds migration 0013.
- **Review-thread GraphQL fetch/resolve + check-run/deploy aggregation events** — Task 316 (fills the `list_review_threads`/`resolve_thread`/`list_deployments` bodies + the `checks.<wa>.<repo>` emission). Signatures are frozen here.
- **Linear + Jira native clients + Atlassian OAuth** — Task 317 (fills the `fetch_issue` router's Linear/Jira arms + the `Linear*`/`Jira*` `VcsSecretSlot` slots' usage). This task freezes the slots + the router seam.
- **PR-set semantics / `merge_order` / coordinated merge** — Tasks 319/320 (on the `Workareas` service per `PHASE3_PLANNING §4.5`); this task ships per-PR `merge_pr`/`revert_pr` only.
- **LLM-composed PR title/body** — Task 321 (reuses 312's `OneShotLlm`); this task's `create_pr` takes the title/body as given (deterministic). The `OneShotLlm` seam is 312's, not here.
- **The real GitHub API round-trip** — Tier-3 Phase-3 checklist line ("run a coordinated PR-set merge against a real GitHub repo with a live webhook"; "confirm review threads sync"). This task proves logic against `wiremock` only.
- Desktop UI for PRs/checks — Task 324.

## Public interface this task locks
- **`crates/vcs` crate name + `[lib] name = "concerto-vcs"`** — FROZEN (siblings reference `concerto-vcs/testkit`).
- **The `VcsProvider` trait (FROZEN, `design/13 §3.8`):**
  ```rust
  #[async_trait::async_trait]
  pub trait VcsProvider: Send + Sync + 'static {
      async fn create_pr(&self, req: CreatePrRequest) -> Result<PullRequest>;
      async fn get_pr(&self, id: ProviderPrId) -> Result<PullRequest>;
      async fn list_check_runs(&self, repo: &str, sha: &str) -> Result<Vec<CheckRun>>;
      async fn merge_pr(&self, id: ProviderPrId, method: MergeMethod) -> Result<MergeReport>;
      async fn revert_pr(&self, id: ProviderPrId) -> Result<RevertReport>;
      async fn list_review_threads(&self, id: ProviderPrId) -> Result<Vec<ReviewThread>>;
      async fn resolve_thread(&self, id: ThreadId) -> Result<()>;
      async fn list_deployments(&self, repo: &str, ref_: &str) -> Result<Vec<Deployment>>;
      async fn fetch_issue(&self, url: &Url) -> Result<Option<Issue>>;
  }
  ```
  plus the value types `CreatePrRequest`, `PullRequest`, `ProviderPrId` (newtype over the provider node/number id), `CheckRun`, `MergeMethod { Merge, Squash, Rebase }`, `MergeReport`, `RevertReport`, `ReviewThread`, `ThreadId`, `Deployment`, `Issue` — field sets FROZEN (design them minimally where §3.8 leaves them implicit; `Issue` mirrors the existing `vcs.proto` `Issue` shape — `number/title/body/state/url/labels`). `Result` is `concerto_error::Result`.
- **Keychain `VcsSecretSlot` accessor (FROZEN, `PHASE3_PLANNING §4.1`):** `Secrets::{get,set,delete}_vcs_secret(scope_id: &str, slot: VcsSecretSlot)`; account string `vcs.<scope_id>.<slot_slug>`; `VcsSecretSlot ∈ { GithubAppPrivateKey, WebhookSecret, LinearAccessToken, LinearRefreshToken, JiraAccessToken, JiraRefreshToken }` with slugs `github_app_private_key | webhook_secret | linear_access_token | linear_refresh_token | jira_access_token | jira_refresh_token` (each slug is public protocol, round-trip-tested). NOT closed `SecretKind` variants. VCS secrets NEVER land in `vcs_credentials` or `cores.json`.
- **Migration `0012` `vcs_credentials` (FROZEN columns):** `id TEXT PRIMARY KEY`, `provider TEXT NOT NULL` (`github`|`linear`|`jira`), `scope_id TEXT NOT NULL` (app_id / repo_id / provider account id), `external_account TEXT` (login / org), `app_id TEXT`, `installation_id TEXT`, `token_expires_at INTEGER` (epoch ms, nullable), `created_at INTEGER NOT NULL`, `updated_at INTEGER NOT NULL`, `UNIQUE(provider, scope_id)`. **Non-secret metadata only** — no key material, no tokens (those are keychain, per `VcsSecretSlot`).
- **`testkit` feature surface (FROZEN):** `concerto-vcs/testkit` exposes `FakeGitHub`/`FakeLinear`/`FakeJira` builder types (each returns a `wiremock::MockServer` base URL + a provider constructed against it), recorded fixtures under `crates/vcs/tests/fixtures/`, and a `synthetic rate-limit headers + synthetic clock` hook 314 consumes. Consumers enable it as `concerto-vcs = { path = "../vcs", features = ["testkit"] }` under `[dev-dependencies]`.
- **New workspace pins:** `octocrab`, `graphql_client`, `wiremock` (`wiremock` is `testkit`/dev only) — exact versions chosen in-task, each cargo-deny-clean, each with a justification comment in `Cargo.toml`.

## Implementation notes
- **`octocrab` MUST be rustls, no native-tls/openssl** — mirror the `reqwest = { features = ["rustls", …] }` posture (Task 112 comment) so the Windows lane (Task 113) builds and `cargo deny` stays green. octocrab is built on `reqwest`/`hyper`; pin its TLS feature explicitly and verify the transitive tree (`cargo tree -i openssl-sys` must be empty). **Run `cargo deny check` on the new tree before committing** — if any crate resolves to a disallowed SPDX or carries a security advisory, that's a Stop-and-ask (operator decision per `PHASE3_PLANNING §2`), not a silent `deny.toml` ignore.
- **Transcribe the trait, don't redesign it.** `design/13 §3.8` is the canonical surface; the `// ...` in the doc means "these are the V1.0 methods" — implement exactly the listed nine. Where a value type's fields aren't spelled out, design minimally + append-friendly and FREEZE; the trait is a V2.0 stability contract (GitLab/Bitbucket implement it), so getting the method set + value types right now matters more than the impl bodies (several are 316's to fill).
- **`GitHubProviderViaCli` is a wrap, not a rewrite.** Move `gh_cli.rs` into the crate unchanged; the new code is the trait-impl adapter mapping `gh`'s `serde` structs onto the trait value types. Keep the never-log-subprocess-output rule and the `gh.exe`-on-Windows resolution already present.
- **Keychain mirror discipline.** Copy the `CoreSecretSlot` pattern beat-for-beat: `slug()` returning a `&'static str`, `core_account_string`-style `vcs_account_string(scope_id, slot)` = `format!("vcs.{scope_id}.{}", slot.slug())`, `get/set/delete_vcs_secret` impls in `lib.rs` calling a `vcs_entry` helper, the `tracing::info!(target: "concerto::keychain", …, "vcs secret accessed/written/deleted")` events (scope_id + slot, **never** the value), and an account-string round-trip test per slot (the `account_strings` test module). Keep the closed `SecretKind` enum + its `Copy` derive untouched.
- **`vcs_credentials` is metadata only.** The hard rule (D4): SQLite holds the *references* (which app/installation/account, when the token expires) so the Core can decide whether to refresh; the *secret material* (App private key, webhook secret, OAuth tokens) lives only in the keychain via `VcsSecretSlot`. A reviewer should be able to `grep` the migration and find zero key/token columns.
- **`VcsState` skeleton.** Define the `design/13 §4` struct (`providers`, `pr_cache`, `check_cache`, `threads_cache`, `rate_limits: HashMap<ProviderKey, RateLimitBudget>`, `webhook_secrets`) but only populate `providers` + the issue/PR caches here; `rate_limits` is 314's, `webhook_secrets`/`threads_cache` are 315/316's. Freeze the `ProviderKey` enum shape (`GithubPat | GithubApp(app_id) | GhCli` — 314 keys its three pools off it).
- **Cross-platform:** the crate must build on the Windows + Linux CI lanes (Task 113). octocrab/reqwest are pure-Rust-TLS; `gh_cli` already handles `gh.exe`. No `std::os::unix` in the crate's hot path.
- **Boot wiring stays green.** `crates/core/src/boot.rs` constructs `VcsProviderActor::new(persistence)` + probes `check_auth`; whatever shim/move you choose, that boot path and the `crates/core/src/handlers/vcs.rs` gRPC handler must compile and behave identically (the V0.1 `Vcs` gRPC service is unchanged by this task — the trait is the *internal* surface; the proto stays as-is).
- **Parallel build hint:** the three FROZEN surfaces are independent and can be built by helper sub-agents in parallel, then integrated into the one commit — (a) the keychain `VcsSecretSlot` accessor + tests (`crates/keychain`), (b) migration 0012 + `vcs_credentials.rs` persist accessor (`crates/persist`), (c) the `crates/vcs` crate (trait + two providers + dispatcher + router + `testkit`). (c) depends on (a)+(b) only at the wiring seam.
- Regen: new persist schema ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/schema.md`; the keychain `VcsSecretSlot` enum updates `docs/interfaces/rust-api.md`. Commit both. No proto change (the `Vcs` service is untouched).

## Verification
**Tier 2.** The double is the **shared `wiremock`-backed `testkit` harness** (recorded REST + GraphQL fixtures, synthetic rate-limit headers + synthetic clock); the part it does NOT cover (the real GitHub API round-trip) is the Tier-3 Phase-3 checklist line "run a coordinated PR-set merge against a real GitHub repo with a live webhook".

1. `cargo check --workspace` clean (the new `concerto-vcs` member compiles; Core depends on it).
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-vcs` → `GitHubProvider` REST methods against the `testkit` wiremock fixtures (create/get/merge/checks/deployments/issue); `choose_backend` dispatch table; the `fetch_issue` URL router (github → ok, linear/jira → `Unimplemented` seam); the trait swap-fixture test pass.
4. `cargo test -p concerto-keychain` → the `VcsSecretSlot` account-string round-trips pass (one per slot; the existing `SecretKind`/`CoreSecretSlot` account-string tests stay green).
5. `cargo test -p concerto-persist` → the `vcs_credentials` round-trip (upsert/get/list, `UNIQUE(provider, scope_id)`) passes.
6. `cargo test --workspace --no-fail-fast` → all pass.
7. `cargo deny check` → green. **The new `octocrab`/`graphql_client`/`wiremock` trees resolve to allowed SPDX** (or you added + justified the SPDX in `deny.toml`; an advisory-ignore was a Stop-and-ask). Confirm `cargo tree -i openssl-sys` is empty (rustls-only).
8. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`schema.md` gains `vcs_credentials`; `rust-api.md` gains `VcsSecretSlot`).
9. `scripts/smoke.sh` → unchanged gate (this task adds no capability; the V0.1 `Vcs` gRPC path still boots through the refactored crate). Exits 0.

## Definition of Done
- [ ] `crates/vcs` crate created (`concerto-vcs`), added to `[workspace] members`; Core depends on it; the V0.1 `gh` shell-out lives in it as `GitHubProviderViaCli` (verbatim reuse)
- [ ] `VcsProvider` trait transcribed from `design/13 §3.8` + value types, FROZEN (field numbers/names per Public interface); GraphQL methods are signature-frozen stubs (316)
- [ ] `GitHubProvider` (octocrab, rustls, PAT, GitHub-Enterprise base URL) implements the REST methods; `choose_backend` per §6.1; `VcsHandle::fetch_issue(url)` router (GitHub arm live, Linear/Jira seam → 317)
- [ ] Keychain `VcsSecretSlot` parameterized accessor (`get/set/delete_vcs_secret`, account `vcs.<scope_id>.<slot_slug>`) + per-slot round-trip tests; closed `SecretKind` untouched
- [ ] Migration 0012 `vcs_credentials` (metadata only, no secrets) + `vcs_credentials.rs` accessor + round-trip test
- [ ] `testkit` feature exposes `FakeGitHub`/`FakeLinear`/`FakeJira` + recorded fixtures + synthetic rate-limit/clock hook; consumed as a dev-dep by siblings
- [ ] `octocrab`/`graphql_client`/`wiremock` pins clear `cargo deny` (rustls-only; SPDX on the allow-list or justified); no openssl in the tree
- [ ] Builds on Windows + Linux CI lanes; boot + V0.1 `Vcs` gRPC behavior unchanged
- [ ] All Verification commands pass on a clean checkout; interfaces regenerated + committed; smoke gate unchanged (green)
- [ ] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen stubs return a typed `Err(Unimplemented)`, not the macro — documented in Handoff)
- [ ] No files outside Outputs modified
- [ ] Single commit with the message below

## Outputs
- `crates/vcs/Cargo.toml` (new — `concerto-vcs`, `[features] testkit`, `octocrab`/`graphql_client`, `wiremock` under dev/`testkit`)
- `crates/vcs/src/lib.rs` (new — crate root, re-exports)
- `crates/vcs/src/provider.rs` (new — the FROZEN `VcsProvider` trait + value types)
- `crates/vcs/src/github.rs` (new — `GitHubProvider` octocrab impl)
- `crates/vcs/src/gh_cli.rs` (moved from `crates/core/src/vcs/gh_cli.rs` — verbatim) + `crates/vcs/src/github_cli.rs` (new — `GitHubProviderViaCli` trait adapter)
- `crates/vcs/src/dispatch.rs` (new — `choose_backend` + `VcsHandle::fetch_issue` URL router + `VcsState`/`ProviderKey` skeleton)
- `crates/vcs/src/actor.rs` (new — `VcsProviderActor`/`VcsHandle`/`VcsConfig`, moved/adapted from `crates/core/src/vcs/actor.rs`, Task-45 method sigs preserved)
- `crates/vcs/src/testkit.rs` (new — `#[cfg(feature = "testkit")]` `FakeGitHub`/`FakeLinear`/`FakeJira`)
- `crates/vcs/tests/fixtures/*.json` (new — recorded REST/GraphQL responses + a rate-limit-header fixture)
- `crates/vcs/tests/provider_github.rs` (new — Tier-2 wiremock tests + the trait swap-fixture test)
- `crates/core/src/vcs/mod.rs` (modified — re-export `concerto_vcs` so `boot.rs`/`handlers/vcs.rs` are unchanged) + `crates/core/Cargo.toml` (modified — `concerto-vcs` path dep)
- `crates/keychain/src/api.rs` + `crates/keychain/src/lib.rs` (modified — `VcsSecretSlot` + `get/set/delete_vcs_secret` + `vcs_account_string` + round-trip tests)
- `crates/persist/migrations/0012_vcs_credentials.sql` (new) + `crates/persist/src/vcs_credentials.rs` (new) + `crates/persist/src/lib.rs` (modified — `pub mod vcs_credentials`)
- `Cargo.toml` (modified — `octocrab`/`graphql_client`/`wiremock` workspace pins + justification comments) + `deny.toml` (modified only if a new posture-equivalent SPDX needs ratifying) + `Cargo.lock` (modified)
- `docs/interfaces/rust-api.md` + `docs/interfaces/schema.md` (regenerated)

## Commit message
```
phase-3: crates/vcs — VcsProvider trait + octocrab GitHubProvider + gh fallback

New concerto-vcs crate: the FROZEN VcsProvider trait (design/13 §3.8),
a default octocrab GitHubProvider (rustls, PAT) + gh-CLI fallback +
per-call choose_backend dispatch + a fetch_issue URL router. Freezes the
keychain VcsSecretSlot accessor, migration 0012 vcs_credentials (metadata
only), and the wiremock testkit harness 314/315/316/317/320 reuse.
octocrab/graphql_client/wiremock vetted cargo-deny-clean (rustls, no openssl).

Refs: tasks/v1.0/313-vcs-provider-github.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan — —
- Open questions for next task — —
- Deliberate debt — —
- Smoke-gate state — —
