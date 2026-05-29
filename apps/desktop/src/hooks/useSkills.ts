// React Query hook for `Skills.ListSkills`.
//
// Used by the right-rail Skills tab from Task 46. The project_id filter
// scopes the list to skills discovered for the active project plus
// personal-scope skills (server returns both when project_id is set).

import { useQuery } from "@tanstack/react-query";

import { listSkills } from "../api/skills";

export function useSkills(projectId: string | null | undefined) {
  return useQuery({
    queryKey: ["skills", projectId] as const,
    queryFn: async () => {
      if (!projectId) return { skills: [] };
      return listSkills({ projectId });
    },
    enabled: !!projectId,
  });
}
