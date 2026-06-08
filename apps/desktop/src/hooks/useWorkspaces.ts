// React Query hooks for the Workspaces surface.
//
// Per the locked Phase 2 contract (`design/15 §3.3`), React Query
// owns all server state. Component code never reaches for `invoke`
// directly; it goes through `src/api/` via these hooks.

import { useQuery } from "@tanstack/react-query";

import { getWorkspace, listWorkspaces, type Workspace } from "../api/workspaces";

/// Lists ALL workspaces (the global registry — the Project layer was
/// collapsed away, so there is no per-project scoping anymore).
export function useWorkspaces() {
  return useQuery({
    queryKey: ["workspaces"] as const,
    queryFn: async () => {
      return listWorkspaces({ includeArchived: false });
    },
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
