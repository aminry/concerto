// @vitest-environment jsdom
//
// Component tests for the workarea-wide coordinated-merge actions (Task 324,
// design/15 §3.4 + design/03 §6.4). Proves:
//  - the PR-set list renders in merge_order;
//  - drag-to-reorder writes Workareas.SetMergeOrder;
//  - "Merge workarea PR set" is disabled when a repo has a red check
//    (aggregated across the set);
//  - a mocked merge lifecycle (driven over `pr_set.events.<wa>`) renders the
//    running step, then the pause-on-fail "Step N of M failed — auto-revert?"
//    prompt, and "Auto-revert" calls Workareas.RevertWorkareaPrSet.
//
// Tier-2 double: mocked `invoke` + a captured `pr_set.events.<wa>` callback
// (the opaque-frame event bus). No Core, no real merge.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

const eventCallbacks = new Map<string, Array<(p: unknown) => void>>();
function emit(subject: string, payload: unknown): void {
  for (const cb of eventCallbacks.get(subject) ?? []) cb(payload);
}
vi.mock("../../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/client")>();
  return {
    ...actual,
    onConcertoEvent: vi.fn(async (subject: string, cb: (p: unknown) => void) => {
      const list = eventCallbacks.get(subject) ?? [];
      list.push(cb);
      eventCallbacks.set(subject, list);
      return () => {
        const cur = eventCallbacks.get(subject);
        if (cur) eventCallbacks.set(subject, cur.filter((c) => c !== cb));
      };
    }),
    subscribe: vi.fn(async () => "sub-1"),
    unsubscribe: vi.fn(async () => {}),
  };
});

import { PrSetActions } from "./PrSetActions";
import { renderWithClient } from "../test-utils";
import type { PullRequest } from "../../api/vcs";
import type { Repository } from "../../api/repositories";

function pr(repoId: string, prNumber: number, mergeOrder: number, fullName: string): PullRequest {
  return {
    id: `pr-${repoId}`,
    workarea_id: "wa-1",
    repository_id: repoId,
    provider: "github",
    pr_number: prNumber,
    base_ref: "main",
    head_ref: `concerto/${repoId}`,
    state: "open",
    title: `PR ${prNumber}`,
    body: "",
    url: `https://gh/${prNumber}`,
    head_sha: `sha-${repoId}`,
    created_at: 0,
    updated_at: 0,
    merge_order: mergeOrder,
    repository_full_name: fullName,
  };
}

const repos: Repository[] = [
  { id: "repo-a", name: "api", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
  { id: "repo-b", name: "web", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
];

// The PR set: repo-a first (merge_order 0), repo-b second (1).
const prSet = [pr("repo-a", 1, 0, "acme/api"), pr("repo-b", 2, 1, "acme/web")];

// Per-repo checks: both green by default.
let checksByRepo: Record<string, { conclusion: string }[]>;

function bytes(obj: unknown): number[] {
  return Array.from(new TextEncoder().encode(JSON.stringify(obj)));
}

beforeEach(() => {
  invoke.mockReset();
  eventCallbacks.clear();
  checksByRepo = {
    "repo-a": [{ conclusion: "success" }],
    "repo-b": [{ conclusion: "success" }],
  };
  invoke.mockImplementation((cmd: string, args: { method?: string; payload?: { repository_id?: string; workarea_id?: string; repository_full_name?: string } }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    switch (args.method) {
      case "Workareas.GetWorkareaPrSet":
        return Promise.resolve({ pull_requests: prSet });
      case "Vcs.GetChecks": {
        const repoId = args.payload?.repository_id ?? "";
        const runs = (checksByRepo[repoId] ?? []).map((c) => ({
          name: "ci",
          status: "completed",
          conclusion: c.conclusion,
          details_url: "",
        }));
        return Promise.resolve({ checks: runs });
      }
      case "Workareas.SetMergeOrder":
        // Echo back a re-ordered set (repo-b first now).
        return Promise.resolve({ pull_requests: [prSet[1], prSet[0]] });
      case "Workareas.MergeWorkareaPrSet":
        return Promise.resolve(undefined);
      case "Workareas.RevertWorkareaPrSet":
        return Promise.resolve({ workarea_id: "wa-1", steps: [] });
      default:
        return Promise.resolve(undefined);
    }
  });
});

function renderActions(dirty: Record<string, boolean> = {}) {
  return renderWithClient(
    <PrSetActions workareaId="wa-1" repos={repos} dirtyByRepo={dirty} />,
  );
}

describe("PrSetActions", () => {
  it("renders the PR set ordered by merge_order", async () => {
    renderActions();
    const rows = await screen.findAllByTestId("pr-set-row");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveAttribute("data-repo", "repo-a");
    expect(rows[1]).toHaveAttribute("data-repo", "repo-b");
  });

  it("drag-to-reorder writes SetMergeOrder", async () => {
    renderActions();
    const rows = await screen.findAllByTestId("pr-set-row");
    // Drag repo-b (index 1) onto repo-a (index 0).
    fireEvent.dragStart(rows[1]);
    fireEvent.dragOver(rows[0]);
    fireEvent.drop(rows[0]);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Workareas.SetMergeOrder",
        payload: { workarea_id: "wa-1", repository_id: "repo-b", merge_order: 0 },
      }),
    );
  });

  it("disables Merge PR set when any repo has a red check", async () => {
    checksByRepo["repo-b"] = [{ conclusion: "failure" }];
    renderActions();
    await screen.findAllByTestId("pr-set-row");
    await waitFor(() =>
      expect(screen.getByTestId("merge-pr-set")).toBeDisabled(),
    );
    expect(screen.getByTestId("red-checks-warning")).toBeInTheDocument();
  });

  it("enables Merge PR set when all checks are green", async () => {
    renderActions();
    await screen.findAllByTestId("pr-set-row");
    await waitFor(() =>
      expect(screen.getByTestId("merge-pr-set")).not.toBeDisabled(),
    );
  });

  it("drives the running step then pause-on-fail and auto-revert", async () => {
    renderActions();
    await waitFor(() =>
      expect(screen.getByTestId("merge-pr-set")).not.toBeDisabled(),
    );

    await userEvent.click(screen.getByTestId("merge-pr-set"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", expect.objectContaining({
        method: "Workareas.MergeWorkareaPrSet",
      })),
    );

    // Step 1 of 2 completes (running view).
    emit("pr_set.events.wa-1", {
      checks_opaque: bytes({
        kind: "merge_step_completed",
        workarea_id: "wa-1",
        step: 1,
        total: 2,
        repository_full_name: "acme/api",
        pr_number: 1,
        merge_sha: "deadbeef",
      }),
    });
    await waitFor(() =>
      expect(screen.getByTestId("merge-running")).toHaveTextContent("step 1 of 2"),
    );

    // Step 2 fails → pause-on-fail prompt.
    emit("pr_set.events.wa-1", {
      checks_opaque: bytes({
        kind: "merge_failed_step",
        workarea_id: "wa-1",
        step: 2,
        total: 2,
        reason: "checks failed",
      }),
    });
    const paused = await screen.findByTestId("merge-paused");
    expect(paused).toHaveTextContent("Step 2 of 2 failed");
    expect(paused).toHaveTextContent("checks failed");

    await userEvent.click(screen.getByTestId("auto-revert"));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Workareas.RevertWorkareaPrSet",
        payload: { workarea_id: "wa-1", hard_reset: false },
      }),
    );
  });
});
