// React Query hooks for the workarea's PR set (Task 324).
//
// - `usePrSet(workareaId)` reads `Workareas.GetWorkareaPrSet` — the implicit
//   PR set ordered `(merge_order, pr_number)` (Task 319). The cache is keyed
//   `["prSet", workareaId]`.
// - `useSetMergeOrder()` is the drag-to-reorder mutation: it writes one PR's
//   `merge_order` via `Workareas.SetMergeOrder` and seeds the cache from the
//   returned re-ordered set (one round-trip, authoritative order).
// - `useSetReadiness(workareaId)` aggregates per-repo check state across the
//   set so the workarea-wide "Merge PR set" button can disable on ANY red
//   check (`design/15 §3.4` — read the union, not a single PR).

import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";

import {
  getChecks,
  getWorkareaPrSet,
  hasRed,
  setMergeOrder,
  type GetWorkareaPrSetResponse,
  type PullRequest,
} from "../api/vcs";
import { checksQueryKey } from "./useChecks";

export function prSetQueryKey(workareaId: string | null | undefined) {
  return ["prSet", workareaId] as const;
}

/// The workarea's PR set, ordered `(merge_order, pr_number)` by the Core
/// (Task 319). Empty list when the workarea has no PRs yet.
export function usePrSet(workareaId: string | null | undefined) {
  return useQuery<PullRequest[]>({
    queryKey: prSetQueryKey(workareaId),
    queryFn: async () => {
      if (!workareaId) return [];
      const res = await getWorkareaPrSet(workareaId);
      return res.pull_requests;
    },
    enabled: !!workareaId,
  });
}

/// Drag-to-reorder write. Calls `SetMergeOrder` and primes the PR-set cache
/// with the authoritative re-ordered set the RPC returns (Task 319). The
/// caller applies an optimistic local reorder for snappy UI and reconciles
/// here.
export function useSetMergeOrder(workareaId: string | null | undefined) {
  const queryClient = useQueryClient();
  return useMutation<
    GetWorkareaPrSetResponse,
    unknown,
    { repositoryId: string; mergeOrder: number }
  >({
    mutationFn: async ({ repositoryId, mergeOrder }) => {
      if (!workareaId) throw new Error("no workarea selected");
      return setMergeOrder({ workareaId, repositoryId, mergeOrder });
    },
    onSuccess: (res) => {
      queryClient.setQueryData<PullRequest[]>(
        prSetQueryKey(workareaId),
        res.pull_requests,
      );
    },
  });
}

/// Aggregated PR-set readiness: fetches every member PR's checks (keyed
/// identically to `useChecks` so the per-repo panel's fetch is reused) and
/// reports whether ANY repo in the set has a red check. Drives the
/// disable-on-red gate on "Merge workarea PR set".
///
/// A PR with no `head_sha` (rare; pre-octocrab row) contributes no checks
/// (not treated as red — the gate is conservative on *known* red only).
export function useSetReadiness(
  workareaId: string | null | undefined,
  prs: PullRequest[],
): { anyRed: boolean; isLoading: boolean } {
  const results = useQueries({
    queries: prs.map((pr) => ({
      queryKey: checksQueryKey(workareaId, pr.repository_id),
      queryFn: async () => {
        if (!pr.head_sha) return [];
        const res = await getChecks(pr.repository_id, pr.head_sha);
        return res.checks;
      },
      enabled: !!workareaId && !!pr.repository_id && !!pr.head_sha,
    })),
  });

  const anyRed = results.some((r) => hasRed(r.data ?? []));
  const isLoading = results.some((r) => r.isLoading);
  return { anyRed, isLoading };
}
