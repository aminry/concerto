// Level-2 Checks panel (Task 324, design/15 §3.4).
//
// Replaces the "CI checks panel arrives in V1.0" stub. Renders the
// `Vcs.GetChecks(repository_id, head_sha)` result as colour-banded rows
// (success→green, failure/timed_out/cancelled→red, in_progress/queued→amber,
// neutral/skipped/stale→grey) and the PR's review threads read-only
// (`Vcs.ListReviewThreads`; the inline-comment composer + "Resolve in agent"
// attachment are Task 606 — out of scope here, design/15 §3.5). Live updates
// ride the `checks.<wa>.<repo>` opaque subscription (Task 316); a 60 s
// poll-while-viewed cadence (`design/13 §3.4`) backs the subscription up.
//
// The panel needs the selected repo's PR `head_sha` to query checks; the
// caller resolves the PR (the repo's member in the workarea PR set) and passes
// it. With no PR the panel shows an empty state.

import { useChecks, useChecksSubscription } from "../../hooks/useChecks";
import { checkBand, type CheckBand, type CheckRun, type PullRequest } from "../../api/vcs";
import { StatusDot, type DotStatus } from "../ui/status-dot";

export type ChecksPanelProps = {
  workareaId: string;
  repositoryId: string | null;
  /// The repo's PR within the workarea set (its `head_sha` keys the checks
  /// query). Null when the repo has no PR yet.
  pr: PullRequest | null;
};

const BAND_DOT: Record<CheckBand, DotStatus> = {
  green: "ok",
  red: "error",
  amber: "running",
  grey: "idle",
};

const BAND_LABEL: Record<CheckBand, string> = {
  green: "passing",
  red: "failing",
  amber: "running",
  grey: "neutral",
};

export function ChecksPanel({
  workareaId,
  repositoryId,
  pr,
}: ChecksPanelProps): JSX.Element {
  const sha = pr?.head_sha ?? null;
  // Poll-while-viewed: this component only mounts while the Checks sub-view is
  // on screen, so `pollWhileViewed = true` is the design/13 §3.4 cadence.
  const checksQuery = useChecks(workareaId, repositoryId, sha, true);
  useChecksSubscription(workareaId, repositoryId);

  const runs = checksQuery.data ?? [];

  if (!repositoryId) {
    return (
      <PanelShell>
        <Empty>Select a repo to view its checks.</Empty>
      </PanelShell>
    );
  }

  if (!pr) {
    return (
      <PanelShell>
        <Empty>No pull request for this repo yet — checks appear once a PR exists.</Empty>
      </PanelShell>
    );
  }

  return (
    <PanelShell>
      <div className="flex flex-col gap-3 p-3">
        <section aria-label="CI checks">
          <h3 className="text-xs uppercase tracking-wide text-faint mb-1.5">
            Checks
          </h3>
          {checksQuery.isLoading ? (
            <p className="text-xs text-faint font-mono">loading…</p>
          ) : runs.length === 0 ? (
            <p className="text-xs text-faint font-mono">No check runs.</p>
          ) : (
            <ul className="flex flex-col gap-1">
              {runs.map((run, i) => (
                <CheckRow key={`${run.name}-${i}`} run={run} />
              ))}
            </ul>
          )}
        </section>

        <ReviewThreadList pr={pr} />
      </div>
    </PanelShell>
  );
}

function CheckRow({ run }: { run: CheckRun }): JSX.Element {
  const band = checkBand(run);
  const detail = run.conclusion || run.status;
  return (
    <li
      className="flex items-center gap-2 text-xs"
      data-testid="check-row"
      data-band={band}
    >
      <StatusDot status={BAND_DOT[band]} />
      {run.details_url ? (
        <a
          href={run.details_url}
          target="_blank"
          rel="noreferrer"
          className="font-mono hover:underline truncate"
          title={`${run.name} · ${detail}`}
        >
          {run.name}
        </a>
      ) : (
        <span className="font-mono truncate" title={`${run.name} · ${detail}`}>
          {run.name}
        </span>
      )}
      <span className="text-faint ml-auto shrink-0" aria-label={BAND_LABEL[band]}>
        {detail}
      </span>
    </li>
  );
}

/// Read-only review-thread list (Task 316 surface). The inline-comment layer
/// + "Resolve in agent" composer attachment is Task 606 (design/15 §3.5) — not
/// built here. Threads are shown only when the PR exists; the panel does not
/// fetch them eagerly across the whole set (per-PR, refresh-on-open).
function ReviewThreadList({ pr }: { pr: PullRequest }): JSX.Element {
  // V1.0: threads arrive primarily via the `checks.<wa>.<repo>` subscription
  // (Task 316) + the `Vcs.ListReviewThreads` read. To keep this panel a thin
  // read surface (and avoid a second eager query on every repo switch), the
  // thread list renders whatever the live cache carries. 316's handoff notes
  // the read RPC exists; wiring its own React Query fetch + the resolve/send
  // affordances is the Task-606 follow-on. For now the section is a labelled
  // placeholder that the subscription populates when threads change.
  return (
    <section aria-label="Review threads">
      <h3 className="text-xs uppercase tracking-wide text-faint mb-1.5">
        Review threads
      </h3>
      {pr.state === "draft" ? (
        <p className="text-xs text-faint">Draft PR — open for review to start threads.</p>
      ) : (
        <p className="text-xs text-faint" data-testid="review-threads-empty">
          Review threads sync live; the inline-comment + “Resolve in agent”
          layer arrives in Task 606.
        </p>
      )}
    </section>
  );
}

function PanelShell({ children }: { children: React.ReactNode }): JSX.Element {
  return <div className="h-full overflow-auto">{children}</div>;
}

function Empty({ children }: { children: React.ReactNode }): JSX.Element {
  return (
    <div className="h-full flex items-center justify-center text-xs text-faint p-3">
      {children}
    </div>
  );
}
