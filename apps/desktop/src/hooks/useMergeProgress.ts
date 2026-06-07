// Coordinated-merge driver hook (Task 324, design/03 §6.4 / design/13 §3.5).
//
// Drives `Workareas.MergeWorkareaPrSet` and renders its lifecycle. The merge
// RPC is a server-stream on the wire; with no `src-tauri` streaming command
// for it (this task is `web-ts`, no Rust — see the Handoff drift note), the
// renderer consumes the SAME lifecycle through the `pr_set.events.<wa>` pub/sub
// subject (Task 320: "the stream is the source of truth for the merging client;
// `pr_set.events` is for everyone else"). Each frame is the opaque
// `Event.checks_opaque` JSON the Core builds; `prSetFrameToProgress` normalizes
// it onto the `MergeProgress` shape the UI switches on.
//
// State machine (mirrors `MergeProgress`):
//   idle → running (per step_completed) → merged (set_merged)
//                                       ↘ paused (set_paused / merge_failed_step)
// `paused` is the "Step N of M failed — auto-revert?" surface; the UI offers
// `revert()` (→ `Workareas.RevertWorkareaPrSet`). The subscription is torn down
// on unmount by `useEventSubscription`.

import { useCallback, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import {
  decodeOpaqueFrame,
  mergeWorkareaPrSet,
  parsePrSetFrame,
  prSetFrameToProgress,
  revertWorkareaPrSet,
  type MergeProgress,
  type OpaqueEvent,
  type RevertReport,
} from "../api/vcs";
import { prSetQueryKey } from "./usePrSet";
import { useEventSubscription } from "./useEventSubscription";

export type MergePhase = "idle" | "running" | "merged" | "paused" | "reverted";

export type MergeProgressState = {
  phase: MergePhase;
  /// The latest progress frame (drives the running-step / paused message).
  latest: MergeProgress | null;
  /// 1-based step the merge is paused at (only set in `paused`).
  pausedAtStep: number | null;
  /// Total steps in the plan (from the latest frame).
  total: number | null;
  /// Human reason for the pause (only set in `paused`).
  reason: string | null;
};

const INITIAL: MergeProgressState = {
  phase: "idle",
  latest: null,
  pausedAtStep: null,
  total: null,
  reason: null,
};

/// The opaque event-bus subject for a workarea's PR-set lifecycle.
export function prSetEventsSubject(workareaId: string): string {
  // The Core filters `pr_set.events.<workarea_id>` (Task 320).
  return `pr_set.events.${workareaId}`;
}

export function useMergeProgress(workareaId: string | null | undefined) {
  const queryClient = useQueryClient();
  const [state, setState] = useState<MergeProgressState>(INITIAL);
  const [revertReport, setRevertReport] = useState<RevertReport | null>(null);

  const subject = workareaId ? prSetEventsSubject(workareaId) : "";

  const onFrame = useCallback(
    (payload: OpaqueEvent) => {
      const frame = parsePrSetFrame(decodeOpaqueFrame(payload));
      if (!frame) return;
      // A `reverted` frame is not a MergeProgress arm — mark reverted +
      // refresh the PR set (rows flip back).
      if (frame.kind === "reverted") {
        setState((s) => ({ ...s, phase: "reverted" }));
        void queryClient.invalidateQueries({
          queryKey: prSetQueryKey(workareaId),
        });
        return;
      }
      const progress = prSetFrameToProgress(frame);
      if (!progress) return;
      applyProgress(progress);
      // A completed step / full merge changes PR rows — keep the set fresh.
      if (progress.kind === "step_completed" || progress.kind === "set_merged") {
        void queryClient.invalidateQueries({
          queryKey: prSetQueryKey(workareaId),
        });
      }
    },
    // `applyProgress` is stable (defined below via the setter form).
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [queryClient, workareaId],
  );

  function applyProgress(progress: MergeProgress): void {
    setState((prev) => {
      switch (progress.kind) {
        case "step_started":
          return {
            phase: "running",
            latest: progress,
            pausedAtStep: null,
            total: progress.data.total,
            reason: null,
          };
        case "step_completed":
          return {
            phase: "running",
            latest: progress,
            pausedAtStep: null,
            total: progress.data.total,
            reason: prev.reason,
          };
        case "set_merged":
          return {
            phase: "merged",
            latest: progress,
            pausedAtStep: null,
            total: progress.data.total,
            reason: null,
          };
        case "set_paused":
          return {
            phase: "paused",
            latest: progress,
            pausedAtStep: progress.data.paused_at_step,
            total: progress.data.total,
            reason: progress.data.reason,
          };
        case "step_failed":
          // A bare step_failed (no following set_paused on `pr_set.events`,
          // which collapses both to merge_failed_step) also surfaces the
          // pause prompt.
          return {
            phase: "paused",
            latest: progress,
            pausedAtStep: progress.data.step,
            total: progress.data.total,
            reason: progress.data.reason,
          };
      }
    });
  }

  useEventSubscription<OpaqueEvent>(subject, onFrame);

  /// Kick off the coordinated merge. Resets local state to `running` (the
  /// first `step_started`/`step_completed` frame overwrites it) and fires the
  /// trigger RPC.
  const start = useCallback(
    async (opts?: {
      method?: "merge" | "squash" | "rebase";
      allowFailingChecks?: boolean;
    }) => {
      if (!workareaId) return;
      setState({ ...INITIAL, phase: "running" });
      setRevertReport(null);
      await mergeWorkareaPrSet({
        workareaId,
        method: opts?.method,
        allowFailingChecks: opts?.allowFailingChecks,
      });
    },
    [workareaId],
  );

  /// The "auto-revert?" action on the pause-on-fail prompt. Walks the merged
  /// members in reverse `merge_order` (Task 320).
  const revert = useCallback(
    async (hardReset = false) => {
      if (!workareaId) return;
      const report = await revertWorkareaPrSet(workareaId, hardReset);
      setRevertReport(report);
      setState((s) => ({ ...s, phase: "reverted" }));
      void queryClient.invalidateQueries({
        queryKey: prSetQueryKey(workareaId),
      });
    },
    [workareaId, queryClient],
  );

  const reset = useCallback(() => {
    setState(INITIAL);
    setRevertReport(null);
  }, []);

  return useMemo(
    () => ({ state, revertReport, start, revert, reset }),
    [state, revertReport, start, revert, reset],
  );
}
