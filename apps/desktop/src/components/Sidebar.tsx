// Sidebar — flat workspace tree (workspaces → workareas → sessions) per
// `design/15 §3.4`. The Project layer was collapsed away: the top level is
// now a flat list of EVERY workspace (the global registry), each a
// collapsible node with its workareas nested underneath.
//
// `selectedWorkspaceId` is the canonical selection (it highlights the
// active workspace). The "+ Workspace" affordance opens the
// `NewWorkspaceModal`, which is the primary creation flow.

import { useQueryClient } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  FolderGit2,
  Plus,
  RefreshCw,
  Settings,
} from "lucide-react";

import { useWorkspaces } from "../hooks/useWorkspaces";
import { useEventSubscription } from "../hooks/useEventSubscription";
import { useUiStore } from "../state/useUiStore";
import { IconButton } from "./ui/icon-button";
import { Button } from "./ui/button";
import { WorkareaList } from "./WorkareaList";
import type { Workspace } from "../api/workspaces";

export function Sidebar(): JSX.Element {
  const queryClient = useQueryClient();
  const setNewWorkspaceModalOpen = useUiStore(
    (s) => s.setNewWorkspaceModalOpen,
  );
  const setSettingsOpen = useUiStore((s) => s.setSettingsOpen);

  const workspacesQuery = useWorkspaces();
  const workspaces = workspacesQuery.data?.workspaces ?? [];

  // Live invalidation: any `workspace.events` frame from the Core
  // refetches the workspace list. Per the spec, this is the
  // event-driven backbone that replaces polling.
  useEventSubscription("workspace.events", () => {
    void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
  });

  // `workarea.events` invalidates the per-workspace workarea list so
  // creating + archiving propagates without polling.
  useEventSubscription("workarea.events", () => {
    void queryClient.invalidateQueries({ queryKey: ["workareas"] });
  });

  function onRefresh(): void {
    void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
    void queryClient.invalidateQueries({ queryKey: ["workareas"] });
  }

  return (
    <aside className="h-full border-r border-border bg-surface flex flex-col min-h-0">
      <header className="px-4 py-3 border-b border-border flex items-center justify-between">
        <h1 className="text-sm font-semibold tracking-wide text-foreground">
          Concerto
        </h1>
        <div className="flex gap-0.5">
          <IconButton label="Refresh" onClick={onRefresh}>
            <RefreshCw size={15} />
          </IconButton>
          <IconButton label="Settings" onClick={() => setSettingsOpen(true)}>
            <Settings size={15} />
          </IconButton>
        </div>
      </header>

      <nav className="flex-1 overflow-y-auto px-2 py-3 space-y-1">
        <div className="flex items-center justify-between px-2 mb-1">
          <p className="text-xs uppercase tracking-wide text-faint">
            Workspaces
          </p>
          <IconButton
            label="New workspace"
            onClick={() => setNewWorkspaceModalOpen(true)}
          >
            <Plus size={14} />
          </IconButton>
        </div>

        {workspacesQuery.isLoading && (
          <p className="px-2 text-xs text-faint">Loading workspaces…</p>
        )}
        {workspacesQuery.isError && (
          <p className="px-2 text-xs text-err">
            Failed to load workspaces: {String(workspacesQuery.error)}
          </p>
        )}
        {workspacesQuery.data && workspaces.length === 0 && (
          <div className="px-2 space-y-2">
            <p className="text-xs text-faint">No workspaces yet.</p>
            <Button size="sm" onClick={() => setNewWorkspaceModalOpen(true)}>
              + New Workspace
            </Button>
          </div>
        )}

        <ul className="space-y-0.5">
          {workspaces.map((ws) => (
            <WorkspaceNode key={ws.id} workspace={ws} />
          ))}
        </ul>
      </nav>
    </aside>
  );
}

type WorkspaceNodeProps = {
  workspace: Workspace;
};

function WorkspaceNode({ workspace }: WorkspaceNodeProps): JSX.Element {
  const selectedWorkspaceId = useUiStore((s) => s.selectedWorkspaceId);
  const setSelectedWorkspace = useUiStore((s) => s.setSelectedWorkspace);
  const expandedWorkspaces = useUiStore((s) => s.expandedWorkspaces);
  const toggleExpanded = useUiStore((s) => s.toggleWorkspaceExpanded);

  const active = workspace.id === selectedWorkspaceId;
  const expanded = expandedWorkspaces.has(workspace.id);

  const buttonClass = active
    ? "flex-1 text-left px-2 py-1 rounded-md text-sm bg-accent/10 text-foreground"
    : "flex-1 text-left px-2 py-1 rounded-md text-sm text-muted hover:bg-surface-2";

  return (
    <li>
      <div className="flex items-center gap-1">
        <button
          type="button"
          className="px-1 text-faint hover:text-foreground"
          onClick={() => toggleExpanded(workspace.id)}
          aria-label={expanded ? "Collapse" : "Expand"}
        >
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </button>
        <button
          type="button"
          className={buttonClass}
          onClick={() => setSelectedWorkspace(workspace.id)}
        >
          <span className="flex items-center gap-2 min-w-0">
            {workspace.icon ? (
              <span className="text-faint shrink-0" aria-hidden>
                {workspace.icon}
              </span>
            ) : (
              <FolderGit2 size={14} className="text-faint shrink-0" />
            )}
            <span className="min-w-0">
              <span className="block truncate">{workspace.name}</span>
              <span className="block text-xs text-faint truncate font-mono">
                {workspace.slug}
              </span>
            </span>
          </span>
        </button>
      </div>
      {expanded && (
        <div className="pl-6 pt-1">
          <WorkareaList workspaceId={workspace.id} />
        </div>
      )}
    </li>
  );
}
