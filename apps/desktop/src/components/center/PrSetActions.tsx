// Workarea-wide coordinated-merge actions (Task 324, design/15 §3.4 +
// design/03 §6.4).
//
// Rendered above the Level-1 repo selector. Surfaces:
//  - "Create PRs for all dirty repos" — one `Vcs.CreatePullRequest` per repo
//    with changes that has no PR yet.
//  - "Merge workarea PR set" — DISABLED when ANY repo in the set has a red
//    check (aggregated across the set via `useSetReadiness`, not a single PR).
//    Drives `Workareas.MergeWorkareaPrSet` (consumed via `pr_set.events`).
//  - "Revert workarea PR set" — `Workareas.RevertWorkareaPrSet` (reverse
//    `merge_order`).
//  - The PR-set list, ordered by `merge_order`, with drag-to-reorder that
//    writes `Workareas.SetMergeOrder` (Task 319, D7 — pure manual reorder, no
//    dependency-graph inference).
//  - The running-step UI + the pause-on-fail "Step N of M failed —
//    auto-revert?" prompt from the merge lifecycle.

import { useCallback, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  createPullRequest,
  type PullRequest,
} from "../../api/vcs";
import { errorMessage } from "../../api/client";
import { usePrSet, useSetMergeOrder, useSetReadiness, prSetQueryKey } from "../../hooks/usePrSet";
import { useMergeProgress } from "../../hooks/useMergeProgress";
import type { Repository } from "../../api/repositories";

export type PrSetActionsProps = {
  workareaId: string;
  /// The workarea's repos (Level-1 selector source) — used to find dirty repos
  /// without a PR for "Create PRs for all".
  repos: Repository[];
  /// Per-repo changed-file counts (from the diff probes the parent already
  /// runs), keyed by repository id. A repo with >0 changed files is "dirty".
  dirtyByRepo: Record<string, boolean>;
};

export function PrSetActions({
  workareaId,
  repos,
  dirtyByRepo,
}: PrSetActionsProps): JSX.Element {
  const queryClient = useQueryClient();
  const prSetQuery = usePrSet(workareaId);
  const prs = prSetQuery.data ?? [];
  const { anyRed } = useSetReadiness(workareaId, prs);
  const merge = useMergeProgress(workareaId);

  const reposWithPr = new Set(prs.map((p) => p.repository_id));
  const dirtyWithoutPr = repos.filter(
    (r) => dirtyByRepo[r.id] && !reposWithPr.has(r.id),
  );

  const createAllMutation = useMutation({
    mutationFn: async () => {
      for (const repo of dirtyWithoutPr) {
        await createPullRequest({
          workareaId,
          repositoryId: repo.id,
          head: `concerto/${repo.name}`,
          title: `Changes in ${repo.name}`,
        });
      }
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: prSetQueryKey(workareaId) });
    },
  });

  const phase = merge.state.phase;
  const merging = phase === "running";

  return (
    <div className="shrink-0 flex flex-col gap-2 border-b border-border pb-2">
      <div className="flex items-center gap-2 flex-wrap">
        <button
          type="button"
          onClick={() => createAllMutation.mutate()}
          disabled={createAllMutation.isPending || dirtyWithoutPr.length === 0}
          className="px-2.5 py-1 text-xs rounded-md bg-surface-2 text-foreground hover:opacity-80 disabled:opacity-50"
        >
          {createAllMutation.isPending
            ? "Creating…"
            : `Create PRs for all dirty repos${
                dirtyWithoutPr.length ? ` (${dirtyWithoutPr.length})` : ""
              }`}
        </button>

        <button
          type="button"
          onClick={() => void merge.start()}
          disabled={anyRed || merging || prs.length === 0}
          title={
            anyRed
              ? "A repo in this set has a failing check"
              : "Merge every PR in the set in order"
          }
          data-testid="merge-pr-set"
          className="px-2.5 py-1 text-xs rounded-md bg-accent text-accent-fg disabled:opacity-50"
        >
          {merging ? "Merging…" : "Merge workarea PR set"}
        </button>

        <button
          type="button"
          onClick={() => void merge.revert()}
          disabled={prs.length === 0 || merging}
          className="px-2.5 py-1 text-xs rounded-md bg-surface-2 text-foreground hover:opacity-80 disabled:opacity-50"
        >
          Revert workarea PR set
        </button>
      </div>

      {createAllMutation.isError ? (
        <p className="text-xs text-err" role="alert">
          {errorMessage(createAllMutation.error)}
        </p>
      ) : null}

      {anyRed ? (
        <p className="text-xs text-warn" data-testid="red-checks-warning">
          A repo in this set has a failing check — resolve it before merging the
          set.
        </p>
      ) : null}

      <MergeProgressView
        phase={phase}
        state={merge.state}
        onAutoRevert={() => void merge.revert()}
      />

      {prs.length > 0 ? (
        <PrSetList workareaId={workareaId} prs={prs} />
      ) : (
        <p className="text-xs text-faint">No PRs in this workarea yet.</p>
      )}
    </div>
  );
}

