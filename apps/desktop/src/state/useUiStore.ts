// Ephemeral renderer UI state. Server state lives in React Query
// (see `src/hooks/`). Per `design/15 §3.3`, Zustand owns nothing
// derived from the Core — just what's local to the user's current
// window session.
//
// Task 25 locks the selection trio (`selectedProjectId`,
// `selectedWorkspaceId`, `selectedWorkareaId`) as the canonical
// renderer selection state. Selecting a workarea implicitly pins its
// parent workspace; selecting a workspace clears the workarea.
//
// Task 46 adds the three-panel layout state — sidebar width, session
// region height, right-rail collapsed boolean, and active right-rail
// tab. The layout state shape is frozen as the V0.1 wire shape in
// `localStorage` (see `LAYOUT_STORAGE_KEY` below).

import { create } from "zustand";

/// Right-rail tabs per `design/15 §3.4` (V0.1 list). The Code & PRs
/// surface (`diff` / `checks` / `pr`) moved here from the center panel's
/// bottom region so the session terminal occupies the full center height.
export type RightRailTab =
  | "scheduler"
  | "skills"
  | "todos"
  | "mcp"
  | "files"
  | "diff"
  | "checks"
  | "pr";

/// Task 47 — Monaco diff view modes. `split` shows the side-by-side
/// editor; `unified` flips Monaco's `renderSideBySide` off so the
/// before/after collapse into a single column. The state is persisted
/// alongside the rest of the layout so the choice survives reloads.
export type DiffViewMode = "split" | "unified";

/// `localStorage` key for the persisted layout state. Task 46 locks
/// the schema:
///
///   { sidebarWidth: number, sessionRegionHeight: number,
///     rightRailCollapsed: boolean, rightRailTab: string }
///
/// Numbers are percentages (0–100) of the parent container, which is
/// what `react-resizable-panels` natively accepts. `rightRailTab` is
/// the `RightRailTab` string.
export const LAYOUT_STORAGE_KEY = "concerto.layout.v1";

/// Defaults used when no persisted state exists. Mirrors the design
/// doc's diagram (sidebar ~20%, center split 55/45, right rail open
/// on Scheduler).
export const LAYOUT_DEFAULTS = {
  sidebarWidth: 20,
  sessionRegionHeight: 55,
  rightRailCollapsed: false,
  rightRailTab: "scheduler" as RightRailTab,
  diffViewMode: "split" as DiffViewMode,
};

export type LayoutState = typeof LAYOUT_DEFAULTS;

