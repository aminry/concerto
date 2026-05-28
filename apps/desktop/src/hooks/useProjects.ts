// React Query hook for `Projects.ListProjects`.

import { useQuery } from "@tanstack/react-query";

import { listProjects } from "../api/projects";

export function useProjects() {
  return useQuery({
    queryKey: ["projects"] as const,
    queryFn: listProjects,
  });
}
