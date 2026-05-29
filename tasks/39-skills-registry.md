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
- [x] Verification commands pass.
- [x] Discovery covers personal + project scopes.
- [x] Toggle persists.
- [x] Malformed files don't break discovery.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **Migration number is `0005_skills_index.sql`** (not `0004_skills_index.sql` as the task spec named it). Tasks 30/36/38 had already taken `0002`/`0003`/`0004` between the spec being written and Task 39 landing, so the next free slot is `0005`. The schema is otherwise as the spec describes — `id / scope / project_id / name / slash_command / description / tools_json / source_path / enabled / discovered_at` with `UNIQUE(scope, project_id, name)`. Per-scope CHECK enforces the four-scope contract from `design/06 §1`; the `marketplace_id`, `pinned_version`, `visibility`, `last_used_at`, `invocation_count`, `kind` columns from `design/06 §4` are intentionally omitted in V0.1 and arrive with V1.0's marketplace migration.
  - **`ApiServerActor::with_managers` grew a 9th argument** (`skills_registry: Option<SkillsRegistryHandle>`). `#[allow(clippy::too_many_arguments)]` was already on the constructor for the same reason Tasks 19/20/22/23/38 each added a slot. The `run_uds` glue takes the matching 10th positional argument and adds `Skills` to the registered services block.
  - **Boot-time discovery runs once before the gRPC server starts accepting traffic** (`main.rs` calls `skills_handle.refresh(None).await` after the actor is spawned). Errors are logged + swallowed so a broken `~/.claude/skills/` directory does not gate Core boot; the UI just sees an empty list until the user calls `Skills.RefreshMarketplaces`.
  - **Hand-rolled YAML frontmatter splitter** in `crates/core/src/skills/discovery.rs::parse_frontmatter` instead of pulling in `gray_matter` (which adds a transitive dep tree). Strategy: strip BOM, require the first non-BOM line to be `---`, accumulate body lines until the next `---`, parse the body via `serde_yaml::from_str::<SkillFrontmatter>`. Empty file / missing leading delim / missing trailing delim / malformed YAML all surface as a descriptive `String` error the walker pushes onto `report.errors`. Unit tests pin all four failure modes in `discovery::tests`.
  - **`SkillScope` derives `PartialOrd, Ord`** (beyond the spec-implied minimum) so the integration test can `sort()` a `Vec<(SkillScope, &str)>` for deterministic comparison. Cheap copy enum; no impact on the wire surface.
  - **`SkillsRegistryHandle::refresh` returns a `SkillsRefreshReport`** (`{ discovered_count: u64, errors: Vec<String> }`) which the gRPC handler converts into `RefreshMarketplacesResponse { discovered_count: i64, errors: repeated string }`. The wire field carries `i64` because `repeated/int64` is the standard proto3 pattern; the persistence layer's `u64` row count is widened on the way out.
- **Open questions for next task:**
  - **Task 40 (suggestion rule engine)** is the next consumer of the skills surface — once it lands, the engine will read the same `skills_index` rows to drive its rule matching. The `SkillsRegistryHandle` is already the right plumbing point; no new persistence layer needed.
  - **The fs watcher hinted at in `design/06 §3` is V1.0**. V0.1 ships on-demand refresh via `Skills.RefreshMarketplaces`; the watcher arrives once we wire `notify-rs` into the workspace, which the design doc reserves for the marketplace surface anyway.
  - **The `plugin` / `enterprise` scopes** are reserved on the row + the wire but not actively walked. When plugin discovery lands (V1.0), the addition is purely additive — a new `walk_scope` call inside `discover` and a new gRPC field on the wire that already accepts the string value.
- **Deliberate debt:**
  - Plugin/enterprise scopes are stubs (the V0.1 walk only touches `personal` + `project`). Slash-command execution surface (the Maestro/agent flow) is V1.0. Marketplace install / sandbox / invocation tracking are V1.0+ per `tasks/39 §"Scope — out"`. The fs watcher that `design/06 §3` describes is V1.0; V0.1 ships on-demand refresh via the same `Skills.RefreshMarketplaces` RPC name so the wire shape does not break when the marketplace half lands behind it.
  - No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code; rustfmt-clean, clippy-clean (`-D warnings`).
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` (v2) still boots the Core, exercises the project/repo/workspace/workarea + echo session flow, and shuts down cleanly. The Task 39 RPCs (`Skills.*`) are exercised by `crates/core/tests/skills_discovery.rs` against a `SkillsRegistryHandle` directly — no separate gRPC integration test is needed because the handler is a thin wrapper.
