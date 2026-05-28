# Task 03 — Smoke Gate Scaffolding

| Field | Value |
|---|---|
| Phase | 0 |
| Size | small (≤4h) |
| Depends on | 01, 02 |
| Touches subsystem(s) | 01 (Runtime) |
| Smoke gate | new |

## Goal
Establish `scripts/smoke.sh` — the end-to-end check that every Phase 1+ task keeps green. After this task, the script exists, runs cleanly, and reports its current scope (which is "nothing yet"). Subsequent tasks extend it incrementally. Without this scaffolding, the "smoke gate after every task" verification rule from `tasks/README.md` §5 cannot be enforced.

## Inputs to read before starting
- `tasks/README.md` §5 (verification model — three layers; smoke gate is layer 2).
- `tasks/02-ci-and-license-enforcement.md` → "Handoff Notes" — to confirm CI is up.

## Scope — in
- Create `scripts/smoke.sh` (executable; `chmod +x` committed via `git update-index --chmod=+x`).
- The script has a clear top-level structure:
  ```sh
  #!/usr/bin/env bash
  set -euo pipefail
  
  : "${CONCERTO_HOME:=$(mktemp -d)}"
  export CONCERTO_HOME
  trap 'rm -rf "$CONCERTO_HOME"' EXIT
  
  echo "Smoke gate: starting (CONCERTO_HOME=$CONCERTO_HOME)"
  
  # Phase 1 checks — added in Task 15
  # Phase 2 checks — added in Task 27
  # Phase 3 checks — added in Tasks 42 + 44
  # Phase 4 checks — added in Task 52
  
  echo "Smoke gate: PASSED (no checks active yet — Phase 0)"
  ```
- Add `.github/workflows/smoke.yml` that runs `scripts/smoke.sh` on Linux (the smoke gate must be cross-platform-friendly, but CI runs it on Linux to start; macOS-specific paths are added in Task 49).
- Add a helper at `scripts/lib/common.sh` with shared functions: `wait_for_port(port, timeout)`, `wait_for_log(file, regex, timeout)`, `pid_alive(pid)`, `fail(msg)`. These will be used by Phase 1+ task additions.
- Document the smoke gate's responsibilities in a top-of-file comment matching `tasks/README.md` §5.

## Scope — out
- No actual checks yet — the script is a passing no-op until Task 15.
- No Windows port (`scripts/smoke.ps1`) — V1.0; deny via comment "Linux/macOS only in V0.1".
- No performance budget checks (Task 50).

## Public interface this task locks
- Path: `scripts/smoke.sh` is the canonical smoke gate. Every later task references it by this exact path.
- Environment variable contract: `CONCERTO_HOME` points to a tempdir for the duration of the script. Tasks must not rely on `~/concerto/`.
- Exit codes: 0 = pass, non-zero = fail. Output is human-readable; structured JSON is not required.

## Implementation notes
- `set -euo pipefail` is non-negotiable — silent failures are the entire failure mode this script is designed to prevent.
- The `trap` for cleanup must use single quotes around `$CONCERTO_HOME` so the value at trap time is used, not the value at definition time.
- Make the helper functions in `scripts/lib/common.sh` POSIX-bash compatible (no zsh-specific syntax) — macOS ships old bash.
- Use `command -v` instead of `which` for portability.

## Verification
1. `bash -n scripts/smoke.sh` → no syntax errors.
2. `bash -n scripts/lib/common.sh` → no syntax errors.
3. `scripts/smoke.sh` → exits 0, prints "Smoke gate: PASSED".
4. `ls -l scripts/smoke.sh` → executable bit set.
5. Smoke workflow runs green on a CI push.
6. `shellcheck scripts/smoke.sh scripts/lib/common.sh` → 0 errors (warnings OK). Install shellcheck via Homebrew/apt if not present.

## Definition of Done
- [ ] All Verification commands pass on a clean checkout.
- [ ] Smoke gate runs in <5 seconds (it's a no-op).
- [ ] Script handles `CONCERTO_HOME` override and tempdir cleanup correctly (verified by inspection).
- [ ] No `TODO` / `FIXME` in scripts.
- [ ] Single commit created.

## Outputs
- `scripts/smoke.sh` (new, executable)
- `scripts/lib/common.sh` (new)
- `.github/workflows/smoke.yml` (new)

## Commit message
```
phase-0: smoke gate scaffolding

Adds scripts/smoke.sh as the end-to-end check every Phase 1+ task
keeps green. Includes common bash helpers and a CI workflow.
Currently a passing no-op — Task 15 adds the first real check.

Refs: tasks/03-smoke-gate-scaffolding.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`CONCERTO_HOME` cleanup is conditional, not unconditional.** The `Scope — in` snippet would have `trap 'rm -rf "$CONCERTO_HOME"' EXIT` fire even when the caller pre-supplied `CONCERTO_HOME` — that would delete the caller's directory on exit. The DoD bullet "Script handles `CONCERTO_HOME` override and tempdir cleanup correctly (verified by inspection)" is the authoritative behavior, so the script only registers the cleanup trap when it created the tempdir itself (`if [ -z "${CONCERTO_HOME:-}" ]; then ... mktemp -d ... trap ...; fi`). Verified both branches by inspection and by running the script twice — once with the override, once without — and confirming the supplied directory survived while the self-created one was removed.
  - **shellcheck directives added.** `scripts/lib/common.sh` had no shebang, so shellcheck flagged SC2148 — added `# shellcheck shell=bash` as the first line so the file is checked as bash. `scripts/smoke.sh` sources `lib/common.sh` via a relative path; added `# shellcheck source-path=SCRIPTDIR` + `# shellcheck source=lib/common.sh` so shellcheck resolves the source correctly from the script's own directory regardless of the invoking shell's CWD. `shellcheck scripts/smoke.sh scripts/lib/common.sh` now exits 0.
  - **CI dedupe pattern applied to `.github/workflows/smoke.yml`.** Uses `on: push: branches: [main]` + `on: pull_request:`, matching the post-task-02 fix to `ci.yml` / `deny.yml` / `format.yml`. Without this, every PR push fires the smoke workflow twice (once as `push`, once as `pull_request.synchronize`). The original task spec just said "on push and PR"; this is the same intent, deduped.
  - **`wait_for_port` uses `/dev/tcp`**, which is a bash builtin (not POSIX). The script shebang is bash and the task explicitly says "POSIX-bash compatible (no zsh-specific syntax) — macOS ships old bash", so bash 3.2 features are in scope. Verified syntactically on macOS bash 3.2 via `bash -n`. Not yet exercised against a real port — first caller will be Task 15's smoke-gate v1.
- **Open questions for next task:**
  - The smoke workflow runs only on Linux (per `Scope — in`). macOS-side smoke checks are deferred to Task 49 per the task spec — but Phase 1 tasks (15) will probably want a local-macOS-runnable smoke gate too. The script is already cross-platform; only the CI workflow is Linux-only.
  - `scripts/lib/common.sh` exposes four helpers — `fail`, `pid_alive`, `wait_for_port`, `wait_for_log`. If Phase 1 needs more (a UDS socket probe in particular — `wait_for_port` is TCP-only), add them in the task that needs them, not a speculative helper expansion now.
  - `shellcheck` is now a soft dev-tool dependency. Not added to CI yet — could be a future hygiene task. The smoke.yml workflow does not currently shellcheck the scripts; it only runs them.
- **Deliberate debt:** —
- **Smoke-gate state:** **new — infrastructure in place; no checks yet.** Script exits 0 with "Smoke gate: PASSED (no checks active yet — Phase 0)". Task 15 (smoke gate v1) adds the first real assertions.
