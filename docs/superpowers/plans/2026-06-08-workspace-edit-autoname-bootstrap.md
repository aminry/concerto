# Workspace edit, auto-name, and auto-bootstrap — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a gear-button "edit workspace" flow with full create-form parity (name/icon/description + repos), auto-generate the workspace name from selected repos, and auto-create the first workarea + a `claude` session when a workspace is created.

**Architecture:** Three layers. (1) Frontend-only auto-name + auto-bootstrap on the existing create modal. (2) New backend RPCs `Workspaces.UpdateWorkspace` and `Workspaces.ListWorkspaceRepos` (proto → persist → actor → handler → Tauri `rpc.rs`), reusing the actor's existing `update_workspace_repos()`. (3) A shared `WorkspaceForm` used by both a create modal and a new edit modal, with a gear button per sidebar workspace.

**Tech Stack:** Rust (tonic/prost gRPC, sqlx/SQLite, tokio), Tauri shell (serde JSON RPC bridge), React + TypeScript + TanStack Query + Zustand, vitest.

**Design doc:** `docs/superpowers/specs/2026-06-08-workspace-edit-autoname-bootstrap-design.md`

**Build/test commands:**
- Rust: `cargo test -p concerto-persist`, `cargo test -p concerto-core` (proto regenerates via `crates/proto/build.rs` on `cargo build`).
- Frontend: from `apps/desktop/`: `pnpm vitest run <path>` (single file), `pnpm test` (all), `pnpm typecheck`.

---

## File Structure

**Backend (Part 3):**
- Modify `crates/proto/proto/concerto/v1/workspaces.proto` — new messages + 2 RPCs.
- Modify `crates/persist/src/workspaces.rs` — `set_metadata`, `list_repo_cones`.
- Modify `crates/core/src/workspace_manager/actor.rs` — `update_workspace`, `list_workspace_repos`, `WorkspaceEvent::Updated`.
- Modify `crates/core/src/handlers/workspaces.rs` — two handler methods.
- Modify `crates/core/tests/workspace_lifecycle.rs` — integration coverage.
- Modify `apps/desktop/src-tauri/src/rpc.rs` — payload structs + dispatch cases.

**Frontend:**
- Modify `apps/desktop/src/api/workspaces.ts` — `updateWorkspace`, `listWorkspaceRepos`.
- Create `apps/desktop/src/components/workspaceName.ts` — `deriveWorkspaceName` helper.
- Create `apps/desktop/src/components/workspaceName.test.ts`.
- Create `apps/desktop/src/components/WorkspaceForm.tsx` — shared form body (extracted from `NewWorkspaceModal`).
- Modify `apps/desktop/src/components/NewWorkspaceModal.tsx` — thin create wrapper around `WorkspaceForm` + bootstrap.
- Create `apps/desktop/src/components/EditWorkspaceModal.tsx` — edit wrapper.
- Create `apps/desktop/src/components/bootstrapWorkspace.ts` — first workarea + session helper.
- Modify `apps/desktop/src/state/useUiStore.ts` — `editWorkspaceId`.
- Modify `apps/desktop/src/components/Sidebar.tsx` — gear button.
- Modify `apps/desktop/src/components/AppLayout.tsx` — mount `EditWorkspaceModal`.
- Tests: `NewWorkspaceModal.test.tsx` (existing, extend), `EditWorkspaceModal.test.tsx` (new), `bootstrapWorkspace.test.ts` (new).

Build order: backend first (Tasks 1–6) so the frontend RPCs exist, then frontend (Tasks 7–13). Parts 1 and 2 (frontend-only) could ship independently but are sequenced after the API task for a single coherent branch.

---

## Task 1: Proto — UpdateWorkspace + ListWorkspaceRepos

**Files:**
- Modify: `crates/proto/proto/concerto/v1/workspaces.proto`

- [ ] **Step 1: Add the new messages and RPCs**

In `crates/proto/proto/concerto/v1/workspaces.proto`, after the `UpdateWorkspaceSettingsRequest` message (around line 84) and before `service Workspaces`, add:

```proto
// Caller-supplied workspace edit (full-form parity beyond
// UpdateWorkspaceSettings). Absent `optional` fields = leave that column
// unchanged; present (incl. empty string) = set to that value. `repos`
// empty = leave the repo set unchanged; non-empty = replace the whole set
// (a workspace can never have zero repos, so empty is an unambiguous
// "no change" sentinel). Slug is NOT editable — it is the stable handle
// minted at creation.
message UpdateWorkspaceRequest {
  string workspace_id = 1;
  optional string name = 2;
  optional string icon = 3;
  optional string description = 4;
  repeated WorkspaceRepoSpec repos = 5;
}

// One repo's attachment as read back for the edit form: its id + the
// per-(workspace, repo) sparse-cone snapshot. Empty `sparse_cones` = full
// working tree.
message WorkspaceRepoEntry {
  string repository_id = 1;
  repeated string sparse_cones = 2;
}

message ListWorkspaceReposResponse {
  // Position-ordered (declaration order).
  repeated WorkspaceRepoEntry repos = 1;
}
```

Then inside `service Workspaces { ... }`, after the `UpdateWorkspaceSettings` line, add:

```proto
  // Edit name/icon/description and/or replace the repo set. Returns the
  // updated Workspace row.
  rpc UpdateWorkspace(UpdateWorkspaceRequest) returns (Workspace);
  // Read a workspace's declared repos + per-repo cones (to pre-fill the
  // edit form).
  rpc ListWorkspaceRepos(WorkspaceId) returns (ListWorkspaceReposResponse);
```

- [ ] **Step 2: Regenerate + verify the proto compiles**

Run: `cargo build -p concerto-proto`
Expected: builds clean (tonic_build regenerates the Rust types from the `.proto`).

- [ ] **Step 3: Commit**

```bash
git add crates/proto/proto/concerto/v1/workspaces.proto
git commit -m "proto: add Workspaces.UpdateWorkspace + ListWorkspaceRepos"
```

---

## Task 2: Persist — set_metadata + list_repo_cones

**Files:**
- Modify: `crates/persist/src/workspaces.rs`

- [ ] **Step 1: Write the failing tests**

Add a `#[cfg(test)]` module at the end of `crates/persist/src/workspaces.rs` (or extend an existing one if present). These tests use an in-memory pool. Match the crate's existing test helper style — if `crates/persist/src/workspaces.rs` has no test module, mirror the setup used in `crates/persist/tests/workspace_repos_position.rs` (open a pool, run migrations, insert a workspace). Add:

