// Bottom region of the center panel — per-repo tabs hosting Diff /
// Checks / PR sub-tabs per `design/15 §3.4`.
//
// V0.1 workareas pin a single repository, so one repo tab is rendered
// (the workarea's `branch_name` is used as the tab label until the
// repository association is surfaced through a richer RPC). All three
// sub-tabs are stub cards in Task 46:
//
//   - Diff   → real Monaco diff lands in Task 47.
//   - Checks → V0.1 stub. CI integration is V1.0.
//   - PR     → V0.1 stub. GitHub PR surface ships with Task 45.
//
// The region is shape-only — no data fetched here.

import { useState } from "react";

import type { Workarea } from "../../api/workareas";

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
  const repoLabel = workarea?.branch_name ?? "repo";

  return (
    <section className="h-full flex flex-col min-h-0 p-2 gap-2">
      <div className="shrink-0 flex items-center gap-2">
        <span className="text-xs uppercase tracking-wider text-slate-500">
          Code & PRs:
        </span>
        <button
          type="button"
          className="px-2 py-0.5 text-xs rounded bg-slate-800 text-slate-100 font-mono"
          aria-pressed="true"
        >
          {repoLabel}
        </button>
      </div>
      <div className="shrink-0 flex items-center gap-1 border-b border-slate-800 pb-1">
        {SUB_TABS.map((t) => {
          const active = t.id === activeSubTab;
          const cls = active
            ? "px-2 py-0.5 text-xs rounded bg-slate-800 text-slate-100"
            : "px-2 py-0.5 text-xs rounded text-slate-400 hover:bg-slate-900";
          return (
            <button
              key={t.id}
              type="button"
              className={cls}
              onClick={() => setActiveSubTab(t.id)}
              aria-pressed={active}
            >
              {t.label}
            </button>
          );
        })}
      </div>
      <div className="flex-1 min-h-0 rounded border border-dashed border-slate-800 flex items-center justify-center text-xs text-slate-500 p-3">
        <SubTabPlaceholder tab={activeSubTab} />
      </div>
    </section>
  );
}

function SubTabPlaceholder({ tab }: { tab: SubTab }): JSX.Element {
  switch (tab) {
    case "diff":
      return (
        <span>
          Monaco diff arrives in the next desktop task.
        </span>
      );
    case "checks":
      return <span>CI checks panel arrives in V1.0.</span>;
    case "pr":
      return <span>Pull-request panel arrives with the GitHub surface.</span>;
  }
}
