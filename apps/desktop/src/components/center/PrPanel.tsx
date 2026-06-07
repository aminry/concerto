// Level-2 PR panel (Task 324, design/15 §3.4).
//
// Replaces the "Pull-request panel arrives with the GitHub surface" stub.
// - No PR for the selected repo → a "Create PR" button
//   (`Vcs.CreatePullRequest`, head = the workarea branch).
// - A PR exists → its state + "Merge PR" (`Vcs.MergePullRequest`) + "Open in
//   browser" (the PR `url`).
//
// "Mark ready for review" (design/15 §3.4) has NO wire support on the FROZEN
// `Vcs` surface (no `MarkReady`/draft-toggle RPC), so it's a documented
// follow-on, not built. Per-PR base/labels editing is out (V1.5+).

import { useMutation, useQueryClient } from "@tanstack/react-query";

import {
  createPullRequest,
  mergePullRequest,
  type PullRequest,
} from "../../api/vcs";
import { errorMessage } from "../../api/client";
import { prSetQueryKey } from "../../hooks/usePrSet";
import { StatusDot, type DotStatus } from "../ui/status-dot";

export type PrPanelProps = {
  workareaId: string;
  repositoryId: string | null;
  /// The repo's branch (the agent-pushed `head` for Create PR). Falls back to
  /// the workarea branch the caller resolves.
  headBranch: string | null;
  /// The repo's PR within the workarea set, or null when none exists.
  pr: PullRequest | null;
};

const STATE_DOT: Record<string, DotStatus> = {
  open: "running",
  draft: "idle",
  merged: "ok",
  closed: "error",
};

export function PrPanel({
  workareaId,
  repositoryId,
  headBranch,
  pr,
}: PrPanelProps): JSX.Element {
  const queryClient = useQueryClient();

  const createMutation = useMutation({
    mutationFn: async () => {
      if (!repositoryId || !headBranch) {
        throw new Error("no repo / branch to open a PR from");
      }
      return createPullRequest({
        workareaId,
        repositoryId,
        head: headBranch,
        title: `Changes from ${headBranch}`,
      });
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: prSetQueryKey(workareaId) });
    },
  });

  const mergeMutation = useMutation({
    mutationFn: async () => {
      if (!pr) throw new Error("no PR to merge");
      await mergePullRequest(pr.repository_id, pr.pr_number);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: prSetQueryKey(workareaId) });
    },
  });

  if (!repositoryId) {
    return <Empty>Select a repo to view its pull request.</Empty>;
  }

  if (!pr) {
    return (
      <div className="flex flex-col gap-2 p-3">
        <p className="text-xs text-faint">No pull request for this repo yet.</p>
        <button
          type="button"
          onClick={() => createMutation.mutate()}
          disabled={createMutation.isPending || !headBranch}
          className="self-start px-2.5 py-1 text-xs rounded-md bg-accent text-accent-fg disabled:opacity-50"
        >
          {createMutation.isPending ? "Creating…" : "Create PR"}
        </button>
        {createMutation.isError ? (
          <p className="text-xs text-err" role="alert">
            {errorMessage(createMutation.error)}
          </p>
        ) : null}
      </div>
    );
  }

  const isMerged = pr.state === "merged";

  return (
    <div className="flex flex-col gap-2 p-3">
      <div className="flex items-center gap-2">
        <StatusDot status={STATE_DOT[pr.state] ?? "idle"} />
        <span className="text-sm font-medium truncate" title={pr.title}>
          {pr.title}
        </span>
      </div>
      <div className="flex items-center gap-2 text-xs text-faint font-mono">
        <span data-testid="pr-state">#{pr.pr_number} · {pr.state}</span>
        <span className="truncate">{pr.head_ref} → {pr.base_ref}</span>
      </div>

      <div className="flex items-center gap-2 mt-1">
        <button
          type="button"
          onClick={() => mergeMutation.mutate()}
          disabled={mergeMutation.isPending || isMerged}
          className="px-2.5 py-1 text-xs rounded-md bg-accent text-accent-fg disabled:opacity-50"
        >
          {mergeMutation.isPending ? "Merging…" : isMerged ? "Merged" : "Merge PR"}
        </button>
        <button
          type="button"
          onClick={() => {
            if (pr.url) window.open(pr.url, "_blank", "noopener,noreferrer");
          }}
          disabled={!pr.url}
          className="px-2.5 py-1 text-xs rounded-md bg-surface-2 text-foreground hover:opacity-80 disabled:opacity-50"
        >
          Open in browser
        </button>
      </div>
      {mergeMutation.isError ? (
        <p className="text-xs text-err" role="alert">
          {errorMessage(mergeMutation.error)}
        </p>
      ) : null}
    </div>
  );
}

function Empty({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <div className="h-full flex items-center justify-center text-xs text-faint p-3">
      {children}
    </div>
  );
}
