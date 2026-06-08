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

import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { useUiStore } from "../state/useUiStore";
import {
  addRepository,
  estimateRepoSize,
  listRepositories,
  type Repository,
  type SizeReport,
} from "../api/repositories";
import {
  cloneRepository,
  onCloneProgress,
  type CloneProgressEvent,
} from "../api/client";
import { formatError } from "../api/errors";
import { useDebouncedValue } from "../hooks/useConeEstimate";
import { formatBytes } from "./ConePicker";
import { Button } from "./ui/button";
import { Input } from "./ui/input";
import { Progress } from "./ui/progress";
import { Segmented } from "./ui/segmented";

// The three strategy choices the picker offers. Treeless is intentionally
// absent — never shown in the UI (design/02 §12 R-1). Each maps to the
// `(clone_strategy, with_sparse)` pair `AddRepository` takes (Task 301).
type StrategyChoice = "full" | "blobless" | "blobless-sparse";

const STRATEGY_ITEMS: ReadonlyArray<{ id: StrategyChoice; label: string }> = [
  { id: "full", label: "Full" },
  { id: "blobless", label: "Blobless" },
  { id: "blobless-sparse", label: "Blobless + Sparse" },
];

const STRATEGY_BLURB: Record<StrategyChoice, string> = {
  full: "Every blob on disk. Best for small repos and offline work.",
  blobless: "Faster clone; file contents fetched on demand.",
  "blobless-sparse":
    "Blobless plus a sparse cone — only the directories you pick land on disk.",
};

/// Map the `SizeReport` recommendation (design/02 §3.5 heuristic, computed
/// on the Core) onto a picker choice. `recommended_strategy` is `full` or
/// `blobless` (treeless is never recommended); `recommend_sparse` promotes
/// blobless to the "+ Sparse" tier (>10 GB).
function choiceFromReport(report: SizeReport): StrategyChoice {
  if (report.recommended_strategy === "blobless") {
    return report.recommend_sparse ? "blobless-sparse" : "blobless";
  }
  return "full";
}

/// Split a picker choice back into the `(cloneStrategy, withSparse)` the
/// `addRepository` binding sends.
function choiceToWire(choice: StrategyChoice): {
  cloneStrategy: "full" | "blobless";
  withSparse: boolean;
} {
  switch (choice) {
    case "full":
      return { cloneStrategy: "full", withSparse: false };
    case "blobless":
      return { cloneStrategy: "blobless", withSparse: false };
    case "blobless-sparse":
      return { cloneStrategy: "blobless", withSparse: true };
  }
}

const STRATEGY_LABEL: Record<StrategyChoice, string> = {
  full: "Full",
  blobless: "Blobless",
  "blobless-sparse": "Blobless + Sparse",
};

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

  // Clone-strategy picker. `strategy` is the user-facing choice (defaults to
  // Full so the form is usable before any probe). `userOverrode` flips true
  // once the user touches the selector — after that a fresh recommendation
  // no longer stomps their choice.
  const [strategy, setStrategy] = useState<StrategyChoice>("full");
  const userOverrodeRef = useRef(false);

  // Pre-clone size probe (Task 301). Debounce the URL so each keystroke
  // doesn't hit the remote; only probe a non-empty, trimmed URL. A probe
  // failure (private/offline repo) is NOT fatal — `retry: false` and the
  // form falls back to a manual pick with a note (design/02 §3.5/§7.1).
  const trimmedUrl = url.trim();
  const debouncedUrl = useDebouncedValue(trimmedUrl, 500);
  const sizeQuery = useQuery<SizeReport>({
    queryKey: ["repoSize", debouncedUrl] as const,
    queryFn: () => estimateRepoSize(debouncedUrl),
    enabled: debouncedUrl.length > 0,
    retry: false,
    staleTime: 60_000,
  });

  const recommendedChoice = useMemo(
    () => (sizeQuery.data ? choiceFromReport(sizeQuery.data) : null),
    [sizeQuery.data],
  );

  // Default the selector to the recommendation when one arrives, unless the
  // user has already overridden it.
  useEffect(() => {
    if (recommendedChoice && !userOverrodeRef.current) {
      setStrategy(recommendedChoice);
    }
  }, [recommendedChoice]);

  // Reset the "user overrode" latch whenever the URL changes, so a brand-new
  // repo's recommendation can take effect again.
  useEffect(() => {
    userOverrodeRef.current = false;
  }, [debouncedUrl]);
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
      const wire = choiceToWire(strategy);
      const repo = await addRepository({
        projectId,
        name: trimmedName,
        url: trimmedUrl,
        defaultBranch: defaultBranch.trim() || undefined,
        cloneStrategy: wire.cloneStrategy,
        withSparse: wire.withSparse,
      });
      return repo;
    },
    onSuccess: async (repo) => {
      setUrl("");
      setName("");
      setDefaultBranch("");
      setErrorMsg(null);
      setStrategy("full");
      userOverrodeRef.current = false;
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
              c.repositoryId === repo.id ? { ...c, error: formatError(e) } : c,
            ),
          );
        });
    },
    onError: (e) => {
      setErrorMsg(formatError(e));
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

        {trimmedUrl.length > 0 && (
          <CloneStrategyPicker
            strategy={strategy}
            onSelect={(choice) => {
              userOverrodeRef.current = true;
              setStrategy(choice);
            }}
            probing={sizeQuery.isFetching}
            report={sizeQuery.data ?? null}
            recommended={recommendedChoice}
            probeFailed={sizeQuery.isError}
          />
        )}

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
        {!projectId && (
          <p className="text-xs text-warn">
            No project selected. Create one from the sidebar's
            “+ New Project” button.
          </p>
        )}
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

/// The clone-strategy block: a size→strategy recommendation line (design/02
/// §3.5, surfaced per design/15 §7.1) plus a Full / Blobless / Blobless +
/// Sparse selector defaulting to the recommendation. Treeless is never an
/// option (R-1). A probe failure (private/offline) degrades to a manual pick
/// with a note rather than blocking the add.
function CloneStrategyPicker({
  strategy,
  onSelect,
  probing,
  report,
  recommended,
  probeFailed,
}: {
  strategy: StrategyChoice;
  onSelect: (choice: StrategyChoice) => void;
  probing: boolean;
  report: SizeReport | null;
  recommended: StrategyChoice | null;
  probeFailed: boolean;
}): JSX.Element {
  return (
    <div className="space-y-1.5">
      <label className="block text-xs uppercase tracking-wider text-faint">
        Clone strategy
      </label>

      {probing && (
        <p className="text-xs text-faint">Estimating repository size…</p>
      )}

      {!probing && report && recommended && (
        <p className="text-xs text-faint">
          ≈ {formatBytes(report.size_bytes)}{" "}
          <span className="opacity-70">(est.)</span> ·{" "}
          {report.object_count.toLocaleString()} objects → recommended:{" "}
          <span className="font-semibold text-foreground">
            {STRATEGY_LABEL[recommended]}
          </span>
        </p>
      )}

      {!probing && probeFailed && (
        <p className="text-xs text-warn">
          Couldn’t reach the remote to estimate its size (it may be private or
          offline). Pick a strategy manually — defaulting to Full.
        </p>
      )}

      <Segmented<StrategyChoice>
        items={STRATEGY_ITEMS}
        active={strategy}
        onSelect={onSelect}
      />
      <p className="text-xs text-faint">{STRATEGY_BLURB[strategy]}</p>
    </div>
  );
}