```rust
#[cfg(test)]
mod metadata_tests {
    use super::*;
    use crate::api::{NewWorkspace, RepositoryId, WorkspaceId, WorkspaceRepoCones};

    // Helper: open an in-memory DB with migrations applied. Mirror the
    // existing pattern in tests/workspace_repos_position.rs (sqlx::migrate!).
    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    async fn seed_ws(pool: &SqlitePool, id: &str, name: &str, slug: &str) {
        let mut conn = pool.acquire().await.unwrap();
        insert(
            &mut conn,
            NewWorkspace {
                id: WorkspaceId(id.into()),
                name: name.into(),
                slug: slug.into(),
                icon: None,
                description: None,
                permission_mode: None,
                created_at: 0,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn set_metadata_updates_only_patched_columns_and_keeps_slug() {
        let pool = pool().await;
        seed_ws(&pool, "ws1", "Old Name", "old-slug").await;
        let mut conn = pool.acquire().await.unwrap();
        set_metadata(
            &mut conn,
            &WorkspaceId("ws1".into()),
            Some("New Name"),
            Some(Some("🚀")),
            None, // description untouched
        )
        .await
        .unwrap();
        drop(conn);
        let ws = get(&pool, &WorkspaceId("ws1".into())).await.unwrap().unwrap();
        assert_eq!(ws.name, "New Name");
        assert_eq!(ws.icon.as_deref(), Some("🚀"));
        assert_eq!(ws.slug, "old-slug"); // slug immutable
    }

    #[tokio::test]
    async fn set_metadata_can_clear_description_to_null() {
        let pool = pool().await;
        seed_ws(&pool, "ws2", "N", "s").await;
        let mut conn = pool.acquire().await.unwrap();
        set_metadata(&mut conn, &WorkspaceId("ws2".into()), None, None, Some(Some("hi"))).await.unwrap();
        set_metadata(&mut conn, &WorkspaceId("ws2".into()), None, None, Some(None)).await.unwrap();
        drop(conn);
        let ws = get(&pool, &WorkspaceId("ws2".into())).await.unwrap().unwrap();
        assert_eq!(ws.description, None);
    }

    #[tokio::test]
    async fn list_repo_cones_returns_position_ordered_pairs() {
        let pool = pool().await;
        seed_ws(&pool, "ws3", "N", "s3").await;
        // Two repos must exist in the registry (FK). Insert minimal rows.
        for r in ["repoA", "repoB"] {
            sqlx::query(
                "INSERT INTO repositories (id, name, url, default_branch, clone_strategy, cone_defaults_json, created_at) \
                 VALUES (?, ?, '', 'main', 'full', '[]', 0)",
            )
            .bind(r).bind(r).execute(&pool).await.unwrap();
        }
        let mut conn = pool.acquire().await.unwrap();
        update_repos(
            &mut conn,
            &WorkspaceId("ws3".into()),
            &[
                WorkspaceRepoCones { repository_id: RepositoryId("repoA".into()), sparse_cones_json: "[\"src\"]".into() },
                WorkspaceRepoCones::empty_cones(RepositoryId("repoB".into())),
            ],
        )
        .await
        .unwrap();
        drop(conn);
        let got = list_repo_cones(&pool, &WorkspaceId("ws3".into())).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0 .0, "repoA");
        assert_eq!(got[0].1, "[\"src\"]");
        assert_eq!(got[1].0 .0, "repoB");
        assert_eq!(got[1].1, "[]");
    }
}
```

Note: confirm the `repositories` INSERT column list matches migration 0001 (`crates/persist/migrations/0001_initial_schema.sql`); adjust column names if the schema differs. Run `sqlx::migrate!` path is `./migrations` relative to the crate root.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p concerto-persist metadata_tests`
Expected: FAIL — `set_metadata` and `list_repo_cones` are not defined.

- [ ] **Step 3: Implement `set_metadata` and `list_repo_cones`**

In `crates/persist/src/workspaces.rs`, after `set_permission_mode` (around line 293), add:

```rust
/// Patch the editable `workspaces.*` metadata columns. Only the columns
/// whose patch is `Some` are written; `slug` is never touched (it is the
/// stable handle minted at creation). `icon`/`description` use a nested
/// `Option`: `Some(Some(v))` sets the value, `Some(None)` clears it to
/// NULL, `None` leaves it unchanged. `name` has no NULL state, so a plain
/// `Option<&str>` suffices.
pub async fn set_metadata(
    conn: &mut SqliteConnection,
    id: &WorkspaceId,
    name: Option<&str>,
    icon: Option<Option<&str>>,
    description: Option<Option<&str>>,
) -> Result<()> {
    if let Some(name) = name {
        sqlx::query("UPDATE workspaces SET name = ? WHERE id = ?")
            .bind(name)
            .bind(&id.0)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
    }
    if let Some(icon) = icon {
        sqlx::query("UPDATE workspaces SET icon = ? WHERE id = ?")
            .bind(icon)
            .bind(&id.0)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
    }
    if let Some(description) = description {
        sqlx::query("UPDATE workspaces SET description = ? WHERE id = ?")
            .bind(description)
            .bind(&id.0)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
    }
    Ok(())
}

/// List a workspace's declared repos as `(repository_id, sparse_cones_json)`
/// pairs, ordered by `(position, repository_id)` — the same canonical order
/// as [`list_repos`]. Used to pre-fill the edit form.
pub async fn list_repo_cones(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
) -> Result<Vec<(RepositoryId, String)>> {
    let rows = sqlx::query(
        "SELECT repository_id, sparse_cones_json FROM workspace_repos \
         WHERE workspace_id = ? ORDER BY position, repository_id",
    )
    .bind(&workspace_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                RepositoryId(r.get::<String, _>("repository_id")),
                r.get::<String, _>("sparse_cones_json"),
            )
        })
        .collect())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p concerto-persist metadata_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/persist/src/workspaces.rs
git commit -m "persist: add workspace set_metadata + list_repo_cones"
```

---

## Task 3: Actor — update_workspace + list_workspace_repos

**Files:**
- Modify: `crates/core/src/workspace_manager/actor.rs`

- [ ] **Step 1: Add the `Updated` event variant**

In `crates/core/src/workspace_manager/actor.rs`, in `enum WorkspaceEvent` (around line 89), add a variant after `ReposUpdated`:

```rust
    /// A workspace's editable metadata (name/icon/description) changed.
    /// Payload is the post-update row. Repo-set edits use `ReposUpdated`.
    Updated(Workspace),
```

- [ ] **Step 2: Write the failing unit test**

In the same file's `#[cfg(test)]` module (find it via `grep -n "mod tests" crates/core/src/workspace_manager/actor.rs`; mirror how existing tests build a `WorkspaceManager` and register repos), add:

```rust
    #[tokio::test]
    async fn update_workspace_changes_metadata_and_repos() {
        let (mgr, _persist) = test_manager().await; // mirror existing helper
        let repo_a = seed_repo(&mgr, "repoA").await; // mirror existing helper
        let repo_b = seed_repo(&mgr, "repoB").await;
        let ws = mgr
            .create_workspace("Orig", &[spec(&repo_a)], None, None, None)
            .await
            .unwrap();

        // Metadata + add repoB.
        let updated = mgr
            .update_workspace(
                &ws.id,
                Some("Renamed".to_string()),
                Some(Some("🚀".to_string())),
                None,
                &[spec(&repo_a), spec(&repo_b)],
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.icon.as_deref(), Some("🚀"));
        assert_eq!(updated.slug, ws.slug); // slug immutable

        let repos = mgr.list_workspace_repos(&ws.id).await.unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].repository_id.0, repo_a);
        assert_eq!(repos[1].repository_id.0, repo_b);
    }
```

Where `test_manager`, `seed_repo`, and `spec` mirror whatever helpers the existing actor test module uses. If the module lacks a `spec` helper, define one locally:

```rust
    fn spec(repo_id: &str) -> WorkspaceRepoSpec {
        WorkspaceRepoSpec { repository_id: RepositoryId(repo_id.to_string()), sparse_cones: vec![] }
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p concerto-core update_workspace_changes_metadata_and_repos`
Expected: FAIL — `update_workspace` / `list_workspace_repos` not defined.

- [ ] **Step 4: Implement `update_workspace` and `list_workspace_repos`**

In `crates/core/src/workspace_manager/actor.rs`, after `update_workspace_repos` (ends ~line 427), add:

