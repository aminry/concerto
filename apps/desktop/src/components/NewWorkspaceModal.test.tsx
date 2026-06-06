// @vitest-environment jsdom
//
// Component tests for the multi-repo New Workspace modal (Task 322).
// Proves: the repo field is a multi-select (checkbox list); submit is
// disabled until name + ≥1 repo; submitting sends
// `CreateWorkspaceRequest.repository_ids` with every checked repo.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { NewWorkspaceModal } from "./NewWorkspaceModal";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";

const repos = [
  { id: "repo-a", project_id: "p1", name: "api", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
  { id: "repo-b", project_id: "p1", name: "android", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
  { id: "repo-c", project_id: "p1", name: "ios", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
];

function mockInvoke(): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd === "concerto_rpc" && args.method === "Repositories.ListByProject")
      return Promise.resolve({ repositories: repos });
    if (cmd === "concerto_rpc" && args.method === "Workspaces.CreateWorkspace")
      return Promise.resolve({ id: "ws-new", project_id: "p1", name: "x", slug: "x" });
    return Promise.resolve(undefined);
  });
}

beforeEach(() => {
  invoke.mockReset();
  mockInvoke();
  useUiStore.setState({
    selectedProjectId: "p1",
    newWorkspaceModalOpen: true,
  });
});

describe("NewWorkspaceModal (multi-repo)", () => {
  it("renders a checkbox per repo and disables submit until name + ≥1 repo", async () => {
    renderWithClient(<NewWorkspaceModal />);

    const checkboxes = await screen.findAllByRole("checkbox");
    expect(checkboxes).toHaveLength(3);

    const submit = screen.getByRole("button", { name: /create/i });
    expect(submit).toBeDisabled();

    await userEvent.type(screen.getByPlaceholderText(/Test 1/i), "Cross-repo");
    // Name present but 0 repos → still disabled.
    expect(submit).toBeDisabled();

    await userEvent.click(checkboxes[0]);
    expect(submit).toBeEnabled();
  });

  it("submits repository_ids with every checked repo", async () => {
    renderWithClient(<NewWorkspaceModal />);

    await userEvent.type(screen.getByPlaceholderText(/Test 1/i), "Cross-repo");
    const checkboxes = await screen.findAllByRole("checkbox");
    await userEvent.click(checkboxes[0]); // api
    await userEvent.click(checkboxes[2]); // ios

    await userEvent.click(screen.getByRole("button", { name: /create/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Workspaces.CreateWorkspace",
        payload: {
          project_id: "p1",
          name: "Cross-repo",
          repository_ids: ["repo-a", "repo-c"],
          description: undefined,
          permission_mode: undefined,
        },
      }),
    );
  });
});
