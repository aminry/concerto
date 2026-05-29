// Top-level three-panel layout per `design/15 §3.4` (Task 46).
//
// Horizontal split: sidebar | center | right-rail. The widths are
// persisted in the Zustand store under `sidebarWidth` and
// `rightRailCollapsed`; the App root debounces them to `localStorage`.
//
// When a workspace is selected without a workarea, the center panel
// degrades to the Task 25 JSON workspace view so the existing flow
// (create workspace → create workarea) keeps working. Once a workarea
// is selected, the three-panel layout takes over.

import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";

import { Sidebar } from "./Sidebar";
import { CenterPanel } from "./CenterPanel";
import { WorkspaceDetail } from "./WorkspaceDetail";
import { RightRail } from "./RightRail";
import { useUiStore } from "../state/useUiStore";

const RIGHT_RAIL_WIDTH = 22;

export function AppLayout(): JSX.Element {
  const selectedWorkareaId = useUiStore((s) => s.selectedWorkareaId);
  const sidebarWidth = useUiStore((s) => s.sidebarWidth);
  const setSidebarWidth = useUiStore((s) => s.setSidebarWidth);
  const rightRailCollapsed = useUiStore((s) => s.rightRailCollapsed);

  return (
    <PanelGroup
      direction="horizontal"
      onLayout={(sizes) => {
        if (sizes[0] !== undefined) setSidebarWidth(sizes[0]);
      }}
    >
      <Panel defaultSize={sidebarWidth} minSize={12} maxSize={40}>
        <Sidebar />
      </Panel>
      <PanelResizeHandle className="w-1 bg-slate-800 hover:bg-slate-700 transition-colors" />
      <Panel minSize={30}>
        {selectedWorkareaId ? <CenterPanel /> : <WorkspaceDetail />}
      </Panel>
      {!rightRailCollapsed && (
        <PanelResizeHandle className="w-1 bg-slate-800 hover:bg-slate-700 transition-colors" />
      )}
      <Panel
        defaultSize={rightRailCollapsed ? 3 : RIGHT_RAIL_WIDTH}
        minSize={rightRailCollapsed ? 3 : 12}
        maxSize={rightRailCollapsed ? 3 : 40}
      >
        <RightRail />
      </Panel>
    </PanelGroup>
  );
}
