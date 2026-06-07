// Data-layer tests for the VCS / coordinated-merge bindings (Task 324).
// Mocks `@tauri-apps/api/core`'s `invoke` so the method strings + payload
// shapes are pinned without a running Tauri shell (the Task 218 pattern), and
// exercises the pure colour-band / opaque-frame parsing helpers.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  aggregateBand,
  checkBand,
  createPullRequest,
  decodeOpaqueFrame,
  getChecks,
  getWorkareaMergePlan,
  getWorkareaPrSet,
  hasRed,
  mergePullRequest,
  mergeWorkareaPrSet,
  parseChecksFrame,
  parsePrSetFrame,
  prSetFrameToProgress,
  revertWorkareaPrSet,
  setMergeOrder,
  type CheckRun,
} from "./vcs";

function run(name: string, status: string, conclusion = ""): CheckRun {
  return { name, status, conclusion, details_url: "" };
}

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
});

describe("colour banding (design/15 §3.4)", () => {
  it("maps conclusions onto bands", () => {
    expect(checkBand(run("a", "completed", "success"))).toBe("green");
    expect(checkBand(run("a", "completed", "failure"))).toBe("red");
    expect(checkBand(run("a", "completed", "timed_out"))).toBe("red");
    expect(checkBand(run("a", "completed", "cancelled"))).toBe("red");
    expect(checkBand(run("a", "completed", "neutral"))).toBe("grey");
    expect(checkBand(run("a", "completed", "skipped"))).toBe("grey");
    expect(checkBand(run("a", "completed", "stale"))).toBe("grey");
  });

  it("treats in_progress/queued as amber regardless of conclusion", () => {
    expect(checkBand(run("a", "in_progress", "failure"))).toBe("amber");
    expect(checkBand(run("a", "queued", ""))).toBe("amber");
  });

  it("aggregates: any red wins, then amber, then green, else grey", () => {
    expect(
      aggregateBand([run("a", "completed", "success"), run("b", "completed", "failure")]),
    ).toBe("red");
    expect(
      aggregateBand([run("a", "completed", "success"), run("b", "in_progress")]),
    ).toBe("amber");
    expect(aggregateBand([run("a", "completed", "success")])).toBe("green");
    expect(aggregateBand([])).toBe("grey");
  });

  it("hasRed flags a failing set", () => {
    expect(hasRed([run("a", "completed", "success")])).toBe(false);
    expect(hasRed([run("a", "completed", "failure")])).toBe(true);
  });
});

describe("bindings dispatch the FROZEN method strings", () => {
  it("getChecks → Vcs.GetChecks", async () => {
    invoke.mockResolvedValue({ checks: [] });
    await getChecks("repo-a", "sha-1");
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Vcs.GetChecks",
      payload: { repository_id: "repo-a", sha: "sha-1" },
    });
  });

  it("createPullRequest → Vcs.CreatePullRequest with base/body defaults", async () => {
    invoke.mockResolvedValue({});
    await createPullRequest({
      workareaId: "wa-1",
      repositoryId: "repo-a",
      head: "concerto/api",
      title: "T",
    });
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Vcs.CreatePullRequest",
      payload: {
        workarea_id: "wa-1",
        repository_id: "repo-a",
        base: "",
        head: "concerto/api",
        title: "T",
        body: "",
      },
    });
  });

  it("mergePullRequest → Vcs.MergePullRequest defaults method=merge", async () => {
    await mergePullRequest("repo-a", 7);
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Vcs.MergePullRequest",
      payload: { repository_id: "repo-a", pr_number: 7, method: "merge" },
    });
  });

  it("getWorkareaPrSet → Workareas.GetWorkareaPrSet with {value} wrapper", async () => {
    invoke.mockResolvedValue({ pull_requests: [] });
    await getWorkareaPrSet("wa-1");
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Workareas.GetWorkareaPrSet",
      payload: { value: "wa-1" },
    });
  });

  it("setMergeOrder → Workareas.SetMergeOrder", async () => {
    invoke.mockResolvedValue({ pull_requests: [] });
    await setMergeOrder({ workareaId: "wa-1", repositoryId: "repo-b", mergeOrder: 3 });
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Workareas.SetMergeOrder",
      payload: { workarea_id: "wa-1", repository_id: "repo-b", merge_order: 3 },
    });
  });

  it("getWorkareaMergePlan → Workareas.GetWorkareaMergePlan", async () => {
    invoke.mockResolvedValue({ workarea_id: "wa-1", steps: [] });
    await getWorkareaMergePlan("wa-1");
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Workareas.GetWorkareaMergePlan",
      payload: { value: "wa-1" },
    });
  });

  it("revertWorkareaPrSet → Workareas.RevertWorkareaPrSet", async () => {
    invoke.mockResolvedValue({ workarea_id: "wa-1", steps: [] });
    await revertWorkareaPrSet("wa-1", true);
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Workareas.RevertWorkareaPrSet",
      payload: { workarea_id: "wa-1", hard_reset: true },
    });
  });

  it("mergeWorkareaPrSet → Workareas.MergeWorkareaPrSet trigger", async () => {
    await mergeWorkareaPrSet({ workareaId: "wa-1" });
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Workareas.MergeWorkareaPrSet",
      payload: {
        workarea_id: "wa-1",
        method: "merge",
        timeout_secs: 0,
        allow_failing_checks: false,
      },
    });
  });
});

