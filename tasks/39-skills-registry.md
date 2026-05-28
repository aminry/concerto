# Task 39 — Skills Registry: Discovery + Per-Project Toggle

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 19 |
| Touches subsystem(s) | 06 (Skills Registry) |
| Smoke gate | unchanged |

## Goal
Discover skills across the four scopes (personal `~/.claude/skills/`, project `<repo>/.claude/skills/`, plugin, enterprise) and surface them via gRPC with a per-project enable/disable toggle. V0.1 ships discovery + toggle only — marketplace install (`design/06 §3.x`) is V1.0.

## Inputs to read before starting
- `design/06_Skills_Registry.md` (entire — it's ~20KB; for V0.1 focus on §1 scope, §2 V0.1 row, §3 discovery, §4 schema, §5 RPC surface).
- `design/09_Persistence.md` §4.5 (`skills_index` schema).

## Scope — in
- Add migration `0004_skills_index.sql` per `design/09 §4.5`.
- Implement `crates/core/src/skills/`:
  - `SkillsRegistryActor` (impl `Actor`).
  - `discover() -> Result<Vec<SkillEntry>>`:
    - Personal: walk `~/.claude/skills/*/SKILL.md` (each subdir is one skill).
    - Project: per repo, walk `<repo_worktree>/.claude/skills/*/SKILL.md`.
    - Plugin / Enterprise: V0.1 stubs.
  - Each `SKILL.md` has YAML frontmatter (`name`, `description`, optional `slash-command`, `tools`). Parse with `serde_yaml`.
  - Each discovered skill upserts a `skills_index` row keyed on `(scope, project_id, name)`.
  - `enable(project_id, skill_id)` / `disable(...)` toggles `skills_index.enabled`.
  - `list(filter)` returns rows joined to in-memory metadata.
- Run discovery on Core start AND on demand via `Skills.RefreshMarketplaces` (V0.1 reuses this RPC for refresh; V1.0 adds real marketplace).
- gRPC: `Skills.ListSkills`, `Skills.ToggleSkill`, `Skills.RefreshMarketplaces`.
- Slash-command discovery: skills that declare `slash-command: /foo` are surfaced; the Maestro/agent flow that invokes slash commands is V1.0.
- Tests:
  - Fixture filesystem with two skills (personal + project); discover returns both.
  - Toggle: disable a skill; re-list shows `enabled=false`.
  - Malformed `SKILL.md`: warned, skipped, doesn't break discovery.

## Scope — out
- Marketplace install (V1.0).
- Sandbox test of a skill (V1.0).
- Enterprise allow/deny lists (V2.0).
- Slash-command execution surface (V1.0 — needs Maestro + agent SDK integration).

## Public interface this task locks
- Rust: `crates/core/src/skills/mod.rs` — `SkillsRegistryHandle::list`, `.toggle`, `.refresh`. Frozen.
- Proto: `Skills` service with three RPCs (`ListSkills`, `ToggleSkill`, `RefreshMarketplaces`). Frozen field numbers.
- DB migration `0004_skills_index.sql`. Frozen.
- File layout discovered: `~/.claude/skills/<name>/SKILL.md` (personal) and `<repo>/.claude/skills/<name>/SKILL.md` (project).

## Implementation notes
- `serde_yaml` for YAML frontmatter (it's the de-facto Rust YAML lib; add as a workspace dep).
- The SKILL.md file format: lines 1-N are YAML frontmatter delimited by `---`; the rest is markdown. Parse with `gray_matter` or a small hand-rolled splitter.
- Discovery cost: `walkdir` over ~/.claude/skills/ — fast even with 100 skills.
- Use `tracing::warn!` for malformed-file cases; the audit log writer arrives in Task 44.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core skills` → tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: place a SKILL.md fixture under `~/.claude/skills/test-skill/`; call `Skills.ListSkills`; verify it appears.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Discovery covers personal + project scopes.
- [ ] Toggle persists.
- [ ] Malformed files don't break discovery.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/persist/migrations/0004_skills_index.sql` (new)
- `crates/persist/src/skills.rs` (new)
- `crates/core/src/skills/mod.rs` (new)
- `crates/core/src/skills/actor.rs` (new)
- `crates/core/src/skills/discovery.rs` (new)
- `crates/proto/proto/concerto/v1/skills.proto` (new)
- `crates/core/src/handlers/skills.rs` (new)
- `crates/core/src/main.rs` (modified)
- `crates/core/tests/skills_discovery.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md`, `schema.md` (regenerated)

## Commit message
```
phase-3: skills registry — discovery + per-project toggle

Walks ~/.claude/skills/ and per-repo .claude/skills/, upserts
skills_index rows. ToggleSkill enables/disables per project.
Marketplace install + sandbox is V1.0.

Refs: tasks/39-skills-registry.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** plugin/enterprise scopes are stubs; slash-command execution is V1.0.
- **Smoke-gate state:** unchanged.
