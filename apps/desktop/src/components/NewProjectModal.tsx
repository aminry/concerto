// "New Project" modal.
//
// Form contract:
//   - name: text, required (trimmed, non-empty).
//   - icon: optional emoji / short string.
//   - First repository (all optional): if a URL is given we add + clone a
//     repository into the new project in the same flow, with the same
//     clone-strategy picker / size→strategy recommendation as Settings →
//     "Add Repository" (shared `useCloneStrategy`). Leaving the URL blank
//     creates an empty project, exactly as before.
//
// On success, invalidates the `["projects"]` query so the sidebar
// re-renders and the new project is auto-selected. When a repo was added we
// also invalidate `["repositories", projectId]`; the clone runs in the
// background and its progress is visible in Settings → "Add Repository".

import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { createProject } from "../api/projects";
import { addRepository } from "../api/repositories";
import { cloneRepository } from "../api/client";
import { formatError } from "../api/errors";
import { useUiStore } from "../state/useUiStore";
import { CloneStrategyPicker, useCloneStrategy } from "./cloneStrategy";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import { Input } from "./ui/input";

/// Best-effort repository name from a git URL: the last path segment with any
/// trailing `.git` / slash stripped (e.g. `…/acme/web.git` → `web`). Used as
/// the default when the user leaves the repo name blank.
function deriveRepoName(url: string): string {
  const trimmed = url.trim().replace(/\/+$/, "");
  const last = trimmed.split(/[/\\]/).pop() ?? "";
  return last.replace(/\.git$/i, "");
}

export function NewProjectModal(): JSX.Element {
  const open = useUiStore((s) => s.newProjectModalOpen);
  const setOpen = useUiStore((s) => s.setNewProjectModalOpen);
  const setSelectedProject = useUiStore((s) => s.setSelectedProject);
  const queryClient = useQueryClient();

  const [name, setName] = useState("");
  const [icon, setIcon] = useState("");
  const [repoUrl, setRepoUrl] = useState("");
  const [repoName, setRepoName] = useState("");
  const [repoBranch, setRepoBranch] = useState("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Shared clone-strategy picker + size probe over the repo URL.
  const cloneStrategy = useCloneStrategy(repoUrl);
  const resetStrategy = cloneStrategy.reset;

  useEffect(() => {
    if (open) {
      setName("");
      setIcon("");
      setRepoUrl("");
      setRepoName("");
      setRepoBranch("");
      setErrorMsg(null);
      resetStrategy();
    }
  }, [open, resetStrategy]);

  const mutation = useMutation({
    mutationFn: async () => {
      const project = await createProject({
        name: name.trim(),
        icon: icon.trim() || undefined,
      });

      // Optionally add + clone a first repository in the same flow.
      const trimmedUrl = repoUrl.trim();
      if (trimmedUrl) {
        const wire = cloneStrategy.wire;
        const repo = await addRepository({
          projectId: project.id,
          name: repoName.trim() || deriveRepoName(trimmedUrl) || "repo",
          url: trimmedUrl,
          defaultBranch: repoBranch.trim() || undefined,
          cloneStrategy: wire.cloneStrategy,
          withSparse: wire.withSparse,
        });
        // Kick the clone off in the background — progress shows in Settings.
        void cloneRepository(repo.id)
          .then(() => {
            void queryClient.invalidateQueries({
              queryKey: ["repositories", project.id],
            });
          })
          .catch(() => {
            /* surfaced in Settings → Add Repository */
          });
      }

      return project;
    },
    onSuccess: (project) => {
      void queryClient.invalidateQueries({ queryKey: ["projects"] });
      void queryClient.invalidateQueries({
        queryKey: ["repositories", project.id],
      });
      setSelectedProject(project.id);
      setOpen(false);
    },
    onError: (e) => {
      setErrorMsg(formatError(e));
    },
  });

  const canSubmit = name.trim().length > 0 && !mutation.isPending;
  const willAddRepo = repoUrl.trim().length > 0;

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

        <div className="space-y-3 rounded border border-border p-3">
          <p className="text-xs uppercase tracking-wider text-slate-500">
            First repository <span className="text-slate-600">(optional)</span>
          </p>
          <div>
            <label className="block text-xs uppercase tracking-wider text-faint mb-1">
              URL
            </label>
            <Input
              value={repoUrl}
              onChange={(e) => setRepoUrl(e.target.value)}
              placeholder="git URL or file://"
            />
          </div>

          <CloneStrategyPicker {...cloneStrategy.pickerProps} />

          <div>
            <label className="block text-xs uppercase tracking-wider text-faint mb-1">
              Name <span className="text-faint">(optional)</span>
            </label>
            <Input
              value={repoName}
              onChange={(e) => setRepoName(e.target.value)}
              placeholder={
                willAddRepo ? deriveRepoName(repoUrl) || "repo" : "short label"
              }
            />
          </div>
          <div>
            <label className="block text-xs uppercase tracking-wider text-faint mb-1">
              Default branch <span className="text-faint">(optional)</span>
            </label>
            <Input
              value={repoBranch}
              onChange={(e) => setRepoBranch(e.target.value)}
              placeholder="main"
            />
          </div>
        </div>

        {errorMsg && <p className="text-xs text-rose-400">{errorMsg}</p>}
        <div className="flex justify-end gap-2 pt-2">
          <Button type="button" variant="ghost" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button type="submit" disabled={!canSubmit}>
            {mutation.isPending
              ? "Creating…"
              : willAddRepo
                ? "Create + Clone"
                : "Create"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
