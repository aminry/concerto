# Task 314 — GitHub App option + dual rate-limit pools + degraded polling cadence

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 313 |
| Touches subsystem(s) | 13 (VCS Provider Integration), 12 (Security — keychain), 05 (Scheduler — shared backoff) |
| Smoke gate | unchanged |

## Goal
Add the **GitHub App authentication option** alongside the PAT (`design/13 R-7`) and the **per-provider rate-limit budget** machinery (`design/13 §3.9`) that 315/316/318/320 all rely on to stay under GitHub's limits. Today `crates/vcs` (Task 313) ships a single PAT-backed `GitHubProvider` with no rate-limit awareness and an empty `rate_limits` map in `VcsState`. This task lets a repo authenticate as a GitHub **App installation** (JWT signed with the App private key → short-lived installation token, minted + transparently refreshed on expiry), keys **three distinct rate-limit pools** off the `ProviderKey` enum (App = 15000/hr, PAT = 5000/hr, `gh` CLI = its own separately-tracked pool — itself a §3.1 fallback trigger), seeds each budget from the live GitHub `X-RateLimit-*` response headers, and **degrades cadence** when a pool drops below 10% (doubles the `design/13 §3.3` polling intervals + deprioritizes background work — deployments/threads — under user-driven work — create/merge). It emits `vcs.rate_limit_warning` (broadcast) below 20%, surfaces the pools to Settings → Diagnostics, and on exhaustion fails calls with `RateLimited { reset_at }` (queue + resume on reset). The App private key + app/installation ids are stored via 313's keychain `VcsSecretSlot::GithubAppPrivateKey` + the `vcs_credentials` table. All logic is proven with the 313 `testkit` synthetic-header + synthetic-clock double; real App-token mint against GitHub is the Tier-3 gate.

