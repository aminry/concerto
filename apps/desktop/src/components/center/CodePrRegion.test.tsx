// @vitest-environment jsdom
//
// Component tests for the Level-1 per-repo selector (Task 322, design/15
// §3.4). Proves: one selector entry per workarea repo (sourced from the
// workarea-SCOPED `Workareas.ListWorkareaRepos`, NOT the global registry);
// the selected repo's id is passed to `DiffViewer`; clicking another repo
// entry switches the DiffViewer's `repositoryId`. `DiffViewer` is mocked (it
// mounts Monaco) so the test only asserts the prop wiring.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Task 324: CodePrRegion now mounts PrSetActions, which subscribes to
// `pr_set.events.<wa>` via `onConcertoEvent`. Stub the event-bus bridge so the
// jsdom run has no Tauri runtime (mirrors SessionRegion.test).
vi.mock("../../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../api/client")>();
  return {
    ...actual,
    onConcertoEvent: vi.fn(async () => () => {}),
    subscribe: vi.fn(async () => "sub-1"),
    unsubscribe: vi.fn(async () => {}),
  };
});

// Capture the props DiffViewer receives without mounting Monaco.
const diffViewerSpy = vi.fn();
vi.mock("./DiffViewer", () => ({
  DiffViewer: (props: { workareaId: string; repositoryId: string | null }) => {
    diffViewerSpy(props);
    return <div data-testid="diff-viewer">repo:{props.repositoryId}</div>;
  },
}));

import { CodePrRegion } from "./CodePrRegion";
import { renderWithClient } from "../test-utils";
import { useUiStore } from "../../state/useUiStore";

const repos = [
  { id: "repo-a", name: "api", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
  { id: "repo-b", name: "web", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
];

const workarea = {
  id: "wa-1",
  workspace_id: "ws-1",
  composer_name: "bach",
  branch_name: "concerto/bach",
  worktree_root: "/tmp/wt",
  status: "active",
};

function mockInvoke(): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    switch (args.method) {
      case "Workareas.GetWorkarea":
        return Promise.resolve(workarea);
      // The Diff panel's repo list is workarea-scoped: it comes from
      // `Workareas.ListWorkareaRepos` (only the repos this workarea
      // materialized), NOT the unscoped global `Repositories.ListRepositories`.
      case "Workareas.ListWorkareaRepos":
        return Promise.resolve({ repositories: repos });
      case "Workareas.GetWorkareaRepoDiff":
        return Promise.resolve({ files: [] });
      case "Workareas.GetWorkareaPrSet":
        return Promise.resolve({ pull_requests: [] });
      case "Vcs.GetChecks":
        return Promise.resolve({ checks: [] });
      default:
        return Promise.resolve(undefined);
    }
  });
}

beforeEach(() => {
  invoke.mockReset();
  diffViewerSpy.mockReset();
  mockInvoke();
  useUiStore.setState({
    selectedWorkareaId: "wa-1",
    selectedRepoId: null,
  });
});

describe("CodePrRegion Level-1 repo selector", () => {
  it("renders one selector entry per workarea repo", async () => {
    renderWithClient(<CodePrRegion subTab="diff" />);
    expect(
      await screen.findByRole("button", { name: /api/ }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /web/ })).toBeInTheDocument();
  });

  it("auto-selects the first repo and passes its id to DiffViewer", async () => {
    renderWithClient(<CodePrRegion subTab="diff" />);
    await waitFor(() =>
      expect(diffViewerSpy).toHaveBeenCalledWith(
        expect.objectContaining({ workareaId: "wa-1", repositoryId: "repo-a" }),
      ),
    );
  });

  it("switches DiffViewer's repositoryId when another repo is selected", async () => {
    renderWithClient(<CodePrRegion subTab="diff" />);
    await screen.findByRole("button", { name: /web/ });

    await userEvent.click(screen.getByRole("button", { name: /web/ }));

    await waitFor(() =>
      expect(diffViewerSpy).toHaveBeenLastCalledWith(
        expect.objectContaining({ repositoryId: "repo-b" }),
      ),
    );
    expect(useUiStore.getState().selectedRepoId).toBe("repo-b");
  });
});
