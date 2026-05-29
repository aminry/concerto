# Task 45 — VCS Integration via `gh` CLI Shell-Out

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 18, 19, 20, 22 |
| Touches subsystem(s) | 13 (VCS Provider) |
| Smoke gate | unchanged |

## Goal
Implement V0.1's GitHub integration entirely via `gh` CLI shell-out (per `design/13` V0.1 row). After this task, the Core can list a workarea's PRs, create a PR for a workarea, view PR state, and fetch checks — using `gh` invoked as a subprocess. The full GitHub API client (REST + GraphQL + webhooks) is V1.0.

## Inputs to read before starting
- `design/13_VCS_Provider_Integration.md` (whole — for V0.1 focus on §2 phase row "gh CLI shell-out", §5 RPC surface, §6 internal architecture for the gh-cli fallback path).
- `design/09_Persistence.md` §4.5 (`pull_requests` schema).

## Scope — in
- Add migration `0007_pull_requests.sql` per `design/09 §4.5`.
- Implement `crates/core/src/vcs/`:
  - `VcsProviderActor` (impl `Actor`).
  - `gh_cli` module that shells out to `gh` with environment isolation (no GITHUB_TOKEN leaked into our log).
  - Functions: `list_prs(repo)`, `view_pr(repo, number)`, `create_pr(repo, head, base, title, body) -> PrNumber`, `get_check_runs(repo, sha)`, `merge_pr(repo, number, method)`.
  - All call `gh` with `--json <fields>` for structured output.
  - First-run check: `gh auth status --hostname github.com`; on failure, return a typed `VcsError::NotAuthenticated` with a remediation hint.
- gRPC surface (per `design/10 §5.1` `Vcs` service):
  - `Vcs.GetPullRequest`, `Vcs.CreatePullRequest`, `Vcs.MergePullRequest`, `Vcs.GetChecks`, `Vcs.FetchIssue`.
- Persist `pull_requests` rows per `design/09 §4.5`. Upsert on `(workarea_id, repository_id)`. Sync from `gh` on RPC.
- Update workarea state: extend `Workareas` proto with `GetWorkareaPrSet(WorkareaId)` returning the list of PRs in this workarea.
- Tests:
  - Mock `gh` (replace the binary path with a small Rust script that produces canned `--json` output).
  - Round-trip: `create_pr` → `view_pr` → `get_check_runs` → `merge_pr`.
  - Auth check: when mocked-gh returns auth error, the wrapper returns `NotAuthenticated`.

## Scope — out
- GitHub REST/GraphQL API (V1.0).
- Webhook receiver (V1.0).
- PR set semantics + coordinated merge (V1.0 — see `design/03 §3.9`).
- Review threads sync (V1.0).
- GitLab / Bitbucket (V2.0).

## Public interface this task locks
- Rust: `crates/core/src/vcs/mod.rs` — `VcsHandle::list_prs`, `.view_pr`, `.create_pr`, `.merge_pr`, `.get_check_runs`. Frozen.
- Proto: `Vcs` service per the design's V0.1 subset. Frozen.
- DB migration `0007_pull_requests.sql`. Frozen.
- Required dependency on `gh` CLI on PATH (documented in Task 51's README).

## Implementation notes
- Use `which`-crate or check `Command::new("gh").arg("--version").status()` once at startup; fail fast with a clear error if missing.
- Capture stdout, parse via `serde_json::from_slice`.
- The PR title / body in `create_pr` can contain newlines — pass via `--title-file` and `--body-file` (temp files) to avoid escaping headaches.
- Inject `GH_TOKEN` only when the user has supplied one via keychain (Task 10's GithubPat); otherwise let `gh` use its own stored auth (the V0.1 default).
- Don't log the body of any subprocess output that might contain tokens; the redaction filter from Task 16 helps.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core vcs` → tests pass (using a mocked `gh`).
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual with real `gh` + a GitHub test repo:
   - From a workarea on a branch with a commit pushed, call `Vcs.CreatePullRequest`.
   - Verify the PR exists on GitHub.
   - Call `Vcs.GetPullRequest`; verify state.
   - Call `Vcs.GetChecks` for the PR's SHA; verify checks list.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes (smoke gate doesn't require a remote GitHub repo).

## Definition of Done
- [x] Verification commands pass.
- [x] Manual end-to-end against real GitHub verified. *(Deferred to operator per task pre-decision §12; mocked-`gh` integration test covers the wire glue.)*
- [x] `gh` missing or unauthenticated returns a clean typed error.
- [x] Token never logged.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/persist/migrations/0007_pull_requests.sql` (new)
- `crates/persist/src/pull_requests.rs` (new)
- `crates/core/src/vcs/mod.rs` (new)
- `crates/core/src/vcs/actor.rs` (new)
- `crates/core/src/vcs/gh_cli.rs` (new)
- `crates/proto/proto/concerto/v1/vcs.proto` (new)
- `crates/proto/proto/concerto/v1/workareas.proto` (modified — adds GetWorkareaPrSet)
- `crates/core/src/handlers/vcs.rs` (new)
- `crates/core/src/handlers/workareas.rs` (modified)
- `crates/core/src/main.rs` (modified)
- `crates/core/tests/vcs_gh_cli.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md`, `schema.md` (regenerated)

## Commit message
```
phase-3: VCS via gh CLI shell-out (V0.1)

Vcs service exposes Get/Create/Merge PullRequest + GetChecks +
FetchIssue, all by shelling out to gh. pull_requests table persists
per-(workarea, repository) PR state. GitHub API + webhooks are V1.0.

Refs: tasks/45-vcs-gh-cli.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** migration is `0008_pull_requests.sql` (not `0007_*` as the task body suggests) because tasks 30/36/38/39/40/43 already consumed 0002–0007. Pre-decision §1 calls this out. Persistence column set follows pre-decision §2 (frozen): the `external_id`, `repository_full_name`, and `merge_order` columns from `design/09 §4.5` are deferred to V1.0 (PR-set coordinated merge is V1.0 too). The V0.1 row carries `head_sha` instead, which `Vcs.GetChecks` keys off without a second round-trip.
- **Open questions for next task:** Desktop UI for PR creation / checks panel (Task 46+) — should the workarea panel pre-populate the title from the agent's last user message, or always defer to `gh`'s default editor? The Rust handle accepts an explicit `title` arg today, so either policy plugs in without an interface change.
- **Deliberate debt:** no webhooks, no PR-set coordinated merge, no review threads, no `octocrab` REST/GraphQL client, no Linear/Jira (all V1.0). `GH_TOKEN` keychain injection is also deferred — V0.1 lets `gh` use its own stored auth (`design/13 §3.1`). Real-GitHub end-to-end smoke is the operator's job per pre-decision §12.
- **Smoke-gate state:** unchanged. The smoke client doesn't exercise `Vcs.*` and `gh` is not required on `$PATH` for the gate (the VCS handle resolves `gh` lazily on first RPC; the boot probe logs a warning but does not abort).
