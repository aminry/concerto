// Workarea sub-tree — the third level of the sidebar.
//
// Lazy-fetches via `useWorkareas` when a workspace node is expanded;
// the React Query gate (`enabled: !!workspaceId`) handles the "don't
// fetch until needed" rule from `tasks/25 §Implementation notes`.
//
// Status dot colors mirror `design/15 §3.4`:
//   - active   → green
//   - awaiting → amber
//   - running  → blue
//   - everything else (created, paused, archived, crashed) → grey

import { useWorkareas } from "../hooks/useWorkareas";
import { useUiStore } from "../state/useUiStore";
import { StatusDot, type DotStatus } from "./ui/status-dot";

export type WorkareaListProps = {
  workspaceId: string;
};

export function WorkareaList({ workspaceId }: WorkareaListProps): JSX.Element {
  const query = useWorkareas(workspaceId);
  const selectedWorkareaId = useUiStore((s) => s.selectedWorkareaId);
  const setSelectedWorkarea = useUiStore((s) => s.setSelectedWorkarea);

  if (query.isLoading) {
    return <p className="text-xs text-faint">Loading workareas…</p>;
  }
  if (query.isError) {
    return (
      <p className="text-xs text-err">
        Failed: {String(query.error)}
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
          ? "w-full text-left px-2 py-1 rounded-md text-xs bg-accent/10 text-foreground"
          : "w-full text-left px-2 py-1 rounded-md text-xs text-muted hover:bg-surface-2";
        return (
          <li key={wa.id}>
            <button
              type="button"
              className={buttonClass}
              onClick={() => setSelectedWorkarea(wa.id)}
            >
              <span className="flex items-center gap-2">
                <StatusDot status={statusToDot(wa.status)} />
                <span className="truncate">{wa.composer_name}</span>
                <span className="ml-auto text-faint truncate font-mono">
                  {wa.branch_name}
                </span>
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}

// Maps a `concerto.v1.Workarea` status string to a `DotStatus`.
// Workarea statuses ∈ { created | active | running | awaiting |
// paused | archived | crashed }. Unknown values fall back to "idle".
function statusToDot(status: string): DotStatus {
  switch (status) {
    // `active` reads as green (the workarea is live/healthy) per
    // design/15 §3.4; `running` (an agent actively executing) is blue.
    case "active":
    case "ok":
      return "ok";
    case "running":
    case "starting":
      return "running";
    case "awaiting":
    case "blocked":
      return "warning";
    case "failed":
    case "error":
    case "crashed":
      return "error";
    case "archived":
    case "idle":
    case "done":
    case "stopped":
    case "paused":
    case "created":
      return "idle";
    default:
      return "idle";
  }
}