## Inputs to read before starting
- `design/13_VCS_Provider_Integration.md` §3.1 (the `gh` CLI pool is **separately tracked** and a fallback trigger — "a rate-limit headers from API indicate we should back off and `gh`'s rate limit pool is separately tracked"), §3.3 (the FROZEN polling cadence numbers: PR state 30s foreground / 5min background; **check runs exponential backoff 1s, 2s, 4s, 8s, 16s, 30s cap — same as 05 §3.9**; review threads 60s; deployments 60s; "tuned to GitHub's rate limits — 5000/hr PAT, 15000/hr GitHub App"), §3.9 (rate-limit handling: **< 10% remaining → polling doubles + new calls deprioritize background over user-driven + UI soft warning**; **exhausted → calls fail `RateLimited{reset_at}` + banner + queue/resume on reset**), §5.3 (the `vcs.rate_limit_warning` broadcast event "Budget below 20%"), §8 (the 403 `X-RateLimit-Remaining=0` row → "Queue calls until reset; UI banner; degrade polling cadence"), §12 R-7 (**GitHub App in V1.0, alongside PAT** — "Higher rate limit, finer scope, easier rotation"), §4 (the `rate_limits: HashMap<ProviderKey, RateLimitBudget>` map skeleton 313 froze — you populate it).
- `design/05_Scheduler.md` §3.9 (the `wait_for_check_runs` backoff sequence — **must stay byte-identical to §3.3's**; 318 consumes the same cadence; do not let the two drift). Read enough to confirm the exact sequence `1,2,4,8,16,30(cap)`.
- `tasks/v1.0/313-vcs-provider-github.md` → "Public interface this task locks" + "Handoff Notes" — the FROZEN `crates/vcs` surface this builds on: the `VcsProvider` trait, `GitHubProvider` (octocrab, the PAT path you extend with App auth), the `ProviderKey` enum (`GithubPat | GithubApp(app_id) | GhCli` — you key the three pools off it), the `VcsState.rate_limits` map skeleton, the `testkit` `FakeGitHub` harness + its **synthetic rate-limit headers + synthetic clock** hook (your primary double), and the keychain `VcsSecretSlot::GithubAppPrivateKey` slot + the `vcs_credentials` table (provider/scope_id/app_id/installation_id/token_expires_at columns).
- `tasks/v1.0/PHASE3_PLANNING.md` §1 D2 (the shared `wiremock` `testkit` is 313's; 314 reuses it as a dev-dep — do NOT build a second double), §3 (314 adds **no migration** — App/installation ids + token expiry live in 313's `vcs_credentials` table; the private key is a keychain `VcsSecretSlot`), §4.3.
- `crates/vcs/src/github.rs` + `crates/vcs/src/dispatch.rs` (313's octocrab provider + `choose_backend`/`VcsState`/`ProviderKey` — extend, don't fork) and `crates/vcs/src/testkit.rs` (the `FakeGitHub` builder + the synthetic-clock hook to extend with rate-limit-header scripting).
- `crates/keychain/src/api.rs` — 313's `VcsSecretSlot::GithubAppPrivateKey` + `Secrets::get_vcs_secret(scope_id, slot)` (`scope_id = app_id` for App creds). The PEM private key is read here; never logged, never in SQLite.
- `octocrab`'s App-auth surface — octocrab supports App/installation auth (JWT → installation token); confirm the exact API in the pinned version (313 chose it). If a JWT helper crate is needed (e.g. `jsonwebtoken`) it is a **new workspace pin** that must clear `cargo deny` (rustls/ring posture; a disallowed SPDX or advisory is a Stop-and-ask) — prefer octocrab's built-in App auth to avoid a new dep.

## Scope — in
- **GitHub App auth on `GitHubProvider`**: given an `app_id` + installation id (from `vcs_credentials`) + the App private key (from `VcsSecretSlot::GithubAppPrivateKey`), mint a JWT, exchange it for a short-lived **installation token**, cache it with its expiry (the `token_expires_at` already in `vcs_credentials`), and **transparently refresh** before/at expiry. Selectable **per repo** alongside PAT (a repo configured for App auth uses it; otherwise PAT; otherwise gh). `choose_backend` (313) gains the App arm: `has_github_app(repo) → GitHubProvider{App}`.
- **`RateLimitBudget`** keyed by `ProviderKey` (313's enum). Three pools: `GithubApp(app_id)` (15000/hr), `GithubPat` (5000/hr), `GhCli` (separate). Each budget holds `{ limit, remaining, reset_at }`, **seeded from the GitHub `X-RateLimit-Limit/Remaining/Reset` response headers** on every octocrab call (parse them off the response; the `gh` pool is updated from `gh api -i` / `gh api rate_limit` reads). Populate `VcsState.rate_limits`.
- **Degraded cadence** (`design/13 §3.9`): when a pool's `remaining/limit < 0.10`, **double** the §3.3 cadence numbers for work on that pool AND mark background ops (deployments, review-thread polls) as deprioritized vs user-driven ops (create PR, merge) — a simple priority gate, not a full scheduler. The cadence numbers themselves stay FROZEN from §3.3 (shared with 05 §3.9 / 318 — keep them in one place so they cannot drift).
- **Warning + exhaustion**: emit `vcs.rate_limit_warning` (broadcast, per §5.3) when a pool drops below **20%**; surface the three pools' state to Settings → Diagnostics (a read accessor the existing diagnostics path can call — full UI is 324/709's, not here). On **exhaustion** (`remaining == 0` / a 403 with `X-RateLimit-Remaining: 0`), fail the call with a typed `Error` carrying `reset_at`, and **queue + resume** background work on reset (a small per-pool resume timer; user-driven calls surface the error to the caller).
- **Credential storage**: App `app_id`/`installation_id`/`token_expires_at` → 313's `vcs_credentials` (via its accessor; provider=`github`, scope_id=`app_id`); the App private key (PEM) → keychain `VcsSecretSlot::GithubAppPrivateKey` (scope_id=`app_id`). The minted installation token is held **in memory only** (never persisted — it's short-lived).
- Tests (Tier 2, against 313's `testkit` extended with header/clock scripting): budget seeds from `X-RateLimit-*` headers; crossing 20% emits `vcs.rate_limit_warning`; crossing 10% doubles cadence + deprioritizes background; exhaustion (`remaining==0`) → `RateLimited{reset_at}` + a background op queues + resumes after the synthetic clock passes `reset_at`; the three pools are tracked **independently** (draining the PAT pool does not degrade the App pool); App-token mint + refused-stale-token refresh against a scripted token endpoint (synthetic clock advances past expiry → a refresh is issued).

## Scope — out
- **The `wait_for_check_runs` Scheduler primitive** — Task 318 (consumes the same §3.3/§3.9 backoff; this task only OWNS the cadence constants 318 imports). Keep the constants in `crates/vcs` and let 318 reuse them, or co-locate per the 318 author's call — document where they live.
- **Webhook-driven cache updates** (which reduce polling) — Task 315; this task's degraded cadence is the poll-only fallback path.
- **Review-thread / deployment polling bodies** — Task 316 (this task only classifies them as "background" for the priority gate; it does not implement the polls).
- **The Settings → Diagnostics UI / the diagnostics RPC** — Tasks 324 / 709 (this task exposes a read accessor for the three pools' state; it does not build the panel or the RPC).
- **A new migration** — none (App ids + expiry reuse 313's `vcs_credentials`; the key is a keychain slot). If you find a metadata field 313's table lacks, that's a **Stop-and-ask** (do not silently add a 0012.5).
- **Real GitHub App installation on a real org / real App-token mint** — Tier-3 Phase-3 checklist (operator installs the App; this task's `testkit` scripts the JWT→token exchange + the rate-limit headers synthetically). Confirm the phase checklist carries "mint a real GitHub App installation token + observe a real degraded-cadence transition".

## Public interface this task locks
- **`RateLimitBudget` (FROZEN):** `struct RateLimitBudget { limit: u32, remaining: u32, reset_at: <epoch-ms> }` with `fn observe_headers(&mut self, headers)` (seed from `X-RateLimit-*`), `fn fraction_remaining(&self) -> f64`, `fn is_degraded(&self) -> bool` (`< 0.10`), `fn is_warning(&self) -> bool` (`< 0.20`), `fn is_exhausted(&self) -> bool` (`remaining == 0`). Keyed in `VcsState.rate_limits` by 313's `ProviderKey` (`GithubApp(app_id) | GithubPat | GhCli`).
- **Cadence constants (FROZEN, `design/13 §3.3` = `05 §3.9`):** the check-run backoff sequence `[1s, 2s, 4s, 8s, 16s, 30s(cap)]`; PR-state `30s` (fg) / `5min` (bg); review-thread `60s`; deployment `60s`. Degraded = these **doubled**. Exposed as named constants (single source of truth; 318 imports them).
- **`RateLimited` error (FROZEN):** the typed error variant carrying `reset_at` (epoch ms) the gRPC handler maps to a `RESOURCE_EXHAUSTED` status + the reset hint; the queue/resume path keys off it.
- **`vcs.rate_limit_warning` event (FROZEN per `design/13 §5.3`):** broadcast, fired once per pool per warning-threshold crossing (debounced so it does not spam every call below 20%), payload identifies the pool (provider + scope_id) + `reset_at`.
- **GitHub App auth config:** stored as 313's `vcs_credentials` row (provider=`github`, scope_id=`app_id`, `app_id`, `installation_id`, `token_expires_at`) + keychain `VcsSecretSlot::GithubAppPrivateKey` (scope_id=`app_id`). No new persisted secret class (313 froze the slot).

## Implementation notes
- **Three pools, not one — and the `gh` pool is special.** GitHub bills App-installation calls, PAT calls, and `gh`-CLI calls against **separate** quotas; conflating them is the bug. Key strictly off `ProviderKey`. The `gh` pool dropping is itself a §3.1 reason to prefer octocrab (or vice-versa) — keep the pools independent so the dispatcher can read them.
- **Installation tokens expire (~1h) and MUST refresh transparently.** Mint lazily; cache `(token, expires_at)`; refresh when within a small skew of expiry (e.g. 60s) or on a 401. Persist only the *expiry* (`vcs_credentials.token_expires_at`) so the Core knows staleness across restarts without holding the token; the token itself is in-memory. The App private key stays in the keychain and is read only to sign the JWT.
- **Seed budgets from real headers, degrade off the seeded state.** Every octocrab response carries `X-RateLimit-*`; parse them into the matching pool on each call (cheap). Do NOT hardcode 5000/15000 as the live value — they are the *defaults/expectations*; the headers are authoritative (Enterprise/secondary limits differ). The hardcoded numbers only seed an unprimed pool before the first response.
- **Degraded cadence is a multiplier, not a rewrite.** When `is_degraded()`, callers that poll multiply the FROZEN interval by 2 and skip/deprioritize background ops. Implement the priority gate as a simple "is this op user-driven?" boolean threaded from the call site (create/merge = user; deployments/threads = background); under degradation, background ops yield to user ops on the same pool.
- **The synthetic clock is the heart of the test.** Reuse 313's `testkit` clock seam: inject a controllable "now" so the budget's `reset_at` logic, token-expiry refresh, and queue-resume timer are deterministic in CI (no real sleeps). The `wiremock` `FakeGitHub` scripts the `X-RateLimit-*` headers + the App-token endpoint response per request. Mirrors `design/13 §10` rows "Rate-limit budget logic / Synthetic time" + "Per-backend method implementations / wiremock".
- **Keep the cadence constants in one module** so this task and 318 (`wait_for_check_runs`) import the same literals; a divergence between §3.3 and §3.9 is a latent bug. Name the module/consts explicitly in Handoff so 318's author finds them.
- **Cross-platform:** pure-Rust JWT + reqwest/octocrab (rustls); no `std::os::unix`. Builds on the Windows + Linux lanes (Task 113).
- Regen: no schema change (reuses `vcs_credentials`) and no new keychain slot (313 froze it) ⇒ `docs/interfaces/` likely unchanged; run `regen-interfaces.sh` anyway and commit only if it moves.

## Verification
**Tier 2.** The double is 313's **shared `wiremock` `testkit` harness extended with synthetic `X-RateLimit-*` header scripting + the synthetic clock** (no real GitHub, no real App). What it does NOT cover: real App-installation-token mint against GitHub + a real rate-limit degradation under live load — the Tier-3 Phase-3 checklist line "mint a real GitHub App installation token + observe a real degraded-cadence transition".

1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-vcs rate_limit` (and `… app_auth`) → budget seeds from headers; 20%-cross emits `vcs.rate_limit_warning` (once, debounced); 10%-cross doubles cadence + deprioritizes background; exhaustion → `RateLimited{reset_at}`; a queued background op resumes after the synthetic clock passes `reset_at`; the three pools track independently; App-token mint + transparent refresh on synthetic expiry pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (reuses 313's pins; **only if a JWT helper crate was unavoidable**, its SPDX is on the allow-list — rustls/ring posture — or a Stop-and-ask was raised; prefer octocrab's built-in App auth so no new pin lands).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → clean (no schema/keychain change expected; commit only if it moves).
7. `scripts/smoke.sh` → unchanged gate (PAT-only co-located boot path is unaffected). Exits 0.

## Definition of Done
- [ ] GitHub App auth on `GitHubProvider` (JWT → installation token, cached + transparently refreshed), selectable per repo alongside PAT; `choose_backend` gains the App arm
- [ ] `RateLimitBudget` keyed by `ProviderKey`; three independent pools (App 15000/PAT 5000/gh separate) seeded from live `X-RateLimit-*` headers; `VcsState.rate_limits` populated
- [ ] Degraded cadence below 10% (cadence doubled + background deprioritized) using the FROZEN §3.3=§3.9 constants from a single source of truth
- [ ] `vcs.rate_limit_warning` broadcast below 20% (debounced); three-pool state exposed to a diagnostics read accessor
- [ ] Exhaustion fails with `RateLimited{reset_at}`; background work queues + resumes on reset
- [ ] App `app_id`/`installation_id`/`token_expires_at` in 313's `vcs_credentials`; App private key in `VcsSecretSlot::GithubAppPrivateKey`; installation token in-memory only — no new migration, no new secret class
- [ ] Tests against 313's `testkit` (synthetic headers + synthetic clock) cover seed/warn/degrade/exhaust/resume/independence + App mint + refresh
- [ ] Cadence constants co-located so 318 reuses them (location noted in Handoff)
- [ ] Builds on Windows + Linux lanes; PAT-only path + smoke gate unchanged (green)
- [ ] `cargo deny` green (no unvetted new pin; any JWT helper SPDX allow-listed or Stop-and-ask)
- [ ] No TODO/FIXME/unimplemented!()/todo!() in new code
- [ ] No files outside Outputs modified
- [ ] Single commit with the message below

## Outputs
- `crates/vcs/src/rate_limit.rs` (new — `RateLimitBudget`, the three-pool tracker, the FROZEN cadence constants, the warning/degrade/exhaust logic, the queue-resume timer)
- `crates/vcs/src/github.rs` (modified — App JWT→installation-token mint/refresh; per-call `X-RateLimit-*` parsing into the matching pool)
- `crates/vcs/src/dispatch.rs` (modified — `choose_backend` App arm; the user-driven-vs-background priority gate; the `RateLimited` error variant + the diagnostics read accessor)
- `crates/vcs/src/testkit.rs` (modified — `FakeGitHub` gains scripted `X-RateLimit-*` headers + a scripted App-token endpoint + the synthetic-clock advance hook 314 drives)
- `crates/vcs/tests/rate_limit.rs` (new — Tier-2 budget/warn/degrade/exhaust/resume/independence tests) + `crates/vcs/tests/app_auth.rs` (new — mint + refresh tests)
- `crates/vcs/Cargo.toml` (modified only if a JWT helper crate is unavoidable — prefer octocrab's built-in App auth)
- `Cargo.toml` / `deny.toml` / `Cargo.lock` (modified only if a new JWT pin lands + is justified)
- `docs/interfaces/*.md` (regenerated; expected unchanged)

## Commit message
```
phase-3: GitHub App auth + dual rate-limit pools + degraded cadence

Adds GitHub App installation-token auth (JWT mint + transparent refresh)
alongside PAT, and per-provider RateLimitBudget pools (App 15000/PAT 5000/gh
separate) seeded from X-RateLimit-* headers. Below 10% the design/13 §3.3
cadence doubles and background ops yield; below 20% emits vcs.rate_limit_warning;
exhaustion fails RateLimited{reset_at} and queues/resumes on reset. Reuses
313's keychain VcsSecretSlot + vcs_credentials + the wiremock testkit
(synthetic headers + synthetic clock); no new migration.

Refs: tasks/v1.0/314-github-app-rate-limits.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan — —
- Open questions for next task — —
- Deliberate debt — —
- Smoke-gate state — —
