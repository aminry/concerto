// Right-rail Scheduler tab — lists `/loop` schedules for the selected
// workarea (Task 38 surface). V0.1 is read-only; create / pause / delete
// affordances arrive with the Maestro work in a later phase.

import { formatError } from "../../api/errors";
import { useUiStore } from "../../state/useUiStore";
import { useSchedules } from "../../hooks/useSchedules";

export function SchedulerTab(): JSX.Element {
  const workareaId = useUiStore((s) => s.selectedWorkareaId);
  const query = useSchedules(workareaId);

  if (!workareaId) {
    return (
      <p className="text-xs text-faint p-3">
        Select a workarea to see its schedules.
      </p>
    );
  }
  if (query.isLoading) {
    return <p className="text-xs text-faint p-3">Loading…</p>;
  }
  if (query.isError) {
    return (
      <p className="text-xs text-err p-3">
        Failed to load schedules: {formatError(query.error)}
      </p>
    );
  }
  const schedules = query.data?.schedules ?? [];
  if (schedules.length === 0) {
    return (
      <p className="text-xs text-faint p-3">
        No schedules yet. Use <span className="font-mono text-accent">/loop</span> in a session to add one.
      </p>
    );
  }
  return (
    <ul className="p-2 space-y-1">
      {schedules.map((s) => (
        <li
          key={s.id}
          className="rounded border border-border bg-surface-2 px-2 py-1.5"
        >
          <div className="flex items-center gap-2">
            <span
              className={`inline-block h-2 w-2 rounded-full ${
                s.paused ? "bg-faint" : "bg-accent"
              }`}
              aria-label={s.paused ? "paused" : "active"}
            />
            <span className="text-xs font-mono text-foreground">{s.kind}</span>
            <span className="ml-auto text-xs text-faint">
              every {s.interval_seconds}s
            </span>
          </div>
          <p className="mt-1 text-xs text-muted truncate">{s.prompt}</p>
        </li>
      ))}
    </ul>
  );
}