```rust
    /// Edit a workspace's metadata (name/icon/description) and/or replace
    /// its repo set. `name`/`icon`/`description` use `Option`: `None` =
    /// leave unchanged. For icon/description the inner `Option` selects
    /// set-vs-clear (`Some(Some(v))` set, `Some(None)` clear). `repos`
    /// empty = leave the repo set unchanged; non-empty = replace via
    /// [`update_workspace_repos`].
    ///
    /// Slug is never re-derived (it is the stable handle from creation).
    /// Repo-set edits affect FUTURE workareas only — existing workareas keep
    /// their materialized worktrees (see [`update_workspace_repos`]).
    pub async fn update_workspace(
        &self,
        id: &WorkspaceId,
        name: Option<String>,
        icon: Option<Option<String>>,
        description: Option<Option<String>>,
        repos: &[WorkspaceRepoSpec],
    ) -> Result<Workspace> {
        // Existence check (also the NotFound path).
        if self.get(id).await?.is_none() {
            return Err(Error::NotFound(format!("workspace {id} not found")));
        }
        if let Some(n) = name.as_deref() {
            if n.is_empty() {
                return Err(Error::Validation("name must not be empty".into()));
            }
        }

        // Metadata patch (only when at least one field is present).
        let has_metadata = name.is_some() || icon.is_some() || description.is_some();
        if has_metadata {
            let mut writer = self.persistence.writer().await;
            let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            concerto_persist::workspaces::set_metadata(
                &mut tx,
                id,
                name.as_deref(),
                icon.as_ref().map(|o| o.as_deref()),
                description.as_ref().map(|o| o.as_deref()),
            )
            .await?;
            tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            drop(writer);
        }

        // Repo-set replacement (only when a non-empty set is supplied).
        // `update_workspace_repos` validates + emits `ReposUpdated`.
        let repos_changed = !repos.is_empty();
        if repos_changed {
            self.update_workspace_repos(id, repos).await?;
        }

        let updated = self
            .get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("workspace {id} vanished mid-update")))?;

        // Emit `Updated` only for a metadata-only change; a repo change
        // already emitted `ReposUpdated` (whose payload carries the full row).
        if has_metadata && !repos_changed {
            let _ = self.events.send(WorkspaceEvent::Updated(updated.clone()));
        }
        Ok(updated)
    }

    /// List a workspace's declared repos with their per-(workspace, repo)
    /// cone snapshots, position-ordered (for the edit form pre-fill).
    pub async fn list_workspace_repos(
        &self,
        id: &WorkspaceId,
    ) -> Result<Vec<WorkspaceRepoSpec>> {
        let pairs =
            concerto_persist::workspaces::list_repo_cones(self.persistence.readers(), id).await?;
        let mut out = Vec::with_capacity(pairs.len());
        for (repo_id, cones_json) in pairs {
            let sparse_cones: Vec<String> = serde_json::from_str(&cones_json).map_err(|e| {
                Error::Internal(format!("parse sparse_cones for {}: {e}", repo_id.0))
            })?;
            out.push(WorkspaceRepoSpec { repository_id: repo_id, sparse_cones });
        }
        Ok(out)
    }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p concerto-core update_workspace_changes_metadata_and_repos`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/workspace_manager/actor.rs
git commit -m "core: WorkspaceManager.update_workspace + list_workspace_repos"
```

---

## Task 4: Handler — wire the two RPCs

**Files:**
- Modify: `crates/core/src/handlers/workspaces.rs`

- [ ] **Step 1: Extend the proto import**

In `crates/core/src/handlers/workspaces.rs`, add the new types to the `use concerto_proto::v1::{...}` block (around lines 18–21):

```rust
use concerto_proto::v1::{
    CreateWorkspaceRequest, ListWorkspaceReposResponse, ListWorkspacesRequest,
    ListWorkspacesResponse, PermissionMode, UpdateWorkspaceRequest,
    UpdateWorkspaceSettingsRequest, Workspace as ProtoWorkspace, WorkspaceId as ProtoWorkspaceId,
    WorkspaceRepoEntry,
};
```

- [ ] **Step 2: Implement the two handler methods**

In the `impl WorkspacesService for WorkspacesHandler` block, after `update_workspace_settings` (ends ~line 174), add:

```rust
    #[tracing::instrument(skip_all, name = "Workspaces::UpdateWorkspace")]
    async fn update_workspace(
        &self,
        request: Request<UpdateWorkspaceRequest>,
    ) -> Result<Response<ProtoWorkspace>, Status> {
        let req = request.into_inner();
        if req.workspace_id.is_empty() {
            return Err(Status::invalid_argument("workspace_id is required"));
        }
        // `optional string` → Option<String>. icon/description map to the
        // actor's nested Option (present = set/clear, absent = no change).
        let name = req.name;
        let icon = req.icon.map(Some);
        let description = req.description.map(Some);
        let repos: Vec<WorkspaceRepoSpec> = req
            .repos
            .into_iter()
            .map(|r| WorkspaceRepoSpec {
                repository_id: concerto_persist::RepositoryId(r.repository_id),
                sparse_cones: r.sparse_cones,
            })
            .collect();
        let id = PersistWorkspaceId(req.workspace_id);
        let row = self
            .workspace_manager
            .update_workspace(&id, name, icon, description, &repos)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workspace_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workspaces::ListWorkspaceRepos")]
    async fn list_workspace_repos(
        &self,
        request: Request<ProtoWorkspaceId>,
    ) -> Result<Response<ListWorkspaceReposResponse>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workspace id is required"));
        }
        let id = PersistWorkspaceId(req.value);
        let repos = self
            .workspace_manager
            .list_workspace_repos(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListWorkspaceReposResponse {
            repos: repos
                .into_iter()
                .map(|r| WorkspaceRepoEntry {
                    repository_id: r.repository_id.0,
                    sparse_cones: r.sparse_cones,
                })
                .collect(),
        }))
    }
```

Note: `optional string` proto fields generate `Option<String>` in prost, where an absent field is `None`. An empty `name` that is `Some("")` is rejected by the actor's validation (Task 3); icon/description `Some("")` is a deliberate "set to empty string" — acceptable.

- [ ] **Step 3: Build to verify the trait is satisfied**

Run: `cargo build -p concerto-core`
Expected: builds clean (the generated `WorkspacesService` trait now requires both methods; they're implemented).

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/handlers/workspaces.rs
git commit -m "core: handler wiring for UpdateWorkspace + ListWorkspaceRepos"
```

---

## Task 5: Core integration test (gRPC end-to-end)

**Files:**
- Modify: `crates/core/tests/workspace_lifecycle.rs`

- [ ] **Step 1: Write the failing integration test**

