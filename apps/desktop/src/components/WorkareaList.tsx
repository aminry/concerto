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

export type WorkareaListProps = {
  workspaceId: string;
};

export function WorkareaList({ workspaceId }: WorkareaListProps): JSX.Element {
  const query = useWorkareas(workspaceId);
  const selectedWorkareaId = useUiStore((s) => s.selectedWorkareaId);
  const setSelectedWorkarea = useUiStore((s) => s.setSelectedWorkarea);

  if (query.isLoading) {
    return <p className="text-xs text-slate-500">Loading workareas…</p>;
  }
  if (query.isError) {
    return (
      <p className="text-xs text-rose-400">
        Failed: {String(query.error)}
      </p>
    );
  }
  if (!query.data || query.data.workareas.length === 0) {
    return <p className="text-xs text-slate-500">No workareas yet.</p>;
  }

  return (
    <ul className="space-y-0.5">
      {query.data.workareas.map((wa) => {
        const active = wa.id === selectedWorkareaId;
        const buttonClass = active
          ? "w-full text-left px-2 py-1 rounded text-xs bg-slate-800 text-slate-100"
          : "w-full text-left px-2 py-1 rounded text-xs text-slate-300 hover:bg-slate-900";
        return (
          <li key={wa.id}>
            <button
              type="button"
              className={buttonClass}
              onClick={() => setSelectedWorkarea(wa.id)}
            >
              <span className="flex items-center gap-2">
                <StatusDot status={wa.status} />
                <span className="truncate">{wa.composer_name}</span>
                <span className="ml-auto text-slate-500 truncate">
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

function StatusDot({ status }: { status: string }): JSX.Element {
  const color = statusColor(status);
  return (
    <span
      className={`inline-block h-2 w-2 rounded-full ${color}`}
      aria-label={`status: ${status}`}
    />
  );
}

function statusColor(status: string): string {
  switch (status) {
    case "active":
      return "bg-green-500";
    case "awaiting":
      return "bg-amber-500";
    case "running":
      return "bg-blue-500";
    default:
      return "bg-gray-400";
  }
}
