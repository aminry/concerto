// Code & PRs surface — per-repo Diff / Checks / PR content.
//
// Originally the bottom region of the center panel; moved into the right
// rail (see `RightRail.tsx`) so the session terminal occupies the full
// center height. The rail's tab strip now drives which sub-view shows, so
// this component is controlled via the `subTab` prop instead of owning its
// own tab state, and resolves the active workarea from the store rather
// than receiving it as a prop.
//
// ── Task 322: the Level-1 repo selector (design/15 §3.4) ─────────────
// V0.1 faked the workarea's repo as the project's first repo
// (`repositories[0]`) behind a non-interactive "Repo:" button. This task
// replaces that with a real Level-1 repo selector: one entry per repo in
// the workarea, each with a per-repo status dot, and selecting a repo
// drives which repo `DiffViewer` renders. The repo list is the workarea's
// repos (= the workspace's declared repos; see `useWorkareaRepos` for why
// that's the FROZEN-respecting source). The active-repo selection lives in
// `useUiStore` (UI-only, reset on workarea switch).
//
// Task 47 wired the real Monaco diff viewer into the `Diff` view; `Checks`
// and `PR` remain stub cards — those panels (+ full CI status dots) are
// Task 324's job, so a clean/dirty/neutral dot here is sufficient.
//
// DRIFT (recorded in the 322 Handoff per decision D8): design/15 §3.4
// wants Code & PRs in the center-bottom region, not the right rail. This
// task introduces the repo dimension the center IA needs but does NOT move
// the panels out of the right rail — that physical relocation is a
// follow-on Desktop task. The selector works wherever `CodePrRegion`
// currently mounts.

import { useEffect } from "react";
import { useQueries } from "@tanstack/react-query";

import { getWorkareaRepoDiff } from "../../api/diff";
import { diffQueryKey } from "../../hooks/useDiff";
import { useWorkareaRepos } from "../../hooks/useWorkareaRepos";
import { useUiStore } from "../../state/useUiStore";
import { useWorkarea } from "../../hooks/useWorkareas";
import { StatusDot, type DotStatus } from "../ui/status-dot";
import { DiffViewer } from "./DiffViewer";

/// The three Code & PRs views, keyed to the matching right-rail tab ids.
export type CodePrSubTab = "diff" | "checks" | "pr";

export type CodePrRegionProps = {
  subTab: CodePrSubTab;
};

export function CodePrRegion({ subTab }: CodePrRegionProps): JSX.Element {
  const workareaId = useUiStore((s) => s.selectedWorkareaId);
  const projectId = useUiStore((s) => s.selectedProjectId);
  const selectedRepoId = useUiStore((s) => s.selectedRepoId);
  const setSelectedRepo = useUiStore((s) => s.setSelectedRepo);

  const workareaQuery = useWorkarea(workareaId);
  const workarea = workareaQuery.data ?? null;

  const reposQuery = useWorkareaRepos(workareaId, projectId);
  const repos = reposQuery.data ?? [];

  // Auto-select the workarea's first repo when none is selected — mirrors
  // SessionRegion's first-session auto-select effect. `selectedRepoId` is
  // cleared on workarea switch (see `useUiStore.setSelectedWorkarea`), so
  // this re-fires for each workarea.
  useEffect(() => {
    if (!selectedRepoId && repos.length > 0) {
      setSelectedRepo(repos[0].id);
    }
  }, [selectedRepoId, repos, setSelectedRepo]);

  // Per-repo dirty/clean status: a repo with ≥1 changed file is "dirty"
  // (running dot), 0 files is "clean" (ok dot), and an in-flight/errored
  // probe is neutral (idle). Full CI status dots are Task 324. Each probe
  // shares `useDiff`'s query key so the DiffViewer's fetch for the active
  // repo is reused, not duplicated.
  const diffResults = useQueries({
    queries: repos.map((r) => ({
      queryKey: diffQueryKey(workareaId, r.id),
      queryFn: async () => {
        if (!workareaId) return { files: [] };
        return getWorkareaRepoDiff(workareaId, r.id);
      },
      enabled: !!workareaId && !!r.id,
    })),
  });

  const selectedRepo =
    repos.find((r) => r.id === selectedRepoId) ?? repos[0] ?? null;

  return (
    <section className="h-full flex flex-col min-h-0 p-2 gap-2">
      <div className="shrink-0 flex items-center gap-2 overflow-x-auto">
        <span className="text-xs uppercase tracking-wide text-faint shrink-0">
          Repo:
        </span>
        {repos.length === 0 ? (
          <span className="text-xs text-faint font-mono">
            {reposQuery.isLoading ? "loading…" : "no repos"}
          </span>
        ) : (
          repos.map((r, i) => {
            const result = diffResults[i];
            const fileCount = result?.data?.files.length ?? 0;
            const dot: DotStatus =
              result?.isError || result?.isLoading
                ? "idle"
                : fileCount > 0
                  ? "running"
                  : "ok";
            const active = r.id === selectedRepo?.id;
            return (
              <button
                key={r.id}
                type="button"
                aria-pressed={active}
                onClick={() => setSelectedRepo(r.id)}
                title={`${r.name} · ${fileCount} changed file${
                  fileCount === 1 ? "" : "s"
                }`}
                className={`flex items-center gap-1.5 px-2 py-0.5 text-xs rounded-md font-mono shrink-0 ${
                  active
                    ? "bg-accent text-accent-fg"
                    : "bg-surface-2 text-foreground hover:opacity-80"
                }`}
              >
                <StatusDot status={dot} />
                {r.name}
              </button>
            );
          })
        )}
      </div>
      <div className="flex-1 min-h-0 rounded border border-border overflow-hidden">
        {subTab === "diff" ? (
          workarea ? (
            <DiffViewer
              workareaId={workarea.id}
              repositoryId={selectedRepo?.id ?? null}
            />
          ) : (
            <Placeholder>Select a workarea to view its diff.</Placeholder>
          )
        ) : (
          <Placeholder>
            {subTab === "checks" ? (
              <span>CI checks panel arrives in V1.0.</span>
            ) : (
              <span>Pull-request panel arrives with the GitHub surface.</span>
            )}
          </Placeholder>
        )}
      </div>
    </section>
  );
}

function Placeholder({
  children,
}: {
  children: React.ReactNode;
}): JSX.Element {
  return (
    <div className="h-full flex items-center justify-center text-xs text-faint p-3 border border-dashed border-border m-px rounded">
      {children}
    </div>
  );
}
