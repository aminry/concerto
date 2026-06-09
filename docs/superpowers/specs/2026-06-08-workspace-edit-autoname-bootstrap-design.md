# Workspace edit, auto-name, and auto-bootstrap — design

**Date:** 2026-06-08
**Status:** Approved (design)

## Summary

Three related improvements to the workspace lifecycle UX in the desktop app:

1. **Auto-generated workspace name** — the create form's Name field auto-fills
   from the selected repositories, and stops tracking once the user edits it.
2. **Auto-bootstrap on create** — creating a workspace automatically creates its
   first workarea and first session (a `claude` session).
3. **Edit workspace** — every workspace row gets a gear button that opens the
   same form, pre-filled, to edit name / icon / description **and the repo set**
   (add/remove repos, re-pick sparse cones). This requires new backend surface.

Parts 1 and 2 are frontend-only. Part 3 spans proto + Rust (persist, actor,
handler, Tauri RPC) + frontend.

## Context (current state)

- `apps/desktop/src/components/NewWorkspaceModal.tsx` is the create form: name,
  icon, description, a repo multi-select (existing registry + add-by-URL + add
  local folder), and per-repo checkout (full vs sparse cone). On submit it calls
  `createWorkspace` and selects the new workspace. The Name field is fully
  manual today.
- `WorkspaceDetail.tsx` hosts the "+ new workarea" flow (cone picker →
  `createWorkarea`). `SessionRegion.tsx` hosts the "+ new session" menu →
  `createSession({ workareaId, agentKind })`.
- Only `claude` (and the internal `echo`) is implemented server-side;
  `codex`/`gemini` return `agent.not_implemented` (Phase 3). There is **no**
  "which agents are available" RPC; the session menu list is hardcoded.
- The `WorkspaceManager` actor already has `update_workspace_repos()` (replace
  the declared repo set; existing workareas keep their worktrees, the new set
  applies to future workareas) and `update_workspace_settings()` (permission
  mode only). **Neither name/icon/description updates nor a repo-editing RPC is
  exposed** — only `Workspaces.UpdateWorkspaceSettings` (permission mode) reaches
  the wire, and even that is not wired through the Tauri `rpc.rs` allowlist.
- There is **no** RPC to read a workspace's declared repos + their cones
  (`workspace_repos.sparse_cones_json`); needed to pre-fill the edit form.
- `apps/desktop/src-tauri/src/rpc.rs` is an explicit `match method { ... }`
  allowlist; any new RPC must be added there too.

## Part 1 — Auto-generated workspace name (frontend only)

**File:** `NewWorkspaceModal.tsx` (and a small extracted helper).

- Add a pure helper `deriveWorkspaceName(names: string[]): string`, format A:
  - `[]` → `""`
  - `[a]` → `a`
  - `[a, b]` → `a + b`
  - `[a, b, c, ...]` → `a + b + N more`, where `N = names.length - 2`
  - Names are in **selection order** (the existing `selectionOrder` array maps to
    `repoById.get(id)?.name`).
- Add a `nameEdited` boolean state (default `false`).
- Effect: whenever `selectionOrder` (or the resolved names) changes and
  `!nameEdited`, `setName(deriveWorkspaceName(selectedNames))`.
- Name field `onChange`: set the value and set `nameEdited = true`. If the user
  clears the field to empty, reset `nameEdited = false` so auto-fill resumes.
- Reset `nameEdited = false` whenever the dialog re-opens (alongside the existing
  form reset).

**Edge:** a newly added repo (URL / local) lands selected and therefore feeds the
auto-name immediately via the same path.

## Part 2 — Auto-bootstrap first workarea + session on create

**File:** `NewWorkspaceModal.tsx` create mutation `onSuccess`, plus a small
helper.

- Add `const DEFAULT_FIRST_AGENT = "claude";` (single source of truth for the
  bootstrap agent; swapping in real availability detection later is a one-line
  change).
- Extract `bootstrapWorkspace(workspaceId): Promise<{ workareaId; sessionId } | null>`
  that calls `createWorkarea(workspaceId)` (no cones → inherits the
  workspace/repo cone defaults) then
  `createSession({ workareaId, agentKind: DEFAULT_FIRST_AGENT })`.
