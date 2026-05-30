// Right rail — vertical tab strip with a collapsible drawer per
// `design/15 §3.4`.
//
// The strip is always visible; clicking a tab when the drawer is
// collapsed reopens it on that tab. Clicking the active tab again
// collapses the drawer. The Task 46 V0.1 set of tabs is fixed
// (Scheduler / Skills / Todos / MCP / Files); each tab's body renders
// from the matching component under `right-rail/`.

import {
  Clock,
  Sparkles,
  ListChecks,
  Blocks,
  Folder,
  GitCompare,
  CircleCheck,
  GitPullRequest,
  type LucideIcon,
} from "lucide-react";
import { useUiStore, type RightRailTab } from "../state/useUiStore";
import { Tooltip } from "./ui/tooltip";
import { SchedulerTab } from "./right-rail/SchedulerTab";
import { SkillsTab } from "./right-rail/SkillsTab";
import { TodosTab } from "./right-rail/TodosTab";
import { McpTab } from "./right-rail/McpTab";
import { FilesTab } from "./right-rail/FilesTab";
import { CodePrRegion } from "./center/CodePrRegion";

type TabSpec = { id: RightRailTab; label: string; Icon: LucideIcon };

const TABS: readonly TabSpec[] = [
  { id: "scheduler", label: "Scheduler", Icon: Clock },
  { id: "skills", label: "Skills", Icon: Sparkles },
  { id: "todos", label: "Todos", Icon: ListChecks },
  { id: "mcp", label: "MCP", Icon: Blocks },
  { id: "files", label: "Files", Icon: Folder },
  // Code & PRs — moved here from the center panel's bottom region so the
  // session terminal occupies the full center height.
  { id: "diff", label: "Diff", Icon: GitCompare },
  { id: "checks", label: "Checks", Icon: CircleCheck },
  { id: "pr", label: "PR", Icon: GitPullRequest },
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
    <aside className="h-full flex flex-row border-l border-border bg-surface min-h-0">
      {!collapsed && (
        <div className="flex-1 min-w-0 flex flex-col min-h-0">
          <header className="shrink-0 px-3 py-2 border-b border-border flex items-center justify-between">
            <h3 className="text-xs uppercase tracking-wide text-muted">
              {TABS.find((t) => t.id === activeTab)?.label ?? "Panel"}
            </h3>
          </header>
          {/* Body fills the remaining height (the Diff view's Monaco editor
              needs a definite height) and scrolls for the list-style tabs. */}
          <div className="flex-1 min-h-0 overflow-y-auto">
            <RightRailBody tab={activeTab} />
          </div>
        </div>
      )}
      <nav
        className="shrink-0 flex flex-col items-stretch border-l border-border"
        aria-label="Right rail tabs"
      >
        {TABS.map((t) => {
          const isActive = t.id === activeTab && !collapsed;
          const cls = isActive
            ? "relative grid h-9 w-11 place-items-center text-accent bg-accent/10"
            : "relative grid h-9 w-11 place-items-center text-muted hover:bg-surface-2 hover:text-foreground";
          return (
            <Tooltip key={t.id} label={t.label} side="left">
              <button
                type="button"
                className={cls}
                onClick={() => onTabClick(t.id)}
                aria-pressed={isActive}
                aria-label={t.label}
              >
                {isActive && (
                  <span className="absolute left-0 top-1.5 bottom-1.5 w-0.5 rounded-full bg-accent" />
                )}
                <t.Icon size={17} />
              </button>
            </Tooltip>
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
    case "diff":
    case "checks":
    case "pr":
      return <CodePrRegion subTab={tab} />;
  }
}
