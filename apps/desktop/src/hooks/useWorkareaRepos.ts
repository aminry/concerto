// React Query hook for the repos that belong to a workarea.
//
// ── Where the repo list comes from (workarea-scoped, NOT the registry) ───
// `Workareas.ListWorkareaRepos` returns exactly the repos materialized in the
// workarea (the `workarea_repos` junction), so every repo it returns is one
// `GetWorkareaRepoDiff` will accept. The global `Repositories.ListRepositories`
// is the unscoped registry (every repo across all workspaces) and must NOT be
// used here: it would offer repos this workarea never materialized, and the
// backend correctly rejects a diff request for them ("repository … is not
// attached to workarea …") — the Diff-panel bug this hook previously caused.
//
// Keyed by `["workareaRepos", workareaId]` so the cache is scoped per
// workarea and the Level-1 selector's data invalidates cleanly when the
// workarea changes. Server-canonical data stays in React Query; only the
// active-repo *selection* lands in Zustand.

import { useQuery } from "@tanstack/react-query";

import type { Repository } from "../api/repositories";
import { listWorkareaRepos } from "../api/workareas";

export function workareaReposQueryKey(workareaId: string | null | undefined) {
  return ["workareaRepos", workareaId] as const;
}

/// Returns the repositories attached to (materialized in) the given workarea.
/// Short-circuits to an empty list when `workareaId` is null so a caller can
/// render before a selection is made without firing a request.
export function useWorkareaRepos(workareaId: string | null | undefined) {
  return useQuery<Repository[]>({
    queryKey: workareaReposQueryKey(workareaId),
    queryFn: async () => {
      if (!workareaId) return [];
      const res = await listWorkareaRepos(workareaId);
      return res.repositories;
    },
    enabled: !!workareaId,
  });
}
