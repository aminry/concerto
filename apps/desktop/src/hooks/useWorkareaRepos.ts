// React Query hook for the repos that belong to a workarea (Task 322).
//
// ── Where the repo list comes from (the FROZEN-respecting binding) ───
// Tasks 306/307 did NOT freeze a `Workarea.repository_ids` field or a
// `Workareas.ListWorkareaRepos` RPC (see `api/workareas.ts` for the full
// reasoning). A V1.0 workspace declares its repos and every workarea
// materializes one worktree per declared repo, so the workspace's declared
// repos ARE the workarea's repos. Since the Project→Workspace collapse,
// repositories live in a single GLOBAL registry, so they are fetched via
// `Repositories.ListRepositories` (the existing read RPC, now unscoped).
//
// Keyed by `["workareaRepos", workareaId]` so the cache is scoped per
// workarea and the Level-1 selector's data invalidates cleanly when the
// workarea changes. Server-canonical data stays in React Query; only the
// active-repo *selection* lands in Zustand.

import { useQuery } from "@tanstack/react-query";

import { listRepositories, type Repository } from "../api/repositories";

export function workareaReposQueryKey(workareaId: string | null | undefined) {
  return ["workareaRepos", workareaId] as const;
}

/// Returns the repositories materialized in the given workarea (= the global
/// registry's repos). Short-circuits to an empty list when `workareaId` is
/// null so a caller can render before a selection is made without firing a
/// request.
export function useWorkareaRepos(workareaId: string | null | undefined) {
  return useQuery<Repository[]>({
    queryKey: workareaReposQueryKey(workareaId),
    queryFn: async () => {
      if (!workareaId) return [];
      const res = await listRepositories();
      return res.repositories;
    },
    enabled: !!workareaId,
  });
}
