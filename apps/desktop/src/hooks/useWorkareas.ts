// React Query hooks for the Workareas surface.
//
// `useWorkareas(workspaceId)` returns the workarea list for a given
// workspace. Per the Task 25 spec, the sidebar only fetches workareas
// when a workspace node is expanded — gating happens at the caller
// via the `enabled` flag (we mirror that here by short-circuiting on
// a null id, so passing `undefined` produces an empty result without
// firing a request).

import { useQuery } from "@tanstack/react-query";

import { getWorkarea, listWorkareas, type Workarea } from "../api/workareas";

export function useWorkareas(workspaceId: string | null | undefined) {
  return useQuery({
    queryKey: ["workareas", workspaceId] as const,
    queryFn: async () => {
      if (!workspaceId) return { workareas: [] };
      return listWorkareas(workspaceId);
    },
    enabled: !!workspaceId,
  });
}

export function useWorkarea(id: string | null | undefined) {
  return useQuery<Workarea | null>({
    queryKey: ["workarea", id] as const,
    queryFn: async () => {
      if (!id) return null;
      return getWorkarea(id);
    },
    enabled: !!id,
  });
}
