// Placeholder detail panel. V0.1 renders the selected workspace's
// JSON; the real three-panel layout (composer status, agent
// transcript, diff viewer) arrives in Task 46+. Keeping the panel
// minimal here makes the event-driven invalidation easy to verify
// by eye during manual testing.

import { useUiStore } from "../state/useUiStore";
import { useWorkspace } from "../hooks/useWorkspaces";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";

export function WorkspaceDetail(): JSX.Element {
  const selectedWorkspaceId = useUiStore((s) => s.selectedWorkspaceId);
  const workspaceQuery = useWorkspace(selectedWorkspaceId);

  if (!selectedWorkspaceId) {
    return (
      <main className="flex-1 p-6 text-slate-400">
        <p>Select a workspace on the left to inspect it.</p>
      </main>
    );
  }

  return (
    <main className="flex-1 p-6 overflow-auto">
      <Card>
        <CardHeader>
          <CardTitle>Workspace</CardTitle>
        </CardHeader>
        <CardContent>
          {workspaceQuery.isLoading && <p>Loading…</p>}
          {workspaceQuery.isError && (
            <p className="text-rose-400">
              Failed to load: {String(workspaceQuery.error)}
            </p>
          )}
          {workspaceQuery.data && (
            <pre className="text-xs whitespace-pre-wrap text-emerald-300">
              {JSON.stringify(workspaceQuery.data, null, 2)}
            </pre>
          )}
        </CardContent>
      </Card>
    </main>
  );
}