In `crates/core/tests/workspace_lifecycle.rs`, extend the proto import to include the new types and add a test. Append:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn update_workspace_edits_metadata_and_repos() {
    use concerto_proto::v1::{
        ListWorkspaceReposResponse, UpdateWorkspaceRequest,
    };
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let repo_a = register_repo(&core, "edit-a").await;
    let repo_b = register_repo(&core, "edit-b").await;
    let mut wsc = core.workspaces_client().await.expect("workspaces client");

    let ws = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "Before".to_string(),
            repos: vec![spec(&repo_a)],
            permission_mode: None,
            description: None,
            icon: None,
        })
        .await
        .expect("create")
        .into_inner();
    let original_slug = ws.slug.clone();

    // Edit name + add repo_b.
    let updated = wsc
        .update_workspace(UpdateWorkspaceRequest {
            workspace_id: ws.id.clone(),
            name: Some("After".to_string()),
            icon: Some("🚀".to_string()),
            description: None,
            repos: vec![spec(&repo_a), spec(&repo_b)],
        })
        .await
        .expect("update")
        .into_inner();
    assert_eq!(updated.name, "After");
    assert_eq!(updated.icon.as_deref(), Some("🚀"));
    assert_eq!(updated.slug, original_slug, "slug stays fixed on rename");

    // ListWorkspaceRepos returns both, in declaration order.
    let listed: ListWorkspaceReposResponse = wsc
        .list_workspace_repos(WorkspaceId { value: ws.id.clone() })
        .await
        .expect("list repos")
        .into_inner();
    assert_eq!(listed.repos.len(), 2);
    assert_eq!(listed.repos[0].repository_id, repo_a);
    assert_eq!(listed.repos[1].repository_id, repo_b);

    // Metadata-only edit (empty repos = leave unchanged).
    let only_desc = wsc
        .update_workspace(UpdateWorkspaceRequest {
            workspace_id: ws.id.clone(),
            name: None,
            icon: None,
            description: Some("now described".to_string()),
            repos: vec![],
        })
        .await
        .expect("update desc")
        .into_inner();
    assert_eq!(only_desc.description.as_deref(), Some("now described"));
    assert_eq!(only_desc.name, "After"); // name untouched

    let still_two = wsc
        .list_workspace_repos(WorkspaceId { value: ws.id })
        .await
        .expect("list repos again")
        .into_inner();
    assert_eq!(still_two.repos.len(), 2, "empty repos = no change");
}
```

- [ ] **Step 2: Run the test to verify it fails (then passes)**

Run: `cargo test -p concerto-core --test workspace_lifecycle update_workspace_edits_metadata_and_repos`
Expected: with Tasks 1–4 already implemented, this should PASS. If it FAILs on a missing RPC, confirm `cargo build` regenerated the proto. (TDD note: if you reached this task before 4 was complete it would fail to compile — that's the red state.)

- [ ] **Step 3: Run the whole workspace_lifecycle suite**

Run: `cargo test -p concerto-core --test workspace_lifecycle`
Expected: PASS (existing tests + the new one).

- [ ] **Step 4: Commit**

```bash
git add crates/core/tests/workspace_lifecycle.rs
git commit -m "core: integration test for UpdateWorkspace + ListWorkspaceRepos"
```

---

## Task 6: Tauri rpc.rs — payload structs + dispatch cases

**Files:**
- Modify: `apps/desktop/src-tauri/src/rpc.rs`

- [ ] **Step 1: Extend the proto import + add payload structs**

In `apps/desktop/src-tauri/src/rpc.rs`, add `UpdateWorkspaceRequest` to the `use concerto_proto::v1::{...}` block (around lines 23–31):

```rust
    WorkareaId as ProtoWorkareaId, WorkspaceId as ProtoWorkspaceId, WorkspaceRepoSpec,
    UpdateWorkspaceRequest,
```

Then, after `CreateWorkspacePayload` (ends ~line 120), add:

```rust
#[derive(Debug, Deserialize)]
struct UpdateWorkspacePayload {
    workspace_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    repos: Vec<WorkspaceRepoSpecPayload>,
}
```

- [ ] **Step 2: Add the two dispatch cases**

In the big `match method { ... }` (around line 240), after the `"Workspaces.CreateWorkspace" => { ... }` arm (ends ~line 353), add:

```rust
        "Workspaces.UpdateWorkspace" => {
            let req: UpdateWorkspacePayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for UpdateWorkspace: {e}"))
            })?;
            let mut client = WorkspacesClient::new(channel);
            client
                .update_workspace(UpdateWorkspaceRequest {
                    workspace_id: req.workspace_id,
                    name: req.name,
                    icon: req.icon,
                    description: req.description,
                    repos: req
                        .repos
                        .into_iter()
                        .map(|r| WorkspaceRepoSpec {
                            repository_id: r.repository_id,
                            sparse_cones: r.sparse_cones,
                        })
                        .collect(),
                })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
        "Workspaces.ListWorkspaceRepos" => {
            let req: IdPayload = serde_json::from_value(payload).map_err(|e| {
                CoreClientError::Rpc(format!("invalid payload for ListWorkspaceRepos: {e}"))
            })?;
            let mut client = WorkspacesClient::new(channel);
            client
                .list_workspace_repos(ProtoWorkspaceId { value: req.id })
                .await
                .map(|r| serde_json::to_value(r.into_inner()).unwrap_or(Value::Null))
        }
```

- [ ] **Step 3: Build the Tauri shell**

Run: `cargo build -p concerto-desktop` (or the shell crate name — check `apps/desktop/src-tauri/Cargo.toml` `[package] name`; use `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml` if unsure).
Expected: builds clean.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src-tauri/src/rpc.rs
git commit -m "desktop(shell): route Workspaces.UpdateWorkspace + ListWorkspaceRepos"
```

---

## Task 7: Frontend API — updateWorkspace + listWorkspaceRepos

**Files:**
- Modify: `apps/desktop/src/api/workspaces.ts`

- [ ] **Step 1: Add the typed wrappers**

In `apps/desktop/src/api/workspaces.ts`, after `createWorkspace` (end of file), add:

```typescript
/// One repo's attachment as read back for the edit form (mirrors
/// `concerto.v1.WorkspaceRepoEntry`).
export type WorkspaceRepoEntry = {
  repository_id: string;
  sparse_cones: string[];
};

export type ListWorkspaceReposResponse = {
  repos: WorkspaceRepoEntry[];
};

/// `Workspaces.ListWorkspaceRepos` — the workspace's declared repos +
/// per-repo cones, position-ordered. Used to pre-fill the edit form.
export async function listWorkspaceRepos(
  id: string,
): Promise<ListWorkspaceReposResponse> {
  return callRpc<{ id: string }, ListWorkspaceReposResponse>(
    "Workspaces.ListWorkspaceRepos",
    { id },
  );
}

/// `Workspaces.UpdateWorkspace` — edit name/icon/description and/or replace
/// the repo set. An omitted field leaves that value unchanged; an omitted
/// (or empty) `repos` leaves the repo set unchanged.
export async function updateWorkspace(input: {
  id: string;
  name?: string;
  icon?: string;
  description?: string;
  repos?: WorkspaceRepoSpec[];
}): Promise<Workspace> {
  return callRpc<
    {
      workspace_id: string;
      name?: string;
      icon?: string;
      description?: string;
      repos: { repository_id: string; sparse_cones: string[] }[];
    },
    Workspace
  >("Workspaces.UpdateWorkspace", {
    workspace_id: input.id,
    name: input.name,
    icon: input.icon,
    description: input.description,
    repos: (input.repos ?? []).map((r) => ({
      repository_id: r.repositoryId,
      sparse_cones: r.sparseCones,
    })),
  });
}
```

- [ ] **Step 2: Typecheck**

Run: from `apps/desktop/`: `pnpm typecheck`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/api/workspaces.ts
git commit -m "desktop(api): updateWorkspace + listWorkspaceRepos wrappers"
```

---

## Task 8: Auto-name helper (Part 1, pure function)

**Files:**
- Create: `apps/desktop/src/components/workspaceName.ts`
- Test: `apps/desktop/src/components/workspaceName.test.ts`

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src/components/workspaceName.test.ts`:

