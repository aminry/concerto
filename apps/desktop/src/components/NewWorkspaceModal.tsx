// "New Workspace" modal.
//
// Form contract (Task 25 public interface):
//   - name: text, non-empty.
//   - repository: single-select, required (V0.1 single-repo only;
//     `CreateWorkspaceRequest.repository_ids.len() == 1`).
//   - description: optional.
//
// Submit is disabled until both name + repo are present. On success
// the modal closes; the sidebar auto-refreshes via the existing
// `workspace.events` subscription wired in `Sidebar.tsx`.

import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../state/useUiStore";
import { createWorkspace } from "../api/workspaces";
import { listRepositories, type Repository } from "../api/repositories";
import { useQuery } from "@tanstack/react-query";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import { Input } from "./ui/input";

export function NewWorkspaceModal(): JSX.Element {
  const open = useUiStore((s) => s.newWorkspaceModalOpen);
  const setOpen = useUiStore((s) => s.setNewWorkspaceModalOpen);
  const projectId = useUiStore((s) => s.selectedProjectId);
  const queryClient = useQueryClient();

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [repositoryId, setRepositoryId] = useState<string>("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Reset form fields each time the dialog re-opens; this is the
  // cheapest way to keep stale typing out of the next session.
  useEffect(() => {
    if (open) {
      setName("");
      setDescription("");
      setRepositoryId("");
      setErrorMsg(null);
    }
  }, [open]);

  const reposQuery = useQuery({
    queryKey: ["repositories", projectId] as const,
    queryFn: async () => {
      if (!projectId) return { repositories: [] as Repository[] };
      return listRepositories(projectId);
    },
    enabled: open && !!projectId,
  });

  const mutation = useMutation({
    mutationFn: async () => {
      if (!projectId) throw new Error("no project selected");
      return createWorkspace({
        projectId,
        name: name.trim(),
        repositoryIds: [repositoryId],
        description: description.trim() || undefined,
      });
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      setOpen(false);
    },
    onError: (e) => {
      setErrorMsg(String(e));
    },
  });

  const canSubmit =
    !!projectId &&
    name.trim().length > 0 &&
    repositoryId.length > 0 &&
    !mutation.isPending;

  function onSubmit(e: React.FormEvent): void {
    e.preventDefault();
    if (!canSubmit) return;
    setErrorMsg(null);
    mutation.mutate();
  }

  return (
    <Dialog open={open} onClose={() => setOpen(false)} title="New Workspace">
      <form className="space-y-3" onSubmit={onSubmit}>
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Name
          </label>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. Test 1"
            autoFocus
          />
        </div>
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Repository
          </label>
          {reposQuery.isLoading && (
            <p className="text-xs text-faint">Loading repositories…</p>
          )}
          {reposQuery.isError && (
            <p className="text-xs text-err">
              Failed to load: {String(reposQuery.error)}
            </p>
          )}
          {reposQuery.data && reposQuery.data.repositories.length === 0 && (
            <p className="text-xs text-faint">
              No repositories yet. Add one from Settings first.
            </p>
          )}
          {reposQuery.data && reposQuery.data.repositories.length > 0 && (
            <select
              className="w-full rounded-md border border-border-strong bg-background px-2.5 py-1.5 text-sm text-foreground focus:outline-none focus-visible:ring-2 focus-visible:ring-accent"
              value={repositoryId}
              onChange={(e) => setRepositoryId(e.target.value)}
            >
              <option value="">— pick one —</option>
              {reposQuery.data.repositories.map((r) => (
                <option key={r.id} value={r.id}>
                  {r.name}
                </option>
              ))}
            </select>
          )}
        </div>
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Description <span className="text-faint">(optional)</span>
          </label>
          <Input
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder=""
          />
        </div>
        {errorMsg && <p className="text-xs text-err">{errorMsg}</p>}
        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" variant="ghost" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!canSubmit}>
            {mutation.isPending ? "Creating…" : "Create"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
