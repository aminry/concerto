# Task 108 — Composable Smoke Gate + V1.0 Capability Manifest

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | infra-ops |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | — |
| Touches subsystem(s) | (verification infrastructure) |
| Smoke gate | new:composable |

## Goal
V0.1's `scripts/smoke.sh` is a single 13.6 KB monolith covering the whole Phase-1→3 happy path. V1.0 will turn on many new capabilities (pairing, files transfer, multi-repo, maestro digest, push fan-out) one task at a time. Refactor the smoke gate into **composable per-capability checks** driven by a manifest, so each future task can declare `Smoke gate: extends:<capability>` and the gate grows additively without editing one ever-growing script. Behavior must stay identical for the V0.1 capabilities that already pass.

## Inputs to read before starting
- `scripts/smoke.sh` (the current monolith) and `scripts/smoke-embedded.sh`.
- `scripts/lib/` (existing shared shell helpers).
- `tasks/v1.0/README.md` §5.3 (smoke-gate growth model) — each task's `Smoke gate` field is now `unchanged` / `extends:<capability>` / `new:<capability>`.
- `.github/workflows/smoke.yml` (CI invocation to keep working).

## Scope — in
- Split `smoke.sh` into per-capability check functions/scripts under `scripts/smoke.d/` (e.g. `00-core-boot`, `10-project-repo-clone`, `20-workspace-workarea`, `30-echo-session`, `40-streams-subscribe`, `50-permission-flip`, `60-audit-log`, `70-loop`, `80-skills`, `90-mcp`) — one named capability per file, each idempotent and independently skippable.
- A `scripts/smoke.sh` driver that reads a manifest (e.g. `scripts/smoke.manifest`) listing enabled capabilities in order and runs them, with `--only <capability>` and `--list` flags for development.
- The manifest seeded with exactly the V0.1 capabilities currently passing — net behavior unchanged.
- `shellcheck` clean across all new scripts.
- Keep `smoke-embedded.sh` working (it can reuse the same `smoke.d` checks against an embedded Core).

## Scope — out
- Adding any new capability check (future tasks do that via `extends:`/`new:`).
- Changing what the V0.1 checks assert.

## Public interface this task locks
- The smoke-gate contract: `scripts/smoke.sh` (runs the manifest; exit 0 = pass), `scripts/smoke.d/<NN>-<capability>.sh` layout, and the `scripts/smoke.manifest` format. Future tasks add a `smoke.d` file + a manifest line.

## Implementation notes
- Preserve the exact log lines later tasks/CI grep for (or update both sides). The orchestrator and `smoke.yml` rely on `scripts/smoke.sh` exiting 0.
- Make each check echo a clear `PASS <capability>` / `FAIL <capability>` so failures are legible in CI.
- Keep the driver POSIX-sh-friendly where the originals were; don't introduce a bashism the CI runner lacks.

## Verification
Tier 1 (infra).
1. `shellcheck scripts/smoke.sh scripts/smoke.d/*.sh scripts/lib/*.sh` → clean.
2. `scripts/smoke.sh --list` → prints the enabled capabilities.
3. `scripts/smoke.sh` → exits 0 on a clean checkout (identical pass set to before the refactor).
4. `scripts/smoke.sh --only core-boot` → runs just that check, exits 0.
5. `scripts/smoke-embedded.sh` → still exits 0.
6. CI: `.github/workflows/smoke.yml` still green.

## Definition of Done
- [x] `smoke.sh` split into manifest-driven `smoke.d/` capability checks
- [x] V0.1 capability set passes identically; `--only` and `--list` work
- [x] `shellcheck` clean; `smoke-embedded.sh` still works
- [x] CI smoke workflow green
- [x] Single commit created with the message below

## Outputs
- `scripts/smoke.sh` (rewritten as driver)
- `scripts/smoke.d/*.sh` (new — extracted checks)
- `scripts/smoke.manifest` (new)
- `scripts/smoke-embedded.sh` (modified if needed)
- `scripts/lib/*` (modified if helpers move)