```typescript
import { describe, expect, it } from "vitest";
import { deriveWorkspaceName } from "./workspaceName";

describe("deriveWorkspaceName", () => {
  it("returns empty for no repos", () => {
    expect(deriveWorkspaceName([])).toBe("");
  });
  it("returns the single repo name", () => {
    expect(deriveWorkspaceName(["payments"])).toBe("payments");
  });
  it("joins two repos with a plus", () => {
    expect(deriveWorkspaceName(["payments", "billing"])).toBe(
      "payments + billing",
    );
  });
  it("summarizes 3+ repos with an N-more suffix", () => {
    expect(deriveWorkspaceName(["payments", "billing", "ledger"])).toBe(
      "payments + billing + 1 more",
    );
    expect(
      deriveWorkspaceName(["a", "b", "c", "d", "e"]),
    ).toBe("a + b + 3 more");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: from `apps/desktop/`: `pnpm vitest run src/components/workspaceName.test.ts`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the helper**

Create `apps/desktop/src/components/workspaceName.ts`:

```typescript
// Auto-generated workspace name from selected repo names (design Part 1,
// format A). Names arrive in selection order.
//
//   []            -> ""
//   [a]           -> "a"
//   [a, b]        -> "a + b"
//   [a, b, c,...] -> "a + b + N more"   (N = count - 2)

export function deriveWorkspaceName(names: string[]): string {
  if (names.length === 0) return "";
  if (names.length === 1) return names[0];
  if (names.length === 2) return `${names[0]} + ${names[1]}`;
  const more = names.length - 2;
  return `${names[0]} + ${names[1]} + ${more} more`;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: from `apps/desktop/`: `pnpm vitest run src/components/workspaceName.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/workspaceName.ts apps/desktop/src/components/workspaceName.test.ts
git commit -m "desktop: deriveWorkspaceName helper (auto-name)"
```

---

## Task 9: Bootstrap helper (Part 2, first workarea + session)

**Files:**
- Create: `apps/desktop/src/components/bootstrapWorkspace.ts`
- Test: `apps/desktop/src/components/bootstrapWorkspace.test.ts`

- [ ] **Step 1: Write the failing test**

Create `apps/desktop/src/components/bootstrapWorkspace.test.ts`:

```typescript
import { describe, expect, it, vi, beforeEach } from "vitest";

vi.mock("../api/workareas", () => ({
  createWorkarea: vi.fn(),
}));
vi.mock("../api/sessions", () => ({
  createSession: vi.fn(),
}));

import { createWorkarea } from "../api/workareas";
import { createSession } from "../api/sessions";
import { bootstrapWorkspace, DEFAULT_FIRST_AGENT } from "./bootstrapWorkspace";

describe("bootstrapWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("creates a workarea then a claude session and returns both ids", async () => {
    (createWorkarea as ReturnType<typeof vi.fn>).mockResolvedValue({ id: "wa1" });
    (createSession as ReturnType<typeof vi.fn>).mockResolvedValue({ id: "s1" });

    const result = await bootstrapWorkspace("ws1");

    expect(createWorkarea).toHaveBeenCalledWith("ws1");
    expect(createSession).toHaveBeenCalledWith({
      workareaId: "wa1",
      agentKind: DEFAULT_FIRST_AGENT,
    });
    expect(result).toEqual({ workareaId: "wa1", sessionId: "s1" });
  });

  it("defaults the first agent to claude", () => {
    expect(DEFAULT_FIRST_AGENT).toBe("claude");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: from `apps/desktop/`: `pnpm vitest run src/components/bootstrapWorkspace.test.ts`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the helper**

Create `apps/desktop/src/components/bootstrapWorkspace.ts`:

```typescript
// Part 2 — after a workspace is created, auto-create its first workarea
// and first session. The first session's agent is isolated behind
// DEFAULT_FIRST_AGENT so swapping in real availability detection later is
// a one-line change. Only `claude` is implemented server-side today.

import { createWorkarea } from "../api/workareas";
import { createSession } from "../api/sessions";

export const DEFAULT_FIRST_AGENT = "claude";

export type BootstrapResult = {
  workareaId: string;
  sessionId: string;
};

/// Create the first workarea (inheriting workspace/repo cone defaults — no
/// cones passed) then the first session. Throws if either step fails; the
/// caller decides how to surface a partial-bootstrap (the workspace itself
/// is already committed).
export async function bootstrapWorkspace(
  workspaceId: string,
): Promise<BootstrapResult> {
  const workarea = await createWorkarea(workspaceId);
  const session = await createSession({
    workareaId: workarea.id,
    agentKind: DEFAULT_FIRST_AGENT,
  });
  return { workareaId: workarea.id, sessionId: session.id };
}
```

- [ ] **Step 4: Run to verify it passes**

Run: from `apps/desktop/`: `pnpm vitest run src/components/bootstrapWorkspace.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/bootstrapWorkspace.ts apps/desktop/src/components/bootstrapWorkspace.test.ts
git commit -m "desktop: bootstrapWorkspace helper (first workarea + session)"
```

---

## Task 10: Extract shared `WorkspaceForm` (refactor)

This task extracts the form body from `NewWorkspaceModal.tsx` into a reusable
`WorkspaceForm` so both create and edit modals share it. Behaviour for create
must stay identical (the existing `NewWorkspaceModal.test.tsx` is the guard).

**Files:**
- Create: `apps/desktop/src/components/WorkspaceForm.tsx`
- Modify: `apps/desktop/src/components/NewWorkspaceModal.tsx`

- [ ] **Step 1: Create `WorkspaceForm.tsx` with the shared body**

Create `apps/desktop/src/components/WorkspaceForm.tsx`. Move the following from `NewWorkspaceModal.tsx` into it, exported as a `WorkspaceForm` component: the `RepoCheckout` type, `deriveRepoName`, `AddByUrlPanel`, `AddLocalFolderPanel`, `RepoCheckoutRow`, and `normalizeConeSelection` usage. The component owns all field state (name, icon, description, selected/selectionOrder, repoSearch, addSource, errorMsg) and the repo query. It accepts:

```typescript
export type WorkspaceFormInitial = {
  name: string;
  icon: string;
  description: string;
  // repositoryId -> checkout; selectionOrder gives the order.
  selected: Record<string, { mode: "full" | "sparse"; cones: string[] }>;
  selectionOrder: string[];
};

export type WorkspaceFormSubmit = {
  name: string;
  icon?: string;
  description?: string;
  repos: { repositoryId: string; sparseCones: string[] }[];
};

export type WorkspaceFormProps = {
  // "create" enables auto-name; "edit" disables it (nameEdited starts true).
  mode: "create" | "edit";
  initial?: WorkspaceFormInitial;
  submitLabel: string;
  pendingLabel: string;
  pending: boolean;
  externalError?: string | null;
  // Shown above the buttons in edit mode when the workspace has workareas.
  notice?: string | null;
  onCancel: () => void;
  onSubmit: (values: WorkspaceFormSubmit) => void;
};
```

Implement it by lifting the existing JSX from `NewWorkspaceModal`'s `<form>…</form>` (the name/icon/description grid, the repo picker, the add-source panels, the per-repo checkout rows, the error line, and the action buttons). Wire auto-name (Part 1) here:

```typescript
import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listRepositories, type Repository } from "../api/repositories";
import { normalizeConeSelection } from "./RepoTreeBrowser";
import { deriveWorkspaceName } from "./workspaceName";
// ...plus the moved helpers/components and ui imports.

