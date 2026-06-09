// "New Workspace" modal — the primary creation flow after the
// Project→Workspace collapse.
//
// A workspace is a top-level node over the GLOBAL repository registry. This
// modal lets the user:
//   - name the workspace (+ optional icon + optional description).
//   - assemble its repos from THREE sources:
//       1. multi-select EXISTING registry repos (`listRepositories()`).
//       2. "Add by URL" — register a new repo via `addRepository({ url })`
//          with the shared clone-strategy picker + size→strategy
//          recommendation; the new repo lands selected.
//       3. "Add local folder" — open the native Tauri folder picker
//          (`@tauri-apps/plugin-dialog` → `open({ directory: true })`) and
//          adopt the on-disk repo via `addRepository({ localPath, name })`.
//   - for each SELECTED repo, choose a per-repo checkout: "Full working
//     tree" vs "Sparse" (the `RepoTreeBrowser` pre-seeded from that repo's
//     `cone_defaults`).
//
// On submit it assembles `repos: { repositoryId, sparseCones }[]`
// (sparseCones = [] for full) and calls `createWorkspace`, then invalidates
// the workspace list and selects the new workspace.

import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { ChevronDown, ChevronRight, FolderOpen, Plus, X } from "lucide-react";

import { useUiStore } from "../state/useUiStore";
import { createWorkspace } from "../api/workspaces";
import {
  addRepository,
  listRepositories,
  type Repository,
} from "../api/repositories";
import { cloneRepository } from "../api/client";
import { formatError } from "../api/errors";
import { CloneStrategyPicker, useCloneStrategy } from "./cloneStrategy";
import {
  normalizeConeSelection,
  RepoTreeBrowser,
} from "./RepoTreeBrowser";
import { Button } from "./ui/button";
import { Dialog } from "./ui/dialog";
import { Input } from "./ui/input";

/// Best-effort repository name from a git URL or filesystem path: the last
/// path segment with any trailing `.git` / slash stripped.
export function deriveRepoName(input: string): string {
  const trimmed = input.trim().replace(/\/+$/, "");
  const last = trimmed.split(/[/\\]/).pop() ?? "";
  return last.replace(/\.git$/i, "");
}

/// Per-selected-repo checkout choice. `mode: "full"` ⇒ whole working tree
/// (`sparseCones: []`); `mode: "sparse"` ⇒ the chosen cone directories.
type RepoCheckout = {
  mode: "full" | "sparse";
  cones: string[];
};

