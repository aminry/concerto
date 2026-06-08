// Sidebar — full project tree (projects → workspaces → workareas) per
// `design/15 §3.4`. Originally (Tasks 24/25) the sidebar showed only the
// first project returned by `Projects.ListProjects`; it now renders EVERY
// project as a top-level, collapsible tree node with its workspaces (and
// their workareas) nested underneath.
//
// "Current project" model: `selectedProjectId` is still the canonical
// selection (it scopes the New Workspace modal and highlights the active
// project). Clicking a project — or its "New workspace" button — pins it.
// Projects are expanded by default; the user-collapsed set lives in the
// store (`collapsedProjects`).

import { useQueryClient } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronRight,
  FolderGit2,
  Folders,
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
import type { Project } from "../api/projects";
import type { Workspace } from "../api/workspaces";

export function Sidebar(): JSX.Element {
  const queryClient = useQueryClient();
  const setNewProjectModalOpen = useUiStore((s) => s.setNewProjectModalOpen);
  const setSettingsOpen = useUiStore((s) => s.setSettingsOpen);

  const projectsQuery = useProjects();
  const projects = projectsQuery.data?.projects ?? [];

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

      <nav className="flex-1 overflow-y-auto px-2 py-3 space-y-1">
        <div className="flex items-center justify-between px-2 mb-1">
          <p className="text-xs uppercase tracking-wide text-faint">Projects</p>
          <IconButton
            label="New project"
            onClick={() => setNewProjectModalOpen(true)}
          >
            <Plus size={14} />
          </IconButton>
        </div>

        {projectsQuery.isLoading && (
          <p className="px-2 text-xs text-faint">Loading projects…</p>
        )}
        {projectsQuery.isError && (
          <p className="px-2 text-xs text-err">
            Failed to load projects: {String(projectsQuery.error)}
          </p>
        )}
        {projectsQuery.data && projects.length === 0 && (
          <div className="px-2 space-y-2">
            <p className="text-xs text-faint">No projects yet.</p>
            <Button size="sm" onClick={() => setNewProjectModalOpen(true)}>
              + New Project
            </Button>
          </div>
        )}

        {projects.map((project) => (
          <ProjectNode key={project.id} project={project} />
        ))}
      </nav>
    </aside>
  );
}

type ProjectNodeProps = {
  project: Project;
};

function ProjectNode({ project }: ProjectNodeProps): JSX.Element {
  const selectedProjectId = useUiStore((s) => s.selectedProjectId);
  const setSelectedProject = useUiStore((s) => s.setSelectedProject);
  const selectedWorkspaceId = useUiStore((s) => s.selectedWorkspaceId);
  const setSelectedWorkspace = useUiStore((s) => s.setSelectedWorkspace);
  const expandedWorkspaces = useUiStore((s) => s.expandedWorkspaces);
  const toggleExpanded = useUiStore((s) => s.toggleWorkspaceExpanded);
  const collapsedProjects = useUiStore((s) => s.collapsedProjects);
  const toggleProjectExpanded = useUiStore((s) => s.toggleProjectExpanded);
  const setNewWorkspaceModalOpen = useUiStore(
    (s) => s.setNewWorkspaceModalOpen,
  );

  const expanded = !collapsedProjects.has(project.id);
  const active = project.id === selectedProjectId;
  const workspacesQuery = useWorkspaces(expanded ? project.id : undefined);

  function onNewWorkspace(): void {
    // Scope the New Workspace modal to this project.
    setSelectedProject(project.id);
    setNewWorkspaceModalOpen(true);
  }

  const headerClass = active
    ? "flex-1 text-left px-1.5 py-1 rounded-md text-sm font-medium bg-accent/10 text-foreground"
    : "flex-1 text-left px-1.5 py-1 rounded-md text-sm font-medium text-foreground hover:bg-surface-2";

  return (
    <section>
      <div className="flex items-center gap-1">
        <button
          type="button"
          className="px-1 text-faint hover:text-foreground"
          onClick={() => toggleProjectExpanded(project.id)}
          aria-label={expanded ? "Collapse" : "Expand"}
        >
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </button>
        <button
          type="button"
          className={headerClass}
          onClick={() => setSelectedProject(project.id)}
        >
          <span className="flex items-center gap-2 min-w-0">
            <Folders size={14} className="text-faint shrink-0" />
            <span className="truncate">{project.name}</span>
          </span>
        </button>
        <IconButton label="New workspace" onClick={onNewWorkspace}>
          <Plus size={14} />
        </IconButton>
      </div>

      {expanded && (
        <div className="pl-3 mt-0.5">
          {workspacesQuery.isLoading && (
            <p className="px-2 py-0.5 text-xs text-faint">Loading…</p>
          )}
          {workspacesQuery.isError && (
            <p className="px-2 py-0.5 text-xs text-err">
              Failed to load workspaces: {String(workspacesQuery.error)}
            </p>
          )}
          {workspacesQuery.data &&
            workspacesQuery.data.workspaces.length === 0 && (
              <p className="px-2 py-0.5 text-xs text-faint">
                No workspaces yet.
              </p>
            )}
          <ul className="space-y-0.5">
            {workspacesQuery.data?.workspaces.map((ws) => (
              <WorkspaceNode
                key={ws.id}
                workspace={ws}
                projectId={project.id}
                active={ws.id === selectedWorkspaceId}
                expanded={expandedWorkspaces.has(ws.id)}
                onSelect={() => {
                  setSelectedProject(project.id);
                  setSelectedWorkspace(ws.id);
                }}
                onToggleExpanded={() => toggleExpanded(ws.id)}
              />
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

type WorkspaceNodeProps = {
  workspace: Workspace;
  projectId: string;
  active: boolean;
  expanded: boolean;
  onSelect: () => void;
  onToggleExpanded: () => void;
};

function WorkspaceNode({
  workspace,
  projectId,
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
          <WorkareaList workspaceId={workspace.id} projectId={projectId} />
        </div>
      )}
    </li>
  );
}
