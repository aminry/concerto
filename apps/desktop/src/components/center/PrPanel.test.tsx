// @vitest-environment jsdom
//
// Component tests for the Level-2 PR panel (Task 324, design/15 §3.4).
// Proves: Create PR when none exists; Merge + Open in browser when one does.
//
// Tier-2 double: mocked `invoke` + a stubbed `window.open`. No Core.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { PrPanel } from "./PrPanel";
import { renderWithClient } from "../test-utils";
import type { PullRequest } from "../../api/vcs";

const openPr: PullRequest = {
  id: "pr-1",
  workarea_id: "wa-1",
  repository_id: "repo-a",
  provider: "github",
  pr_number: 7,
  base_ref: "main",
  head_ref: "concerto/api",
  state: "open",
  title: "Add feature",
  body: "",
  url: "https://gh/pr/7",
  head_sha: "sha-1",
  created_at: 0,
  updated_at: 0,
  merge_order: 0,
};

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    if (args.method === "Vcs.CreatePullRequest") return Promise.resolve(openPr);
    if (args.method === "Vcs.MergePullRequest") return Promise.resolve(null);
    if (args.method === "Workareas.GetWorkareaPrSet")
      return Promise.resolve({ pull_requests: [] });
    return Promise.resolve(undefined);
  });
});

describe("PrPanel", () => {
  it("shows Create PR when the repo has no PR", async () => {
    renderWithClient(
      <PrPanel
        workareaId="wa-1"
        repositoryId="repo-a"
        headBranch="concerto/api"
        pr={null}
      />,
    );
    const btn = screen.getByRole("button", { name: "Create PR" });
    await userEvent.click(btn);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Vcs.CreatePullRequest",
        payload: expect.objectContaining({
          repository_id: "repo-a",
          head: "concerto/api",
        }),
      }),
    );
  });

  it("shows Merge + Open when a PR exists, and merges via the RPC", async () => {
    renderWithClient(
      <PrPanel
        workareaId="wa-1"
        repositoryId="repo-a"
        headBranch="concerto/api"
        pr={openPr}
      />,
    );
    expect(screen.getByTestId("pr-state")).toHaveTextContent("#7 · open");

    await userEvent.click(screen.getByRole("button", { name: "Merge PR" }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Vcs.MergePullRequest",
        payload: { repository_id: "repo-a", pr_number: 7, method: "merge" },
      }),
    );
  });

  it("opens the PR url in a new tab", async () => {
    const open = vi.fn();
    vi.stubGlobal("open", open);
    renderWithClient(
      <PrPanel
        workareaId="wa-1"
        repositoryId="repo-a"
        headBranch="concerto/api"
        pr={openPr}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Open in browser" }));
    expect(open).toHaveBeenCalledWith("https://gh/pr/7", "_blank", "noopener,noreferrer");
    vi.unstubAllGlobals();
  });

  it("disables Merge for an already-merged PR", () => {
    renderWithClient(
      <PrPanel
        workareaId="wa-1"
        repositoryId="repo-a"
        headBranch="concerto/api"
        pr={{ ...openPr, state: "merged" }}
      />,
    );
    expect(screen.getByRole("button", { name: "Merged" })).toBeDisabled();
  });
});
