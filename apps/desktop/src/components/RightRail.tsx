// Right rail — vertical tab strip with a collapsible drawer per
// `design/15 §3.4`.
//
// The strip is always visible; clicking a tab when the drawer is
// collapsed reopens it on that tab. Clicking the active tab again
// collapses the drawer. The Task 46 V0.1 set of tabs is fixed
// (Scheduler / Skills / Todos / MCP / Files); each tab's body renders
// from the matching component under `right-rail/`.

import { useUiStore, type RightRailTab } from "../state/useUiStore";
import { SchedulerTab } from "./right-rail/SchedulerTab";
import { SkillsTab } from "./right-rail/SkillsTab";
import { TodosTab } from "./right-rail/TodosTab";
import { McpTab } from "./right-rail/McpTab";
import { FilesTab } from "./right-rail/FilesTab";

type TabSpec = {
  id: RightRailTab;
  label: string;
  short: string;
};

const TABS: readonly TabSpec[] = [
  { id: "scheduler", label: "Scheduler", short: "Sch" },
  { id: "skills", label: "Skills", short: "Skl" },
  { id: "todos", label: "Todos", short: "Tdo" },
  { id: "mcp", label: "MCP", short: "MCP" },
  { id: "files", label: "Files", short: "Fil" },
];

export function RightRail(): JSX.Element {
  const activeTab = useUiStore((s) => s.rightRailTab);
  const collapsed = useUiStore((s) => s.rightRailCollapsed);
  const setActiveTab = useUiStore((s) => s.setRightRailTab);
  const setCollapsed = useUiStore((s) => s.setRightRailCollapsed);

  function onTabClick(id: RightRailTab): void {
    if (id === activeTab && !collapsed) {
      setCollapsed(true);
      return;
    }
    setActiveTab(id);
    if (collapsed) setCollapsed(false);
  }

  return (
    <aside className="h-full flex flex-row border-l border-slate-800 bg-slate-950 min-h-0">
      {!collapsed && (
        <div className="flex-1 min-w-0 overflow-y-auto">
          <header className="px-3 py-2 border-b border-slate-800 flex items-center justify-between">
            <h3 className="text-xs uppercase tracking-wider text-slate-400">
              {TABS.find((t) => t.id === activeTab)?.label ?? "Panel"}
            </h3>
          </header>
          <RightRailBody tab={activeTab} />
        </div>
      )}
      <nav
        className="shrink-0 flex flex-col items-stretch border-l border-slate-800"
        aria-label="Right rail tabs"
      >
        {TABS.map((t) => {
          const isActive = t.id === activeTab && !collapsed;
          const cls = isActive
            ? "px-2 py-3 text-xs text-slate-100 bg-slate-800 border-l-2 border-emerald-500"
            : "px-2 py-3 text-xs text-slate-400 hover:bg-slate-900 border-l-2 border-transparent";
          return (
            <button
              key={t.id}
              type="button"
              className={cls}
              onClick={() => onTabClick(t.id)}
              title={t.label}
              aria-pressed={isActive}
            >
              {t.short}
            </button>
          );
        })}
      </nav>
    </aside>
  );
}

function RightRailBody({ tab }: { tab: RightRailTab }): JSX.Element {
  switch (tab) {
    case "scheduler":
      return <SchedulerTab />;
    case "skills":
      return <SkillsTab />;
    case "todos":
      return <TodosTab />;
    case "mcp":
      return <McpTab />;
    case "files":
      return <FilesTab />;
  }
}
