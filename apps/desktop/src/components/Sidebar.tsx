// Sidebar — three-level tree (project → workspaces → workareas) per
// `design/15 §3.4`. Task 25 extends Task 24's project+workspace tree
// with the workarea level and adds the "New Workspace" button.
//
// "Current project" model: V0.1 has no project-creation UI. The
// sidebar picks the first project returned by `Projects.ListProjects`
// and shows it. The developer seeds a project via the persistence
// helpers; see `tasks/19-workspace-creation.md` Handoff Notes for
// the rationale.

import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  FolderGit2,
  Plus,
  RefreshCw,
  Settings,
} from "lucide-react";

import { useProjects } from "../hooks/useProjects";
import { useWorkspaces } from "../hooks/useWorkspaces";
import { useEventSubscription } from "../hooks/useEventSubscription";
import { useUiStore } from "../state/useUiStore";
import { IconButton } from "./ui/icon-button";
import { Button } from "./ui/button";
import { WorkareaList } from "./WorkareaList";
import type { Workspace } from "../api/workspaces";

export function Sidebar(): JSX.Element {
  const queryClient = useQueryClient();
  const selectedWorkspaceId = useUiStore((s) => s.selectedWorkspaceId);
  const setSelectedWorkspace = useUiStore((s) => s.setSelectedWorkspace);
  const selectedProjectId = useUiStore((s) => s.selectedProjectId);
  const setSelectedProject = useUiStore((s) => s.setSelectedProject);
  const expandedWorkspaces = useUiStore((s) => s.expandedWorkspaces);
  const toggleExpanded = useUiStore((s) => s.toggleWorkspaceExpanded);
  const setNewWorkspaceModalOpen = useUiStore(
    (s) => s.setNewWorkspaceModalOpen,
  );
  const setNewProjectModalOpen = useUiStore(
    (s) => s.setNewProjectModalOpen,
  );
  const setSettingsOpen = useUiStore((s) => s.setSettingsOpen);

  const projectsQuery = useProjects();
  const project =
    projectsQuery.data?.projects.find((p) => p.id === selectedProjectId) ??
    projectsQuery.data?.projects[0] ??
    null;

  // Pin the first project as the current selection once it loads.
  useEffect(() => {
    if (project && !selectedProjectId) {
      setSelectedProject(project.id);
    }
  }, [project, selectedProjectId, setSelectedProject]);

  const workspacesQuery = useWorkspaces(project?.id);

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
    void queryClient.invalidateQueries({ queryKey: ["projects"] });
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

      <nav className="flex-1 overflow-y-auto px-2 py-3 space-y-3">
        {projectsQuery.isLoading && (
          <p className="px-2 text-xs text-faint">Loading projects…</p>
        )}
        {projectsQuery.isError && (
          <p className="px-2 text-xs text-err">
            Failed to load projects: {String(projectsQuery.error)}
          </p>
        )}
        {projectsQuery.data && projectsQuery.data.projects.length === 0 && (
          <div className="px-2 space-y-2">
            <p className="text-xs text-faint">No projects yet.</p>
            <Button size="sm" onClick={() => setNewProjectModalOpen(true)}>
              + New Project
            </Button>
          </div>
        )}

        {project && (
          <section>
            <div className="flex items-center justify-between px-2 mb-1">
              <p className="text-xs uppercase tracking-wide text-faint">
                Project
              </p>
              <IconButton
                label="New project"
                onClick={() => setNewProjectModalOpen(true)}
              >
                <Plus size={14} />
              </IconButton>
            </div>
            <p className="px-2 text-sm text-foreground mb-2">{project.name}</p>

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
              <p className="px-2 text-xs text-faint">Loading…</p>
            )}
            {workspacesQuery.isError && (
              <p className="px-2 text-xs text-err">
                Failed to load workspaces: {String(workspacesQuery.error)}
              </p>
            )}
            {workspacesQuery.data &&
              workspacesQuery.data.workspaces.length === 0 && (
                <p className="px-2 text-xs text-faint">No workspaces yet.</p>
              )}
            <ul className="space-y-0.5">
              {workspacesQuery.data?.workspaces.map((ws) => (
                <WorkspaceNode
                  key={ws.id}
                  workspace={ws}
                  active={ws.id === selectedWorkspaceId}
                  expanded={expandedWorkspaces.has(ws.id)}
                  onSelect={() => setSelectedWorkspace(ws.id)}
                  onToggleExpanded={() => toggleExpanded(ws.id)}
                />
              ))}
            </ul>
          </section>
        )}
      </nav>
    </aside>
  );
}

type WorkspaceNodeProps = {
  workspace: Workspace;
  active: boolean;
  expanded: boolean;
  onSelect: () => void;
  onToggleExpanded: () => void;
};

function WorkspaceNode({
  workspace,
  active,
  expanded,
  onSelect,
  onToggleExpanded,
}: WorkspaceNodeProps): JSX.Element {
  const buttonClass = active
    ? "flex-1 text-left px-2 py-1 rounded-md text-sm bg-accent/10 text-foreground"
    : "flex-1 text-left px-2 py-1 rounded-md text-sm text-muted hover:bg-surface-2";
  return (
    <li>
      <div className="flex items-center gap-1">
        <button
          type="button"
          className="px-1 text-faint hover:text-foreground"
          onClick={onToggleExpanded}
          aria-label={expanded ? "Collapse" : "Expand"}
        >
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </button>
        <button type="button" className={buttonClass} onClick={onSelect}>
          <span className="flex items-center gap-2 min-w-0">
            <FolderGit2 size={14} className="text-faint shrink-0" />
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
