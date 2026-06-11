// Workarea sub-tree — the third level of the sidebar.
//
// Lazy-fetches via `useWorkareas` when a workspace node is expanded;
// the React Query gate (`enabled: !!workspaceId`) handles the "don't
// fetch until needed" rule from `tasks/25 §Implementation notes`.
//
// Status dot colors mirror `design/15 §3.4` — see the shared mapper in
// `lib/workareaStatus.ts`:
//   - active   → green
//   - running  → blue
//   - awaiting → amber
//   - crashed  → red
//   - created | paused | archived → grey
//
// ── Per-workarea Maestro-visibility toggle (Task 417, design/08 §3.3) ─────────
// Each row carries an Eye/EyeOff menu calling `Maestro.SetWorkareaVisibility`
// (FULL ⇔ HARD_FACTS_ONLY). HARD_FACTS_ONLY sets the workarea's
// `exclude_from_maestro` flag (Task 311/413 own the server-side blanking; this
// only drives the toggle + reflects state via a "private" badge). The mutation
// is optimistic and invalidates the workarea list on settle.

import { Eye, EyeOff } from "lucide-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { formatError } from "../api/errors";
import {
  MaestroVisibility,
  setWorkareaVisibility,
  type MaestroVisibilityValue,
} from "../api/maestro";
import type { ListWorkareasResponse, Workarea } from "../api/workareas";
import { useWorkareas } from "../hooks/useWorkareas";
import { useUiStore } from "../state/useUiStore";
import { Badge } from "./ui/badge";
import { Menu } from "./ui/menu";
import { StatusDot } from "./ui/status-dot";
import { workareaStatusToDot } from "../lib/workareaStatus";

export type WorkareaListProps = {
  workspaceId: string;
};

export function WorkareaList({
  workspaceId,
}: WorkareaListProps): JSX.Element {
  const query = useWorkareas(workspaceId);
  const selectedWorkareaId = useUiStore((s) => s.selectedWorkareaId);
  const setSelectedWorkarea = useUiStore((s) => s.setSelectedWorkarea);

  if (query.isLoading) {
    return <p className="text-xs text-faint">Loading workareas…</p>;
  }
  if (query.isError) {
    return (
      <p className="text-xs text-err">
        Failed: {formatError(query.error)}
      </p>
    );
  }
  if (!query.data || query.data.workareas.length === 0) {
    return <p className="text-xs text-faint">No workareas yet.</p>;
  }

  return (
    <ul className="space-y-0.5">
      {query.data.workareas.map((wa) => {
        const active = wa.id === selectedWorkareaId;
        const buttonClass = active
          ? "flex-1 min-w-0 text-left px-2 py-1 rounded-md text-xs bg-accent/10 text-foreground"
          : "flex-1 min-w-0 text-left px-2 py-1 rounded-md text-xs text-muted hover:bg-surface-2";
        const isPrivate = !!wa.exclude_from_maestro;
        return (
          <li key={wa.id} className="flex items-center gap-1">
            <button
              type="button"
              className={buttonClass}
              onClick={() => {
                setSelectedWorkarea(wa.id);
              }}
            >
              <span className="flex items-center gap-2">
                <StatusDot status={workareaStatusToDot(wa.status)} />
                <span className="truncate">{wa.composer_name}</span>
                {isPrivate && (
                  <Badge
                    variant="neutral"
                    className="text-faint"
                    data-testid={`private-badge-${wa.id}`}
                  >
                    private
                  </Badge>
                )}
                <span className="ml-auto text-faint truncate font-mono">
                  {wa.branch_name}
                </span>
              </span>
            </button>
            <WorkareaVisibilityMenu workspaceId={workspaceId} workarea={wa} />
          </li>
        );
      })}
    </ul>
  );
}

/// The per-row Maestro-visibility control. An Eye/EyeOff trigger opening a menu
/// with the two `MaestroVisibility` options; the current visibility is
/// reflected by the icon (open eye ⇒ FULL/visible, crossed eye ⇒
/// HARD_FACTS_ONLY/private). Selecting an option fires
/// `Maestro.SetWorkareaVisibility` optimistically (the row's `private` badge
/// flips immediately) and invalidates the workarea list on settle so the
/// server-derived `exclude_from_maestro` reconciles.
function WorkareaVisibilityMenu({
  workspaceId,
  workarea,
}: {
  workspaceId: string;
  workarea: Workarea;
}): JSX.Element {
  const queryClient = useQueryClient();
  const queryKey = ["workareas", workspaceId] as const;
  const isPrivate = !!workarea.exclude_from_maestro;

  const mutation = useMutation({
    mutationFn: (visibility: MaestroVisibilityValue) =>
      setWorkareaVisibility(workarea.id, visibility),
    onMutate: async (visibility) => {
      await queryClient.cancelQueries({ queryKey });
      const previous =
        queryClient.getQueryData<ListWorkareasResponse>(queryKey);
      const exclude = visibility === MaestroVisibility.HARD_FACTS_ONLY;
      queryClient.setQueryData<ListWorkareasResponse>(queryKey, (old) =>
        old
          ? {
              ...old,
              workareas: old.workareas.map((w) =>
                w.id === workarea.id
                  ? { ...w, exclude_from_maestro: exclude }
                  : w,
              ),
            }
          : old,
      );
      return { previous };
    },
    onError: (_e, _v, ctx) => {
      if (ctx?.previous) queryClient.setQueryData(queryKey, ctx.previous);
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey });
    },
  });

  return (
    <Menu
      align="right"
      label={`Maestro visibility for ${workarea.composer_name}`}
      trigger={() => (
        <span
          className="grid h-6 w-6 place-items-center rounded-md text-faint hover:text-accent hover:bg-surface-2"
          title={
            isPrivate
              ? "Maestro: private (name + hard facts only)"
              : "Maestro: visible (full summaries)"
          }
          data-testid={`visibility-trigger-${workarea.id}`}
        >
          {isPrivate ? <EyeOff size={13} /> : <Eye size={13} />}
        </span>
      )}
      items={[
        {
          id: "full",
          label: "Visible to Concerto chat",
          description: "full summaries",
        },
        {
          id: "private",
          label: "Private",
          description: "name + hard facts only",
        },
      ]}
      onSelect={(id) =>
        mutation.mutate(
          id === "private"
            ? MaestroVisibility.HARD_FACTS_ONLY
            : MaestroVisibility.FULL,
        )
      }
    />
  );
}