describe("opaque-frame decode + parse (PHASE3_PLANNING §2)", () => {
  function frameBytes(obj: unknown): number[] {
    return Array.from(new TextEncoder().encode(JSON.stringify(obj)));
  }

  it("decodes a checks check_run_updated frame from a u8 array", () => {
    const frame = {
      kind: "check_run_updated",
      workarea_id: "wa-1",
      repository_id: "repo-a",
      entity: { sha: "sha-1", runs: [run("ci", "completed", "success")] },
    };
    const decoded = decodeOpaqueFrame({ checks_opaque: frameBytes(frame) });
    const parsed = parseChecksFrame(decoded);
    expect(parsed?.kind).toBe("check_run_updated");
    if (parsed?.kind === "check_run_updated") {
      expect(parsed.entity.runs[0].conclusion).toBe("success");
    }
  });

  it("returns null for an absent or malformed opaque frame", () => {
    expect(decodeOpaqueFrame({ checks_opaque: null })).toBeNull();
    expect(decodeOpaqueFrame({ checks_opaque: [255, 254] })).toBeNull(); // bad UTF-8/JSON
    expect(parseChecksFrame({ kind: "nope" })).toBeNull();
  });

  it("normalizes pr_set frames onto MergeProgress", () => {
    const completed = parsePrSetFrame({
      kind: "merge_step_completed",
      workarea_id: "wa-1",
      step: 1,
      total: 2,
      repository_full_name: "acme/api",
      pr_number: 5,
      merge_sha: "deadbeef",
    });
    expect(completed?.kind).toBe("merge_step_completed");
    expect(prSetFrameToProgress(completed!)).toEqual({
      kind: "step_completed",
      data: { step: 1, total: 2, merge_sha: "deadbeef" },
    });

    const failed = parsePrSetFrame({
      kind: "merge_failed_step",
      workarea_id: "wa-1",
      step: 2,
      total: 2,
      reason: "checks failed",
    });
    expect(prSetFrameToProgress(failed!)).toEqual({
      kind: "set_paused",
      data: { paused_at_step: 2, total: 2, reason: "checks failed" },
    });

    const merged = parsePrSetFrame({ kind: "merged", workarea_id: "wa-1", total: 2 });
    expect(prSetFrameToProgress(merged!)).toEqual({
      kind: "set_merged",
      data: { total: 2 },
    });

    const reverted = parsePrSetFrame({
      kind: "reverted",
      workarea_id: "wa-1",
      repository_full_name: "acme/api",
      pr_number: 5,
    });
    expect(prSetFrameToProgress(reverted!)).toBeNull();
  });
});
