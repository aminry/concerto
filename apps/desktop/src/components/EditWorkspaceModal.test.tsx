// @vitest-environment jsdom
//
// Component tests for the Edit Workspace modal (Task 13). The modal pre-fills
// the shared `WorkspaceForm` (edit mode) from `getWorkspace` +
// `listWorkspaceRepos`, surfaces a notice when the workspace already has
// workareas, and saves via `updateWorkspace`. In edit mode the create-only
// auto-name MUST NOT overwrite the saved name when the repo selection changes.
//
// The three api modules the modal + form touch are mocked: `../api/workspaces`
// (getWorkspace/listWorkspaceRepos/updateWorkspace), `../api/workareas`
// (listWorkareas), and `../api/repositories` (listRepositories, queried by the
// shared WorkspaceForm so the registry repo rows render).

import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

vi.mock("../api/workspaces", () => ({
  getWorkspace: vi.fn().mockResolvedValue({
    id: "ws1",
    name: "Payments",
    slug: "payments",
    icon: "💸",
    description: "desc",
  }),
  listWorkspaceRepos: vi.fn().mockResolvedValue({
    repos: [{ repository_id: "repoA", sparse_cones: [] }],
  }),
  updateWorkspace: vi.fn().mockResolvedValue({ id: "ws1" }),
}));

// Default: no workareas. The notice test overrides this per-test.
vi.mock("../api/workareas", () => ({
  listWorkareas: vi.fn().mockResolvedValue({ workareas: [] }),
}));

vi.mock("../api/repositories", () => ({
  listRepositories: vi.fn().mockResolvedValue({
    repositories: [
      {
        id: "repoA",
        name: "repoA",
        url: "",
        local_path: "",
        clone_strategy: "full",
        default_branch: "main",
        cone_defaults: [],
      },
      {
        id: "repoB",
        name: "repoB",
        url: "",
        local_path: "",
        clone_strategy: "full",
        default_branch: "main",
        cone_defaults: [],
      },
    ],
  }),
}));

import { getWorkspace, updateWorkspace } from "../api/workspaces";
import { listWorkareas } from "../api/workareas";
import { useUiStore } from "../state/useUiStore";
import { renderWithClient } from "./test-utils";
import { EditWorkspaceModal } from "./EditWorkspaceModal";

describe("EditWorkspaceModal", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useUiStore.setState({ editWorkspaceId: "ws1" });
  });

  afterEach(() => {
    useUiStore.setState({ editWorkspaceId: null });
  });

  it("pre-fills name/icon/description from the workspace + its repos", async () => {
    renderWithClient(<EditWorkspaceModal />);

    await waitFor(() => expect(getWorkspace).toHaveBeenCalledWith("ws1"));

    // Name, icon, and description land pre-filled.
    expect(await screen.findByDisplayValue("Payments")).toBeInTheDocument();
    expect(screen.getByDisplayValue("💸")).toBeInTheDocument();
    expect(screen.getByDisplayValue("desc")).toBeInTheDocument();

    // The declared repo is pre-selected ⇒ its checkout row renders (once the
    // registry list query — owned by the shared WorkspaceForm — resolves).
    expect(
      await screen.findByRole("radiogroup", { name: /Checkout for repoA/i }),
    ).toBeInTheDocument();
  });

  it("shows a workareas notice when the workspace has workareas", async () => {
    vi.mocked(listWorkareas).mockResolvedValueOnce({
      workareas: [{ id: "wa1" }],
    } as never);

    renderWithClient(<EditWorkspaceModal />);

    expect(
      await screen.findByText(/existing workareas keep their current repos/i),
    ).toBeInTheDocument();
  });

  it("does NOT show the notice when there are no workareas", async () => {
    renderWithClient(<EditWorkspaceModal />);

    // Wait for the form (pre-filled name) before asserting absence.
    await screen.findByDisplayValue("Payments");
    expect(
      screen.queryByText(/existing workareas keep their current repos/i),
    ).not.toBeInTheDocument();
  });

  it("saves edits via updateWorkspace", async () => {
    renderWithClient(<EditWorkspaceModal />);

    const name = await screen.findByDisplayValue("Payments");
    await userEvent.clear(name);
    await userEvent.type(name, "Payments v2");

    await userEvent.click(
      screen.getByRole("button", { name: /save changes/i }),
    );

    await waitFor(() =>
      expect(updateWorkspace).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "ws1",
          name: "Payments v2",
          repos: expect.arrayContaining([
            { repositoryId: "repoA", sparseCones: [] },
          ]),
        }),
      ),
    );
  });

  it("does not auto-overwrite the saved name when the repo selection changes", async () => {
    renderWithClient(<EditWorkspaceModal />);

    const name = (await screen.findByDisplayValue(
      "Payments",
    )) as HTMLInputElement;

    // Toggle on another registry repo. In create mode this would auto-rewrite
    // the name from the selected repo names; in edit mode the name is owned
    // from the start and must stay put.
    const group = await screen.findByRole("group", { name: "Repositories" });
    const repoBCheckbox = within(group)
      .getAllByRole("checkbox")
      .find((cb) => cb.closest("label")?.textContent?.includes("repoB"));
    expect(repoBCheckbox).toBeDefined();
    await userEvent.click(repoBCheckbox as HTMLElement);

    // Name unchanged despite the selection change.
    expect(name.value).toBe("Payments");
  });
});
