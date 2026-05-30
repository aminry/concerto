// Top-level desktop shell. Task 46 replaces the Task 25 two-column
// layout with the full three-panel layout (`AppLayout`) per
// `design/15 §3.4`. The layout state (sidebar width, session region
// height, right-rail collapsed boolean, active right-rail tab) lives
// in the Zustand store and is debounced into `localStorage` here so
// the choice survives a window reload.
//
// Modals + toasts hang off the App root so they overlay the entire
// three-panel split.

import { useEffect, useRef } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { AppLayout } from "./components/AppLayout";
import { NewProjectModal } from "./components/NewProjectModal";
import { NewWorkspaceModal } from "./components/NewWorkspaceModal";
import { SettingsPanel } from "./components/SettingsPanel";
import { AutoUpdateToast, FirstRunClaudeToast } from "./components/Toast";
import { useThemeController } from "./hooks/useTheme";
import { LAYOUT_STORAGE_KEY, useUiStore } from "./state/useUiStore";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 1000 * 30,
      gcTime: 1000 * 60 * 5,
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});

/// Debounce window for persisting layout state to `localStorage`. Task
/// 46 spec pins this at 300ms to keep the write rate low under active
/// drag-resize without lagging real edits.
const LAYOUT_PERSIST_DEBOUNCE_MS = 300;

function App(): JSX.Element {
  useLayoutPersistence();
  useThemeController();
  return (
    <QueryClientProvider client={queryClient}>
      <div className="h-screen w-screen bg-background text-foreground font-sans">
        <AppLayout />
        <NewProjectModal />
        <NewWorkspaceModal />
        <SettingsPanel />
        <FirstRunClaudeToast />
        <AutoUpdateToast />
      </div>
    </QueryClientProvider>
  );
}

/// Subscribes to the four layout-state fields and writes them to
/// `localStorage` on a debounced trailing-edge schedule. The first
/// render does NOT write — the initial values are loaded from
/// `localStorage` by the store itself, so writing them back would be
/// a no-op churn.
function useLayoutPersistence(): void {
  const sidebarWidth = useUiStore((s) => s.sidebarWidth);
  const sessionRegionHeight = useUiStore((s) => s.sessionRegionHeight);
  const rightRailCollapsed = useUiStore((s) => s.rightRailCollapsed);
  const rightRailTab = useUiStore((s) => s.rightRailTab);
  const diffViewMode = useUiStore((s) => s.diffViewMode);
  const firstRunRef = useRef(true);
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    if (firstRunRef.current) {
      firstRunRef.current = false;
      return;
    }
    if (typeof window === "undefined" || !window.localStorage) return;
    if (timerRef.current !== null) {
      window.clearTimeout(timerRef.current);
    }
    timerRef.current = window.setTimeout(() => {
      try {
        window.localStorage.setItem(
          LAYOUT_STORAGE_KEY,
          JSON.stringify({
            sidebarWidth,
            sessionRegionHeight,
            rightRailCollapsed,
            rightRailTab,
            diffViewMode,
          }),
        );
      } catch {
        // localStorage may be unavailable (private mode, quota). The
        // layout still works in-memory; we just lose the persistence.
      }
      timerRef.current = null;
    }, LAYOUT_PERSIST_DEBOUNCE_MS);
    return () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, [
    sidebarWidth,
    sessionRegionHeight,
    rightRailCollapsed,
    rightRailTab,
    diffViewMode,
  ]);
}

export default App;