## Commit message
```
phase-1: composable smoke gate driven by a capability manifest

Splits the monolithic smoke.sh into per-capability checks under
scripts/smoke.d/ run from scripts/smoke.manifest, so V1.0 tasks extend
the gate additively. V0.1 capability coverage is unchanged.

Refs: tasks/v1.0/108-smoke-gate-refactor.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **smoke.d files created, mapped to the old monolith sections** (10 files; the
    suggested `00..90` skeleton was followed but only for capabilities the monolith
    actually exercised — no invented checks):
    - `00-core-boot.sh` ← monolith L82–160 (build binaries, scratch HOME, boot Core,
      wait for `core.sock`, `GetServerCapabilities` UDS assertion; sets up `SMOKE_CLIENT`,
      `CORE_*`, `SOCKET`, `CONCERTO_DATA_DIR`).
    - `10-project-repo-clone.sh` ← L167–185 (seeded bare repo, `add-project`, `add-repo`,
      `clone`).
    - `20-workspace-workarea.sh` ← L186–210 (`new-workspace`/`new-workarea` + on-disk
      `.context/` + repo `.git` worktree-layout verification).
    - `30-echo-session.sh` ← L212–219 (`start-session --agent-kind echo`).
    - `40-streams-subscribe.sh` ← L220–237 (`stream-session-io` + non-empty output check +
      `stop-session`).
    - `50-permission-flip.sh` ← L244–249 (`set-perm-mode --mode auto`).
    - `60-audit-log.sh` ← L251–261 (`workspace_created` in the audit JSONL).
    - `70-loop.sh` ← L263–268 (`create-loop` + `list-loops`).
    - `80-skills.sh` ← L270–280 (plant `SKILL.md`, `list-skills`).
    - `90-mcp.sh` ← L282–296 (plant `mcp.json`, `list-mcp`).
    The shared build/boot scaffolding stays in `00-core-boot`; the clean-shutdown +
    tmpdir cleanup stay in the **driver** (`smoke.sh`), not a capability file.
  - **State-sharing contract (sourced functions, not subprocesses):** the driver SOURCES
    each `smoke.d/<NN>-<cap>.sh` into one shell process; each file defines `check_<cap>`
    (dashes→underscores). Because functions use no `local`, the variables they assign are
    process-global, so the single Core boot, the `cleanup`/`trap`, `CORE_PID`, and the
    `PROJECT_ID → REPO_ID → WS_ID → WA_ID → SID` chain persist across checks exactly as in
    the monolith. Each file's header documents the vars it **requires** before running and
    the vars it **exports** for later checks.
  - **`--only` prereq handling:** the V0.1 checks are a strictly sequential state chain
    with no per-check dependency metadata, so `--only <cap>` runs the manifest PREFIX up to
    and including `<cap>` (every check that had to run to satisfy its shared-state
    preconditions). `--only core-boot` → just core-boot; `--only permission-flip` →
    core-boot…permission-flip. Clean shutdown then runs. `--list` prints capabilities
    without booting Core; invalid `--only`/unknown flags fail fast before the multi-minute
    build.
  - **`--ci-mode` preserved** exactly: parsed, exported, currently a no-op skip set (same
    as the monolith). `.github/workflows/smoke.yml` is **unchanged** — it still invokes
    `scripts/smoke.sh --ci-mode`. No log line CI/other tasks grep for was altered (the
    `Smoke gate v3:` prefixes and the final `PASSED` line are preserved); added only the new
    per-capability `PASS <cap>` / `FAIL <cap>` lines.
- **Open questions for next task:** none blocking. When 109–112 add a capability, decide
  whether its check needs a fixture planted under `FAKE_HOME` (like skills/mcp) or new
  shared state — if it introduces a new ID in the chain, export it from its file header
  contract so later checks can read it.
- **Deliberate debt:** `smoke-embedded.sh` does NOT yet source the shared `smoke.d` checks —
  embedded mode boots Core in-process with no UDS surface, and every `smoke.d` check drives
  Core via the UDS smoke-client, so they can't be reused as-is. It still proves the
  in-process boot via the existing cargo integration tests (unchanged, still green). A
  header comment marks where it can grow to source the shared checks once an embedded
  loopback transport exists (a later V1.0 transport task).
- **Smoke-gate state:** `new:composable`. The gate is now **manifest-driven**:
  `scripts/smoke.sh` is a thin driver that reads `scripts/smoke.manifest` (one capability
  per line, in run order; `#`-comments allowed) and sources + runs the matching
  `scripts/smoke.d/<NN>-<cap>.sh` files. **To extend the gate, a future task (109/110/111/112)
  adds ONE file `scripts/smoke.d/<NN>-<newcap>.sh` defining `check_<newcap>` (documenting the
  vars it requires/exports) and appends `<newcap>` to `scripts/smoke.manifest` in the right
  order — no edit to the driver.** Public interface LOCKED: the `smoke.sh` contract (runs the
  manifest, exit 0 = pass, accepts `--ci-mode`/`--only`/`--list`), the
  `smoke.d/<NN>-<capability>.sh` layout (sourced; defines `check_<capability>`), and the
  `smoke.manifest` format.
