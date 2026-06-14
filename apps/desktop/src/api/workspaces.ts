// Typed wrappers around `Workspaces.*` RPCs.

import { callRpc } from "./client";

/// The Maestro system workspace id. Mirrors the Core sentinel
/// `MAESTRO_SYSTEM_WORKSPACE_ID` (`crates/persist/src/lib.rs`) — the hidden
/// workspace that hosts the Maestro singleton session. The Core already filters
/// it from `Workspaces.ListWorkspaces` (Task 4); we filter defensively here so
/// no UI path ever surfaces it. Kept in ONE place in the TS layer.
export const MAESTRO_SYSTEM_WORKSPACE_ID = "__maestro_system__";

// Mirrors `concerto.v1.Workspace`. Per the proto serde shim,
// timestamps land as `[seconds, nanos]` tuples or null.
//
// The Project layer was collapsed away: a Workspace is now a top-level
// node over the global Repository registry (no `project_id`), and carries
// an optional `icon`.
export type Workspace = {
  id: string;
  name: string;
  slug: string;
  icon?: string | null;
  description?: string | null;
  // The proto's `PermissionMode` enum serializes as its integer
  // ordinal under prost-serde. UI code that needs to display it
  // should map ordinal → label; V0.1 just shows the workspace name.
  permission_mode?: number | null;
  created_at?: [number, number] | null;
  archived_at?: [number, number] | null;
};

export type ListWorkspacesResponse = {
  workspaces: Workspace[];
};

/// `Workspaces.ListWorkspaces` — lists ALL workspaces (global registry).
/// `includeArchived` toggles archived rows; defaults to false.
export async function listWorkspaces(opts?: {
  includeArchived?: boolean;
}): Promise<ListWorkspacesResponse> {
  const res = await callRpc<
    { include_archived: boolean },
    ListWorkspacesResponse
  >("Workspaces.ListWorkspaces", {
    include_archived: opts?.includeArchived ?? false,
  });
  // Belt-and-suspenders: the Core already excludes the Maestro system
  // workspace (Task 4), but filter defensively so no path surfaces it.
  return {
    workspaces: res.workspaces.filter(
      (w) => w.id !== MAESTRO_SYSTEM_WORKSPACE_ID,
    ),
  };
}

export async function getWorkspace(id: string): Promise<Workspace> {
  return callRpc<{ id: string }, Workspace>("Workspaces.GetWorkspace", { id });
}

/// One repository's checkout config within a CreateWorkspace call. `sparseCones`
/// empty ⇒ full working tree; non-empty ⇒ a sparse cone of those directories.
export type WorkspaceRepoSpec = {
  repositoryId: string;
  sparseCones: string[];
};

export async function createWorkspace(input: {
  name: string;
  icon?: string;
  description?: string;
  permissionMode?: number;
  repos: WorkspaceRepoSpec[];
}): Promise<Workspace> {
  return callRpc<
    {
      name: string;
      icon?: string;
      description?: string;
      permission_mode?: number;
      repos: { repository_id: string; sparse_cones: string[] }[];
    },
    Workspace
  >("Workspaces.CreateWorkspace", {
    name: input.name,
    icon: input.icon,
    description: input.description,
    permission_mode: input.permissionMode,
    repos: input.repos.map((r) => ({
      repository_id: r.repositoryId,
      sparse_cones: r.sparseCones,
    })),
  });
}

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
