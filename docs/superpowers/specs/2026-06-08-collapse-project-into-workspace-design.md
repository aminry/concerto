# Collapse Project → Workspace — Design

| Field | Value |
|---|---|
| Status | Approved for planning (2026-06-08) |
| Owner | Amin Roudaki |
| Scope | Replace the 4-level `Project → Workspace → Workarea → Session` hierarchy with a 3-level `Workspace → Workarea → Session` hierarchy backed by a global Repository registry. No backward compatibility (pre-release; no deployed data). |
| Supersedes / amends | `design/00`, `02`, `03`, `09`, `10`, `13`, `15` (canonical model); adds an amendment note to `tasks/v1.0/README.md`. The `tasks/v1.0/*` task files are frozen execution history and are **not** rewritten. |

---

## 1. Motivation

Today Concerto has four levels of nesting:

```
Project              owns repositories + shared settings (scripts, files-to-copy, permission defaults)
  └── Workspace      a workstream; selects a subset of the project's repos
        └── Workarea worktrees on disk per repo; branch; PR set
              └── Session  an agent run
```

In practice the **Project** level is friction: users think in terms of "the work I'm doing" (a workspace) and "which repos that work touches." Project is an extra container that mostly exists to own the repo set and shared settings. We collapse Project into Workspace so that **creating a workspace is the single act of**: name the work, pick one or more repositories, and (per repo) choose full vs. sparse checkout and which directories. Each workarea then materializes worktrees for those repos.

This is a clean, backward-incompatible refactor: the product is pre-release, there is no deployed database, and the directive is to remove deprecated code entirely rather than keep compatibility shims.

## 2. Target conceptual model

```
Repository registry   global pool of shared clones (the .git object stores)
        ▲ referenced by (workspace_repos)
Workspace             names the work; selects repos + per-repo checkout config;
  │                   owns shared settings/scripts + permission defaults + icon
  └── Workarea        worktrees on disk, one per repo; branch; PR set; FSM
        └── Session   an agent run (Claude / Codex / Gemini)
```

- **Repository (global registry).** A repository is a cloned `.git` object store living at `~/concerto/repos/<repository_id>/.git`, **shared** across every workspace and workarea that references it. This sharing is the foundation of the blobless+sparse monorepo story (a 40 GB monorepo is cloned once even if many workspaces use it). `clone_strategy` (`full | blobless | treeless`) is a property of the clone and therefore repository-global. A repository carries editable **default sparse directories** (`cone_defaults_json`).
- **Workspace.** Absorbs everything Project did (shared settings, scripts, permission/deliberation defaults, icon) plus what it already did (workstream name, repo selection). References repos via `workspace_repos`, and stores per-`(workspace, repo)` checkout config (the sparse cone snapshot).
- **Workarea / Session.** Structurally unchanged; the FK chain now roots at `workspaces` instead of `projects → workspaces`.

## 3. Decisions locked (this design)

