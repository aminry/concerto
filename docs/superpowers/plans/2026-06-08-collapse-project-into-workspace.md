# Collapse Project → Workspace — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 4-level `Project → Workspace → Workarea → Session` hierarchy with a 3-level `Workspace → Workarea → Session` model over a global Repository registry, removing the Project concept entirely (no backward compatibility).

**Architecture:** Foundation-up. Rewrite the SQLite schema in place, then the persistence types/CRUD, then the proto contract (regenerated), then the Core managers/handlers/settings/permission, then the Desktop UI, then the design docs. The Rust compiler + the existing test suite are the authoritative gate for the large mechanical call-site sweeps; new behavior (local-folder adopt, cone seeding/snapshot, the re-keyed settings/permission chains) is driven test-first.

**Tech Stack:** Rust (sqlx/SQLite, tonic/prost, tokio), TypeScript/React (Vite, vitest, React Testing Library, zustand, TanStack Query), Tauri.

**Spec:** `docs/superpowers/specs/2026-06-08-collapse-project-into-workspace-design.md` (decisions D1–D9).

---

## Conventions for this plan

- **Verification gate per Rust task** (the v1 `README §5.3` fast-local gate): `cargo check --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --all -- --check` · the named tests · `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` when a `pub` type / proto / SQL changed.
- **Verification gate per Desktop task:** `pnpm -C apps/desktop typecheck` · `pnpm -C apps/desktop lint` · `pnpm -C apps/desktop test` · `pnpm -C apps/desktop build`.
- **Sweep tasks** ("update every call site of X") are verified by `cargo check --workspace` reaching green plus the named regression tests. The grep given in the task is the worklist; the compiler confirms completeness.
- **Commit** after each task with the message shown.
- Per spec D5 the schema is rewritten in place — there is **no** new migration file; `0001` is edited.

---

# Phase 0 — Schema rewrite (migrations)

### Task 0.1: Rewrite `0001_initial_schema.sql` — drop `projects`, re-root `repositories`/`workspaces`

**Files:**
- Modify: `crates/persist/migrations/0001_initial_schema.sql`
- Test: `crates/persist/tests/initial_schema.rs`

- [ ] **Step 1: Update the schema assertion test to the new shape**

In `crates/persist/tests/initial_schema.rs`, find the assertions that the `projects` table exists and that `repositories`/`workspaces` have a `project_id` column. Replace them with assertions that:
- there is **no** `projects` table (`SELECT count(*) FROM sqlite_master WHERE type='table' AND name='projects'` returns `0`);
- `repositories` has no `project_id` column and has a `UNIQUE(url)` and `UNIQUE(name)` index;
- `workspaces` has no `project_id` column, has an `icon` column, and a `UNIQUE(slug)` index;
- `workspace_repos` has a `sparse_cones_json` column.

Use the existing helper style in that test file (it already introspects `PRAGMA table_info`). Example assertion to add:

```rust
#[tokio::test]
async fn schema_has_no_projects_table() {
    let pool = fresh_pool().await; // existing helper in this test file
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='projects'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "projects table must be gone after the collapse");
}

#[tokio::test]
async fn workspace_repos_has_sparse_cones_json() {
    let pool = fresh_pool().await;
    let cols: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('workspace_repos')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(cols.iter().any(|c| c == "sparse_cones_json"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p concerto-persist --test initial_schema -- schema_has_no_projects_table workspace_repos_has_sparse_cones_json`
Expected: FAIL (the `projects` table still exists; `sparse_cones_json` column missing).

- [ ] **Step 3: Edit `0001_initial_schema.sql`**

Delete the `CREATE TABLE projects (...)` block entirely. Edit `repositories`:

```sql
CREATE TABLE repositories (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    url                 TEXT NOT NULL,
    local_path          TEXT NOT NULL,
    clone_strategy      TEXT NOT NULL,
    default_branch      TEXT NOT NULL,
    cone_defaults_json  TEXT NOT NULL DEFAULT '[]',
    fs_monitor_pid      INTEGER,
    last_fetch_at       INTEGER,
    UNIQUE(url),
    UNIQUE(name)
);
```

Edit `workspaces`:

```sql
CREATE TABLE workspaces (
    id                          TEXT PRIMARY KEY,
    name                        TEXT NOT NULL,
    slug                        TEXT NOT NULL,
    icon                        TEXT,
    description                 TEXT,
    permission_mode             TEXT CHECK (permission_mode IS NULL OR permission_mode IN ('strict','normal','auto','yolo')),
    bypass_destructive_guard    INTEGER CHECK (bypass_destructive_guard IS NULL OR bypass_destructive_guard IN (0,1)),
    settings_json               TEXT NOT NULL DEFAULT '{}',
    created_at                  INTEGER NOT NULL,
    archived_at                 INTEGER,
    UNIQUE(slug)
);
```

Edit `workspace_repos` to add the position column (was migration 0009 — fold it in here since 0009 will be neutralized in Task 0.2) and the new `sparse_cones_json` column:

```sql
CREATE TABLE workspace_repos (
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    repository_id     TEXT NOT NULL REFERENCES repositories(id),
    position          INTEGER NOT NULL DEFAULT 0,
    sparse_cones_json TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (workspace_id, repository_id)
);
```

