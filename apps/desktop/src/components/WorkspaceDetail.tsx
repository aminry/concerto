// Workspace detail panel. Renders the selected workspace's JSON and
// hosts the "+ New Workarea" button. V0.1 deliberately keeps the panel
// narrow; the real three-panel layout (composer status, agent
// transcript, diff viewer) arrives in Task 46+.

import { useMutation, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../state/useUiStore";
import { useWorkspace } from "../hooks/useWorkspaces";
import { createWorkarea } from "../api/workareas";
import { Button } from "./ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";

export function WorkspaceDetail(): JSX.Element {
  const selectedWorkspaceId = useUiStore((s) => s.selectedWorkspaceId);
  const setWorkspaceExpanded = useUiStore((s) => s.setWorkspaceExpanded);
  const workspaceQuery = useWorkspace(selectedWorkspaceId);
  const queryClient = useQueryClient();

  const mutation = useMutation({
    mutationFn: async () => {
      if (!selectedWorkspaceId) throw new Error("no workspace selected");
      return createWorkarea(selectedWorkspaceId);
    },
    onSuccess: () => {
      if (selectedWorkspaceId) {
        // Expand the parent workspace so the new workarea is visible.
        setWorkspaceExpanded(selectedWorkspaceId, true);
        void queryClient.invalidateQueries({
          queryKey: ["workareas", selectedWorkspaceId],
        });
      }
    },
  });

  if (!selectedWorkspaceId) {
    return (
      <main className="h-full p-6 text-slate-400 overflow-auto">
        <p>Select a workspace on the left to inspect it.</p>
      </main>
    );
  }

  return (
    <main className="h-full p-6 overflow-auto space-y-4">
      <div className="flex justify-end">
        <Button
          disabled={mutation.isPending}
          onClick={() => mutation.mutate()}
        >
          {mutation.isPending ? "Creating…" : "+ New Workarea"}
        </Button>
      </div>
      {mutation.isError && (
        <p className="text-xs text-rose-400">
          Failed to create workarea: {String(mutation.error)}
        </p>
      )}
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
