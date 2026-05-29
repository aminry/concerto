// Center panel of the three-panel layout (Task 46).
//
// Vertically split into two resizable regions per `design/15 §3.4`:
//
//   - Top    → `SessionRegion` (session tab strip + xterm panel + composer).
//   - Bottom → `CodePrRegion` (per-repo tabs with Diff / Checks / PR sub-tabs).
//
// The header above the split carries the workarea composer + branch
// chip + status dot (moved here from Task 26's `WorkareaDetail`). The
// vertical split persists its size in the store via the App-root
// effect; the default is 55/45 per the design diagram.

import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";

import { useUiStore } from "../state/useUiStore";
import { useWorkarea } from "../hooks/useWorkareas";
import { SessionRegion } from "./center/SessionRegion";
import { CodePrRegion } from "./center/CodePrRegion";

export function CenterPanel(): JSX.Element {
  const workareaId = useUiStore((s) => s.selectedWorkareaId);
  const sessionRegionHeight = useUiStore((s) => s.sessionRegionHeight);
  const setSessionRegionHeight = useUiStore((s) => s.setSessionRegionHeight);
  const workareaQuery = useWorkarea(workareaId);
  const workarea = workareaQuery.data ?? null;

  if (!workareaId) {
    return (
      <main className="h-full flex items-center justify-center p-6 text-slate-400 text-sm">
        Select a workarea to start a session.
      </main>
    );
  }

  return (
    <main className="h-full flex flex-col min-h-0">
      <header className="shrink-0 border-b border-slate-800 px-3 py-2">
        <div className="flex items-center gap-2 min-w-0">
          {workarea && <StatusDot status={workarea.status} />}
          <h2 className="text-sm font-semibold text-slate-200 truncate">
            {workarea?.composer_name ?? "Workarea"}
          </h2>
          {workarea && (
            <>
              <span className="text-xs px-1.5 py-0.5 rounded bg-slate-800 text-slate-300 font-mono">
                {workarea.branch_name}
              </span>
              <span className="text-xs text-slate-500">{workarea.status}</span>
            </>
          )}
        </div>
      </header>
      <div className="flex-1 min-h-0">
        <PanelGroup
          direction="vertical"
          onLayout={(sizes) => {
            if (sizes[0] !== undefined) setSessionRegionHeight(sizes[0]);
          }}
        >
          <Panel defaultSize={sessionRegionHeight} minSize={20}>
            <SessionRegion workareaId={workareaId} />
          </Panel>
          <PanelResizeHandle className="h-1 bg-slate-800 hover:bg-slate-700 transition-colors" />
          <Panel minSize={15}>
            <CodePrRegion workarea={workarea} />
          </Panel>
        </PanelGroup>
      </div>
    </main>
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
