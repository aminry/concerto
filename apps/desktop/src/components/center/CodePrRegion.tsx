// Bottom region of the center panel — per-repo tabs hosting Diff /
// Checks / PR sub-tabs per `design/15 §3.4`.
//
// V0.1 workareas pin a single repository. Task 47 wires the real
// Monaco diff viewer into the `Diff` sub-tab; `Checks` and `PR` remain
// stub cards in V0.1.
//
// Repository resolution: the workarea wire surface still doesn't carry
// the linked `repository_id` (see Task 46 Handoff Notes for the gap).
// V0.1 single-repo projects make picking the first repository under
// `selectedProjectId` correct; multi-repo workareas are V1.0.

import { useState } from "react";
import { useQuery } from "@tanstack/react-query";

import type { Workarea } from "../../api/workareas";
import { listRepositories } from "../../api/repositories";
import { useUiStore } from "../../state/useUiStore";
import { DiffViewer } from "./DiffViewer";
import { Tabs } from "../ui/tabs";

export type CodePrRegionProps = {
  workarea: Workarea | null | undefined;
};

type SubTab = "diff" | "checks" | "pr";

const SUB_TABS: readonly { id: SubTab; label: string }[] = [
  { id: "diff", label: "Diff" },
  { id: "checks", label: "Checks" },
  { id: "pr", label: "PR" },
];

export function CodePrRegion({ workarea }: CodePrRegionProps): JSX.Element {
  const [activeSubTab, setActiveSubTab] = useState<SubTab>("diff");
  const projectId = useUiStore((s) => s.selectedProjectId);

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
        <span className="text-xs uppercase tracking-wide text-faint">
          Code & PRs:
        </span>
        <button
          type="button"
          className="px-2 py-0.5 text-xs rounded-md bg-surface-2 text-foreground font-mono"
          aria-pressed="true"
        >
          {repoLabel}
        </button>
      </div>
      <div className="shrink-0">
        <Tabs items={SUB_TABS} active={activeSubTab} onSelect={setActiveSubTab} />
      </div>
      <div className="flex-1 min-h-0 rounded border border-border overflow-hidden">
        {activeSubTab === "diff" ? (
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
            {activeSubTab === "checks" ? (
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