export function WorkspaceForm(props: WorkspaceFormProps): JSX.Element {
  const { mode, initial } = props;
  const [name, setName] = useState(initial?.name ?? "");
  const [icon, setIcon] = useState(initial?.icon ?? "");
  const [description, setDescription] = useState(initial?.description ?? "");
  const [selected, setSelected] = useState<
    Record<string, { mode: "full" | "sparse"; cones: string[] }>
  >(initial?.selected ?? {});
  const [selectionOrder, setSelectionOrder] = useState<string[]>(
    initial?.selectionOrder ?? [],
  );
  const [repoSearch, setRepoSearch] = useState("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [addSource, setAddSource] = useState<"url" | "local" | null>(null);
  // Edit mode: a saved name is never auto-overwritten.
  const [nameEdited, setNameEdited] = useState(mode === "edit");

  const queryClient = useQueryClient();
  const reposQuery = useQuery({
    queryKey: ["repositories"] as const,
    queryFn: () => listRepositories(),
  });
  const repos = useMemo(() => reposQuery.data?.repositories ?? [], [reposQuery.data]);
  const repoById = useMemo(() => {
    const m = new Map<string, Repository>();
    for (const r of repos) m.set(r.id, r);
    return m;
  }, [repos]);

  // Part 1: auto-fill the name from selected repos until the user edits it
  // (create mode only).
  const selectedNames = useMemo(
    () => selectionOrder.map((id) => repoById.get(id)?.name ?? ""),
    [selectionOrder, repoById],
  );
  useEffect(() => {
    if (mode !== "create" || nameEdited) return;
    setName(deriveWorkspaceName(selectedNames.filter(Boolean)));
  }, [mode, nameEdited, selectedNames]);

  function onNameChange(value: string): void {
    setName(value);
    // Typing pins the name; clearing it re-enables auto-fill.
    setNameEdited(value.trim().length > 0);
  }

  // ...selectRepo/deselectRepo/toggleRepo/setCheckout/afterRepoAdded as in
  // NewWorkspaceModal, but reading `selected` checkout shape {mode, cones}.

  const canSubmit =
    name.trim().length > 0 && selectionOrder.length > 0 && !props.pending;

  function handleSubmit(e: React.FormEvent): void {
    e.preventDefault();
    if (!canSubmit) return;
    props.onSubmit({
      name: name.trim(),
      icon: icon.trim() || undefined,
      description: description.trim() || undefined,
      repos: selectionOrder.map((id) => {
        const c = selected[id];
        return {
          repositoryId: id,
          sparseCones: c?.mode === "sparse" ? c.cones : [],
        };
      }),
    });
  }

  // The <form> JSX is the existing NewWorkspaceModal form body, with:
  //  - the Name <Input> using value={name} onChange={(e)=>onNameChange(e.target.value)}
  //  - props.notice rendered above the buttons (when set)
  //  - the error line showing (props.externalError ?? errorMsg)
  //  - the submit button using props.submitLabel / props.pendingLabel / props.pending
  //  - Cancel calling props.onCancel
  return (/* ...moved JSX... */);
}
```

Keep `deriveRepoName` exported from `WorkspaceForm.tsx` (the existing test imports it from `NewWorkspaceModal`; re-export in Step 2 to avoid breaking it).

- [ ] **Step 2: Rewrite `NewWorkspaceModal.tsx` as a thin create wrapper**

Replace the body of `apps/desktop/src/components/NewWorkspaceModal.tsx` so it renders the `Dialog` + `WorkspaceForm` in create mode, owns the `createWorkspace` mutation, and runs the Part-2 bootstrap on success (Task 11 fills the bootstrap in; for this step wire the mutation without bootstrap so create still works). Re-export `deriveRepoName` for back-compat:

```typescript
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useUiStore } from "../state/useUiStore";
import { createWorkspace } from "../api/workspaces";
import { formatError } from "../api/errors";
import { Dialog } from "./ui/dialog";
import { WorkspaceForm, type WorkspaceFormSubmit } from "./WorkspaceForm";

export { deriveRepoName } from "./WorkspaceForm";

export function NewWorkspaceModal(): JSX.Element {
  const open = useUiStore((s) => s.newWorkspaceModalOpen);
  const setOpen = useUiStore((s) => s.setNewWorkspaceModalOpen);
  const setSelectedWorkspace = useUiStore((s) => s.setSelectedWorkspace);
  const queryClient = useQueryClient();

  const mutation = useMutation({
    mutationFn: (values: WorkspaceFormSubmit) =>
      createWorkspace({
        name: values.name,
        icon: values.icon,
        description: values.description,
        repos: values.repos,
      }),
    onSuccess: (workspace) => {
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      setSelectedWorkspace(workspace.id);
      setOpen(false);
    },
  });

  if (!open) return <></>;
  return (
    <Dialog open={open} onClose={() => setOpen(false)} title="New Workspace">
      <WorkspaceForm
        mode="create"
        submitLabel="Create Workspace"
        pendingLabel="Creating…"
        pending={mutation.isPending}
        externalError={mutation.isError ? formatError(mutation.error) : null}
        onCancel={() => setOpen(false)}
        onSubmit={(values) => mutation.mutate(values)}
      />
    </Dialog>
  );
}
```

Note: `WorkspaceForm` mounts only while the dialog is open (the `if (!open)` guard), so the repo query's previous `enabled: open` behavior is preserved by mount/unmount. The previous "reset form on re-open" effect is replaced by remount.

- [ ] **Step 3: Run the existing create-modal test**

Run: from `apps/desktop/`: `pnpm vitest run src/components/NewWorkspaceModal.test.tsx`
Expected: PASS. If any assertion imported `deriveRepoName` from `NewWorkspaceModal`, the re-export keeps it working. If a test asserts the old single-`useEffect` reset behavior, adjust it to the mount-based reset.

- [ ] **Step 4: Typecheck**

Run: from `apps/desktop/`: `pnpm typecheck`
Expected: no errors.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/WorkspaceForm.tsx apps/desktop/src/components/NewWorkspaceModal.tsx
git commit -m "desktop: extract shared WorkspaceForm; auto-name in create form"
```

---

## Task 11: Wire Part-2 bootstrap + auto-name test into create flow

**Files:**
- Modify: `apps/desktop/src/components/NewWorkspaceModal.tsx`
- Modify: `apps/desktop/src/components/NewWorkspaceModal.test.tsx`

- [ ] **Step 1: Write a failing test for auto-name + bootstrap**