export function NewWorkspaceModal(): JSX.Element {
  const open = useUiStore((s) => s.newWorkspaceModalOpen);
  const setOpen = useUiStore((s) => s.setNewWorkspaceModalOpen);
  const setSelectedWorkspace = useUiStore((s) => s.setSelectedWorkspace);
  const queryClient = useQueryClient();

  const [name, setName] = useState("");
  const [icon, setIcon] = useState("");
  const [description, setDescription] = useState("");
  // The selected repo ids, in selection order, each with its checkout choice.
  const [selected, setSelected] = useState<Record<string, RepoCheckout>>({});
  const [selectionOrder, setSelectionOrder] = useState<string[]>([]);
  const [repoSearch, setRepoSearch] = useState("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Which add-source sub-panel is open ("url" | "local" | null).
  const [addSource, setAddSource] = useState<"url" | "local" | null>(null);

  // Reset the whole form each time the dialog re-opens.
  useEffect(() => {
    if (open) {
      setName("");
      setIcon("");
      setDescription("");
      setSelected({});
      setSelectionOrder([]);
      setRepoSearch("");
      setErrorMsg(null);
      setAddSource(null);
    }
  }, [open]);

  const reposQuery = useQuery({
    queryKey: ["repositories"] as const,
    queryFn: () => listRepositories(),
    enabled: open,
  });
  const repos = useMemo(
    () => reposQuery.data?.repositories ?? [],
    [reposQuery.data],
  );
  const repoById = useMemo(() => {
    const m = new Map<string, Repository>();
    for (const r of repos) m.set(r.id, r);
    return m;
  }, [repos]);

  const filteredRepos = useMemo(() => {
    const q = repoSearch.trim().toLowerCase();
    if (!q) return repos;
    return repos.filter((r) => r.name.toLowerCase().includes(q));
  }, [repos, repoSearch]);

  function selectRepo(id: string, repo?: Repository): void {
    setSelected((prev) => {
      if (prev[id]) return prev;
      const resolved = repo ?? repoById.get(id);
      return {
        ...prev,
        [id]: {
          mode: "full",
          cones: normalizeConeSelection(resolved?.cone_defaults ?? []),
        },
      };
    });
    setSelectionOrder((prev) => (prev.includes(id) ? prev : [...prev, id]));
  }

  function deselectRepo(id: string): void {
    setSelected((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
    setSelectionOrder((prev) => prev.filter((x) => x !== id));
  }

  function toggleRepo(id: string): void {
    if (selected[id]) deselectRepo(id);
    else selectRepo(id);
  }

  function setCheckout(id: string, patch: Partial<RepoCheckout>): void {
    setSelected((prev) => {
      const cur = prev[id];
      if (!cur) return prev;
      return { ...prev, [id]: { ...cur, ...patch } };
    });
  }

  // After a newly-added repo lands in the registry, refresh the list +
  // auto-select it (so the URL / local flows feel continuous).
  async function afterRepoAdded(repo: Repository): Promise<void> {
    await queryClient.invalidateQueries({ queryKey: ["repositories"] });
    // Pass the just-returned Repository directly so cone_defaults are seeded
    // from it rather than from the repoById memo (which hasn't re-rendered
    // yet after the invalidation).
    selectRepo(repo.id, repo);
    setAddSource(null);
  }

  const mutation = useMutation({
    mutationFn: async () => {
      const reposPayload = selectionOrder.map((id) => {
        const checkout = selected[id];
        return {
          repositoryId: id,
          sparseCones:
            checkout?.mode === "sparse" ? checkout.cones : [],
        };
      });
      return createWorkspace({
        name: name.trim(),
        icon: icon.trim() || undefined,
        description: description.trim() || undefined,
        repos: reposPayload,
      });
    },
    onSuccess: (workspace) => {
      void queryClient.invalidateQueries({ queryKey: ["workspaces"] });
      setSelectedWorkspace(workspace.id);
      setOpen(false);
    },
    onError: (e) => {
      setErrorMsg(formatError(e));
    },
  });

  const canSubmit =
    name.trim().length > 0 &&
    selectionOrder.length > 0 &&
    !mutation.isPending;

  function onSubmit(e: React.FormEvent): void {
    e.preventDefault();
    if (!canSubmit) return;
    setErrorMsg(null);
    mutation.mutate();
  }

  return (
    <Dialog open={open} onClose={() => setOpen(false)} title="New Workspace">
      <form className="space-y-4" onSubmit={onSubmit}>
        <div className="grid grid-cols-[1fr_5rem] gap-2">
          <div>
            <label className="block text-xs uppercase tracking-wider text-faint mb-1">
              Name
            </label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Payments revamp"
              autoFocus
            />
          </div>
          <div>
            <label className="block text-xs uppercase tracking-wider text-faint mb-1">
              Icon
            </label>
            <Input
              value={icon}
              onChange={(e) => setIcon(e.target.value)}
              placeholder="🧩"
              aria-label="Icon"
            />
          </div>
        </div>

        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Description <span className="text-faint">(optional)</span>
          </label>
          <Input
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="What is this workspace for?"
          />
        </div>

        {/* ── Repo picker ─────────────────────────────────────────── */}
        <div className="space-y-2">
          <label className="block text-xs uppercase tracking-wider text-faint">
            Repositories <span className="text-faint">(one or more)</span>
          </label>

          {/* Source 1: existing registry repos. */}
          <Input
            value={repoSearch}
            onChange={(e) => setRepoSearch(e.target.value)}
            placeholder="Search repositories…"
            aria-label="Search repositories"
          />
          {reposQuery.isLoading && (
            <p className="text-xs text-faint">Loading repositories…</p>
          )}
          {reposQuery.isError && (
            <p className="text-xs text-err">
              Failed to load: {formatError(reposQuery.error)}
            </p>
          )}
          {reposQuery.data && repos.length === 0 && (
            <p className="text-xs text-faint">
              No repositories yet — add one by URL or local folder below.
            </p>
          )}
          {filteredRepos.length > 0 && (
            <ul
              role="group"
              aria-label="Repositories"
              className="max-h-40 overflow-y-auto rounded-md border border-border-strong bg-background divide-y divide-border"
            >
              {filteredRepos.map((r) => (
                <li key={r.id}>
                  <label className="flex items-center gap-2 px-2.5 py-1.5 text-sm text-foreground cursor-pointer hover:bg-surface-2">
                    <input
                      type="checkbox"
                      className="accent-accent"
                      checked={!!selected[r.id]}
                      onChange={() => toggleRepo(r.id)}
                    />
                    <span className="truncate">{r.name}</span>
                    {r.clone_strategy === "blobless" && (
                      <span className="ml-auto text-[10px] uppercase tracking-wide text-faint">
                        blobless
                      </span>
                    )}
                  </label>
                </li>
              ))}
            </ul>
          )}

          {/* Sources 2 + 3: add a new repo. */}
          <div className="flex gap-2">
            <Button
              type="button"
              variant={addSource === "url" ? "primary" : "outline"}
              size="sm"
              onClick={() =>
                setAddSource((s) => (s === "url" ? null : "url"))
              }
            >
              <Plus size={13} /> Add by URL
            </Button>
            <Button
              type="button"
              variant={addSource === "local" ? "primary" : "outline"}
              size="sm"
              onClick={() =>
                setAddSource((s) => (s === "local" ? null : "local"))
              }
            >
              <FolderOpen size={13} /> Add local folder
            </Button>
          </div>

          {addSource === "url" && (
            <AddByUrlPanel
              onAdded={afterRepoAdded}
              onError={setErrorMsg}
            />
          )}
          {addSource === "local" && (
            <AddLocalFolderPanel onAdded={afterRepoAdded} onError={setErrorMsg} />
          )}
        </div>

        {/* ── Per-repo checkout controls ──────────────────────────── */}
        {selectionOrder.length > 0 && (
          <div className="space-y-2">
            <label className="block text-xs uppercase tracking-wider text-faint">
              Checkout
            </label>
            <div className="space-y-2">
              {selectionOrder.map((id) => {
                const repo = repoById.get(id);
                const checkout = selected[id];
                if (!repo || !checkout) return null;
                return (
                  <RepoCheckoutRow
                    key={id}
                    repo={repo}
                    checkout={checkout}
                    onModeChange={(mode) => setCheckout(id, { mode })}
                    onConesChange={(cones) => setCheckout(id, { cones })}
                    onRemove={() => deselectRepo(id)}
                  />
                );
              })}
            </div>
          </div>
        )}

        {errorMsg && <p className="text-xs text-err">{errorMsg}</p>}

        <div className="flex justify-end gap-2 pt-1">
          <Button type="button" variant="ghost" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!canSubmit}>
            {mutation.isPending ? "Creating…" : "Create Workspace"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

/// Source 2 — "Add by URL". Reuses the shared clone-strategy picker +
/// size→strategy recommendation, registers the repo, kicks off the clone in
/// the background, and reports the new repo back so it lands selected.
function AddByUrlPanel({
  onAdded,
  onError,
}: {
  onAdded: (repo: Repository) => Promise<void> | void;
  onError: (msg: string | null) => void;
}): JSX.Element {
  const queryClient = useQueryClient();
  const [url, setUrl] = useState("");
  const [repoName, setRepoName] = useState("");
  const [branch, setBranch] = useState("");
  const cloneStrategy = useCloneStrategy(url);

  const mutation = useMutation({
    mutationFn: async () => {
      const trimmedUrl = url.trim();
      if (!trimmedUrl) throw new Error("url is required");
      const wire = cloneStrategy.wire;
      const repo = await addRepository({
        name: repoName.trim() || deriveRepoName(trimmedUrl) || "repo",
        url: trimmedUrl,
        defaultBranch: branch.trim() || undefined,
        cloneStrategy: wire.cloneStrategy,
        withSparse: wire.withSparse,
      });
      // Kick the clone off in the background; progress shows in Settings.
      void cloneRepository(repo.id)
        .then(() => {
          void queryClient.invalidateQueries({ queryKey: ["repositories"] });
        })
        .catch(() => {
          /* surfaced in Settings → Add Repository */
        });
      return repo;
    },
    onSuccess: async (repo) => {
      onError(null);
      setUrl("");
      setRepoName("");
      setBranch("");
      cloneStrategy.reset();
      await onAdded(repo);
    },
    onError: (e) => onError(formatError(e)),
  });

  return (
    <div className="space-y-2 rounded-md border border-border p-3">
      <div>
        <label className="block text-xs uppercase tracking-wider text-faint mb-1">
          URL
        </label>
        <Input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="git URL or file://"
          aria-label="Repository URL"
        />
      </div>
      <CloneStrategyPicker {...cloneStrategy.pickerProps} />
      <div className="grid grid-cols-2 gap-2">
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Name <span className="text-faint">(optional)</span>
          </label>
          <Input
            value={repoName}
            onChange={(e) => setRepoName(e.target.value)}
            placeholder={deriveRepoName(url) || "repo"}
            aria-label="Repository name"
          />
        </div>
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Default branch <span className="text-faint">(optional)</span>
          </label>
          <Input
            value={branch}
            onChange={(e) => setBranch(e.target.value)}
            placeholder="main"
            aria-label="Default branch"
          />
        </div>
      </div>
      <div className="flex justify-end">
        <Button
          type="button"
          variant="primary"
          size="sm"
          disabled={mutation.isPending || !url.trim()}
          onClick={() => mutation.mutate()}
        >
          {mutation.isPending ? "Adding…" : "Add repository"}
        </Button>
      </div>
    </div>
  );
}

/// Source 3 — "Add local folder". Opens the native folder picker and adopts
/// the chosen on-disk git repo in place (non-destructive).
function AddLocalFolderPanel({
  onAdded,
  onError,
}: {
  onAdded: (repo: Repository) => Promise<void> | void;
  onError: (msg: string | null) => void;
}): JSX.Element {
  const [path, setPath] = useState<string | null>(null);
  const [repoName, setRepoName] = useState("");

  const browseMutation = useMutation({
    mutationFn: async () => {
      const picked = await openDialog({
        directory: true,
        multiple: false,
        title: "Choose a local git repository",
      });
      return typeof picked === "string" ? picked : null;
    },
    onSuccess: (picked) => {
      if (picked) {
        setPath(picked);
        if (!repoName.trim()) setRepoName(deriveRepoName(picked));
      }
    },
    onError: (e) => onError(formatError(e)),
  });

  const addMutation = useMutation({
    mutationFn: async () => {
      if (!path) throw new Error("choose a folder first");
      const repo = await addRepository({
        name: repoName.trim() || deriveRepoName(path) || "repo",
        localPath: path,
      });
      return repo;
    },
    onSuccess: async (repo) => {
      onError(null);
      setPath(null);
      setRepoName("");
      await onAdded(repo);
    },
    onError: (e) => onError(formatError(e)),
  });

  return (
    <div className="space-y-2 rounded-md border border-border p-3">
      <div className="flex items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={browseMutation.isPending}
          onClick={() => browseMutation.mutate()}
        >
          <FolderOpen size={13} /> Choose folder…
        </Button>
        {path && (
          <span className="text-xs font-mono text-foreground truncate">
            {path}
          </span>
        )}
      </div>
      {path && (
        <div>
          <label className="block text-xs uppercase tracking-wider text-faint mb-1">
            Name <span className="text-faint">(optional)</span>
          </label>
          <Input
            value={repoName}
            onChange={(e) => setRepoName(e.target.value)}
            placeholder={deriveRepoName(path) || "repo"}
            aria-label="Repository name"
          />
        </div>
      )}
      <div className="flex justify-end">
        <Button
          type="button"
          variant="primary"
          size="sm"
          disabled={addMutation.isPending || !path}
          onClick={() => addMutation.mutate()}
        >
          {addMutation.isPending ? "Adding…" : "Add repository"}
        </Button>
      </div>
    </div>
  );
}

/// Per-selected-repo checkout control: a Full / Sparse toggle, and when
/// Sparse, the `RepoTreeBrowser` pre-seeded from the repo's `cone_defaults`.
function RepoCheckoutRow({
  repo,
  checkout,
  onModeChange,
  onConesChange,
  onRemove,
}: {
  repo: Repository;
  checkout: RepoCheckout;
  onModeChange: (mode: "full" | "sparse") => void;
  onConesChange: (cones: string[]) => void;
  onRemove: () => void;
}): JSX.Element {
  const [expanded, setExpanded] = useState(checkout.mode === "sparse");

  return (
    <div className="rounded-md border border-border p-2 space-y-2">
      <div className="flex items-center gap-2">
        {checkout.mode === "sparse" ? (
          <button
            type="button"
            className="text-faint hover:text-foreground"
            onClick={() => setExpanded((e) => !e)}
            aria-label={expanded ? "Collapse" : "Expand"}
          >
            {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          </button>
        ) : (
          <span className="inline-block w-[14px]" />
        )}
        <span className="text-sm font-mono text-foreground truncate">
          {repo.name}
        </span>
        <div
          role="radiogroup"
          aria-label={`Checkout for ${repo.name}`}
          className="ml-auto flex items-center gap-1 text-xs"
        >
          <label className="flex items-center gap-1 cursor-pointer">
            <input
              type="radio"
              className="accent-accent"
              name={`checkout-${repo.id}`}
              checked={checkout.mode === "full"}
              onChange={() => onModeChange("full")}
            />
            Full working tree
          </label>
          <label className="flex items-center gap-1 cursor-pointer">
            <input
              type="radio"
              className="accent-accent"
              name={`checkout-${repo.id}`}
              checked={checkout.mode === "sparse"}
              onChange={() => {
                onModeChange("sparse");
                setExpanded(true);
              }}
            />
            Sparse
          </label>
        </div>
        <button
          type="button"
          className="text-faint hover:text-err"
          onClick={onRemove}
          aria-label={`Remove ${repo.name}`}
        >
          <X size={14} />
        </button>
      </div>

      {checkout.mode === "sparse" && expanded && (
        <RepoTreeBrowser
          repositoryId={repo.id}
          value={checkout.cones}
          onChange={onConesChange}
        />
      )}
    </div>
  );
}