Leave `workareas`, `workarea_repos`, `sessions`, `chats`, etc. structurally as-is (they reference `workspaces`/`workareas`, which still exist).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p concerto-persist --test initial_schema`
Expected: PASS (all schema assertions, including the pre-existing ones that don't mention projects).

- [ ] **Step 5: Commit**

```bash
git add crates/persist/migrations/0001_initial_schema.sql crates/persist/tests/initial_schema.rs
git commit -m "persist: rewrite 0001 schema — drop projects, re-root repos/workspaces (D5)"
```

### Task 0.2: Reconcile later migrations that reference `projects`

**Files:**
- Modify: `crates/persist/migrations/0009_workspace_repos_position.sql`
- Modify: `crates/persist/migrations/0011_repositories_action_prefs.sql`
- Modify: `crates/persist/migrations/0005_skills_index.sql`

- [ ] **Step 1: Neutralize 0009 (position now lives in 0001)**

`0009_workspace_repos_position.sql` added `workspace_repos.position`, which Task 0.1 folded into `0001`. Replace the `ALTER TABLE workspace_repos ADD COLUMN position …` with a no-op comment so the migration sequence stays contiguous and re-running a fresh DB doesn't double-add the column:

```sql
-- 0009 (folded into 0001 by the Project→Workspace collapse, 2026-06-08):
-- workspace_repos.position is now declared directly in the initial schema.
-- This file is intentionally a no-op to keep the migration sequence stable.
SELECT 1;
```

- [ ] **Step 2: Fix 0011's project FK language**

`0011_repositories_action_prefs.sql` adds `repositories.action_prefs_json` — keep the column. Its header comment calls it the "three-layer project/repository" layer; update the comment to "workspace/repository". No DDL change needed (the column is on `repositories`, which still exists). Verify it has no `project_id` reference (the grep showed only a comment).

- [ ] **Step 3: Re-scope the `skills_index` project scope**

`0005_skills_index.sql` defines `skills_index` with `scope IN ('personal','project','plugin','enterprise')`, a `project_id TEXT REFERENCES projects(id)`, `UNIQUE(scope, project_id, name)`, and `idx_skills_index_project`. Rewrite to a **workspace** scope:

```sql
    scope           TEXT NOT NULL
        CHECK (scope IN ('personal','workspace','plugin','enterprise')),
    workspace_id    TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    ...
    UNIQUE(scope, workspace_id, name)
```

and rename the index to `idx_skills_index_workspace ON skills_index(workspace_id)`. Update the file's header comment block accordingly (`project` → `workspace`).

- [ ] **Step 4: Run the full persist migration/boot test**

Run: `cargo test -p concerto-persist`
Expected: PASS. (The `skills.rs` CRUD still references `project`-scope strings — those compile errors are fixed in Task 1.5; if `cargo test -p concerto-persist` fails to compile here because of `skills.rs`, that's expected and resolved in Phase 1. Run `cargo test -p concerto-persist --test initial_schema` to confirm the migrations themselves apply.)

Run: `cargo test -p concerto-persist --test initial_schema`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/persist/migrations/0005_skills_index.sql crates/persist/migrations/0009_workspace_repos_position.sql crates/persist/migrations/0011_repositories_action_prefs.sql
git commit -m "persist: reconcile later migrations to the project-less schema"
```

---

# Phase 1 — Persistence layer (`crates/persist`)

### Task 1.1: Reshape the persist API types

**Files:**
- Modify: `crates/persist/src/api.rs`
- Modify: `crates/persist/src/lib.rs` (re-exports)

- [ ] **Step 1: Delete the Project types**

In `api.rs` remove `ProjectId`, `NewProject`, `Project` (the three structs + their `impl` blocks, lines ~361–404 in the current file) and the `Projects + Workspaces (Task 19)` comment banner that introduces them.

- [ ] **Step 2: Drop `project_id` from `NewRepository` and `Repository`**

Remove the `pub project_id: String,` field from both `NewRepository` and `Repository`. Add the `cone_defaults_json` / `action_prefs_json` fields already present on `Repository` — leave those as-is.

- [ ] **Step 3: Reshape `NewWorkspace` and `Workspace`**

```rust
#[derive(Debug, Clone)]
pub struct NewWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
    pub description: Option<String>,
    /// Lowercase SQL form (`"strict" | "normal" | "auto" | "yolo"`) or
    /// `None` for "inherit from workspace defaults".
    pub permission_mode: Option<String>,
    /// Unix epoch milliseconds.
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
    pub description: Option<String>,
    pub permission_mode: Option<String>,
    pub created_at: i64,
    pub archived_at: Option<i64>,
}
```

Update all doc comments that say "inherit from project" → "inherit from workspace defaults" and remove `UNIQUE(project_id, slug)` mentions (now `UNIQUE(slug)`).

- [ ] **Step 4: Update `lib.rs` re-exports**

Remove `Project`, `NewProject`, `ProjectId` from the `pub use api::{…}` list and any `pub mod projects;` / re-export of the `projects` module (the module is deleted in Task 1.3).

