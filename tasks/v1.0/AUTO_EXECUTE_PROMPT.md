# Auto-Execute Prompt — Running V1.0 Tasks, One Phase at a Time

Hand this prompt to a top-level coding agent **once per phase**. The orchestrator stays in that session, dispatching one isolated sub-agent per task, opening + merging a PR per task, adjusting future task files when implementation reveals drift, and looping to the **end of the current phase** — where it stops for the operator's manual Tier-3 checklist before the next phase's task files even exist.

Companion to `tasks/v1.0/PROMPT_TEMPLATE.md` (per-task sub-agent prompt) and `tasks/v1.0/README.md` (build-wide rules, verification tiers, phase inventory). This is the V1.0 variant of the root `tasks/AUTO_EXECUTE_PROMPT.md`; the key differences are: **task-type-aware verification**, **verification tiers**, **spike tasks**, and **a hard stop at each phase boundary**.

---

## How to use

1. Confirm the phase's task files exist (`tasks/v1.0/Pss-*.md` for the target phase `P`). If they don't, the phase hasn't been generated yet (see `README.md §8`) — generate them first (operator step), don't improvise them.
2. Local checkout on `main`, clean, up to date. `gh` authenticated with push + PR-merge rights.
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

- `git status` clean (else Stop).
- `git checkout main && git pull --ff-only`.
- `git checkout -b task-NNN-<slug>`.
- Read the task file. Note its **`Task type`** and **`Verification tier`** — they decide which commands you re-run in Step 4.

### Step 3 — Dispatch the sub-agent

Dispatch one fresh isolated sub-agent. Prompt = the contents of `tasks/v1.0/PROMPT_TEMPLATE.md` from the `---` markers, `{TASK_PATH}` replaced by `tasks/v1.0/NNN-<slug>.md`, verbatim. Tell it it's on branch `task-NNN-<slug>` and its work becomes a PR against `main`. **No parallel sub-agents** — tasks are sequential within a phase (deps in the inventory).

### Step 4 — Validate (type- and tier-aware)

When the sub-agent reports back:

1. **One new commit**, message ending `Refs: tasks/v1.0/NNN-<slug>.md`.
2. **`Handoff Notes` filled in** — all four bullets non-placeholder. For a Tier-2 task, *Open questions* should name what the test double did NOT cover.
3. **No files outside `Outputs`** modified (`git diff --name-only HEAD~1 HEAD`); `Cargo.lock`/`pnpm-lock.yaml`/`docs/interfaces/*` regen are expected — flag larger surprises.
4. **Definition of Done fully ticked.**
5. **Re-run the verification commands for this task's `Task type`** (`README.md §5.3`):
   - `rust` → `cargo check --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace --no-fail-fast` · `cargo deny check` · `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` · `scripts/smoke.sh` (only if the task's Smoke-gate field ≠ unchanged).
   - `web-ts` → `pnpm -C apps/web typecheck && pnpm -C apps/web lint && pnpm -C apps/web test && pnpm -C apps/web build` (+ Playwright suite if the task touched the data layer). For Desktop-renderer tasks under `apps/desktop`, substitute the `apps/desktop` equivalents.
   - `rn-mobile` → `pnpm -C apps/mobile typecheck && pnpm -C apps/mobile lint && pnpm -C apps/mobile test && pnpm -C apps/mobile exec expo prebuild --no-install`.
   - `infra-ops` → the exact gate the task's `Verification` declares (e.g. `shellcheck`, a script dry-run, a workflow lint).
   - `spike` → run the harness command; confirm `design/spikes/<name>-findings.md` exists and ends with a **GO/NO-GO** + measured numbers. **A NO-GO is Stop condition #5.**
   - Always also run any task-specific commands in the task's `Verification`.

If validation fails → **Failure recovery**.

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

### Step 6 — Wait for CI, then merge

- `gh pr checks --watch`. CI runs the matrix relevant to the touched code (Rust lanes for `rust`, web lanes for `web-ts`, etc.).
- Pass → `gh pr merge --squash --delete-branch` (1 commit = 1 task on `main`).
- Fail → **Failure recovery**. Never `--admin`, never merge red.
- After merge → `git checkout main && git pull --ff-only`.

### Step 7 — Propagate drift (same as V0.1)

Read the just-merged task's `Handoff Notes`. If *Drift from plan* changed a locked interface, edit downstream **Phase-{PHASE}** task files that reference it (do NOT touch later, ungenerated phases — record cross-phase drift in your final report instead). If *Open questions* affect the next task, append a one-line `Inputs` bullet. Doc-only edits go via a `chore/task-NNN-drift-followup` branch → PR → squash-merge. Skip if no drift.

### Step 8 — Report and loop

One line: `"Shipped task NNN: <title> — tier <n>; smoke <state>; <0|N> downstream tasks adjusted."` Go to Step 1.

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
- `docs/interfaces/*` regen the sub-agent missed → run `./scripts/regen-interfaces.sh`, `git add`, `git commit --amend --no-edit` (pre-push only), continue.
- Typos / dead links / a missing real `Depends on` in a **Phase-{PHASE}** task file → Step-7 doc-PR.
- `gh pr checks --watch` returns "no checks" for a lane that doesn't apply (e.g. a `rust`-only task on a web-only CI trigger) → proceed to merge.
- Flaky CI → `gh run rerun` once; reproduces twice → Stop #3.

## Failure recovery

Categorize the failure: verification-command failure → likely a code bug, retry the sub-agent; interface-regen drift → fix yourself (regen + amend, `--force-with-lease` to the un-merged branch only); `cargo deny`/`pnpm licenses` deny → Stop #13; smoke-gate failure on a check this task didn't add → real regression, retry; on a check it added → task verification mis-specified, Stop #2; spike NO-GO → Stop #5. To retry: `git reset --hard origin/main` (just-pushed) or `HEAD~1` (pre-push), re-dispatch fresh with the failure context. Don't edit the task file just to make the sub-agent's job easier — if the task is wrong, that's a Step-7 doc-update. Retry once; second failure → Stop #1.

## Boundaries (never violate)

- Never write production code (sub-agents do). Never edit `design/` (except via a `doc` task), the root `tasks/` (V0.1 history), or a merged task file. Never start the next phase. Never force-push `main`, delete others' branches, or bypass branch protection. Always `--squash`. Always re-validate locally before pushing. When unsure whether something is Auto-handle vs Stop — **stop and ask**.

Start now. Read the items, report ready, begin the Phase-{PHASE} loop.

---END PROMPT---

---

## After the orchestrator stops at a phase boundary

1. Work through the printed **Tier-3 manual checklist** for the phase. Each unchecked item is real-world verification only you can do (real devices, real NATs, signing, cross-machine). A failure is a revision task, not a skip.
2. Once the checklist is green, generate the next phase's task files (`README.md §8`): a planning pass reading this README, the relevant `design/` sections, the current `main`, and this phase's Handoff Notes. The spike findings from Phase 1 especially may reshape later phases.
3. Re-run this orchestrator with `{PHASE}` bumped.

Phase 1 (spikes + cleanup) and Phase 7 (signing, launchd/Service, relay deploy) have the most Tier-3 content; expect to drive those phases more hands-on than the middle phases.
