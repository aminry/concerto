// Workspace detail panel — shown when a workspace (not a workarea) is
// selected. Renders the design/15 §3.4 "When a workspace is selected"
// summary (the workspace's parallel workareas with status dots, a
// cross-workarea PR-set slot, and a "+ new workarea" affordance) via
// `WorkspaceSummary`, and hosts the create-workarea flow.
//
// Task 323 replaced the V0.1 `JSON.stringify` dump with the summary so a
// user can compare a workspace's parallel attempts at a glance.
//
// Task 322: "+ new workarea" opens a sparse-cone picker (a small dialog)
// so the user can size each repo's cone before the workarea is
// materialized. The chosen per-repo cones thread into `createWorkarea`,
// which applies them via `Repositories.SetCones` after create (see
// `api/workareas.ts`). A repo left blank inherits the workspace/repo cone
// defaults (the three-layer resolver, Task 302). Task 323 owns the summary
// list around this button; the create/cone-picker flow stays 322's — the
// summary's affordance just triggers the dialog the parent owns.

import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../state/useUiStore";
import { createWorkarea } from "../api/workareas";
import { listRepositories, type Repository } from "../api/repositories";
import { formatError } from "../api/errors";
import { ConePicker, coneSelections } from "./ConePicker";
import { WorkspaceSummary } from "./WorkspaceSummary";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";

export function WorkspaceDetail(): JSX.Element {
  const selectedWorkspaceId = useUiStore((s) => s.selectedWorkspaceId);
  const projectId = useUiStore((s) => s.selectedProjectId);
  const setWorkspaceExpanded = useUiStore((s) => s.setWorkspaceExpanded);
  const queryClient = useQueryClient();

  const [coneModalOpen, setConeModalOpen] = useState(false);
  // Raw cone text per repository id; reset each time the dialog opens.
  const [coneValues, setConeValues] = useState<Record<string, string>>({});

  const reposQuery = useQuery({
    queryKey: ["repositories", projectId] as const,
    queryFn: async () => {
      if (!projectId) return { repositories: [] as Repository[] };
      return listRepositories(projectId);
    },
    enabled: coneModalOpen && !!projectId,
  });
  const repos = reposQuery.data?.repositories ?? [];

  useEffect(() => {
    if (coneModalOpen) setConeValues({});
  }, [coneModalOpen]);

  const mutation = useMutation({
    mutationFn: async () => {
      if (!selectedWorkspaceId) throw new Error("no workspace selected");
      return createWorkarea(selectedWorkspaceId, {
        cones: coneSelections(repos, coneValues),
      });
    },
    onSuccess: () => {
      setConeModalOpen(false);
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
      <main className="h-full p-6 text-muted overflow-auto">
        <p>Select a workspace on the left to inspect it.</p>
      </main>
    );
  }

  return (
    <main className="h-full p-6 overflow-auto space-y-4">
      <WorkspaceSummary
        workspaceId={selectedWorkspaceId}
        onNewWorkarea={() => setConeModalOpen(true)}
      />
      {mutation.isError && (
        <p className="text-xs text-err">
          Failed to create workarea: {formatError(mutation.error)}
        </p>
      )}

      <Dialog
        open={coneModalOpen}
        onClose={() => setConeModalOpen(false)}
        title="New Workarea — sparse cones"
      >
        <div className="space-y-3">
          {reposQuery.isLoading && (
            <p className="text-xs text-faint">Loading repositories…</p>
          )}
          {reposQuery.isError && (
            <p className="text-xs text-err">
              Failed to load repositories: {formatError(reposQuery.error)}
            </p>
          )}
          {reposQuery.data && repos.length === 0 && (
            <p className="text-xs text-faint">
              This workspace has no repositories.
            </p>
          )}
          {repos.length > 0 && (
            <ConePicker
              repos={repos}
              values={coneValues}
              onChange={(repoId, raw) =>
                setConeValues((prev) => ({ ...prev, [repoId]: raw }))
              }
            />
          )}
          {mutation.isError && (
            <p role="alert" className="text-xs text-err">
              {formatError(mutation.error)}
            </p>
          )}
          <div className="flex justify-end gap-2 pt-1">
            <Button
              type="button"
              variant="ghost"
              onClick={() => setConeModalOpen(false)}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="primary"
              disabled={mutation.isPending}
              onClick={() => mutation.mutate()}
            >
              {mutation.isPending ? "Creating…" : "Create workarea"}
            </Button>
          </div>
        </div>
      </Dialog>
    </main>
  );
}
