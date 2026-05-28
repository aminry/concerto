// Typed wrappers around `Workspaces.*` RPCs.

import { callRpc } from "./client";

// Mirrors `concerto.v1.Workspace`. Per the proto serde shim,
// timestamps land as `[seconds, nanos]` tuples or null.
export type Workspace = {
  id: string;
  project_id: string;
  name: string;
  slug: string;
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

export async function listWorkspaces(
  projectId: string,
): Promise<ListWorkspacesResponse> {
  return callRpc<{ project_id: string }, ListWorkspacesResponse>(
    "Workspaces.ListWorkspaces",
    { project_id: projectId },
  );
}

export async function getWorkspace(id: string): Promise<Workspace> {
  return callRpc<{ id: string }, Workspace>("Workspaces.GetWorkspace", { id });
}
