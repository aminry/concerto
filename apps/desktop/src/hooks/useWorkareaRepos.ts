// React Query hook for the repos that belong to a workarea (Task 322).
//
// ── Where the repo list comes from (the FROZEN-respecting binding) ───
// Tasks 306/307 did NOT freeze a `Workarea.repository_ids` field or a
// `Workareas.ListWorkareaRepos` RPC (see `api/workareas.ts` for the full
// reasoning), and 322 may not add Rust/proto. But a V1.0 workspace
// declares its repos (306) and every workarea materializes one worktree
// per declared repo (306 §6.2), so the workspace's declared repos ARE the
// workarea's repos. They are fetched via `Repositories.ListByProject`
// (the existing read RPC) scoped to the workarea's project.
//
// This hook takes the `projectId` directly (the caller already has it via
// `useUiStore.selectedProjectId`); it is keyed by `["workareaRepos",
// workareaId]` so the cache is scoped per workarea (the value is identical
// across workareas of one project, but keying by workarea matches the
// design/15 §3.3 cache-key intent and means the Level-1 selector's data
// invalidates cleanly when the workarea changes). Server-canonical data
// stays in React Query; only the active-repo *selection* lands in Zustand.

import { useQuery } from "@tanstack/react-query";

import { listRepositories, type Repository } from "../api/repositories";

export function workareaReposQueryKey(
  workareaId: string | null | undefined,
  projectId: string | null | undefined,
) {
  return ["workareaRepos", workareaId, projectId] as const;
}

/// Returns the repositories materialized in the given workarea (= the
/// workarea's parent project's declared repos). Short-circuits to an empty
/// list when either id is null so a caller can render before a selection
/// is made without firing a request.
export function useWorkareaRepos(
  workareaId: string | null | undefined,
  projectId: string | null | undefined,
) {
  return useQuery<Repository[]>({
    queryKey: workareaReposQueryKey(workareaId, projectId),
    queryFn: async () => {
      if (!workareaId || !projectId) return [];
      const res = await listRepositories(projectId);
      return res.repositories;
    },
    enabled: !!workareaId && !!projectId,
  });
}
