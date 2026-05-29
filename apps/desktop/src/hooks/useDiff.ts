// React Query hook for `Workareas.GetWorkareaRepoDiff`.
//
// Task 47 wires the Monaco diff viewer. V0.1 polls on focus / explicit
// refresh — Task 30's `diff.<workarea>.<repo>` stream subject doesn't
// exist yet, so we rely on the explicit "Refresh" button in the viewer
// to call `invalidateQueries`.

import { useQuery } from "@tanstack/react-query";

import { getWorkareaRepoDiff, type DiffPayload } from "../api/diff";

export function diffQueryKey(
  workareaId: string | null | undefined,
  repositoryId: string | null | undefined,
) {
  return ["diff", workareaId, repositoryId] as const;
}

export function useDiff(
  workareaId: string | null | undefined,
  repositoryId: string | null | undefined,
) {
  return useQuery<DiffPayload>({
    queryKey: diffQueryKey(workareaId, repositoryId),
    queryFn: async () => {
      if (!workareaId || !repositoryId) return { files: [] };
      return getWorkareaRepoDiff(workareaId, repositoryId);
    },
    enabled: !!workareaId && !!repositoryId,
  });
}