- [ ] **Step 5: Verify (will not fully compile yet — that's expected)**

Run: `cargo check -p concerto-persist`
Expected: errors only in `projects.rs`, `workspaces.rs`, `repositories.rs`, `skills.rs` (fixed in 1.2–1.5). Confirm `api.rs`/`lib.rs` themselves are not the error source. Do not commit until Task 1.5 makes the crate compile; this task's changes are committed together with 1.2–1.5 OR commit now with `--no-verify` discipline waived. **Recommendation:** treat Tasks 1.1–1.5 as one commit boundary (the persist crate must compile to commit). Proceed through 1.5, then commit once.

### Task 1.2: Reshape `workspaces.rs` CRUD

**Files:**
- Modify: `crates/persist/src/workspaces.rs`

- [ ] **Step 1: `insert` — drop project_id, add icon**

```rust
pub async fn insert(conn: &mut SqliteConnection, ws: NewWorkspace) -> Result<WorkspaceId> {
    let id = ws.id.clone();
    sqlx::query(
        "INSERT INTO workspaces (
            id, name, slug, icon, description, permission_mode, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id.0)
    .bind(&ws.name)
    .bind(&ws.slug)
    .bind(&ws.icon)
    .bind(&ws.description)
    .bind(&ws.permission_mode)
    .bind(ws.created_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}
```

- [ ] **Step 2: `get` + `row_to_workspace` — new column set**

Update the `SELECT` in `get` to `id, name, slug, icon, description, permission_mode, created_at, archived_at` and rewrite `row_to_workspace`:

```rust
fn row_to_workspace(row: sqlx::sqlite::SqliteRow) -> Workspace {
    Workspace {
        id: WorkspaceId(row.get::<String, _>("id")),
        name: row.get::<String, _>("name"),
        slug: row.get::<String, _>("slug"),
        icon: row.get::<Option<String>, _>("icon"),
        description: row.get::<Option<String>, _>("description"),
        permission_mode: row.get::<Option<String>, _>("permission_mode"),
        created_at: row.get::<i64, _>("created_at"),
        archived_at: row.get::<Option<i64>, _>("archived_at"),
    }
}
```

- [ ] **Step 3: Replace `list_by_project` with `list_all`**

```rust
/// List every workspace (read-only). Sorted by `name` for deterministic
/// UI / test output.
pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Workspace>> {
    let rows = sqlx::query(
        "SELECT id, name, slug, icon, description,
                permission_mode, created_at, archived_at
         FROM workspaces ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_workspace).collect())
}
```

- [ ] **Step 4: Add per-`(workspace, repo)` cone get/set (D6)**

Add functions for the new column, and update `update_repos` to accept an optional seed. Keep `update_repos`'s ordering contract; add a sibling that also seeds cones:

```rust
/// Read the per-(workspace, repo) sparse-cone snapshot (D6). Returns the
/// decoded JSON string `"[…]"`; `None` if the junction row is absent.
pub async fn get_repo_cones(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
    repo_id: &RepositoryId,
) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT sparse_cones_json FROM workspace_repos \
         WHERE workspace_id = ? AND repository_id = ?",
    )
    .bind(&workspace_id.0)
    .bind(&repo_id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(|r| r.get::<String, _>("sparse_cones_json")))
}

/// Overwrite the per-(workspace, repo) sparse-cone snapshot (D6). Used by
/// "edit this workspace's cones for repo X" and "reset to repo defaults".
pub async fn set_repo_cones(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    repo_id: &RepositoryId,
    cones_json: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE workspace_repos SET sparse_cones_json = ? \
         WHERE workspace_id = ? AND repository_id = ?",
    )
    .bind(cones_json)
    .bind(&workspace_id.0)
    .bind(&repo_id.0)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}
```

Extend `update_repos` to take seed cones per repo so attach seeds the snapshot from repo defaults (D3/D4). Change its signature to accept `&[(RepositoryId, String)]` (id + cones_json):

```rust
pub async fn update_repos(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    repos: &[(RepositoryId, String)],
) -> Result<()> {
    sqlx::query("DELETE FROM workspace_repos WHERE workspace_id = ?")
        .bind(&workspace_id.0)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    for (position, (repo_id, cones_json)) in repos.iter().enumerate() {
        sqlx::query(
            "INSERT INTO workspace_repos \
               (workspace_id, repository_id, position, sparse_cones_json) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&workspace_id.0)
        .bind(&repo_id.0)
        .bind(position as i64)
        .bind(cones_json)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    }
    Ok(())
}
```

> Note: callers that don't reorder cones pass the existing snapshot back (read-modify-write at the manager layer, Task 3.2). The `update_repos` callers in `actor.rs` are updated in Phase 3.

- [ ] **Step 5: Update remaining project language**

Update the `SQLITE_CONSTRAINT_UNIQUE` doc (now `(slug)` not `(project_id, slug)`), `set_permission_mode` doc ("inherit from workspace defaults"), the module header schema comment block, and `get_settings_json` doc (the `cone_defaults` nested-map note is now obsolete — cones live in the column; state that the resolver reads the column, not this JSON). Remove the `list_by_project` function.

- [ ] **Step 6: defer verification to Task 1.5** (crate compiles after the whole phase).

### Task 1.3: Delete `projects.rs`

**Files:**
- Delete: `crates/persist/src/projects.rs`
- Modify: `crates/persist/src/lib.rs` (`mod projects;` removed — done in 1.1)

- [ ] **Step 1:** `git rm crates/persist/src/projects.rs`
- [ ] **Step 2:** Confirm no `mod projects;` remains: `grep -rn "mod projects\|projects::" crates/persist/src` → empty.

### Task 1.4: Reshape `repositories.rs` CRUD (registry, local import)

**Files:**
- Modify: `crates/persist/src/repositories.rs`

- [ ] **Step 1: Drop `project_id` from `insert` + `row_to_repository`**

Remove `project_id` from the INSERT column list and the bind, and from the row projection. Update the `NewRepository` usage accordingly.

- [ ] **Step 2: Replace `list_by_project` with `list_all`**

```rust
pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Repository>> {
    let rows = sqlx::query(
        "SELECT id, name, url, local_path, clone_strategy, default_branch,
                cone_defaults_json, action_prefs_json, last_fetch_at, fs_monitor_pid
         FROM repositories ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_repository).collect())
}
```

(Adjust the column list to match the current `row_to_repository`.)

- [ ] **Step 3: Add a `get_by_url` helper for registry de-dup (D9)**

```rust
/// Look up a repository by its canonical `url` (registry de-dup: adding a
/// URL already present returns the existing row instead of cloning twice).
pub async fn get_by_url(pool: &SqlitePool, url: &str) -> Result<Option<Repository>> {
    let row = sqlx::query(
        "SELECT id, name, url, local_path, clone_strategy, default_branch,
                cone_defaults_json, action_prefs_json, last_fetch_at, fs_monitor_pid
         FROM repositories WHERE url = ?",
    )
    .bind(url)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_repository))
}
```

- [ ] **Step 4:** Keep `update_action_prefs_json`, `update_cone_defaults_json`, `update_fs_monitor_pid` — only remove any `project_id` from their signatures (they key by `repository_id`, so likely unchanged).

### Task 1.5: Re-scope `skills.rs` to workspace scope; make the crate compile

**Files:**
- Modify: `crates/persist/src/skills.rs`
- Modify: `crates/persist/src/sessions.rs` (only if it referenced project — grep showed a hit; likely a comment)

- [ ] **Step 1: Re-key the skills scope**

In `skills.rs`, change the `project` scope handling to `workspace`: rename the `project_id` parameter/column binding to `workspace_id`, the scope literal `"project"` → `"workspace"`, and any `ProjectId` usage to `WorkspaceId`. Update the `UNIQUE(scope, project_id, name)` upsert SQL to `(scope, workspace_id, name)`.

- [ ] **Step 2: Check `sessions.rs`**

`grep -n "project" crates/persist/src/sessions.rs`. If it's only a doc comment, update the wording; if a real `project_id`, remove it.

- [ ] **Step 3: Compile the whole crate**

Run: `cargo check -p concerto-persist`
Expected: PASS (clean).

Run: `cargo test -p concerto-persist`
Expected: the persist-crate unit + integration tests compile. Some integration tests under `crates/persist/tests/` (e.g. `workspace_repos_position.rs`, `workarea_repo_cones.rs`, `repositories_action_prefs.rs`) still construct `NewRepository`/`NewWorkspace` with `project_id` — fix those test fixtures now (remove `project_id`, add `icon: None`, switch `update_repos` calls to the `&[(RepositoryId, String)]` shape, seed `"[]"`). Re-run until green.

- [ ] **Step 4: clippy + fmt + interface regen**

Run: `cargo clippy -p concerto-persist --all-targets -- -D warnings` · `cargo fmt --all -- --check` · `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` (the persist interface summary changes — commit the regenerated file).

- [ ] **Step 5: Commit (Tasks 1.1–1.5 together)**

```bash
git add crates/persist docs/interfaces
git commit -m "persist: collapse Project into Workspace — types, CRUD, skills scope, cone column"
```

---

# Phase 2 — Proto contract (`crates/proto`)

> Field numbers were "frozen"; per the no-backcompat directive this is an explicit re-lock. Update each file's FROZEN header note to say "re-locked 2026-06-08 (Project→Workspace collapse)".

### Task 2.1: Delete `projects.proto` + service registration

**Files:**
- Delete: `crates/proto/proto/concerto/v1/projects.proto`
- Modify: `crates/proto/src/lib.rs` or build include list (wherever `projects.proto` is enumerated for `tonic-build`)
- Modify: `crates/core/src/api_server.rs` (remove `Projects` service add)

- [ ] **Step 1:** `git rm crates/proto/proto/concerto/v1/projects.proto`.
- [ ] **Step 2:** Remove `projects.proto` from the `tonic_build`/`prost` file list (search `build.rs` / `lib.rs` in `crates/proto` for `"projects"`).
- [ ] **Step 3:** In `api_server.rs`, delete the `.add_service(ProjectsServer::new(...))` line and its handler construction (the handler is deleted in Phase 3).
- [ ] **Step 4:** Defer compile to Task 2.3.

### Task 2.2: Edit `workspaces.proto` and `repositories.proto`

**Files:**
- Modify: `crates/proto/proto/concerto/v1/workspaces.proto`
- Modify: `crates/proto/proto/concerto/v1/repositories.proto`

- [ ] **Step 1: `workspaces.proto` — `Workspace` message**

Remove `string project_id = 2;`. Renumber is unnecessary (re-lock); set the new shape:

```proto
message Workspace {
  string id = 1;
  string name = 2;
  string slug = 3;
  optional string icon = 4;
  optional string description = 5;
  optional PermissionMode permission_mode = 6;
  google.protobuf.Timestamp created_at = 7;
  optional google.protobuf.Timestamp archived_at = 8;
}
```

- [ ] **Step 2: `CreateWorkspaceRequest` + per-repo checkout**

```proto
// One repository's checkout config within a CreateWorkspace call. The
// repo must already exist in the registry (added via Repositories.AddRepository
// for new URL/local repos). `sparse_cones` empty = full working tree;
// non-empty = sparse cone over those repo-root-relative directories.
message WorkspaceRepoSpec {
  string repository_id = 1;
  repeated string sparse_cones = 2;
}

message CreateWorkspaceRequest {
  string name = 1;
  repeated WorkspaceRepoSpec repos = 2;
  optional PermissionMode permission_mode = 3;
  optional string description = 4;
  optional string icon = 5;
}
```

- [ ] **Step 3: `ListWorkspacesRequest` — drop project filter**

```proto
message ListWorkspacesRequest {
  bool include_archived = 1;
}
```

- [ ] **Step 4: `repositories.proto` — registry + local source**

In `AddRepoRequest`, remove `string project_id = 1;`. Add a source discriminator for local-folder adopt (D9):

```proto
message AddRepoRequest {
  string name = 1;
  // Exactly one of url / local_path is set. `url` → clone into the shared
  // pool; `local_path` → adopt an existing on-disk git repo in place.
  string url = 2;
  string local_path = 7;             // NEW: local-folder adopt
  string default_branch = 4;
  string clone_strategy = 5;
  bool with_sparse = 6;
}
```

Remove `string project_id` from the `Repository` message. Add the cone-defaults editor RPC:

```proto
message SetRepoConeDefaultsRequest {
  string repository_id = 1;
  repeated string cone_defaults = 2;
}
```

and in `service Repositories { … }` add:

```proto
  rpc SetRepoConeDefaults(SetRepoConeDefaultsRequest) returns (Repository);
```

- [ ] **Step 5:** Defer compile to Task 2.3.

### Task 2.3: Regenerate + lock interfaces

- [ ] **Step 1:** Run: `cargo check -p concerto-proto`
Expected: PASS (proto compiles; generated Rust reflects the new shapes).
- [ ] **Step 2:** Run: `./scripts/regen-interfaces.sh` then `git diff docs/interfaces/` — review the proto interface summary diff (projects gone, workspace/repository reshaped).
- [ ] **Step 3: Commit**

```bash
git add crates/proto docs/interfaces crates/core/src/api_server.rs
git commit -m "proto: drop Projects service; reshape Workspaces/Repositories for the collapse"
```

> The whole workspace will NOT compile until Phase 3 updates the handlers/managers. That is expected; Phase 3 is sequenced to restore green.

---

# Phase 3 — Core managers, handlers, settings, permission (`crates/core`)

### Task 3.1: Delete the Projects handler

**Files:**
- Delete: `crates/core/src/handlers/projects.rs`
- Modify: `crates/core/src/handlers/mod.rs` (remove `pub mod projects;`)

- [ ] **Step 1:** `git rm crates/core/src/handlers/projects.rs`; remove its `mod` line.
- [ ] **Step 2:** Defer compile to Task 3.7.

### Task 3.2: `WorkspaceManager::create_workspace` — drop project, seed cones

**Files:**
- Modify: `crates/core/src/workspace_manager/actor.rs`

- [ ] **Step 1: New `create_workspace` signature + body**

Replace the `project_id`-scoped signature. It now takes per-repo specs (id + cones) and seeds the `workspace_repos.sparse_cones_json` snapshot from each repo's `cone_defaults_json` when the caller passes empty cones (D3/D4):

```rust
pub struct WorkspaceRepoSpec {
    pub repository_id: RepositoryId,
    /// Empty → seed from the repo's cone_defaults at attach time (D3).
    /// Non-empty → use as the workspace's snapshot.
    pub sparse_cones: Vec<String>,
}

pub async fn create_workspace(
    &self,
    name: &str,
    repos: &[WorkspaceRepoSpec],
    permission_mode: Option<String>,
    description: Option<String>,
    icon: Option<String>,
) -> Result<WorkspaceId> {
    if name.trim().is_empty() {
        return Err(Error::Validation("name is required".into()));
    }
    if repos.is_empty() {
        return Err(Error::Validation("a workspace needs at least one repo".into()));
    }
    // Validate every repo exists in the global registry, and resolve the
    // cone snapshot (seed from repo defaults when the spec leaves it empty).
    let mut seeded: Vec<(RepositoryId, String)> = Vec::with_capacity(repos.len());
    for spec in repos {
        let repo = concerto_persist::repositories::get(
            self.persistence.readers(), &spec.repository_id,
        )
        .await?
        .ok_or_else(|| Error::NotFound(format!("repository {} not found", spec.repository_id)))?;
        let cones_json = if spec.sparse_cones.is_empty() {
            repo.cone_defaults_json.clone() // snapshot the repo default (D4)
        } else {
            serde_json::to_string(&spec.sparse_cones)
                .map_err(|e| Error::Validation(e.to_string()))?
        };
        seeded.push((spec.repository_id.clone(), cones_json));
    }
    // … existing slug-derive + auto-suffix retry loop, but NewWorkspace
    // without project_id and with icon; then update_repos(conn, id, &seeded).
}
```

Remove `validate_workspace_repos`'s project scoping — it becomes "every repo exists in the registry" (already inlined above; delete the old helper or simplify it to a registry existence check used by both create + update).

- [ ] **Step 2: `list_by_project` → `list_all`**

```rust
pub async fn list_all(&self) -> Result<Vec<Workspace>> {
    concerto_persist::workspaces::list_all(self.persistence.readers()).await
}
```

- [ ] **Step 3: `update_workspace_repos` — seed + snapshot**

It now resolves the same seed (preserve an existing per-repo snapshot if the repo was already attached, else seed from repo defaults) and calls the new `update_repos(&[(RepositoryId, String)])`. Add a `reset_repo_cones_to_defaults(workspace, repo)` method that writes the repo's current `cone_defaults_json` into `workspace_repos.sparse_cones_json` (the D4 "reset" affordance), and a `set_repo_cones(workspace, repo, cones)` method.

- [ ] **Step 4: Audit JSON**

In the `WorkspaceEvent::Created` audit payload, replace `"project_id": …` with nothing (drop the field); keep `workspace_id`, `name`, `slug`.

- [ ] **Step 5: Update the in-file `#[cfg(test)]` tests**

The module's tests seed a project (`seed_manager` creates `projects` rows and passes `project_id`). Rewrite `seed_manager` to insert only `repositories` (no project) and call the new `create_workspace(name, &specs, None, None, None)`. Update `create_workspace_accepts_multi_repo_in_declaration_order`, `create_workspace_rejects_empty_dup_and_foreign`, etc., to the new signature, and add:

```rust
#[tokio::test]
async fn create_workspace_seeds_repo_cone_defaults_as_snapshot() {
    // repo with cone_defaults ["api/"] ; workspace attaches with empty spec
    // → workspace_repos.sparse_cones_json == ["api/"]; later editing the
    // repo default does NOT change the workspace snapshot (D4).
}
```

- [ ] **Step 6:** Defer crate compile to Task 3.7; run the module unit tests after 3.7.

### Task 3.3: Settings resolver → workspace-keyed

**Files:**
- Rename: `crates/core/src/settings/project_file.rs` → `crates/core/src/settings/workspace_file.rs`
- Modify: `crates/core/src/settings/resolver.rs`, `mod.rs`, `boot.rs`

- [ ] **Step 1:** `git mv crates/core/src/settings/project_file.rs crates/core/src/settings/workspace_file.rs`. Rename `ProjectSettingsSource` → `WorkspaceSettingsSource`, `CheckedInProjectSettings` → `CheckedInWorkspaceSettings`, and the checked-in filename constant `".concerto/project_settings.json"` → `".concerto/workspace_settings.json"` (D7). Keep the per-repo `.concerto/action_prefs.toml` filename unchanged.
- [ ] **Step 2:** In `resolver.rs` rename `ProjectSettingsResolver` → `WorkspaceSettingsResolver`, field `project_id` → `workspace_id`, the `LocalDbProjectSettings` → `LocalDbWorkspaceSettings` reading `workspaces.settings_json` (via `workspaces::get_settings_json`), and the `SettingsSource::LocalDbProject` enum variant doc → "`workspaces.settings_json`". Update `Resolved`/`SettingsSource` `as_str`/`audit_name` strings (`project` → `workspace`).
- [ ] **Step 3:** In `mod.rs`/`boot.rs` update `mod project_file;` → `mod workspace_file;`, the resolver construction (keyed by `workspace_id`, reading the reference repo via `workspaces::list_repos(ws)[0]` for the checked-in layer root), and the `notify`-rs watch path to `workspace_settings.json`.
- [ ] **Step 4:** Update the existing settings tests (`resolver.rs` `#[cfg(test)]` + any `crates/core/tests/*settings*`) to the workspace-keyed API. Defer compile to 3.7.

### Task 3.4: Permission inheritance — drop the project layer

**Files:**
- Modify: `crates/core/src/security/permission.rs`
- Modify: `crates/core/tests/permission_inheritance.rs`, `crates/core/tests/permission_runtime.rs`

- [ ] **Step 1: Update the test first (TDD)**

In `permission_inheritance.rs`, remove the case that sets the default via `projects.settings_json` and add the equivalent via `workspaces.settings_json`. Add an assertion that `ModeSource` has no `Project` variant and that a workspace-level `settings_json.default_permission_mode` is the terminal fallback.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p concerto-core --test permission_inheritance`
Expected: FAIL to compile (`ModeSource::Project` still referenced) / assertion fail.

- [ ] **Step 3: Edit `permission.rs`**

Remove `ModeSource::Project`. Change the join to stop at workspaces and read the workspace settings for the default:

```rust
// SELECT … FROM sessions s
//   JOIN workareas wa  ON wa.id = s.workarea_id
//   JOIN workspaces ws ON ws.id = wa.workspace_id
//   WHERE s.id = ?
// columns: session_mode, workarea_mode, workspace_mode, ws.settings_json AS workspace_settings_json
```

Drop the `JOIN projects p` line and `p.settings_json`. Rename `project_settings_json` → `workspace_settings_json`, `project_default_from_settings` → `workspace_default_from_settings`. The walk becomes:

```rust
let (mut mode, mut source) = if let Some(m) = session_mode.as_deref() {
    (parse_permission_mode(m)?, ModeSource::Session)
} else if let Some(m) = workarea_mode.as_deref() {
    (parse_permission_mode(m)?, ModeSource::Workarea)
} else if let Some(m) = workspace_mode.as_deref() {
    (parse_permission_mode(m)?, ModeSource::Workspace)
} else if let Some(m) = workspace_default_from_settings(&workspace_settings_json)? {
    (m, ModeSource::Workspace)
} else {
    (PermissionMode::Normal, ModeSource::Workspace) // global default
};
```

Update the module doc comment chain (`sessions → workareas → workspaces → default`).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p concerto-core --test permission_inheritance --test permission_runtime`
Expected: PASS (after 3.7 makes the crate compile; if it can't compile yet due to other modules, run after 3.7).

### Task 3.5: Repo manager — registry add + local adopt

**Files:**
- Modify: `crates/core/src/repo_manager/actor.rs`, `mod.rs`
- Modify: `crates/core/src/handlers/repositories.rs`
- Test: `crates/core/tests/repository_clone.rs` (extend) + new `crates/core/tests/repo_local_import.rs`

- [ ] **Step 1: Write the failing local-import test**

```rust
// crates/core/tests/repo_local_import.rs
#[tokio::test]
async fn import_local_adopts_existing_repo_without_cloning() {
    // 1. git init a temp dir with one commit.
    // 2. repo_manager.import_local("name", &path).await -> Repository row
    // 3. assert row.local_path == path (adopted in place, NOT under ~/concerto/repos/<id>)
    // 4. assert it is registered (repositories::get_by_url or list_all finds it)
    // 5. assert the repo dir still has its original .git (non-destructive)
}
```

- [ ] **Step 2: Run to verify it fails** — Run: `cargo test -p concerto-core --test repo_local_import` → FAIL (`import_local` not defined).

- [ ] **Step 3: `add_repository` drops project_id; add `import_local`**

In `repo_manager/actor.rs`, change `add_repository(&self, name, url, default_branch, strategy, with_sparse)` (no `project_id`); on URL already in the registry, return the existing row (`repositories::get_by_url`). Add:

```rust
/// Adopt an existing on-disk git repo into the registry in place (D9).
/// Non-destructive: validates `.git` exists, never re-inits, applies only
/// additive performance config (core.fsmonitor, core.untrackedCache),
/// records the path as-is. Returns the (possibly pre-existing) row.
pub async fn import_local(&self, name: &str, local_path: &Path) -> Result<Repository> {
    // validate it's a git repo (git rev-parse --git-dir)
    // derive default_branch from HEAD
    // insert NewRepository { id, name, url: <origin remote or local_path>, local_path, clone_strategy: "full", default_branch }
    // apply locked git config additively; start fsmonitor
}
```

- [ ] **Step 4: `repositories.rs` handler**

`add_repository`: drop the `project_id` required-arg check; branch on `req.local_path` non-empty → `import_local`, else `url` → `add_repository`. Add the `set_repo_cone_defaults` handler calling `repositories::update_cone_defaults_json`. Update `repository_to_proto` to drop `project_id`.

- [ ] **Step 5: Run** — Run: `cargo test -p concerto-core --test repo_local_import --test repository_clone` → PASS (after 3.7).

### Task 3.6: `workarea.rs` + `files_to_copy.rs` — read cones/settings from the new sources

**Files:**
- Modify: `crates/core/src/workspace_manager/workarea.rs`
- Modify: `crates/core/src/workspace_manager/files_to_copy.rs`

- [ ] **Step 1:** In `workarea.rs`, change the per-`(workarea, repo)` cone seed source: the workarea inherits from `workspace_repos.sparse_cones_json` (via `workspaces::get_repo_cones`) instead of `workspaces.settings_json["cone_defaults"]`. Update the cone-resolution helper accordingly. Remove any `project_id` reads.
- [ ] **Step 2:** In `files_to_copy.rs`, the reference repo + checked-in rules are resolved from the workspace's reference repo (first by `list_repos`) and `workspaces.settings_json` / `.concerto/workspace_settings.json` — drop project lookups.
- [ ] **Step 3:** Update `crates/core/tests/files_to_copy.rs` fixtures (no project; workspace settings carry `files_to_copy_rules`). Defer run to 3.7.

### Task 3.7: Handler + boot + call-site sweep → restore green

**Files (sweep — compiler is the worklist):**
- `crates/core/src/handlers/workspaces.rs`, `workareas.rs`, `sessions.rs`
- `crates/core/src/boot.rs`, `api_server.rs`, `runtime.rs`, `connect_bridge.rs`
- `crates/core/src/audit/event.rs`
- `crates/cli/src/commands/workspace.rs`, `session.rs`
- `crates/test-harness/src/lib.rs`, `clients.rs`
- every file in the Phase-3 grep list under `crates/core/tests/*`

- [ ] **Step 1: `handlers/workspaces.rs`**

`create_workspace`: read `req.repos` (the new `WorkspaceRepoSpec` repeated), map to manager `WorkspaceRepoSpec`, drop `project_id`. `list_workspaces`: drop the `project_id` required check, call `list_all()`. `workspace_to_proto`: drop `project_id`, add `icon`.

- [ ] **Step 2: Sweep the rest**

Run `cargo check --workspace` and fix every error mechanically:
- Test fixtures / `test-harness` that build `NewProject`/insert `projects` rows → delete the project seed; insert `repositories` directly; call `create_workspace` with the new signature.
- `audit/event.rs`: `ProjectSettingsResolved` → `WorkspaceSettingsResolved`; any `project_id` audit field → `workspace_id`.
- `cli/commands/workspace.rs`: `workspace ls` no longer takes a project filter; `cli/commands/session.rs` unchanged except any project plumbing.
- `agent_supervisor`, `skills`, `connect_bridge`, `path_policy`, `cold_resume`: replace `project`-scoped reads with workspace/registry equivalents (most are comments or the skills `workspace` scope).

- [ ] **Step 3: Full Core test suite**

Run: `cargo test -p concerto-core -p concerto-cli -p concerto-test-harness`
Expected: PASS. Fix remaining fixtures until green.

- [ ] **Step 4: Gate**

Run: `cargo check --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --all -- --check` · `cargo test --workspace --no-fail-fast` · `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/`.
Expected: all green.

- [ ] **Step 5: Smoke gate**

Run: `scripts/smoke.sh` (if any capability check referenced projects, update the manifest). Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates docs/interfaces
git commit -m "core: collapse Project into Workspace — managers, handlers, settings, permission, registry+local-import"
```

---

# Phase 4 — Desktop (`apps/desktop`)

### Task 4.1: Remove project state + API + hook

**Files:**
- Delete: `apps/desktop/src/components/NewProjectModal.tsx` + `.test.tsx`
- Delete: `apps/desktop/src/api/projects.ts`
- Delete: `apps/desktop/src/hooks/useProjects.ts`
- Modify: `apps/desktop/src/state/useUiStore.ts`

- [ ] **Step 1:** `git rm` the three deleted files (+ NewProjectModal test).
- [ ] **Step 2:** In `useUiStore.ts` remove `selectedProjectId`, `collapsedProjects`, `newProjectModalOpen`, their setters (`setSelectedProject`, `toggleProjectExpanded`, `setNewProjectModalOpen`). Update the store type + initial state + any persisted-state migration.
- [ ] **Step 3:** Defer typecheck to Task 4.4.

### Task 4.2: Flatten the Sidebar

**Files:**
- Modify: `apps/desktop/src/components/Sidebar.tsx`
- Modify: `apps/desktop/src/hooks/useWorkspaces.ts`

- [ ] **Step 1:** `useWorkspaces.ts`: drop the `projectId` arg; call `listWorkspaces({ includeArchived: false })`.
- [ ] **Step 2:** `Sidebar.tsx`: delete `ProjectNode`; the top level maps `workspacesQuery.data.workspaces` → `WorkspaceNode` (no `projectId` prop). The "+ Project" affordance becomes "+ Workspace" opening `NewWorkspaceModal`. Remove project expand/collapse state usage.
- [ ] **Step 3:** Update `Sidebar` tests (and `WorkareaList.tsx` if it took a `projectId`).

### Task 4.3: Rebuild `NewWorkspaceModal` — 3-source repo picker + per-repo checkout

**Files:**
- Modify: `apps/desktop/src/components/NewWorkspaceModal.tsx` + `.test.tsx`
- Modify: `apps/desktop/src/api/workspaces.ts`, `apps/desktop/src/api/repositories.ts`

- [ ] **Step 1: API wrappers**

`api/workspaces.ts`: `listWorkspaces(opts?: { includeArchived?: boolean })` (no project); `createWorkspace({ name, icon?, description?, permissionMode?, repos: { repositoryId, sparseCones }[] })` mapping to the new `CreateWorkspaceRequest`. `Workspace` type drops `project_id`, adds `icon`.
`api/repositories.ts`: `Repository` drops `project_id`; `addRepository` gains a `localPath` source variant; add `listRepositories()` (no project arg) and `setRepoConeDefaults(repositoryId, coneDefaults)`.

- [ ] **Step 2: Write the modal test first**

In `NewWorkspaceModal.test.tsx`, assert: the modal lists existing registry repos; "Add by URL" and "Add local folder" affordances exist; selecting a repo shows a "Full / Sparse" toggle and (when Sparse) the `ConePicker`; submitting calls `createWorkspace` with the assembled `repos[]`. Use the existing test-utils + mocked `callRpc`.

- [ ] **Step 3: Run to verify it fails** — Run: `pnpm -C apps/desktop test NewWorkspaceModal` → FAIL.

- [ ] **Step 4: Implement the modal**

Compose from existing pieces: a repo list (from `listRepositories()`), the `cloneStrategy` hook + size probe for the URL path, a folder-picker (Tauri dialog) for the local path → `addRepository({ localPath })`, and per-selected-repo a Full/Sparse toggle reusing `ConePicker`/`RepoTreeBrowser`/`SparseConeDialog` seeded from `repo.cone_defaults`. Assemble `repos: WorkspaceRepoSpec[]` and call `createWorkspace`.

- [ ] **Step 5: Run to verify it passes** — Run: `pnpm -C apps/desktop test NewWorkspaceModal` → PASS.

### Task 4.4: Repo cone-defaults editor + sweep desktop green

**Files:**
- Modify: `apps/desktop/src/components/SettingsPanel.tsx` (or `WorkspaceDetail.tsx`) — add "edit default sparse directories" for a repo (`setRepoConeDefaults`) and a per-workspace "reset repo to defaults".
- Sweep: every file in the desktop project-ref grep list.

- [ ] **Step 1:** Add the repo cone-defaults edit surface (reuses `SparseConeDialog`, calls `setRepoConeDefaults`).
- [ ] **Step 2: Sweep** — Run `pnpm -C apps/desktop typecheck` and fix every `project_id`/`projectId`/`selectedProject`/`useProjects`/`listProjects` reference across `App.tsx`, `WorkspaceDetail.tsx`, `WorkareaList.tsx`, `CodePrRegion.tsx`, `SkillsTab.tsx`, `useWorkareaRepos.ts`, `useSkills.ts`, the `repositories`/`workspaces` API tests, etc. The skills "project" scope UI → "workspace".
- [ ] **Step 3: Full desktop gate** — Run: `pnpm -C apps/desktop typecheck` · `pnpm -C apps/desktop lint` · `pnpm -C apps/desktop test` · `pnpm -C apps/desktop build`. Expected: all green.
- [ ] **Step 4: Commit**

```bash
git add apps/desktop
git commit -m "desktop: collapse Project into Workspace — flat sidebar, registry repo picker, local-folder import, cone editor"
```

---

# Phase 5 — Documentation

### Task 5.1: Rewrite the canonical design docs

**Files:** `design/00_Architecture_Overview.md`, `design/02_Repository_Manager.md`, `design/03_Workspace_Session_Manager.md`, `design/09_Persistence.md`, `design/10_Local_API_Protocol.md`, `design/13_VCS_Provider_Integration.md`, `design/15_Desktop_Client.md`

- [ ] **Step 1:** `design/03`: retitle hierarchy to `Workspace → Workarea → Session`; §1 tree, §3.2 (workspace declares repos directly), §3.8 permission chain (drop project layer), §3.10/§3.13 settings move to workspace + reference repo, §4 data model. Add the global registry concept.
- [ ] **Step 2:** `design/02`: repository is a global registry entry; `add_project_repository` → `add_repository`; add local-folder adopt + `set_repo_cone_defaults`; cone inheritance now repo-default → `workspace_repos.sparse_cones_json` → `workarea_repos.sparse_cones_json`.
- [ ] **Step 3:** `design/09 §4.1`: schema — drop `projects`; reshape `repositories`/`workspaces`/`workspace_repos` (new `sparse_cones_json`); `skills_index` workspace scope.
- [ ] **Step 4:** `design/00`, `10`, `13`, `15`: data-model/overview, drop `Projects` service, sidebar tree (no project node), creation flow + repo picker.
- [ ] **Step 5: Commit**

```bash
git add design
git commit -m "docs(design): rewrite canonical docs for the Project→Workspace collapse"
```

### Task 5.2: Amendment note in the v1 task plan

**Files:** `tasks/v1.0/README.md`

- [ ] **Step 1:** Add a short dated amendment note near §4 (decisions) recording the collapse, pointing to `docs/superpowers/specs/2026-06-08-collapse-project-into-workspace-design.md`, and stating the `tasks/v1.0/*` task files remain frozen history (not rewritten) per D8.
- [ ] **Step 2: Commit**

```bash
git add tasks/v1.0/README.md
git commit -m "docs(tasks): amendment note — Project→Workspace collapse (frozen task files unchanged)"
```

---

# Phase 6 — Final verification

### Task 6.1: Whole-repo green + interface lock

- [ ] **Step 1:** `cargo check --workspace` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --all -- --check` · `cargo deny check` · `cargo test --workspace --no-fail-fast`.
- [ ] **Step 2:** `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/`.
- [ ] **Step 3:** `pnpm -C apps/desktop typecheck && pnpm -C apps/desktop lint && pnpm -C apps/desktop test && pnpm -C apps/desktop build`.
- [ ] **Step 4:** `scripts/smoke.sh`.
- [ ] **Step 5:** Grep sweep for stragglers — `rg -n "project_id|ProjectId|projects\b|selectedProject|NewProjectModal|useProjects" crates apps/desktop` should return only intentional matches (e.g. the MCP `Project` scope that the spec deliberately leaves repo-scoped, §12). Confirm each remaining hit is intentional.
- [ ] **Step 6:** Final review: the design spec's §4–§10 each map to a completed task (see Self-Review below).

---

## Self-Review (plan vs. spec)

**Spec coverage:**
- §2 model / §3 D1 global registry → Task 1.4, 3.5 (registry, get_by_url, list_all). ✔
- §3 D2 clone strategy repo-global / sparse per-workspace → proto `WorkspaceRepoSpec.sparse_cones` (2.2), seeding (3.2). ✔
- §3 D3/D4 repo default sparse dirs + snapshot → 1.2 (`set_repo_cones`/seed), 3.2 (seed + reset), 3.5 (`SetRepoConeDefaults`). ✔
- §3 D5 rewrite 0001 in place → 0.1, 0.2. ✔
- §3 D6 `workspace_repos.sparse_cones_json` column → 0.1, 1.2, 3.6. ✔
- §3 D7 rename checked-in file → 3.3. ✔
- §3 D8 docs scope → 5.1, 5.2. ✔
- §3 D9 three add-repo sources incl. local adopt → 2.2, 3.5, 4.3. ✔
- §4 schema → Phase 0. §5 proto → Phase 2. §6 Rust removals/renames → Phase 1+3. §7 desktop → Phase 4. §8 disk layout unchanged (no task needed). §9 testing → embedded per task + Task 6.1. §10 docs → Phase 5. ✔
- §11 risks (permission chain, settings rekey, non-destructive adopt, name collisions) → 3.4, 3.3, 3.5, 1.4 (`UNIQUE(name)` + `get_by_url`). ✔
- §12 out-of-scope (MCP Project scope) → explicitly preserved; verified in 6.1 Step 5. ✔

**Type consistency:** `WorkspaceRepoSpec` (proto message + manager struct) used in 2.2/3.2/4.1–4.3; `list_all` replaces `list_by_project` in persist (1.2/1.4) + manager (3.2) + handler (3.7); `WorkspaceSettingsResolver`/`workspace_file.rs` consistent across 3.3; `ModeSource::Project` removed everywhere it was referenced (3.4) — `permission.rs` + tests. `update_repos` signature change (`&[(RepositoryId, String)]`) propagated to its only caller (`actor.rs`, 3.2) and persist tests (1.5).

**Placeholder scan:** No "TBD"/"handle edge cases"/"similar to". Sweep tasks (3.7, 4.4) are deliberately compiler-gated and name their exact file worklist + the verifying command.
