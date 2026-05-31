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
- [ ] `smoke.sh` split into manifest-driven `smoke.d/` capability checks
- [ ] V0.1 capability set passes identically; `--only` and `--list` work
- [ ] `shellcheck` clean; `smoke-embedded.sh` still works
- [ ] CI smoke workflow green
- [ ] Single commit created with the message below

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
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
