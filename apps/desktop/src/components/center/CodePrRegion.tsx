// Code & PRs surface — per-repo Diff / Checks / PR content.
//
// Originally the bottom region of the center panel; moved into the right
// rail (see `RightRail.tsx`) so the session terminal occupies the full
// center height. The rail's tab strip now drives which sub-view shows, so
// this component is controlled via the `subTab` prop instead of owning its
// own tab state, and resolves the active workarea from the store rather
// than receiving it as a prop.
//
// V0.1 workareas pin a single repository. Task 47 wires the real Monaco
// diff viewer into the `Diff` view; `Checks` and `PR` remain stub cards.
//
// Repository resolution: the workarea wire surface still doesn't carry
// the linked `repository_id` (see Task 46 Handoff Notes for the gap).
// V0.1 single-repo projects make picking the first repository under
// `selectedProjectId` correct; multi-repo workareas are V1.0.

import { useQuery } from "@tanstack/react-query";

import { listRepositories } from "../../api/repositories";
import { useUiStore } from "../../state/useUiStore";
import { useWorkarea } from "../../hooks/useWorkareas";
import { DiffViewer } from "./DiffViewer";

/// The three Code & PRs views, keyed to the matching right-rail tab ids.
export type CodePrSubTab = "diff" | "checks" | "pr";

export type CodePrRegionProps = {
  subTab: CodePrSubTab;
};

export function CodePrRegion({ subTab }: CodePrRegionProps): JSX.Element {
  const workareaId = useUiStore((s) => s.selectedWorkareaId);
  const projectId = useUiStore((s) => s.selectedProjectId);
  const workareaQuery = useWorkarea(workareaId);
  const workarea = workareaQuery.data ?? null;

  // V0.1: workspaces pin a single repo, so the first repo under the
  // active project is the workarea's repo. The dropdown will become
  // real when the workarea wire surface exposes `repository_ids`.
  const reposQuery = useQuery({
    queryKey: ["repositories", projectId] as const,
    queryFn: async () => {
      if (!projectId) return { repositories: [] };
      return listRepositories(projectId);
    },
    enabled: !!projectId,
  });

  const repo = reposQuery.data?.repositories[0] ?? null;
  const repoLabel = repo?.name ?? workarea?.branch_name ?? "repo";

  return (
    <section className="h-full flex flex-col min-h-0 p-2 gap-2">
      <div className="shrink-0 flex items-center gap-2">
        <span className="text-xs uppercase tracking-wide text-faint">Repo:</span>
        <button
          type="button"
          className="px-2 py-0.5 text-xs rounded-md bg-surface-2 text-foreground font-mono"
          aria-pressed="true"
        >
          {repoLabel}
        </button>
      </div>
      <div className="flex-1 min-h-0 rounded border border-border overflow-hidden">
        {subTab === "diff" ? (
          workarea ? (
            <DiffViewer
              workareaId={workarea.id}
              repositoryId={repo?.id ?? null}
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
