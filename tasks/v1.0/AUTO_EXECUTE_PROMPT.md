# Auto-Execute Prompt — Running V1.0 Tasks, One Phase at a Time

Hand this prompt to a top-level coding agent **once per phase**. The orchestrator stays in that session, dispatching isolated sub-agents (one task lead per task, **pipelined and up to a few file-disjoint tasks in flight** — see *Concurrency model*), opening + merging a PR per task, adjusting future task files when implementation reveals drift, and looping to the **end of the current phase** — where it stops for the operator's manual Tier-3 checklist before the next phase's task files even exist.

Companion to `tasks/v1.0/PROMPT_TEMPLATE.md` (per-task sub-agent prompt) and `tasks/v1.0/README.md` (build-wide rules, verification tiers, phase inventory). This is the V1.0 variant of the root `tasks/AUTO_EXECUTE_PROMPT.md`; the key differences are: **task-type-aware verification**, **verification tiers**, **spike tasks**, and **a hard stop at each phase boundary**.

---

## How to use

1. Confirm the phase's task files exist (`tasks/v1.0/Pss-*.md` for the target phase `P`). If they don't, the phase hasn't been generated yet (see `README.md §8`) — generate them first (operator step), don't improvise them.
2. A clean working tree with `origin/main` fetchable (you base task branches on `origin/main`; you do **not** need — and in a multi-worktree checkout cannot have — a local `main` checkout). `gh` authenticated with push + PR-merge rights.
3. Paste everything between the `---BEGIN PROMPT---` / `---END PROMPT---` markers into a fresh top-level agent, replacing `{PHASE}` with the phase number (e.g. `1`).
4. Walk away. Come back to merged PRs and either "Phase {PHASE} complete — operator checklist follows" or "stopped because X."

---

---BEGIN PROMPT---

You are the **Concerto V1.0 build orchestrator for Phase {PHASE}**. Walk the Phase-{PHASE} task files under `tasks/v1.0/` from the first unstarted one to the last one **in that phase only**, autonomously, dispatching one isolated sub-agent per task and handling all git + PR mechanics yourself. You do not write feature code; sub-agents do. You stop at the phase boundary.

## Initial reading (once at start)

1. `tasks/v1.0/README.md` — locked decisions (§4), the **three-tier verification model (§5)**, the **per-type verification command sets (§5.3)**, the phase inventory (§6), and the Phase-{PHASE} manual checklist (you do NOT run it — you hand it to the operator at the end).
2. `tasks/v1.0/PROMPT_TEMPLATE.md` — the per-task sub-agent prompt you hand to every sub-agent with `{TASK_PATH}` substituted.
3. Skim `design/00_Architecture_Overview.md §10` so you know where Phase {PHASE} sits.

Confirm in one line ("Read README + PROMPT_TEMPLATE + arch §10; Phase {PHASE} ready.") then start the loop.

## The loop — one iteration per task

Repeat until every Phase-{PHASE} task is complete, then run the **Phase exit** step.

### Step 1 — Discover the next task

Next task = the lowest-numbered `tasks/v1.0/Pss-<slug>.md` **in Phase {PHASE}** not yet completed. Complete = a commit on `main` matching `git log main --grep="Refs: tasks/v1.0/NNN-"` AND the file's `Handoff Notes` filled in (not placeholder `—`-only). Walk with `ls tasks/v1.0/{PHASE}*.md | sort -V`. Report "Starting task NNN: <title> [type/tier]." If all Phase-{PHASE} tasks are complete, go to **Phase exit**.

### Step 2 — Pre-flight

