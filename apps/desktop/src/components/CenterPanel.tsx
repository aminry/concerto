// Center panel of the three-panel layout (Task 46).
//
// The session terminal (`SessionRegion`: session tab strip + xterm panel
// + composer) fills the full center height. The Code & PRs surface
// (Diff / Checks / PR) used to sit in a vertical split below it; it now
// lives in the right rail (`RightRail.tsx`) so the terminal is no longer
// height-constrained.
//
// The header carries the workarea composer + branch chip + status dot
// (moved here from Task 26's `WorkareaDetail`).

import { useUiStore } from "../state/useUiStore";
import { useWorkarea } from "../hooks/useWorkareas";
import { SessionRegion } from "./center/SessionRegion";
import { StatusDot } from "./ui/status-dot";
import { workareaStatusToDot } from "../lib/workareaStatus";

export function CenterPanel(): JSX.Element {
  const workareaId = useUiStore((s) => s.selectedWorkareaId);
  const workareaQuery = useWorkarea(workareaId);
  const workarea = workareaQuery.data ?? null;

  if (!workareaId) {
    return (
      <main className="h-full flex items-center justify-center p-6 text-muted text-sm">
        Select a workarea to start a session.
      </main>
    );
  }

  return (
    <main className="h-full flex flex-col min-h-0">
      <header className="shrink-0 border-b border-border px-3 py-2">
        <div className="flex items-center gap-2 min-w-0">
          {workarea && <StatusDot status={workareaStatusToDot(workarea.status)} />}
          <h2 className="text-sm font-semibold text-foreground truncate">
            {workarea?.composer_name ?? "Workarea"}
          </h2>
          {workarea && (
            <>
              <span className="text-xs px-1.5 py-0.5 rounded bg-surface-2 text-muted font-mono">
                {workarea.branch_name}
              </span>
              <span className="text-xs text-faint">{workarea.status}</span>
            </>
          )}
        </div>
      </header>
      <div className="flex-1 min-h-0">
        <SessionRegion workareaId={workareaId} />
      </div>
    </main>
  );
}
