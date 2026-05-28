// React Query hooks for the Workspaces surface.
//
// Per the locked Phase 2 contract (`design/15 §3.3`), React Query
// owns all server state. Component code never reaches for `invoke`
// directly; it goes through `src/api/` via these hooks.

import { useQuery } from "@tanstack/react-query";

import { getWorkspace, listWorkspaces, type Workspace } from "../api/workspaces";

export function useWorkspaces(projectId: string | null | undefined) {
  return useQuery({
    queryKey: ["workspaces", projectId] as const,
    queryFn: async () => {
      if (!projectId) return { workspaces: [] };
      return listWorkspaces(projectId);
    },
    enabled: !!projectId,
  });
}

export function useWorkspace(id: string | null | undefined) {
  return useQuery<Workspace | null>({
    queryKey: ["workspace", id] as const,
    queryFn: async () => {
      if (!id) return null;
      return getWorkspace(id);
    },
    enabled: !!id,
  });
}
