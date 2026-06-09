// Edit an existing workspace — same form as create (WorkspaceForm in
// "edit" mode), pre-filled from the workspace + its declared repos/cones.
// Repo edits affect future workareas only; existing workareas keep their
// worktrees (a notice surfaces this when the workspace has workareas).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useUiStore } from "../state/useUiStore";
import { getWorkspace, listWorkspaceRepos, updateWorkspace } from "../api/workspaces";
import { listWorkareas } from "../api/workareas";
import { formatError } from "../api/errors";
import { Dialog } from "./ui/dialog";
import {
  WorkspaceForm,
  type WorkspaceFormInitial,
  type WorkspaceFormSubmit,
} from "./WorkspaceForm";

export function EditWorkspaceModal(): JSX.Element {
  const editId = useUiStore((s) => s.editWorkspaceId);
  const setEditId = useUiStore((s) => s.setEditWorkspaceId);
  const queryClient = useQueryClient();
  const open = editId !== null;

  const wsQuery = useQuery({
    queryKey: ["workspace", editId],
    queryFn: () => getWorkspace(editId as string),
    enabled: open,
  });
  const reposQuery = useQuery({
    queryKey: ["workspaceRepos", editId],
    queryFn: () => listWorkspaceRepos(editId as string),
    enabled: open,
  });
  const workareasQuery = useQuery({
    queryKey: ["workareas", editId],
    queryFn: () => listWorkareas(editId as string),
    enabled: open,
  });

  const mutation = useMutation({
    mutationFn: (values: WorkspaceFormSubmit) =>
      updateWorkspace({
        id: editId as string,
        name: values.name,
        icon: values.icon,
        description: values.description,
        repos: values.repos,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      void queryClient.invalidateQueries({ queryKey: ["workspace", editId] });
      void queryClient.invalidateQueries({ queryKey: ["workspaceRepos", editId] });
      setEditId(null);
    },
  });

  if (!open) return <></>;

  const loading = wsQuery.isLoading || reposQuery.isLoading;
  const ws = wsQuery.data;
  const initial: WorkspaceFormInitial | undefined =
    ws && reposQuery.data
      ? {
          name: ws.name,
          icon: ws.icon ?? "",
          description: ws.description ?? "",
          selectionOrder: reposQuery.data.repos.map((r) => r.repository_id),
          selected: Object.fromEntries(
            reposQuery.data.repos.map((r) => [
              r.repository_id,
              {
                mode: (r.sparse_cones.length > 0 ? "sparse" : "full") as
                  | "full"
                  | "sparse",
                cones: r.sparse_cones,
              },
            ]),
          ),
        }
      : undefined;

  const hasWorkareas = (workareasQuery.data?.workareas?.length ?? 0) > 0;

  return (
    <Dialog open={open} onClose={() => setEditId(null)} title="Edit Workspace">
      {loading || !initial ? (
        <p className="text-xs text-faint">Loading workspace…</p>
      ) : (
        <WorkspaceForm
          mode="edit"
          initial={initial}
          submitLabel="Save changes"
          pendingLabel="Saving…"
          pending={mutation.isPending}
          externalError={mutation.isError ? formatError(mutation.error) : null}
          notice={
            hasWorkareas
              ? "Repo changes apply to new workareas; existing workareas keep their current repos."
              : null
          }
          onCancel={() => setEditId(null)}
          onSubmit={(values) => mutation.mutate(values)}
        />
      )}
    </Dialog>
  );
}
