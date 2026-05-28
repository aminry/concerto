// Ephemeral renderer UI state. Server state lives in React Query
// (see `src/hooks/`). Per `design/15 §3.3`, Zustand owns nothing
// derived from the Core — just what's local to the user's current
// window session.

import { create } from "zustand";

export type UiStore = {
  selectedWorkspaceId: string | null;
  selectedProjectId: string | null;
  sidebarCollapsed: boolean;
  setSelectedWorkspace: (id: string | null) => void;
  setSelectedProject: (id: string | null) => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
};

export const useUiStore = create<UiStore>((set) => ({
  selectedWorkspaceId: null,
  selectedProjectId: null,
  sidebarCollapsed: false,
  setSelectedWorkspace: (id) => set({ selectedWorkspaceId: id }),
  setSelectedProject: (id) => set({ selectedProjectId: id }),
  setSidebarCollapsed: (collapsed) => set({ sidebarCollapsed: collapsed }),
}));
