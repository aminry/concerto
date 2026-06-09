// "When a workspace is selected" summary view (Task 323, design/15 §3.4).
//
// Replaces the V0.1 `JSON.stringify` dump in `WorkspaceDetail` with the
// design's workspace summary: the workspace's workareas as a list of rows
// (composer name + branch chip + status dot), a cross-workarea PR-set
// status column, and a "+ new workarea" affordance. Clicking a row selects
// the workarea (`setSelectedWorkarea`), which switches `AppLayout` to the
// three-panel workarea view.
//
// The summary lets a user compare parallel attempts on one workspace at a
// glance — the V1.0 "parallel workareas" capability the Core permits
// (Tasks 306/307) but the UI never exposed.
//
// Reuse, not re-implementation:
//   - the status dot comes from the shared `workareaStatusToDot` mapper
//     (lib/workareaStatus.ts) so the sidebar tree and this summary always
//     agree on colors (the single-source-of-truth rule). It already maps
//     Task 307's `finished`/`partial` statuses.
//   - the row layout mirrors `WorkareaList.tsx`'s composer-name + branch
//     chip + dot so the two surfaces read identically.
//
// Live updates: a `workarea.events` frame invalidates the per-workspace
// workarea list, mirroring `Sidebar.tsx`. (The query key is scoped to this
// workspace; the broad invalidate matches `Sidebar`'s pattern and refetches
// the visible list.)
//
// Cross-workarea PR-set status (the right-hand column) is a typed
// PLACEHOLDER. The real cross-workarea PR-set aggregation is Task 324; this
// task renders a neutral "—" / "no PRs" slot and binds NO PR-set RPC. Task
// 324 fills `renderPrSetStatus` (or replaces the column) with the live
// aggregation.

import { useQueryClient } from "@tanstack/react-query";

import { formatError } from "../api/errors";
import { useWorkareas } from "../hooks/useWorkareas";
import { useUiStore } from "../state/useUiStore";
import { useEventSubscription } from "../hooks/useEventSubscription";
import { StatusDot } from "./ui/status-dot";
import { workareaStatusToDot } from "../lib/workareaStatus";
import type { Workarea } from "../api/workareas";

export type WorkspaceSummaryProps = {
  /// The selected workspace whose parallel workareas this summarizes.
  workspaceId: string;
  /// Invoked by the "+ new workarea" affordance. The actual create flow
  /// (and, post-Task-322, the sparse-cone picker) is owned by the parent
  /// `WorkspaceDetail` so the two tasks don't both own the button.
  onNewWorkarea: () => void;
};

export function WorkspaceSummary({
  workspaceId,
  onNewWorkarea,
}: WorkspaceSummaryProps): JSX.Element {
  const query = useWorkareas(workspaceId);
  const selectedWorkareaId = useUiStore((s) => s.selectedWorkareaId);
  const setSelectedWorkarea = useUiStore((s) => s.setSelectedWorkarea);
  const queryClient = useQueryClient();

  // Live-update the list when the Core emits a workarea lifecycle event,
  // mirroring `Sidebar.tsx`. Invalidating the whole `["workareas", …]`
  // family is cheap and matches the sidebar's behaviour.
  useEventSubscription("workarea.events", () => {
    void queryClient.invalidateQueries({
      queryKey: ["workareas", workspaceId],
    });
  });

  const workareas = query.data?.workareas ?? [];

  return (
    <section className="space-y-3" aria-label="Workspace summary">
      <header className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-semibold uppercase tracking-wide text-muted">
          Workareas
        </h2>
        <button
          type="button"
          onClick={onNewWorkarea}
          className="inline-flex items-center gap-1.5 rounded-md bg-accent hover:bg-accent-hover text-accent-fg px-2.5 py-1 text-xs font-medium"
        >
          + new workarea
        </button>
      </header>

      {query.isLoading && (
        <p className="text-xs text-faint">Loading workareas…</p>
      )}
      {query.isError && (
        <p className="text-xs text-err">Failed: {formatError(query.error)}</p>
      )}
      {!query.isLoading && !query.isError && workareas.length === 0 && (
        <p className="text-xs text-faint">
          No workareas yet. Create one to start a parallel attempt.
        </p>
      )}

      {workareas.length > 0 && (
        <ul className="space-y-1">
          {workareas.map((wa) => {
            const active = wa.id === selectedWorkareaId;
            const rowClass = active
              ? "w-full text-left px-2.5 py-2 rounded-md text-xs bg-accent/10 text-foreground"
              : "w-full text-left px-2.5 py-2 rounded-md text-xs text-muted hover:bg-surface-2";
            return (
              <li key={wa.id}>
                <button
                  type="button"
                  className={rowClass}
                  onClick={() => setSelectedWorkarea(wa.id)}
                >
                  <span className="flex items-center gap-2">
                    <StatusDot status={workareaStatusToDot(wa.status)} />
                    <span className="truncate font-medium">
                      {wa.composer_name}
                    </span>
                    <span className="text-faint truncate font-mono">
                      {wa.branch_name}
                    </span>
                    <span
                      className="ml-auto shrink-0 text-faint"
                      title="Cross-workarea PR-set status (Task 324)"
                    >
                      {renderPrSetStatus(wa)}
                    </span>
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

/// Cross-workarea PR-set status slot (design/15 §3.4). PLACEHOLDER: the
/// real aggregation across a workspace's parallel workareas is Task 324;
/// here we render a neutral em-dash and bind no PR-set RPC. Task 324
/// replaces this with the live PR-set state.
function renderPrSetStatus(_workarea: Workarea): string {
  return "—";
}