export type UiStore = {
  selectedWorkspaceId: string | null;
  selectedWorkareaId: string | null;
  selectedProjectId: string | null;
  /// Active session tab inside the currently selected workarea. Task 26
  /// adds this; Task 26 caps V0.1 at one session per workarea, but the
  /// terminal panel still uses a tab strip so V1.0's multi-session story
  /// drops into the same surface without a rewrite.
  activeSessionId: string | null;
  /// Task 322 — the active repo in the workarea's Level-1 Code & PRs repo
  /// selector (`design/15 §3.4`). UI-only selection: which of the
  /// workarea's repos drives the Diff view. Reset on workarea switch (see
  /// `setSelectedWorkarea`) so a stale repo id from the previous workarea
  /// never renders. Deliberately NOT persisted (ephemeral selection, like
  /// `activeSessionId`) — `LAYOUT_STORAGE_KEY` is unchanged.
  selectedRepoId: string | null;
  sidebarCollapsed: boolean;
  /// Per-workspace expansion state for the sidebar tree. Tracked here
  /// (not in component-local state) so the choice survives a sidebar
  /// re-mount and so Task 26 can drive it from the session terminal.
  expandedWorkspaces: Set<string>;
  /// Per-project COLLAPSE state for the sidebar tree. The sidebar now
  /// renders every project as a top-level tree node; projects are
  /// expanded by default (so all workspaces are visible at a glance),
  /// so we track the inverse — only the ids the user has explicitly
  /// collapsed. Absence from the set means "expanded".
  collapsedProjects: Set<string>;
  /// True while the New Workspace modal is open. The renderer-only
  /// flag keeps the modal state inspectable in dev tools.
  newWorkspaceModalOpen: boolean;
  /// True while the New Project modal is open. Surfaced by the sidebar
  /// when the user clicks the "+ Project" affordance.
  newProjectModalOpen: boolean;
  /// True when Settings (currently just Add Repository) is on screen.
  settingsOpen: boolean;
  /// Task 219 — true when the Connect-to-Core picker is on screen. UI-only;
  /// the paired-Core list it shows is React-Query-canonical (`src/api/cores.ts`),
  /// never duplicated here. Mirrors `settingsOpen`.
  connectCoreOpen: boolean;
  /// Task 219 — true when the Pair-with-a-remote-Core modal (scan QR / paste
  /// token → name) is on screen. The in-progress pairing draft (the decoded
  /// payload, the chosen name) lives in the modal's component-local state, not
  /// here; this flag is the only piece of pairing UI state that is global.
  pairingOpen: boolean;
  /// Task 46 — three-panel layout state. Persisted to `localStorage`
  /// under [`LAYOUT_STORAGE_KEY`] via the App-root effect.
  sidebarWidth: number;
  sessionRegionHeight: number;
  rightRailCollapsed: boolean;
  rightRailTab: RightRailTab;
  /// Task 47 — selected mode for the Monaco diff viewer in the
  /// `CodePrRegion`'s Diff sub-tab. Persisted with the rest of the
  /// layout state.
  diffViewMode: DiffViewMode;
  setSelectedWorkspace: (id: string | null) => void;
  setSelectedWorkarea: (id: string | null) => void;
  setSelectedProject: (id: string | null) => void;
  setActiveSession: (id: string | null) => void;
  setSelectedRepo: (id: string | null) => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
  toggleWorkspaceExpanded: (workspaceId: string) => void;
  setWorkspaceExpanded: (workspaceId: string, expanded: boolean) => void;
  toggleProjectExpanded: (projectId: string) => void;
  setNewWorkspaceModalOpen: (open: boolean) => void;
  setNewProjectModalOpen: (open: boolean) => void;
  setSettingsOpen: (open: boolean) => void;
  setConnectCoreOpen: (open: boolean) => void;
  setPairingOpen: (open: boolean) => void;
  setSidebarWidth: (width: number) => void;
  setSessionRegionHeight: (height: number) => void;
  setRightRailCollapsed: (collapsed: boolean) => void;
  setRightRailTab: (tab: RightRailTab) => void;
  setDiffViewMode: (mode: DiffViewMode) => void;
};

/// Load the persisted layout state from `localStorage`. Bad / missing
/// data falls back to the defaults — this keeps the renderer alive
/// even when the storage is corrupted.
function loadLayoutState(): LayoutState {
  if (typeof window === "undefined" || !window.localStorage) {
    return { ...LAYOUT_DEFAULTS };
  }
  try {
    const raw = window.localStorage.getItem(LAYOUT_STORAGE_KEY);
    if (!raw) return { ...LAYOUT_DEFAULTS };
    const parsed = JSON.parse(raw) as Partial<LayoutState>;
    return {
      sidebarWidth:
        typeof parsed.sidebarWidth === "number"
          ? clampPercent(parsed.sidebarWidth)
          : LAYOUT_DEFAULTS.sidebarWidth,
      sessionRegionHeight:
        typeof parsed.sessionRegionHeight === "number"
          ? clampPercent(parsed.sessionRegionHeight)
          : LAYOUT_DEFAULTS.sessionRegionHeight,
      rightRailCollapsed:
        typeof parsed.rightRailCollapsed === "boolean"
          ? parsed.rightRailCollapsed
          : LAYOUT_DEFAULTS.rightRailCollapsed,
      rightRailTab: isRightRailTab(parsed.rightRailTab)
        ? parsed.rightRailTab
        : LAYOUT_DEFAULTS.rightRailTab,
      diffViewMode: isDiffViewMode(parsed.diffViewMode)
        ? parsed.diffViewMode
        : LAYOUT_DEFAULTS.diffViewMode,
    };
  } catch {
    return { ...LAYOUT_DEFAULTS };
  }
}