In `apps/desktop/src/components/NewWorkspaceModal.test.tsx`, add tests. Mock `bootstrapWorkspace` and the repo list so two repos are selectable. (Mirror the file's existing mock setup for `createWorkspace`/`listRepositories`.) Add:

```typescript
// at top with other vi.mock calls:
vi.mock("./bootstrapWorkspace", () => ({
  bootstrapWorkspace: vi.fn().mockResolvedValue({ workareaId: "wa1", sessionId: "s1" }),
  DEFAULT_FIRST_AGENT: "claude",
}));
```

```typescript
import { bootstrapWorkspace } from "./bootstrapWorkspace";

it("auto-fills the name from selected repos until edited", async () => {
  // render modal (use the file's existing render helper), select repoA then repoB
  // ...select repoA...
  expect(screen.getByLabelText(/^name$/i)).toHaveValue("repoA");
  // ...select repoB...
  expect(screen.getByLabelText(/^name$/i)).toHaveValue("repoA + repoB");
  // user types -> stops auto-filling
  await userEvent.clear(screen.getByLabelText(/^name$/i));
  await userEvent.type(screen.getByLabelText(/^name$/i), "Custom");
  // ...deselect repoB... name stays "Custom"
  expect(screen.getByLabelText(/^name$/i)).toHaveValue("Custom");
});

it("bootstraps a first workarea + session after create", async () => {
  // render, select a repo, submit
  // ...
  await waitFor(() => expect(bootstrapWorkspace).toHaveBeenCalledWith("ws-new-id"));
});
```

Adapt selectors/render to the existing test file's helpers and the mocked `createWorkspace` return id.

- [ ] **Step 2: Run to verify the bootstrap test fails**

Run: from `apps/desktop/`: `pnpm vitest run src/components/NewWorkspaceModal.test.tsx`
Expected: FAIL — bootstrap is not yet called (and possibly the auto-name test passes already from Task 10).

- [ ] **Step 3: Wire bootstrap into the create mutation**

In `apps/desktop/src/components/NewWorkspaceModal.tsx`, import the helpers and store setters, and update `onSuccess`:

```typescript
import { bootstrapWorkspace } from "./bootstrapWorkspace";
// add store setters:
const setWorkspaceExpanded = useUiStore((s) => s.setWorkspaceExpanded);
const setActiveSession = useUiStore((s) => s.setActiveSession);
const [bootstrapError, setBootstrapError] = useState<string | null>(null);
```

```typescript
    onSuccess: async (workspace) => {
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      setSelectedWorkspace(workspace.id);
      setOpen(false);
      try {
        const { workareaId, sessionId } = await bootstrapWorkspace(workspace.id);
        setWorkspaceExpanded(workspace.id, true);
        setActiveSession(sessionId);
        void queryClient.invalidateQueries({ queryKey: ["workareas", workspace.id] });
        void queryClient.invalidateQueries({ queryKey: ["sessions", workareaId] });
      } catch (e) {
        // Workspace is already committed; keep it and surface a non-fatal note.
        setBootstrapError(formatError(e));
      }
    },
```

Surface `bootstrapError` via the app's existing toast mechanism if one is wired (check `Toast.tsx`); otherwise a `console.warn` plus the next-session manual path is acceptable for V0.1. Keep it non-blocking — do not reopen the dialog.

- [ ] **Step 4: Run tests to verify they pass**

Run: from `apps/desktop/`: `pnpm vitest run src/components/NewWorkspaceModal.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/components/NewWorkspaceModal.tsx apps/desktop/src/components/NewWorkspaceModal.test.tsx
git commit -m "desktop: auto-bootstrap first workarea + session on create"
```

---

## Task 12: UI store + gear button + EditWorkspaceModal

**Files:**
- Modify: `apps/desktop/src/state/useUiStore.ts`
- Modify: `apps/desktop/src/components/Sidebar.tsx`
- Create: `apps/desktop/src/components/EditWorkspaceModal.tsx`
- Modify: `apps/desktop/src/components/AppLayout.tsx`

- [ ] **Step 1: Add `editWorkspaceId` to the UI store**

In `apps/desktop/src/state/useUiStore.ts`:
- Add to the `UiStore` type (near `newWorkspaceModalOpen`):

```typescript
  /// Workspace currently being edited (gear button), or null. Drives the
  /// EditWorkspaceModal. UI-only.
  editWorkspaceId: string | null;
  setEditWorkspaceId: (id: string | null) => void;
```

- Add to the store initializer (near `newWorkspaceModalOpen: false`):

```typescript
  editWorkspaceId: null,
```

- Add the setter (near `setNewWorkspaceModalOpen`):

```typescript
  setEditWorkspaceId: (id) => set({ editWorkspaceId: id }),
```

- [ ] **Step 2: Add the gear button to each workspace row**

In `apps/desktop/src/components/Sidebar.tsx`:
- Import `Settings` is already imported; also pull the setter in `WorkspaceNode`:

```typescript
import { Settings as Gear } from "lucide-react"; // or reuse Settings import
// inside WorkspaceNode:
const setEditWorkspaceId = useUiStore((s) => s.setEditWorkspaceId);
```

(Note: `Settings` is already imported at the top for the header gear. You can reuse it directly — no new import needed; the `Gear` alias above is optional.)

- In `WorkspaceNode`'s `<div className="flex items-center gap-1">`, after the name button, add a gear `IconButton` that opens the edit modal and stops row-selection propagation:

```tsx
        <IconButton
          label="Edit workspace"
          onClick={(e) => {
            e.stopPropagation();
            setEditWorkspaceId(workspace.id);
          }}
        >
          <Settings size={13} />
        </IconButton>
```

Wrap the row so the gear shows on hover/focus: add `group` to the row container `div` and `className="opacity-0 group-hover:opacity-100 focus-visible:opacity-100"` to the `IconButton` (match how other hover affordances are styled in the codebase; if none, leaving it always-visible is acceptable). Import `IconButton` (already imported in `Sidebar.tsx`).

- [ ] **Step 3: Create `EditWorkspaceModal.tsx`**

Create `apps/desktop/src/components/EditWorkspaceModal.tsx`:

```typescript
// Edit an existing workspace — same form as create (WorkspaceForm in
// "edit" mode), pre-filled from the workspace + its declared repos/cones.
// Repo edits affect future workareas only; existing workareas keep their
// worktrees (a notice surfaces this when the workspace has workareas).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useUiStore } from "../state/useUiStore";
import { getWorkspace, listWorkspaceRepos, updateWorkspace } from "../api/workspaces";
import { listWorkareas } from "../api/workareas";
import { formatError } from "../api/errors";
import { Dialog } from "./ui/dialog";
import { WorkspaceForm, type WorkspaceFormSubmit, type WorkspaceFormInitial } from "./WorkspaceForm";

export function EditWorkspaceModal(): JSX.Element {
  const editId = useUiStore((s) => s.editWorkspaceId);
  const setEditId = useUiStore((s) => s.setEditWorkspaceId);
  const queryClient = useQueryClient();
  const open = editId !== null;

  const wsQuery = useQuery({
    queryKey: ["workspace", editId],
    queryFn: () => getWorkspace(editId as string),
    enabled: open,
  });
  const reposQuery = useQuery({
    queryKey: ["workspaceRepos", editId],
    queryFn: () => listWorkspaceRepos(editId as string),
    enabled: open,
  });
  const workareasQuery = useQuery({
    queryKey: ["workareas", editId],
    queryFn: () => listWorkareas(editId as string),
    enabled: open,
  });

  const mutation = useMutation({
    mutationFn: (values: WorkspaceFormSubmit) =>
      updateWorkspace({
        id: editId as string,
        name: values.name,
        icon: values.icon,
        description: values.description,
        repos: values.repos,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      void queryClient.invalidateQueries({ queryKey: ["workspace", editId] });
      void queryClient.invalidateQueries({ queryKey: ["workspaceRepos", editId] });
      setEditId(null);
    },
  });

  if (!open) return <></>;

  const loading = wsQuery.isLoading || reposQuery.isLoading;
  const ws = wsQuery.data;
  const initial: WorkspaceFormInitial | undefined =
    ws && reposQuery.data
      ? {
          name: ws.name,
          icon: ws.icon ?? "",
          description: ws.description ?? "",
          selectionOrder: reposQuery.data.repos.map((r) => r.repository_id),
          selected: Object.fromEntries(
            reposQuery.data.repos.map((r) => [
              r.repository_id,
              {
                mode: r.sparse_cones.length > 0 ? "sparse" : "full",
                cones: r.sparse_cones,
              },
            ]),
          ),
        }
      : undefined;

  const hasWorkareas = (workareasQuery.data?.workareas?.length ?? 0) > 0;

  return (
    <Dialog open={open} onClose={() => setEditId(null)} title="Edit Workspace">
      {loading || !initial ? (
        <p className="text-xs text-faint">Loading workspace…</p>
      ) : (
        <WorkspaceForm
          mode="edit"
          initial={initial}
          submitLabel="Save changes"
          pendingLabel="Saving…"
          pending={mutation.isPending}
          externalError={mutation.isError ? formatError(mutation.error) : null}
          notice={
            hasWorkareas
              ? "Repo changes apply to new workareas; existing workareas keep their current repos."
              : null
          }
          onCancel={() => setEditId(null)}
          onSubmit={(values) => mutation.mutate(values)}
        />
      )}
    </Dialog>
  );
}
```

Keep imports to exactly what compiles (the `WorkspaceFormSubmit`/`WorkspaceFormInitial` types come from `WorkspaceForm.tsx`, defined in Task 10).

- [ ] **Step 4: Mount the edit modal**

In `apps/desktop/src/components/AppLayout.tsx`, find where `<NewWorkspaceModal />` is rendered and add `<EditWorkspaceModal />` next to it:

```tsx
import { EditWorkspaceModal } from "./EditWorkspaceModal";
// ...
      <NewWorkspaceModal />
      <EditWorkspaceModal />
```

(If `NewWorkspaceModal` is mounted elsewhere, grep `rg "NewWorkspaceModal" apps/desktop/src` and place `EditWorkspaceModal` in the same parent.)

- [ ] **Step 5: Typecheck**

Run: from `apps/desktop/`: `pnpm typecheck`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/state/useUiStore.ts apps/desktop/src/components/Sidebar.tsx apps/desktop/src/components/EditWorkspaceModal.tsx apps/desktop/src/components/AppLayout.tsx
git commit -m "desktop: gear button + EditWorkspaceModal (edit workspace)"
```

---

## Task 13: EditWorkspaceModal test (pre-fill + save)

**Files:**
- Create: `apps/desktop/src/components/EditWorkspaceModal.test.tsx`

- [ ] **Step 1: Write the test**

Create `apps/desktop/src/components/EditWorkspaceModal.test.tsx`. Mirror the mock + render conventions of `NewWorkspaceModal.test.tsx` (it shows how `callRpc`/api modules are mocked and how the QueryClient is provided via `test-utils.tsx`). Cover: pre-fill from `getWorkspace` + `listWorkspaceRepos`, no auto-name overwrite in edit mode, and submit calls `updateWorkspace`.

```typescript
import { describe, expect, it, vi, beforeEach } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { renderWithClient } from "./test-utils";

vi.mock("../api/workspaces", () => ({
  getWorkspace: vi.fn().mockResolvedValue({
    id: "ws1", name: "Payments", slug: "payments", icon: "💸", description: "desc",
  }),
  listWorkspaceRepos: vi.fn().mockResolvedValue({
    repos: [{ repository_id: "repoA", sparse_cones: [] }],
  }),
  updateWorkspace: vi.fn().mockResolvedValue({ id: "ws1" }),
}));
vi.mock("../api/workareas", () => ({
  listWorkareas: vi.fn().mockResolvedValue({ workareas: [] }),
}));
vi.mock("../api/repositories", () => ({
  listRepositories: vi.fn().mockResolvedValue({
    repositories: [{ id: "repoA", name: "repoA", cone_defaults: [] }],
  }),
}));

import { getWorkspace, updateWorkspace } from "../api/workspaces";
import { useUiStore } from "../state/useUiStore";
import { EditWorkspaceModal } from "./EditWorkspaceModal";

describe("EditWorkspaceModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useUiStore.setState({ editWorkspaceId: "ws1" });
  });

  it("pre-fills name/icon/description from the workspace", async () => {
    renderWithClient(<EditWorkspaceModal />);
    await waitFor(() => expect(getWorkspace).toHaveBeenCalledWith("ws1"));
    expect(await screen.findByDisplayValue("Payments")).toBeInTheDocument();
    expect(screen.getByDisplayValue("💸")).toBeInTheDocument();
  });

  it("saves edits via updateWorkspace", async () => {
    renderWithClient(<EditWorkspaceModal />);
    const name = await screen.findByDisplayValue("Payments");
    await userEvent.clear(name);
    await userEvent.type(name, "Payments v2");
    await userEvent.click(screen.getByRole("button", { name: /save changes/i }));
    await waitFor(() =>
      expect(updateWorkspace).toHaveBeenCalledWith(
        expect.objectContaining({ id: "ws1", name: "Payments v2" }),
      ),
    );
  });
});
```

Adapt selectors to the actual labels/roles `WorkspaceForm` renders (the Name input uses `aria`/label "Name"; the icon input has `aria-label="Icon"`). If `renderWithClient` lives elsewhere, import from the correct path used by other tests.

- [ ] **Step 2: Run the test**

Run: from `apps/desktop/`: `pnpm vitest run src/components/EditWorkspaceModal.test.tsx`
Expected: PASS (2 tests). Fix selector mismatches against `WorkspaceForm`'s actual markup until green.

- [ ] **Step 3: Run the full frontend suite + typecheck**

Run: from `apps/desktop/`: `pnpm test && pnpm typecheck`
Expected: all PASS, no type errors.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/components/EditWorkspaceModal.test.tsx
git commit -m "desktop: EditWorkspaceModal pre-fill + save tests"
```