| # | Decision | Choice |
|---|---|---|
| D1 | Repository ownership | **Global registry (model A).** Workspaces *select* repos from a shared pool; the shared `.git` is reused across workspaces. |
| D2 | Clone strategy vs. sparse checkout | Clone strategy is **repository-global** (decided once at add-time, size-recommended). The per-`(workspace, repo)` choice is **sparse checkout only**: full working tree, or sparse + chosen directories. |
| D3 | Repo default sparse dirs | A repository has editable **default sparse directories** that seed each workspace's per-repo cone selection. Editable any time. |
| D4 | Edit-repo-default semantics | **Snapshot.** A workspace's per-repo cone set is its own copy once added; editing a repo's defaults affects only *future* workspaces (and workspaces that haven't customized that repo). A "reset to repo defaults" affordance exists for explicit re-pull. |
| D5 | Schema migration approach | **Rewrite `0001_initial_schema.sql` in place** (+ fix later migrations referencing `projects`). One-time exception to the append-only freeze, justified by pre-release / no deployed data. |
| D6 | Per-`(workspace, repo)` cones storage | A new **`workspace_repos.sparse_cones_json` column** (not nested in `workspaces.settings_json`). Mirrors `workarea_repos.sparse_cones_json`. |
| D7 | Checked-in settings filename | Rename `.concerto/project_settings.json` → **`.concerto/workspace_settings.json`**. |
| D8 | Docs scope | Rewrite the **canonical design docs** (`00/02/03/09/10/13/15`) + add a short amendment note to `tasks/v1.0/README.md`. `tasks/v1.0/*` task files are frozen history and are not edited. |
| D9 | Add-repo sources | Three: pick existing (registry), new URL (clone), **local folder (adopt-in-place, NEW capability)**. |

## 4. Data model

### 4.1 Removed

- `projects` table — **dropped**.
- `repositories.project_id` — **dropped**. `UNIQUE(project_id, url)`/`UNIQUE(project_id, name)` become global `UNIQUE(url)` / `UNIQUE(name)`.
- `workspaces.project_id` — **dropped**. `UNIQUE(project_id, slug)` becomes global `UNIQUE(slug)`.

### 4.2 Changed

`workspaces` (after collapse):

```sql
CREATE TABLE workspaces (
    id                          TEXT PRIMARY KEY,            -- UUIDv7
    name                        TEXT NOT NULL,
    slug                        TEXT NOT NULL,
    icon                        TEXT,                        -- moved from projects
    description                 TEXT,
    permission_mode             TEXT CHECK (... NULL OR IN ('strict','normal','auto','yolo')),
    bypass_destructive_guard    INTEGER CHECK (... NULL OR IN (0,1)),
    settings_json               TEXT NOT NULL DEFAULT '{}',  -- absorbs former projects.settings_json
    created_at                  INTEGER NOT NULL,
    archived_at                 INTEGER,
    UNIQUE(slug)
);
```

`workspaces.settings_json` now holds the former project settings: `scripts` (`setup`/`setup_workarea`/`run`/`archive`), `run_script_mode`, `files_to_copy_rules`, `default_permission_mode`, `default_deliberation_mode`, `default_reasoning_level`, `writable_paths_outside_worktree`, `enterprise_data_privacy`.

`workspace_repos` (after collapse):

```sql
CREATE TABLE workspace_repos (
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    repository_id     TEXT NOT NULL REFERENCES repositories(id),
    position          INTEGER NOT NULL DEFAULT 0,   -- ordering + reference-repo selection (first by position)
    sparse_cones_json TEXT NOT NULL DEFAULT '[]',   -- NEW (D6): per-(workspace,repo) cone snapshot
    PRIMARY KEY (workspace_id, repository_id)
);
```

`repositories` (after collapse): drop `project_id`; everything else (`clone_strategy`, `default_branch`, `cone_defaults_json`, `local_path`, `fs_monitor_pid`, `last_fetch_at`) unchanged.

`workareas`, `workarea_repos`, `sessions`, `chats`, etc.: unchanged shapes. The `workarea_repos.sparse_cones_json` seed source becomes `workspace_repos.sparse_cones_json` (instead of `workspaces.settings_json["cone_defaults"]`).

### 4.3 Inheritance chains after collapse

- **Sparse cones:** `repositories.cone_defaults_json` → (snapshot at attach) `workspace_repos.sparse_cones_json` → (snapshot at workarea create) `workarea_repos.sparse_cones_json`.
- **Permission mode / bypass guard:** `sessions` → `workareas` → `workspaces` → global default `normal`. Clamped by `managed.json.maxPermissionMode` (unchanged).
- **Settings (scripts, files-to-copy, etc.):** `managed.json` > checked-in `.concerto/workspace_settings.json` (at the workspace's reference-repo root) > `workspaces.settings_json` (DB) > defaults.

## 5. Interfaces (proto)

> Field numbers were "frozen" under the v1 plan. Per the no-backcompat directive this is an explicit **re-lock at a new version**, documented in each proto file's header comment. No compatibility with old field numbers is preserved.

- **Delete `crates/proto/proto/concerto/v1/projects.proto`** and the `Projects` service (and its registration).
- **`workspaces.proto`:**
  - `Workspace`: remove `project_id`; add `icon`.
  - `CreateWorkspaceRequest`: remove `project_id`; add `icon`; add a repeated per-repo checkout spec carrying `repository_id` (for existing repos) and the sparse selection. New-repo creation (URL/local) is done via `Repositories.AddRepository` first, then attached.
  - `ListWorkspaces`: remove `project_id` filter → lists all (non-archived by default, `include_archived` flag).
- **`repositories.proto`:**
  - `AddRepoRequest`: remove `project_id`; add a `source` oneof/enum distinguishing **URL clone** vs **local-folder adopt** (carrying `local_path`).
  - `Repository`: remove `project_id`.
  - Add `rpc SetRepoConeDefaults(SetRepoConeDefaultsRequest) returns (Repository)` to edit a repo's default sparse directories.
  - `EstimateRepoSize` unchanged.
- **`sessions.proto`:** `UpsertProjectMcp` / `McpScope::Project(RepositoryId)` are already **repo-scoped despite the name** (they read `<repo_local_path>/.mcp.json`). Out of scope to rename in this change; left as-is to keep blast radius bounded. (Noted for a future cleanup.)

## 6. Rust changes (remove, don't deprecate)

**Persistence (`crates/persist`):**
- Delete `src/projects.rs`, the `Project`/`ProjectId` types in `src/api.rs`, and `list_by_project` queries.
- `src/repositories.rs`: drop `project_id` params; `list_all()` replaces `list_by_project`.
- `src/workspaces.rs`: drop `project_id`; add `icon`; `list_all()` replaces `list_by_project`; add `workspace_repos.sparse_cones_json` get/set; add reference-repo helper (first by position).
- Rewrite `migrations/0001_initial_schema.sql` (D5); fix later migrations that name `projects` (notably `0009_workspace_repos_position.sql`, `0011_repositories_action_prefs.sql`, and any `UNIQUE(project_id, …)`).

**Core (`crates/core`):**
- Delete `src/handlers/projects.rs`; remove `Projects` service registration in `api_server.rs`/`boot.rs`.
- `src/workspace_manager/actor.rs`: `create_workspace` drops `project_id`, validates referenced repos exist, attaches `workspace_repos` rows with `sparse_cones_json` seeded from each repo's `cone_defaults_json`; `update_workspace_repos` preserves seeding; slug uniqueness becomes global.
- `src/workspace_manager/files_to_copy.rs`: read rules from the workspace (reference-repo `.concerto/workspace_settings.json` + `workspaces.settings_json`) instead of project.
- `src/repo_manager/*`: `add_repository` drops `project_id`; add `import_local(local_path)` that adopts an existing repo (validates it's a git repo, registers `local_path`, applies the locked git config, starts fsmonitor; no clone).
- `src/settings/*`: rename `ProjectSettingsResolver` → `WorkspaceSettingsResolver`, key by `workspace_id`, DB layer reads `workspaces.settings_json`, checked-in file renamed to `workspace_settings.json` and located at the workspace's reference-repo root. `project_file.rs` → `workspace_file.rs`.
- `src/security/permission.rs`: collapse the inheritance chain to `session → workarea → workspace → default`.
- `src/audit/event.rs`: `ProjectSettingsResolved` → `WorkspaceSettingsResolved` (and any `project_id` audit fields → `workspace_id`).
- `src/handlers/repositories.rs`, `workspaces.rs`, `workareas.rs`, `sessions.rs`: update signatures; remove `project_id` validation.

**Naming:** `ProjectId` is removed; nowhere gets a "project" identifier. Where a repo previously needed project scoping, it is now global.

## 7. Desktop changes (`apps/desktop`)

**Delete:** `components/NewProjectModal.tsx` (+ test), `api/projects.ts`, `hooks/useProjects.ts`, and the project state in `state/useUiStore.ts` (`selectedProjectId`, `collapsedProjects`, `newProjectModalOpen`, their setters, `toggleProjectExpanded`).

**Sidebar (`components/Sidebar.tsx`):** remove the `ProjectNode` layer. Top level becomes a **flat workspace list** → `WorkareaList` → sessions. `useWorkspaces()` no longer takes a project id (lists all).

**Workspace creation (`components/NewWorkspaceModal.tsx`):** becomes the primary creation flow:
- name + icon + description
- the **3-source repo picker** (D9): a list of existing registry repos (search/select), an "Add by URL" path (reusing `cloneStrategy` + size→strategy recommendation), and an **"Add local folder"** path (folder picker → adopt).
- per-repo **checkout config**: full working tree vs. sparse + directories, reusing `ConePicker` / `RepoTreeBrowser` / `SparseConeDialog`, pre-seeded from the repo's `cone_defaults`.

**Repo settings surface:** edit a repository's default sparse directories (`SetRepoConeDefaults`) so future workspaces inherit the adjustment; surfaced in Settings (and/or workspace detail). A "reset to repo defaults" action on a workspace's repo (D4).

**API wrappers:** `api/workspaces.ts` (`listWorkspaces()` no project arg, `createWorkspace` new shape), `api/repositories.ts` (`Repository` drops `project_id`; `addRepository` new source shape; `setRepoConeDefaults`).

## 8. Disk layout — unchanged

- Repos: `~/concerto/repos/<repository_id>/.git` (shared pool).
- Workareas: `~/concerto/workspaces/<workspace-slug>/<composer>/<repo-name>/`.

## 9. Testing strategy (follow existing patterns)

| Layer | What | How |
|---|---|---|
| Unit | Permission inheritance — now `session → workarea → workspace → default` | Table-driven (update `permission_inheritance.rs`) |
| Unit | Cone seeding + snapshot semantics (D3/D4): attach seeds from repo defaults; editing repo defaults doesn't mutate existing workspaces | Table-driven |
| Unit | Composers allocator (per workspace) | Property test (unchanged logic, retargeted) |
| Unit | Workspace settings resolver (renamed) — precedence over the 3 layers | Existing resolver tests, retargeted to workspace |
| Integration | Multi-repo workspace create against the global registry | E2E with stubbed agent |
| Integration | **One clone shared by two workspaces** — verify a single `.git`, two `workspace_repos` rows, independent cones | E2E |
| Integration | **Local-folder import** — adopt an existing repo, attach to workspace, materialize a workarea | E2E |
| Integration | Files-to-copy resolved from workspace reference repo | Fixture filesystem |
| Handler | `Workspaces`/`Repositories` RPCs without `project_id` | Tonic handler tests |
| Desktop | New `NewWorkspaceModal` 3-source picker + per-repo checkout; flat Sidebar | vitest + RTL |
| Smoke | Update the smoke-gate manifest capabilities that referenced projects | `scripts/smoke.sh` |
| Migration | Fresh DB from rewritten `0001` applies cleanly; schema has no `projects` table | Persistence boot test |

**Every existing test referencing `project_id` is migrated, not worked around.** The full per-type gate from `tasks/v1.0/README.md §5.3` applies (`cargo check/clippy/fmt/deny/test` + interface regen for Rust; `pnpm typecheck/lint/test/build` for desktop).

## 10. Documentation changes

- **`design/00_Architecture_Overview.md`** — data-model / hierarchy sections: 4-level → 3-level + global repo registry.
- **`design/02_Repository_Manager.md`** — repository is global (drop `project_id` from API: `add_project_repository` → `add_repository`); add local-folder adopt; cone-defaults editing; `add_project` references become "add repository to registry."
- **`design/03_Workspace_Session_Manager.md`** — retitle the hierarchy; workspace absorbs project settings/scripts/permission defaults; permission chain shrinks; reference-repo concept; `workspace_repos.sparse_cones_json`.
- **`design/09_Persistence.md §4.1`** — schema: drop `projects`, update `repositories`/`workspaces`/`workspace_repos`.
- **`design/10_Local_API_Protocol.md`** — drop `Projects` service; update `Workspaces`/`Repositories` surfaces.
- **`design/13_VCS_Provider_Integration.md`** — any project-scoped references → workspace/repo.
- **`design/15_Desktop_Client.md`** — sidebar tree (no project node); creation flow; repo picker.
- **`tasks/v1.0/README.md`** — a short amendment note recording this collapse and pointing to this spec; `tasks/v1.0/*` task files untouched (frozen history).

## 11. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Permission-inheritance regressions from chain change | Keep the table-driven tests authoritative; add a 3-layer case matrix before refactoring the resolver. |
| Settings resolver re-keying misses a checked-in/managed path | Migrate `settings/*` as a unit with its existing tests retargeted; assert audit `WorkspaceSettingsResolved` fires per field. |
| Local-folder adopt corrupts/misconfigures an existing user repo | Adopt is non-destructive: validate it's a git repo, never re-init, apply only additive config (`core.fsmonitor`, `untrackedCache`), and surface what was changed in the audit log. |
| Global repo name collisions (names were project-scoped) | Global `UNIQUE(name)` with auto-suffix on collision at add-time; URL de-dup returns the existing row. |
| Large blast radius (~70 Rust + ~122 TS refs) | Sequence the work foundation-up (schema → persist types → proto → core managers/handlers → desktop), each step gated green before the next; interface regen committed at the proto step. |

## 12. Out of scope

- Renaming the MCP `Project` scope (it is repo-scoped; left as-is).
- Any grouping level above Workspace (tags/folders) — the top level is a flat workspace list.
- Mobile / web clients (this change targets Core + Desktop; mobile/web are not yet built per the v1 plan).
- Per-repo branch override (still V2.0).