/// The running-step + pause-on-fail surface (design/03 §6.4).
function MergeProgressView({
  phase,
  state,
  onAutoRevert,
}: {
  phase: ReturnType<typeof useMergeProgress>["state"]["phase"];
  state: ReturnType<typeof useMergeProgress>["state"];
  onAutoRevert: () => void;
}): JSX.Element | null {
  if (phase === "idle") return null;

  if (phase === "running") {
    const latest = state.latest;
    const step =
      latest?.kind === "step_started" || latest?.kind === "step_completed"
        ? latest.data.step ?? null
        : null;
    return (
      <p className="text-xs text-foreground" data-testid="merge-running">
        Merging
        {step && state.total ? ` — step ${step} of ${state.total}` : "…"}
      </p>
    );
  }

  if (phase === "merged") {
    return (
      <p className="text-xs text-ok" data-testid="merge-merged">
        Merged all {state.total ?? ""} PRs.
      </p>
    );
  }

  if (phase === "reverted") {
    return (
      <p className="text-xs text-faint" data-testid="merge-reverted">
        PR set reverted.
      </p>
    );
  }

  // paused — the "Step N of M failed — auto-revert?" prompt.
  return (
    <div
      className="flex flex-col gap-1.5 rounded border border-err/50 bg-err/5 p-2"
      data-testid="merge-paused"
      role="alert"
    >
      <p className="text-xs text-err">
        Step {state.pausedAtStep ?? "?"} of {state.total ?? "?"} failed
        {state.reason ? ` — ${state.reason}` : ""}.
      </p>
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={onAutoRevert}
          data-testid="auto-revert"
          className="px-2.5 py-1 text-xs rounded-md bg-err text-accent-fg hover:opacity-80"
        >
          Auto-revert
        </button>
        <span className="text-xs text-faint">
          Reverts the merged members in reverse order.
        </span>
      </div>
    </div>
  );
}

/// The ordered PR-set list with HTML5 drag-to-reorder. On drop, the dragged
/// PR's `merge_order` is set to the target's so the Core re-sorts; the
/// returned set seeds the cache (`useSetMergeOrder`).
function PrSetList({
  workareaId,
  prs,
}: {
  workareaId: string;
  prs: PullRequest[];
}): JSX.Element {
  const setOrder = useSetMergeOrder(workareaId);
  const [dragIndex, setDragIndex] = useState<number | null>(null);

  const onDrop = useCallback(
    (targetIndex: number) => {
      if (dragIndex == null || dragIndex === targetIndex) {
        setDragIndex(null);
        return;
      }
      const dragged = prs[dragIndex];
      const target = prs[targetIndex];
      setDragIndex(null);
      if (!dragged || !target) return;
      // Write the dragged PR to the target's merge_order; the Core resolves the
      // resulting (merge_order, pr_number) order and returns the canonical set.
      setOrder.mutate({
        repositoryId: dragged.repository_id,
        mergeOrder: target.merge_order,
      });
    },
    [dragIndex, prs, setOrder],
  );

  return (
    <ul className="flex flex-col gap-1" aria-label="PR set merge order">
      {prs.map((pr, i) => (
        <li
          key={pr.id}
          draggable
          onDragStart={() => setDragIndex(i)}
          onDragOver={(e) => e.preventDefault()}
          onDrop={() => onDrop(i)}
          data-testid="pr-set-row"
          data-repo={pr.repository_id}
          className="flex items-center gap-2 text-xs px-2 py-1 rounded bg-surface-2 cursor-grab"
        >
          <span className="text-faint font-mono w-5 shrink-0">{i + 1}.</span>
          <span className="font-mono truncate">
            {pr.repository_full_name || pr.repository_id}
          </span>
          <span className="text-faint ml-auto shrink-0">#{pr.pr_number}</span>
        </li>
      ))}
    </ul>
  );
}
