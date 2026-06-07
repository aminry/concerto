// React Query hook for `Vcs.GetChecks` + the live `checks.<wa>.<repo>`
// subscription (Task 324, design/15 §3.4 / design/13 §3.4).
//
// The check runs for a PR's `head_sha` are cached keyed
// `["checks", workareaId, repositoryId]` (NOT keyed by sha) so the
// workarea-wide disable-on-red aggregation (`PrSetActions`) can read every
// repo's check state from one cache shape. The 60 s poll cadence
// (`design/13 §3.4`: "review threads / deployments poll every 60 s while the
// Checks panel is viewed") is a React Query `refetchInterval` gated on the
// panel being visible — passed via `enabled`/`pollWhileViewed` by the caller.
//
// Live invalidation: `useChecksSubscription` subscribes to the opaque
// `checks.<wa>.<repo>` subject (Task 316 — rides `Event.checks_opaque`; no
// proto Event arm) and invalidates the cache when a `check_run_updated` frame
// arrives, so a webhook-driven status change re-fetches without waiting for
// the 60 s poll.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";

import { getChecks, type CheckRun } from "../api/vcs";
import { decodeOpaqueFrame, parseChecksFrame, type OpaqueEvent } from "../api/vcs";
import { useEventSubscription } from "./useEventSubscription";

/// 60 s, per `design/13 §3.4` (poll-while-viewed cadence).
export const CHECKS_POLL_INTERVAL_MS = 60_000;

export function checksQueryKey(
  workareaId: string | null | undefined,
  repositoryId: string | null | undefined,
) {
  return ["checks", workareaId, repositoryId] as const;
}

/// The opaque event-bus subject for a `(workarea, repo)` check stream.
export function checksSubject(workareaId: string, repositoryId: string): string {
  return `checks.${workareaId}.${repositoryId}`;
}

/// Fetch the check runs for `sha` on `(workareaId, repositoryId)`.
/// `pollWhileViewed` toggles the 60 s `refetchInterval` (set true only while
/// the Checks panel is on screen). `enabled` short-circuits when ids/sha are
/// absent.
export function useChecks(
  workareaId: string | null | undefined,
  repositoryId: string | null | undefined,
  sha: string | null | undefined,
  pollWhileViewed = false,
) {
  const enabled = !!workareaId && !!repositoryId && !!sha;
  return useQuery<CheckRun[]>({
    queryKey: checksQueryKey(workareaId, repositoryId),
    queryFn: async () => {
      if (!repositoryId || !sha) return [];
      const res = await getChecks(repositoryId, sha);
      return res.checks;
    },
    enabled,
    refetchInterval: pollWhileViewed ? CHECKS_POLL_INTERVAL_MS : false,
  });
}

/// Subscribe to the live `checks.<wa>.<repo>` opaque stream and invalidate the
/// matching React Query cache on a `check_run_updated` frame. Mount this from
/// the Checks panel so a webhook-driven change re-fetches immediately.
export function useChecksSubscription(
  workareaId: string | null | undefined,
  repositoryId: string | null | undefined,
): void {
  const queryClient = useQueryClient();
  const subject =
    workareaId && repositoryId ? checksSubject(workareaId, repositoryId) : "";

  const onFrame = useCallback(
    (payload: OpaqueEvent) => {
      const frame = parseChecksFrame(decodeOpaqueFrame(payload));
      if (!frame) return;
      // Any check-run change for this (wa, repo) invalidates the cached set.
      if (frame.kind === "check_run_updated") {
        void queryClient.invalidateQueries({
          queryKey: checksQueryKey(workareaId, repositoryId),
        });
      }
      // thread_updated / deployment_updated invalidate the threads query the
      // panel mounts separately (keyed by pr); the panel re-reads via its own
      // ListReviewThreads fetch on invalidation — handled by the panel.
    },
    [queryClient, workareaId, repositoryId],
  );

  // Empty subject ⇒ the hook no-ops (the underlying effect guards on it).
  useEventSubscription<OpaqueEvent>(subject, onFrame);
}
