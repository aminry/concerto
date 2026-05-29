// "Add Repository" form. Lives inside the Settings panel.
//
// Flow (Task 25 §Scope-in):
//   1. User submits { name, url } → call `Repositories.AddRepository`.
//   2. Kick off `clone_repository(repository_id)`; subscribe to
//      `concerto/clone-progress/<id>` events; render a `<Progress>` bar.
//   3. On `done: true` (or stream end), invalidate the
//      `["repositories", projectId]` query so the list refreshes.
//
// We deliberately do not block the form during clone — the user can
// add another repo while the previous one is still cloning.

import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../state/useUiStore";
import {
  addRepository,
  listRepositories,
  type Repository,
} from "../api/repositories";
import {
  cloneRepository,
  onCloneProgress,
  type CloneProgressEvent,
} from "../api/client";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Progress } from "./ui/progress";

type CloneState = {
  repositoryId: string;
  repoName: string;
  progress: CloneProgressEvent | null;
  done: boolean;
  error: string | null;
};

export function AddRepositoryForm(): JSX.Element {
  const projectId = useUiStore((s) => s.selectedProjectId);
  const queryClient = useQueryClient();

  const [url, setUrl] = useState("");
  const [name, setName] = useState("");
  const [defaultBranch, setDefaultBranch] = useState("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [clones, setClones] = useState<CloneState[]>([]);
  // Stash setClones updater in a ref so the per-clone event listener
  // (registered once per clone) can call back into the latest state
  // without re-subscribing on each render.
  const setClonesRef = useRef(setClones);
  setClonesRef.current = setClones;

  const reposQuery = useQuery({
    queryKey: ["repositories", projectId] as const,
    queryFn: async () => {
      if (!projectId) return { repositories: [] as Repository[] };
      return listRepositories(projectId);
    },
    enabled: !!projectId,
  });

  // Tear down clone-progress listeners when the component unmounts.
  const unlistenersRef = useRef<Array<() => void>>([]);
  useEffect(() => {
    return () => {
      for (const unlisten of unlistenersRef.current) {
        unlisten();
      }
      unlistenersRef.current = [];
    };
  }, []);

  const mutation = useMutation({
    mutationFn: async () => {
      if (!projectId) throw new Error("no project selected");
      const trimmedUrl = url.trim();
      const trimmedName = name.trim();
      if (!trimmedUrl) throw new Error("url is required");
      if (!trimmedName) throw new Error("name is required");
      const repo = await addRepository({
        projectId,
        name: trimmedName,
        url: trimmedUrl,
        defaultBranch: defaultBranch.trim() || undefined,
      });
      return repo;
    },
    onSuccess: async (repo) => {
      setUrl("");
      setName("");
      setDefaultBranch("");
      setErrorMsg(null);
      void queryClient.invalidateQueries({
        queryKey: ["repositories", projectId],
      });

      setClonesRef.current((prev) => [
        ...prev,
        {
          repositoryId: repo.id,
          repoName: repo.name,
          progress: null,
          done: false,
          error: null,
        },
      ]);

      const unlisten = await onCloneProgress(repo.id, (payload) => {
        setClonesRef.current((prev) =>
          prev.map((c) =>
            c.repositoryId === repo.id
              ? { ...c, progress: payload, done: payload.done || c.done }
              : c,
          ),
        );
      });
      unlistenersRef.current.push(unlisten);

      // Fire-and-forget — promise resolves when the stream completes.
      void cloneRepository(repo.id)
        .then(() => {
          setClonesRef.current((prev) =>
            prev.map((c) =>
              c.repositoryId === repo.id ? { ...c, done: true } : c,
            ),
          );
          void queryClient.invalidateQueries({
            queryKey: ["repositories", projectId],
          });
        })
        .catch((e: unknown) => {
          setClonesRef.current((prev) =>
            prev.map((c) =>
              c.repositoryId === repo.id ? { ...c, error: String(e) } : c,
            ),
          );
        });
    },
    onError: (e) => {
      setErrorMsg(String(e));
    },
  });

  function onSubmit(e: React.FormEvent): void {
    e.preventDefault();
    if (mutation.isPending || !projectId) return;
    setErrorMsg(null);
    mutation.mutate();
  }

  return (
    <section className="space-y-4">
      <h3 className="text-sm font-semibold uppercase tracking-wider text-muted">
        Add Repository
      </h3>
      <form className="space-y-3" onSubmit={onSubmit}>
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            URL
          </label>
          <Input
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="git URL or file://"
          />
        </div>
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Name
          </label>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="short label"
          />
        </div>
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Default branch <span className="text-faint">(optional)</span>
          </label>
          <Input
            value={defaultBranch}
            onChange={(e) => setDefaultBranch(e.target.value)}
            placeholder="main"
          />
        </div>
        {errorMsg && <p className="text-xs text-err">{errorMsg}</p>}
        <div className="flex justify-end">
          <Button
            type="submit"
            variant="primary"
            disabled={mutation.isPending || !projectId || !url || !name}
          >
            {mutation.isPending ? "Adding…" : "Add + Clone"}
          </Button>
        </div>
      </form>

      <div className="space-y-3">
        <h4 className="text-xs uppercase tracking-wider text-faint">
          Repositories
        </h4>
        {reposQuery.isLoading && (
          <p className="text-xs text-faint">Loading…</p>
        )}
        {reposQuery.data && reposQuery.data.repositories.length === 0 && (
          <p className="text-xs text-faint">None yet.</p>
        )}
        <ul className="space-y-1">
          {reposQuery.data?.repositories.map((r) => {
            const clone = clones.find((c) => c.repositoryId === r.id);
            return (
              <li
                key={r.id}
                className="rounded border border-border px-3 py-2 text-xs"
              >
                <p className="text-foreground">{r.name}</p>
                <p className="font-mono text-faint truncate">{r.url}</p>
                {clone && !clone.done && !clone.error && (
                  <div className="mt-2 space-y-1">
                    <Progress value={percentFromProgress(clone.progress)} />
                    <p className="text-faint">
                      {clone.progress?.phase ?? "starting…"}
                    </p>
                  </div>
                )}
                {clone?.done && (
                  <p className="mt-1 text-ok">clone complete</p>
                )}
                {clone?.error && (
                  <p className="mt-1 text-err">{clone.error}</p>
                )}
              </li>
            );
          })}
        </ul>
      </div>
    </section>
  );
}

function percentFromProgress(p: CloneProgressEvent | null): number {
  if (!p) return 0;
  if (p.total_objects === 0) return 0;
  return Math.round((p.objects_received / p.total_objects) * 100);
}
