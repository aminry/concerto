// "New Workspace" modal.
//
// Form contract (Task 322 — multi-repo, lifting Task 25's single-repo
// form):
//   - name: text, non-empty.
//   - repositories: multi-select (checkbox list), ≥1 required. Produces
//     `CreateWorkspaceRequest.repository_ids: string[]`. Task 306 relaxed
//     the Core's single-repo guard (now rejects only 0 repos); the form
//     mirrors that — submit gates on ≥1 selected.
//   - description: optional.
//
// Submit is disabled until name is non-empty AND ≥1 repo is checked. On
// success the modal closes; the sidebar auto-refreshes via the existing
// `workspace.events` subscription wired in `Sidebar.tsx`.

import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../state/useUiStore";
import { createWorkspace } from "../api/workspaces";
import { listRepositories, type Repository } from "../api/repositories";
import { formatError } from "../api/errors";
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
  const [repositoryIds, setRepositoryIds] = useState<string[]>([]);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Reset form fields each time the dialog re-opens; this is the
  // cheapest way to keep stale typing out of the next session.
  useEffect(() => {
    if (open) {
      setName("");
      setDescription("");
      setRepositoryIds([]);
      setErrorMsg(null);
    }
  }, [open]);

  function toggleRepo(id: string): void {
    setRepositoryIds((prev) =>
      prev.includes(id) ? prev.filter((r) => r !== id) : [...prev, id],
    );
  }

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
        repositoryIds,
        description: description.trim() || undefined,
      });
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      setOpen(false);
    },
    onError: (e) => {
      setErrorMsg(formatError(e));
    },
  });

  const canSubmit =
    !!projectId &&
    name.trim().length > 0 &&
    repositoryIds.length > 0 &&
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
            Repositories <span className="text-faint">(one or more)</span>
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
            <ul
              role="group"
              aria-label="Repositories"
              className="max-h-48 overflow-y-auto rounded-md border border-border-strong bg-background divide-y divide-border"
            >
              {reposQuery.data.repositories.map((r) => (
                <li key={r.id}>
                  <label className="flex items-center gap-2 px-2.5 py-1.5 text-sm text-foreground cursor-pointer hover:bg-surface-2">
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={repositoryIds.includes(r.id)}
                      onChange={() => toggleRepo(r.id)}
                    />
                    <span className="truncate">{r.name}</span>
                  </label>
                </li>
              ))}
            </ul>
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
