// React Query hook for `Skills.ListSkills`.
//
// Used by the right-rail Skills tab from Task 46. The workspace_id filter
// scopes the list to skills discovered for the active workspace plus
// personal-scope skills (server returns both when workspace_id is set).

import { useQuery } from "@tanstack/react-query";

import { listSkills } from "../api/skills";

export function useSkills(workspaceId: string | null | undefined) {
  return useQuery({
    queryKey: ["skills", workspaceId] as const,
    queryFn: async () => {
      if (!workspaceId) return { skills: [] };
      return listSkills({ workspaceId });
    },
    enabled: !!workspaceId,
  });
}