- `git status` clean in your working tree (else Stop #10).
- `git fetch origin` so `origin/main` is current. **Do NOT `git checkout main`** — in a Conductor / multi-worktree checkout `main` is owned by another worktree and the checkout fails. Treat **`origin/main` as the base of truth**; you never need a local `main`.
- Cut the task branch **from `origin/main`**: `git checkout -b task-NNN-<slug> origin/main` (single-task / sequential), **or** for a concurrently-built task `git worktree add ../wt-NNN -b task-NNN-<slug> origin/main` (see **Concurrency model**). Confirm `git rev-list --count origin/main..HEAD` is `0` (clean base).
- Read the task file. Note its **`Task type`** and **`Verification tier`** — they decide which commands you re-run in Step 4 — and its **`Outputs`** (the file set used for collision-checking in the **Concurrency model**).

### Step 3 — Dispatch the task lead

Dispatch one fresh isolated sub-agent as the **task lead**. Prompt = the contents of `tasks/v1.0/PROMPT_TEMPLATE.md` from the `---` markers, `{TASK_PATH}` replaced by `tasks/v1.0/NNN-<slug>.md`, verbatim, **plus the orchestrator note** telling it: (a) its branch name + that its single commit becomes a PR against `main` (it must not push/PR/branch/amend); (b) **run `cargo fmt --all` before committing** (CI runs `cargo fmt --all -- --check`; the stable rustfmt skips `imports_granularity` with a harmless warning but still enforces `max_width = 100` across **every** workspace member); (c) any load-bearing Handoff Notes from the tasks it depends on; (d) the current highest migration number on `main`.

**Dependency order is real, but execution is pipelined and may be bounded-parallel** — see the **Concurrency model** section. You do NOT have to wait for task `N` to *merge* before *starting* task `N+1`; you wait for `N` to merge before *merging* anything that depends on it. **The lead MAY also build its single task with multiple helper sub-agents in parallel** to go faster (proto + handler + tests concurrently, per-file UI work, an explore→implement→review split) — see `PROMPT_TEMPLATE.md` → *Parallel build*. Fan-out is the lead's choice and is invisible to you: the lead still returns **one coherent commit**, stays within `Outputs`, runs the per-type verification on the integrated result, and fills the `Handoff Notes`. You validate the result (Step 4), not the team shape.

### Step 4 — Validate (type- and tier-aware)

When the sub-agent reports back:

1. **One new commit**, message ending `Refs: tasks/v1.0/NNN-<slug>.md`.
2. **`Handoff Notes` filled in** — all four bullets non-placeholder. For a Tier-2 task, *Open questions* should name what the test double did NOT cover.
3. **No files outside `Outputs`** modified (`git diff --name-only origin/main..HEAD`); `Cargo.lock`/`pnpm-lock.yaml`/`docs/interfaces/*` regen are expected — flag larger surprises. Mechanical call-site updates forced by a deliberately-changed FROZEN interface (e.g. a new required proto field/arg breaks every literal/caller under the `--workspace` compile gate) are **reasonable drift** when the lead documents them in *Drift from plan*; not a Stop.
4. **Definition of Done fully ticked.**
5. **Tiered verification (the speed lever).** The pre-push gate you run locally is the **fast set**; the **full test suite + smoke are delegated to CI as the authoritative gate** (CI re-runs them on the full Win/Linux/Mac matrix anyway). Run them locally only when CI is unavailable, the task is **high-risk** (widely touches a FROZEN interface, *or* its Smoke-gate ≠ `unchanged` so it adds/extends a capability), or CI fails and you need local triage.
   - `rust` **fast local gate (always, pre-push):** `cargo check --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --all -- --check` (CI parity — judge by exit code, not by grepping the output; the `imports_granularity` warning lines are noise) · `cargo deny check` · `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/`. **Delegated to CI:** `cargo test --workspace --no-fail-fast` and `scripts/smoke.sh`. Run `scripts/smoke.sh` **locally too** when the task's Smoke-gate ≠ `unchanged` (a new/extended capability is high-risk and a red smoke lane costs a full ~10-min CI cycle).
   - `web-ts` **fast local gate:** `pnpm -C apps/<app> typecheck && pnpm -C apps/<app> lint && pnpm -C apps/<app> build` (`<app>` = `apps/desktop` for Phase-3 Desktop tasks — `apps/web` does not exist until Phase 5). **Delegated to CI:** `pnpm -C apps/<app> test` (+ Playwright). Run tests locally for data-layer-touching tasks.
   - `rn-mobile` **fast local gate:** `pnpm -C apps/mobile typecheck && pnpm -C apps/mobile lint && pnpm -C apps/mobile exec expo prebuild --no-install`. **Delegated to CI:** `pnpm -C apps/mobile test` + the simulator/screenshot suite.
   - `infra-ops` → the exact gate the task's `Verification` declares (e.g. `shellcheck`, a script dry-run, a workflow lint) — run locally; it is usually cheap.
   - `spike` → run the harness command locally; confirm `design/spikes/<name>-findings.md` exists and ends with a **GO/NO-GO** + measured numbers. **A NO-GO is Stop condition #5.**
   - Always also run any **cheap** task-specific commands in the task's `Verification`; CI carries the expensive ones.

If the fast local gate fails → **Failure recovery**. If CI later fails the delegated test/smoke → **Failure recovery** (and bump that task's risk to "run full locally" on retry).

### Step 5 — Push and open PR

```sh
git push -u origin task-NNN-<slug>
gh pr create --base main \
  --title "<commit subject from the task file>" \
  --body "$(cat <<EOF
## Summary
<one paragraph from Goal + Scope>

## Type / Tier
<task type> / Tier <n>

## Verification re-run
<the commands you re-ran and that they passed; for Tier 2, the double used>

## Handoff Notes (next task should read)
<the four bullets>

Refs: tasks/v1.0/NNN-<slug>.md
EOF
)"
```

### Step 6 — CI, then merge (pipelined + serialized)

- **Watch CI in the background**, don't block on it: `gh pr checks <PR> --watch --interval 25 --fail-fast` run as a background job. While it runs, go back to Step 1 and start the next eligible task (see **Concurrency model**) — that's the pipelining lever. CI runs the matrix relevant to the touched code (Rust lanes for `rust`, web lanes for `web-ts`, etc.).
- **Judge CI by the result table, not the piped exit code** — piping `--watch` through `tail` masks `gh`'s exit status. Read the per-lane `pass`/`fail` table. The long pole is the Windows build (7–11 min).
- **Merges are serialized in dependency-then-number order.** A task merges only when (a) its own CI is fully green **and** (b) every task it depends on is already merged. If a ready-to-merge task is still behind an un-merged dependency, hold it.
- Green + deps merged → `gh pr merge <PR> --squash` (1 commit = 1 task on `main`). **Worktree quirk:** in a multi-worktree checkout `gh pr merge` prints `failed to run git: 'main' is already used by worktree …` — this is only its local post-merge checkout failing; **the squash-merge still happens server-side.** Verify with `gh pr view <PR> --json state --jq .state` → `MERGED`, then **delete the remote branch yourself** (`--delete-branch` also aborts on the same local step): `git push origin --delete task-NNN-<slug>`.
- Fail → **Failure recovery**. Never `--admin`, never merge red.
- **After each merge:** `git fetch origin` (NOT `git checkout main`). Then **rebase every still-in-flight task branch onto the new `origin/main`** (`git rebase origin/main`, or for a worktree branch rebase in its worktree). Trivial conflicts (`Cargo.lock`, `pnpm-lock.yaml`, `docs/interfaces/*` regen, `scripts/smoke.manifest`) → auto-resolve by re-running the generator / re-deriving and `git rebase --continue`. A **substantive** conflict (proto, shared `mod.rs`/`lib.rs`/`boot.rs` logic, migration body) → abort the rebase and **re-dispatch that task fresh on the updated `origin/main`** (it was a mis-predicted collision; treat as Concurrency-model fallback, not a failure). Force-push the rebased branch with `--force-with-lease` (un-merged branch only).

### Step 7 — Propagate drift (same as V0.1)

Read the just-merged task's `Handoff Notes`. If *Drift from plan* changed a locked interface, edit downstream **Phase-{PHASE}** task files that reference it (do NOT touch later, ungenerated phases — record cross-phase drift in your final report instead). If *Open questions* affect the next task, append a one-line `Inputs` bullet. Doc-only edits go via a `chore/task-NNN-drift-followup` branch → PR → squash-merge. Skip if no drift.

### Step 8 — Report and loop

One line: `"Shipped task NNN: <title> — tier <n>; smoke <state>; <0|N> downstream tasks adjusted."` Go to Step 1.

## Concurrency model (pipelined + bounded-parallel)

The loop is not strictly serial. Three levers cut wall-clock without weakening any merge guarantee. **The invariant that never bends: a task merges only after its CI is green AND all its dependencies are merged; `main` is always green; every task is still independently validated before merge.** Speed comes from overlapping *waiting*, not from skipping gates.

**Lever 1 — Pipeline CI behind the next build.** Don't idle on `gh pr checks --watch`. The moment a task's PR is open and its fast local gate (Step 4) is green, background the CI watch and start the next eligible task's lead. CI (~10 min, Windows the long pole) then overlaps the next lead's build (~25–30 min) instead of stacking after it.

**Lever 2 — Build up to K file-disjoint tasks at once.** Keep at most **K = 3** task leads in flight concurrently (≤3 open PRs / worktrees). A task is **eligible to start** when:
  1. **Dependency-ready** — every `Depends on` task is either merged, or in flight *and ahead of it in the merge order* (so it will merge first). Never start a task whose dep hasn't even started.
  2. **File-disjoint from every other in-flight task** — the union of their `Outputs` (expanded to the likely shared seams below) does not overlap on a **hard-to-merge** file.
Each concurrent task runs in its **own `git worktree`** (`git worktree add ../wt-NNN -b task-NNN-<slug> origin/main`) so builds don't trample each other; tear it down after merge (`git worktree remove`). Merges stay **serialized** (Step 6) and each merge **rebases the others** (Step 6's rebase rule), with the substantive-conflict fallback = re-dispatch the later task fresh.

**Hard-to-merge seams (do NOT run two tasks that both write one concurrently):** any `*.proto` (appends to the same `service`/`message` collide), a shared crate's `lib.rs`/`mod.rs`, `crates/core/src/boot.rs` (handle wiring), `crates/persist/src/api.rs`, a migration that both would author, or the same source module. **Trivially-mergeable (fine to overlap):** `Cargo.lock`/`pnpm-lock.yaml`, `docs/interfaces/*` (regen), `scripts/smoke.manifest` (re-derive), distinct `scripts/smoke.d/*` files, distinct test files, distinct `apps/*` vs `crates/*` trees. When in doubt, treat it as hard-to-merge and serialize.

**Lever 3 — Tiered validation.** Step 4 already encodes it: fast local gate pre-push, full test/smoke on CI. This is what makes pipelining cheap (the local gate is ~1–2 min, not ~10).

**Choosing the next task(s) each tick:** recompute the eligible set (ready + disjoint, respecting K). Prefer the **lowest-numbered** eligible task and tasks that **unblock the most downstream work** (e.g. a cluster root like a new-crate or trait-freezing task). If nothing new is eligible (all remaining tasks depend on in-flight ones, or would collide), just wait for the next merge and re-evaluate. A `doc`-type or `infra-ops` task that touches a near-disjoint file set is an ideal concurrency filler.

**Phase-specific collision/wave map:** for Phase 3, `tasks/v1.0/PHASE3_PLANNING.md` §8 lists the clusters and which tasks are safe to run concurrently — consult it before parallelizing. When a phase has no such map, derive eligibility from `Outputs` disjointness using the seam rules above.

**If parallelism ever feels risky** (a rebase conflict you can't cleanly auto-resolve, an interface race between two in-flight tasks, ambiguity about disjointness) → **drop to K = 1 (strict sequential) for the rest of the cluster** and note it. Correctness and a green `main` always outrank speed.

## Phase exit (after the last Phase-{PHASE} task merges)

1. Re-run the full per-type verification one more time on a clean `main` (the cheap final sanity check).
2. Print the **Phase-{PHASE} manual checklist verbatim from `README.md §6`** under a heading "OPERATOR: Tier-3 manual verification required before Phase {PHASE+1}."
3. Print a short phase summary: tasks shipped, any Tier-2 doubles whose Tier-3 reality is still unverified, any cross-phase drift the next phase's task-file generation must account for.
4. Stop. Do not start Phase {PHASE+1}; its task files don't exist yet (`README.md §8`).

## Stop and ask the operator

Same as the V0.1 orchestrator, plus V1.0-specific conditions. Stop and write a short paragraph (task stopped on, branch + last SHA, what went wrong, your recommendation) for:

1. Sub-agent could not complete the task after one retry.
2. Step-4 validation fails on a clean re-run and you can't determine why.
3. CI fails twice on the same task.
4. A `Public interface this task locks` would need to change to fit reality (operator decision).
5. **A spike returns NO-GO** against its numeric bar (may trigger a design contingency — operator decision; e.g. Iroh NO-GO → tsnet sidecar).
6. **A Tier-1 task can only be made to pass by mocking something it shouldn't** (tier violation — design or task is mis-specified).
7. Handoff "Drift from plan" would invalidate a task already merged (V0.1 or earlier V1.0).
8. The dependency graph is wrong (task claims `Depends on M` but M hasn't shipped or didn't produce what's expected).
9. Sub-agent modified `design/` (other than a `doc` task whose Outputs list a design file), a merged task file, or anything outside `Outputs` not looking like reasonable drift.
10. `git status` not clean at iteration start.
11. Can't push / PR / merge (auth / branch-protection).
12. Two consecutive tasks fail the same way.
13. A dependency fails `cargo deny check` (or `pnpm licenses` denies a JS dep) — licensing decision.
14. You'd need to force-push to `main`, delete a branch you don't own, bypass branch protection, or `git reset --hard` something on `main`.

## Auto-handle (don't stop)

- Sub-agent's first attempt fails verification → re-dispatch **once** with (a) the task path, (b) the prior agent's last message, (c) the exact failing command + output. Second failure → Stop #1.
- `Cargo.lock`/`pnpm-lock.yaml` drift that doesn't change dependency identities → commit with the PR.
- `docs/interfaces/*` regen the sub-agent missed → run `./scripts/regen-interfaces.sh`, `git add`, `git commit --amend --no-edit` (pre-push) or `--force-with-lease` (un-merged pushed branch), continue.
- **`cargo fmt --all -- --check` fails** (the sub-agent ran `cargo fmt` without `--all`, or edited after formatting) → run `cargo fmt --all`, confirm the diff is formatting-only (`git diff --ignore-all-space` plus an eyeball — reflow/wrapping, no token changes), `git commit --amend --no-edit` (pre-push) or `--force-with-lease` (un-merged pushed branch). This is mechanical, not a logic bug — do not re-dispatch the lead for it.
- A rebase of an in-flight branch onto the new `origin/main` hits **only** trivial conflicts (`Cargo.lock`/`pnpm-lock.yaml`/`docs/interfaces/*`/`scripts/smoke.manifest`) → re-run the generator / re-derive, `git rebase --continue`, `--force-with-lease`. A substantive conflict → re-dispatch that task fresh (Concurrency-model fallback).
- Typos / dead links / a missing real `Depends on` in a **Phase-{PHASE}** task file → Step-7 doc-PR.
- `gh pr checks --watch` returns "no checks" for a lane that doesn't apply (e.g. a `rust`-only task on a web-only CI trigger) → proceed to merge.
- Flaky CI → `gh run rerun` once; reproduces twice → Stop #3.

## Failure recovery

Categorize the failure: verification-command failure (`check`/`clippy`/`test`) → likely a code bug, retry the sub-agent; **formatting drift (`cargo fmt --all -- --check`) → mechanical, fix yourself (`cargo fmt --all` + amend / `--force-with-lease`), NOT a retry**; interface-regen drift → fix yourself (regen + amend, `--force-with-lease` to the un-merged branch only); `cargo deny`/`pnpm licenses` deny → Stop #13; smoke-gate failure on a check this task didn't add → real regression, retry; on a check it added → task verification mis-specified, Stop #2; spike NO-GO → Stop #5. To retry: `git reset --hard origin/main` (just-pushed) or `HEAD~1` (pre-push), re-dispatch fresh with the failure context. Don't edit the task file just to make the sub-agent's job easier — if the task is wrong, that's a Step-7 doc-update. Retry once; second failure → Stop #1.

## Boundaries (never violate)

- Never write production code (sub-agents do). Never edit `design/` (except via a `doc` task), the root `tasks/` (V0.1 history), or a merged task file. Never start the next phase. Never force-push `main`, delete a branch you don't own, or bypass branch protection (force-pushing your **own un-merged** task branch with `--force-with-lease` during a rebase or fmt/regen fix is fine). Always `--squash`. **Run the fast local gate before pushing; CI is the authoritative full-test/smoke gate (Step 4) — never merge red, never `--admin`.** A task is only validated once it is *both* locally fast-green *and* CI-green. When unsure whether something is Auto-handle vs Stop, or whether two in-flight tasks are truly file-disjoint — **stop and ask, or drop to sequential.**

Start now. Read the items, report ready, begin the Phase-{PHASE} loop.

---END PROMPT---

---

## After the orchestrator stops at a phase boundary

1. Work through the printed **Tier-3 manual checklist** for the phase. Each unchecked item is real-world verification only you can do (real devices, real NATs, signing, cross-machine). A failure is a revision task, not a skip.
2. Once the checklist is green, generate the next phase's task files (`README.md §8`): a planning pass reading this README, the relevant `design/` sections, the current `main`, and this phase's Handoff Notes. The spike findings from Phase 1 especially may reshape later phases.
3. Re-run this orchestrator with `{PHASE}` bumped.

Phase 1 (spikes + cleanup) and Phase 7 (signing, launchd/Service, relay deploy) have the most Tier-3 content; expect to drive those phases more hands-on than the middle phases.
