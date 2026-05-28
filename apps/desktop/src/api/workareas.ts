// Typed wrappers around `Workareas.*` RPCs.
//
// Mirrors `concerto.v1.Workarea`. As with the other proto mirrors in
// this directory (see `workspaces.ts`), prost-serde keeps snake_case
// field names; timestamps land as `[seconds, nanos]` tuples or null.

import { callRpc } from "./client";

export type Workarea = {
  id: string;
  workspace_id: string;
  composer_name: string;
  branch_name: string;
  worktree_root: string;
  // status ∈ { created | active | running | awaiting | paused | archived | crashed }
  status: string;
  permission_mode?: number | null;
  created_at?: [number, number] | null;
  last_activity_at?: [number, number] | null;
  archived_at?: [number, number] | null;
};

export type ListWorkareasResponse = {
  workareas: Workarea[];
};

export async function listWorkareas(
  workspaceId: string,
  includeArchived = false,
): Promise<ListWorkareasResponse> {
  return callRpc<
    { workspace_id: string; include_archived: boolean },
    ListWorkareasResponse
  >("Workareas.ListWorkareas", {
    workspace_id: workspaceId,
    include_archived: includeArchived,
  });
}

export async function getWorkarea(id: string): Promise<Workarea> {
  return callRpc<{ id: string }, Workarea>("Workareas.GetWorkarea", { id });
}

export async function createWorkarea(
  workspaceId: string,
  permissionMode?: number,
): Promise<Workarea> {
  return callRpc<
    { workspace_id: string; permission_mode?: number },
    Workarea
  >("Workareas.CreateWorkarea", {
    workspace_id: workspaceId,
    permission_mode: permissionMode,
  });
}
