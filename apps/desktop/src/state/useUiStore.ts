// Ephemeral renderer UI state. Server state lives in React Query
// (see `src/hooks/`). Per `design/15 §3.3`, Zustand owns nothing
// derived from the Core — just what's local to the user's current
// window session.
//
// Task 25 locks the selection trio (`selectedProjectId`,
// `selectedWorkspaceId`, `selectedWorkareaId`) as the canonical
// renderer selection state. Selecting a workarea implicitly pins its
// parent workspace; selecting a workspace clears the workarea.

import { create } from "zustand";

export type UiStore = {
  selectedWorkspaceId: string | null;
  selectedWorkareaId: string | null;
  selectedProjectId: string | null;
  sidebarCollapsed: boolean;
  /// Per-workspace expansion state for the sidebar tree. Tracked here
  /// (not in component-local state) so the choice survives a sidebar
  /// re-mount and so Task 26 can drive it from the session terminal.
  expandedWorkspaces: Set<string>;
  /// True while the New Workspace modal is open. The renderer-only
  /// flag keeps the modal state inspectable in dev tools.
  newWorkspaceModalOpen: boolean;
  /// True when Settings (currently just Add Repository) is on screen.
  settingsOpen: boolean;
  setSelectedWorkspace: (id: string | null) => void;
  setSelectedWorkarea: (id: string | null) => void;
  setSelectedProject: (id: string | null) => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
  toggleWorkspaceExpanded: (workspaceId: string) => void;
  setWorkspaceExpanded: (workspaceId: string, expanded: boolean) => void;
  setNewWorkspaceModalOpen: (open: boolean) => void;
  setSettingsOpen: (open: boolean) => void;
};

export const useUiStore = create<UiStore>((set) => ({
  selectedWorkspaceId: null,
  selectedWorkareaId: null,
  selectedProjectId: null,
  sidebarCollapsed: false,
  expandedWorkspaces: new Set<string>(),
  newWorkspaceModalOpen: false,
  settingsOpen: false,
  setSelectedWorkspace: (id) =>
    set({ selectedWorkspaceId: id, selectedWorkareaId: null }),
  setSelectedWorkarea: (id) => set({ selectedWorkareaId: id }),
  setSelectedProject: (id) => set({ selectedProjectId: id }),
  setSidebarCollapsed: (collapsed) => set({ sidebarCollapsed: collapsed }),
  toggleWorkspaceExpanded: (workspaceId) =>
    set((state) => {
      const next = new Set(state.expandedWorkspaces);
      if (next.has(workspaceId)) {
        next.delete(workspaceId);
      } else {
        next.add(workspaceId);
      }
      return { expandedWorkspaces: next };
    }),
  setWorkspaceExpanded: (workspaceId, expanded) =>
    set((state) => {
      const next = new Set(state.expandedWorkspaces);
      if (expanded) {
        next.add(workspaceId);
      } else {
        next.delete(workspaceId);
      }
      return { expandedWorkspaces: next };
    }),
  setNewWorkspaceModalOpen: (open) => set({ newWorkspaceModalOpen: open }),
  setSettingsOpen: (open) => set({ settingsOpen: open }),
}));
