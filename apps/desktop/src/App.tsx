// Top-level layout: React Query provider wrapping a fixed
// two-pane split (sidebar + workspace detail). The three-panel
// layout from `design/15 §3.4` lands in Task 46+; V0.1 keeps the
// surface intentionally narrow so the event-driven invalidation
// path is the only "interesting" thing on screen.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import { Sidebar } from "./components/Sidebar";
import { WorkspaceDetail } from "./components/WorkspaceDetail";

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
        <WorkspaceDetail />
      </div>
    </QueryClientProvider>
  );
}

export default App;
