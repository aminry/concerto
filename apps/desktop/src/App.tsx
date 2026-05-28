// Top-level layout. Sidebar on the left, detail panel on the right.
// The detail panel routes by selection: a selected workarea wins
// (renders `WorkareaDetail`); otherwise, a selected workspace shows
// `WorkspaceDetail`; otherwise a "select something" placeholder
// inside `WorkspaceDetail` itself.
//
// Modals + toasts hang off the App root so they overlay the entire
// split.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { Sidebar } from "./components/Sidebar";
import { WorkspaceDetail } from "./components/WorkspaceDetail";
import { WorkareaDetail } from "./components/WorkareaDetail";
import { NewWorkspaceModal } from "./components/NewWorkspaceModal";
import { SettingsPanel } from "./components/SettingsPanel";
import { StartSessionPicker } from "./components/StartSessionPicker";
import { FirstRunClaudeToast } from "./components/Toast";
import { useUiStore } from "./state/useUiStore";

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

function App(): JSX.Element {
  return (
    <QueryClientProvider client={queryClient}>
      <div className="flex h-screen bg-slate-950 text-slate-100 font-mono">
        <Sidebar />
        <DetailRouter />
        <NewWorkspaceModal />
        <SettingsPanel />
        <StartSessionPicker />
        <FirstRunClaudeToast />
      </div>
    </QueryClientProvider>
  );
}

function DetailRouter(): JSX.Element {
  const selectedWorkareaId = useUiStore((s) => s.selectedWorkareaId);
  if (selectedWorkareaId) {
    return <WorkareaDetail />;
  }
  return <WorkspaceDetail />;
}

export default App;
