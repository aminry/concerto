// "New Project" modal.
//
// Form contract:
//   - name: text, required (trimmed, non-empty).
//   - icon: optional emoji / short string.
//
// On success, invalidates the `["projects"]` query so the sidebar
// re-renders and the new project is auto-selected by the existing
// "pin the first project" effect in `Sidebar.tsx`.

import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { createProject } from "../api/projects";
import { formatError } from "../api/errors";
import { useUiStore } from "../state/useUiStore";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import { Input } from "./ui/input";

export function NewProjectModal(): JSX.Element {
  const open = useUiStore((s) => s.newProjectModalOpen);
  const setOpen = useUiStore((s) => s.setNewProjectModalOpen);
  const setSelectedProject = useUiStore((s) => s.setSelectedProject);
  const queryClient = useQueryClient();

  const [name, setName] = useState("");
  const [icon, setIcon] = useState("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setName("");
      setIcon("");
      setErrorMsg(null);
    }
  }, [open]);

  const mutation = useMutation({
    mutationFn: async () => {
      return createProject({
        name: name.trim(),
        icon: icon.trim() || undefined,
      });
    },
    onSuccess: (project) => {
      void queryClient.invalidateQueries({ queryKey: ["projects"] });
      setSelectedProject(project.id);
      setOpen(false);
    },
    onError: (e) => {
      setErrorMsg(formatError(e));
    },
  });

  const canSubmit = name.trim().length > 0 && !mutation.isPending;

  function onSubmit(e: React.FormEvent): void {
    e.preventDefault();
    if (!canSubmit) return;
    setErrorMsg(null);
    mutation.mutate();
  }

  return (
    <Dialog open={open} onClose={() => setOpen(false)} title="New Project">
      <form className="space-y-3" onSubmit={onSubmit}>
        <div>
          <label className="block text-xs uppercase tracking-wider text-slate-500 mb-1">
            Name
          </label>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. Concerto"
            autoFocus
          />
        </div>
        <div>
          <label className="block text-xs uppercase tracking-wider text-slate-500 mb-1">
            Icon <span className="text-slate-600">(optional)</span>
          </label>
          <Input
            value={icon}
            onChange={(e) => setIcon(e.target.value)}
            placeholder="emoji or short label"
          />
        </div>
        {errorMsg && <p className="text-xs text-rose-400">{errorMsg}</p>}
        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" variant="ghost" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button type="submit" disabled={!canSubmit}>
            {mutation.isPending ? "Creating…" : "Create"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