- On `createWorkspace` success:
  1. Select the workspace and close the dialog (unchanged).
  2. Invalidate `["workspaces"]` (unchanged).
  3. Run `bootstrapWorkspace(workspace.id)`. On success: expand the workspace
     (`setWorkspaceExpanded(id, true)`), invalidate
     `["workareas", id]` and `["sessions", workareaId]`, and set the new session
     active (`setActiveSession`).
- **Failure handling:** the workspace is already committed. If the workarea or
  session step fails, do **not** roll back the workspace. Surface a non-fatal
  toast/error ("Workspace created, but couldn't start the first session — open it
  to start one") and leave the user to create a session manually.

## Part 3 — Edit workspace

### 3.1 Backend — proto

In `crates/proto/proto/concerto/v1/workspaces.proto`:

```proto
message UpdateWorkspaceRequest {
  string workspace_id = 1;
  // Absent = no change. Present (incl. empty string) = set to that value.
  optional string name = 2;
  optional string icon = 3;
  optional string description = 4;
  // Empty = leave the repo set unchanged. Non-empty = replace the whole set.
  // (A workspace can never have zero repos, so empty is an unambiguous
  // "no change" sentinel.)
  repeated WorkspaceRepoSpec repos = 5;
}

message WorkspaceRepoEntry {
  string repository_id = 1;
  repeated string sparse_cones = 2;
}

message ListWorkspaceReposResponse {
  // Position-ordered (declaration order).
  repeated WorkspaceRepoEntry repos = 1;
}

service Workspaces {
  // ... existing RPCs ...
  rpc UpdateWorkspace(UpdateWorkspaceRequest) returns (Workspace);
  rpc ListWorkspaceRepos(WorkspaceId) returns (ListWorkspaceReposResponse);
}
```

`UpdateWorkspace` is additive; existing `UpdateWorkspaceSettings` (permission
mode) is untouched. Field numbers are appended, not renumbered.

### 3.2 Backend — persist (`crates/persist/src/workspaces.rs`)

- `set_metadata(conn, id, name: Option<&str>, icon: Option<Option<&str>>, description: Option<Option<&str>>)`
  — UPDATEs only the columns whose patch is `Some`. **Slug is not touched.**
  (Nested `Option` for icon/description distinguishes "no change" from "clear to
  NULL"; name has no NULL state so a plain `Option<&str>` suffices.)
- `list_repo_cones(pool, workspace_id) -> Vec<(RepositoryId, String)>` —
  `(repository_id, sparse_cones_json)` ordered by `(position, repository_id)`,
  mirroring `list_repos`.

### 3.3 Backend — actor (`crates/core/src/workspace_manager/actor.rs`)

- `update_workspace(id, name, icon, description, repos)`:
  1. Load the workspace (NotFound if absent).
  2. If any metadata patch is present, validate (non-empty name if provided) and
     `set_metadata`.
  3. If `repos` is non-empty, reuse the existing `update_workspace_repos(id, repos)`
     (validation, cone preservation, `position` re-stamping, `ReposUpdated`
     event all already implemented there).
  4. Return the updated `Workspace` row.
  - Metadata + repos changes need not be one transaction (they touch disjoint
    rows and each is individually atomic). Events: when repos change, emit the
    existing `WorkspaceEvent::ReposUpdated(workspace)`. For a metadata-only
    change, add a `WorkspaceEvent::Updated(Workspace)` variant carrying the
    post-update row (the sidebar already invalidates `["workspaces"]` on any
    `workspace.events` frame, so either variant refreshes the UI). If both
    changed, emitting `ReposUpdated` alone is sufficient (the payload carries the
    full updated row).
- `list_workspace_repos(id) -> Vec<WorkspaceRepoSpec>` — calls `list_repo_cones`
  and parses each `sparse_cones_json` into `sparse_cones`.

### 3.4 Backend — handler + Tauri RPC

- `crates/core/src/handlers/workspaces.rs`: implement `update_workspace` and
  `list_workspace_repos`, mapping proto ↔ actor types (reuse the existing
  `WorkspaceRepoSpec` mapping from `create_workspace`). Empty `name` when the
  field is present is rejected as `INVALID_ARGUMENT`.
- `apps/desktop/src-tauri/src/rpc.rs`: add `"Workspaces.UpdateWorkspace"` and
  `"Workspaces.ListWorkspaceRepos"` cases to the dispatch match.

### 3.5 Frontend — API (`apps/desktop/src/api/workspaces.ts`)

- `updateWorkspace(input: { id; name?; icon?; description?; repos?: WorkspaceRepoSpec[] }): Promise<Workspace>`
  — calls `Workspaces.UpdateWorkspace`. Omitted `repos` sends `[]` (no change);
  omitted name/icon/description omit the proto field.
- `listWorkspaceRepos(id): Promise<{ repos: { repository_id; sparse_cones: string[] }[] }>`
  — calls `Workspaces.ListWorkspaceRepos`.

### 3.6 Frontend — shared form + gear button

- **Extract** the body of `NewWorkspaceModal.tsx` into a reusable
  `WorkspaceForm` (fields, repo picker, add-source panels, checkout rows,
  auto-name logic). Two entry points share it:
  - create mode → submits via `createWorkspace` + runs Part 2 bootstrap;
  - edit mode → pre-fills from `getWorkspace` + `listWorkspaceRepos`, submits via
    `updateWorkspace`, does **not** bootstrap.
  Auto-name (Part 1) is **create-only**; edit mode initializes `nameEdited = true`
  so a saved name is never silently overwritten.
- Edit pre-fill: for each entry from `listWorkspaceRepos`, seed the selection with
  `mode: cones.length > 0 ? "sparse" : "full"` and `cones`.
- **Sidebar** (`Sidebar.tsx` `WorkspaceNode`): add a gear `IconButton` shown on
  hover/focus that opens the edit form for that workspace. UI store
  (`useUiStore.ts`) gains `editWorkspaceId: string | null` + setter; the edit
  modal opens when it is non-null.
- When the workspace being edited already has ≥1 workarea **and** the repo set is
  changed, show a small inline note: "Repo changes apply to new workareas;
  existing workareas keep their current repos."

## Decisions (locked)

- **Edit scope = full create-form parity** including repos (option B).
- **Slug is immutable after creation.** Renaming changes the display name only.
- **Repo edits affect future workareas only** (existing actor behavior); the form
  surfaces this when workareas exist.
- **First agent = `claude`**, isolated behind `DEFAULT_FIRST_AGENT`; no
  availability detection in this work.
- **Permission mode is out of scope** for this form (it has its own
  `UpdateWorkspaceSettings` and is absent from the create form).

## Testing

**Rust:**
- persist: `set_metadata` updates only patched columns and leaves slug intact;
  `list_repo_cones` returns position-ordered `(repo_id, cones_json)`.
- core: `update_workspace` — metadata-only change; repo add preserves existing
  repos' cones and seeds new repos from defaults; repo removal; existing
  workareas keep their worktrees after a repo-set change; `list_workspace_repos`
  round-trips create → list.
- handler: empty-present `name` → `INVALID_ARGUMENT`; empty `repos` leaves the
  set unchanged.

**Frontend (vitest):**
- `deriveWorkspaceName` for 0/1/2/3+ repos.
- Auto-fill-until-edited: name tracks selection, stops on manual edit, resumes on
  clear.
- Create flow runs the workarea + session bootstrap and selects the session;
  bootstrap failure keeps the workspace and surfaces a non-fatal error.
- Edit modal pre-fills name/icon/description + repos/cones and submits via
  `updateWorkspace` without bootstrapping; auto-name does not overwrite in edit
  mode.

## Out of scope

- Agent availability detection (real `which claude/codex/gemini` probing).
- Editing permission mode through this form.
- Re-deriving slug on rename.
- Migrating/destroying existing workareas' worktrees when a workspace's repo set
  changes.