function clampPercent(value: number): number {
  if (Number.isNaN(value)) return 0;
  if (value < 5) return 5;
  if (value > 95) return 95;
  return value;
}

function isRightRailTab(value: unknown): value is RightRailTab {
  return (
    value === "scheduler" ||
    value === "skills" ||
    value === "todos" ||
    value === "mcp" ||
    value === "files" ||
    value === "diff" ||
    value === "checks" ||
    value === "pr"
  );
}

function isDiffViewMode(value: unknown): value is DiffViewMode {
  return value === "split" || value === "unified";
}

const initialLayout = loadLayoutState();

export const useUiStore = create<UiStore>((set) => ({
  selectedWorkspaceId: null,
  selectedWorkareaId: null,
  selectedProjectId: null,
  activeSessionId: null,
  selectedRepoId: null,
  sidebarCollapsed: false,
  expandedWorkspaces: new Set<string>(),
  collapsedProjects: new Set<string>(),
  newWorkspaceModalOpen: false,
  newProjectModalOpen: false,
  settingsOpen: false,
  connectCoreOpen: false,
  pairingOpen: false,
  sidebarWidth: initialLayout.sidebarWidth,
  sessionRegionHeight: initialLayout.sessionRegionHeight,
  rightRailCollapsed: initialLayout.rightRailCollapsed,
  rightRailTab: initialLayout.rightRailTab,
  diffViewMode: initialLayout.diffViewMode,
  setSelectedWorkspace: (id) =>
    set({
      selectedWorkspaceId: id,
      selectedWorkareaId: null,
      activeSessionId: null,
      // Switching workspace clears the workarea, so the active repo
      // selection (keyed to the old workarea) must clear too.
      selectedRepoId: null,
    }),
  setSelectedWorkarea: (id) =>
    // Clearing `selectedRepoId` here keeps the Level-1 repo selector from
    // rendering a repo id that belonged to the previous workarea; the
    // selector re-auto-selects the new workarea's first repo (Task 322).
    set({ selectedWorkareaId: id, activeSessionId: null, selectedRepoId: null }),
  setSelectedProject: (id) => set({ selectedProjectId: id }),
  setActiveSession: (id) => set({ activeSessionId: id }),
  setSelectedRepo: (id) => set({ selectedRepoId: id }),
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
  toggleProjectExpanded: (projectId) =>
    set((state) => {
      const next = new Set(state.collapsedProjects);
      if (next.has(projectId)) {
        next.delete(projectId);
      } else {
        next.add(projectId);
      }
      return { collapsedProjects: next };
    }),
  setNewWorkspaceModalOpen: (open) => set({ newWorkspaceModalOpen: open }),
  setNewProjectModalOpen: (open) => set({ newProjectModalOpen: open }),
  setSettingsOpen: (open) => set({ settingsOpen: open }),
  setConnectCoreOpen: (open) => set({ connectCoreOpen: open }),
  setPairingOpen: (open) => set({ pairingOpen: open }),
  setSidebarWidth: (width) => set({ sidebarWidth: clampPercent(width) }),
  setSessionRegionHeight: (height) =>
    set({ sessionRegionHeight: clampPercent(height) }),
  setRightRailCollapsed: (collapsed) => set({ rightRailCollapsed: collapsed }),
  setRightRailTab: (tab) => set({ rightRailTab: tab }),
  setDiffViewMode: (mode) => set({ diffViewMode: mode }),
}));