---

## Task 14: Full-stack verification + graph update

**Files:** none (verification only).

- [ ] **Step 1: Run the full backend test suites**

Run: `cargo test -p concerto-persist -p concerto-core`
Expected: all PASS.

- [ ] **Step 2: Build the Tauri shell**

Run: `cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml`
Expected: builds clean.

- [ ] **Step 3: Run the full frontend suite + build**

Run: from `apps/desktop/`: `pnpm test && pnpm build`
Expected: tests PASS, `tsc --noEmit && vite build` succeeds.

- [ ] **Step 4: Update the knowledge graph**

Run: `graphify update .`
Expected: graph regenerates (AST-only, no API cost), picking up the new files/RPCs.

- [ ] **Step 5: Commit any graph changes**

```bash
git add graphify-out
git commit -m "graphify: update graph for workspace edit/auto-name/bootstrap" || echo "no graph changes"
```

---

## Self-Review notes

- **Spec coverage:** Part 1 → Tasks 8, 10, 11. Part 2 → Tasks 9, 11. Part 3 backend → Tasks 1–6; Part 3 frontend → Tasks 7, 10, 12, 13. Decisions (fixed slug → Tasks 2/3/5 assertions; repos affect future workareas only → reuses existing `update_workspace_repos` + notice in Task 12; default agent `claude` → Task 9; permission mode out of scope → not touched).
- **Types:** `WorkspaceFormSubmit`/`WorkspaceFormInitial` defined in Task 10 are consumed in Tasks 11–13. `WorkspaceRepoEntry`/`updateWorkspace`/`listWorkspaceRepos` defined in Task 7, consumed in Task 12. `DEFAULT_FIRST_AGENT`/`bootstrapWorkspace` defined in Task 9, consumed in Task 11. `WorkspaceEvent::Updated` defined in Task 3 (no other consumer required — the sidebar invalidates on any `workspace.events` frame).
- **Known follow-ups (out of scope, per spec):** agent availability detection; editing permission mode in this form; slug re-derivation; worktree migration on repo-set change.
