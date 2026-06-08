// @vitest-environment jsdom
//
// Component tests for the rebuilt New Workspace modal (Project→Workspace
// collapse). Covers the three-source repo picker (existing registry repos,
// add-by-URL, add-local-folder), the per-repo Full/Sparse checkout control,
// and the `CreateWorkspaceRequest` submit shape (`repos: { repository_id,
// sparse_cones }[]`, no project_id). `invoke` + the Tauri dialog plugin are
// mocked.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Native folder picker — returns a chosen path (or null when cancelled).
const openDialog = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openDialog(...args),
}));

// Stub the clone side-effect so the add-by-URL flow doesn't drive a stream.
vi.mock("../api/client", async (importActual) => {
  const actual = await importActual<typeof import("../api/client")>();
  return {
    ...actual,
    cloneRepository: vi.fn().mockResolvedValue(undefined),
    onCloneProgress: vi.fn().mockResolvedValue(() => {}),
  };
});

import { NewWorkspaceModal } from "./NewWorkspaceModal";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";

const repos = [
  { id: "repo-a", name: "api", url: "", local_path: "", clone_strategy: "full", default_branch: "main", cone_defaults: ["src"] },
  { id: "repo-b", name: "android", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
  { id: "repo-c", name: "ios", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
];

// Repos added during a test (URL / local-folder flows) accumulate here so a
// post-add `ListRepositories` refetch returns them, mirroring the registry.
let addedRepos: Array<Record<string, unknown>> = [];

function mockInvoke(): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd === "concerto_rpc" && args.method === "Repositories.ListRepositories")
      return Promise.resolve({ repositories: [...repos, ...addedRepos] });
    if (cmd === "concerto_rpc" && args.method === "Repositories.AddRepository") {
      const added = {
        id: "repo-new",
        name: "new",
        url: "",
        local_path: "",
        clone_strategy: "full",
        default_branch: "main",
      };
      addedRepos.push(added);
      return Promise.resolve(added);
    }
    if (cmd === "concerto_rpc" && args.method === "Repositories.ListTree")
      return Promise.resolve({
        entries: [{ name: "src", is_dir: true, path: "src" }],
      });
    if (cmd === "concerto_rpc" && args.method === "Repositories.EstimateConeSize")
      return Promise.resolve({ file_count: 1, disk_size_bytes: 10 });
    if (cmd === "concerto_rpc" && args.method === "Repositories.EstimateRepoSize")
      return Promise.reject({ kind: "Rpc", message: "no probe" });
    if (cmd === "concerto_rpc" && args.method === "Workspaces.CreateWorkspace")
      return Promise.resolve({ id: "ws-new", name: "x", slug: "x" });
    return Promise.resolve(undefined);
  });
}

beforeEach(() => {
  invoke.mockReset();
  openDialog.mockReset();
  addedRepos = [];
  mockInvoke();
  useUiStore.setState({
    selectedWorkspaceId: null,
    newWorkspaceModalOpen: true,
  });
});

/// The repo multi-select checkboxes live in the "Repositories" group; the
/// per-repo Full/Sparse radios are in their own rows. Scope to the group so
/// queries don't collide.
function repoCheckboxes(): HTMLElement[] {
  const group = screen.getByRole("group", { name: "Repositories" });
  return within(group).getAllByRole("checkbox");
}

describe("NewWorkspaceModal — registry repo multi-select", () => {
  it("renders a checkbox per existing repo and gates submit on name + ≥1 repo", async () => {
    renderWithClient(<NewWorkspaceModal />);

    await waitFor(() => expect(repoCheckboxes()).toHaveLength(3));

    const submit = screen.getByRole("button", { name: /create workspace/i });
    expect(submit).toBeDisabled();

    await userEvent.type(
      screen.getByPlaceholderText(/Payments revamp/i),
      "Cross-repo",
    );
    // Name present but 0 repos → still disabled.
    expect(submit).toBeDisabled();

    await userEvent.click(repoCheckboxes()[0]);
    expect(submit).toBeEnabled();
  });

  it("submits repos[] (full checkout ⇒ empty sparse_cones), no project_id", async () => {
    renderWithClient(<NewWorkspaceModal />);
    await waitFor(() => expect(repoCheckboxes()).toHaveLength(3));

    await userEvent.type(
      screen.getByPlaceholderText(/Payments revamp/i),
      "Cross-repo",
    );
    await userEvent.click(repoCheckboxes()[0]); // api
    await userEvent.click(repoCheckboxes()[2]); // ios

    await userEvent.click(
      screen.getByRole("button", { name: /create workspace/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Workspaces.CreateWorkspace",
        payload: {
          name: "Cross-repo",
          icon: undefined,
          description: undefined,
          permission_mode: undefined,
          repos: [
            { repository_id: "repo-a", sparse_cones: [] },
            { repository_id: "repo-c", sparse_cones: [] },
          ],
        },
      }),
    );
  });
});

describe("NewWorkspaceModal — per-repo sparse checkout", () => {
  it("switching a repo to Sparse sends its chosen cones", async () => {
    renderWithClient(<NewWorkspaceModal />);
    await waitFor(() => expect(repoCheckboxes()).toHaveLength(3));

    await userEvent.type(
      screen.getByPlaceholderText(/Payments revamp/i),
      "Sparse WS",
    );
    await userEvent.click(repoCheckboxes()[0]); // api (cone_defaults ["src"])

    // Flip to Sparse for api → the RepoTreeBrowser appears, pre-seeded with
    // the repo's cone_defaults ("src" chip).
    await userEvent.click(screen.getByRole("radio", { name: /Sparse/i }));
    expect(
      await screen.findByRole("button", { name: "Remove src" }),
    ).toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: /create workspace/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Workspaces.CreateWorkspace",
        payload: {
          name: "Sparse WS",
          icon: undefined,
          description: undefined,
          permission_mode: undefined,
          repos: [{ repository_id: "repo-a", sparse_cones: ["src"] }],
        },
      }),
    );
  });
});

describe("NewWorkspaceModal — add by URL", () => {
  it("registers a new repo and lands it selected", async () => {
    renderWithClient(<NewWorkspaceModal />);
    await waitFor(() => expect(repoCheckboxes()).toHaveLength(3));

    await userEvent.click(screen.getByRole("button", { name: /add by url/i }));
    await userEvent.type(
      screen.getByPlaceholderText(/git URL/i),
      "https://example.com/new.git",
    );
    await userEvent.click(
      screen.getByRole("button", { name: /add repository/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith(
        "concerto_rpc",
        expect.objectContaining({ method: "Repositories.AddRepository" }),
      ),
    );
    // The newly-added repo is auto-selected ⇒ a checkout row renders for it.
    expect(
      await screen.findByRole("radio", { name: /Full working tree/i }),
    ).toBeInTheDocument();
  });

  it("seeds cone_defaults from the returned Repository, not the stale repoById memo", async () => {
    // Override AddRepository to return a repo that carries cone_defaults.
    // This simulates the stale-memo scenario: the invalidated refetch hasn't
    // re-rendered yet when selectRepo is called, but the just-returned object
    // already carries the correct cone_defaults.
    invoke.mockImplementation((cmd: string, args: { method?: string }) => {
      if (cmd === "concerto_rpc" && args.method === "Repositories.ListRepositories")
        return Promise.resolve({ repositories: [...repos, ...addedRepos] });
      if (cmd === "concerto_rpc" && args.method === "Repositories.AddRepository") {
        const added = {
          id: "repo-new-cones",
          name: "new-with-cones",
          url: "",
          local_path: "",
          clone_strategy: "full",
          default_branch: "main",
          cone_defaults: ["pkg"],
        };
        addedRepos.push(added);
        return Promise.resolve(added);
      }
      if (cmd === "concerto_rpc" && args.method === "Repositories.ListTree")
        return Promise.resolve({
          entries: [{ name: "pkg", is_dir: true, path: "pkg" }],
        });
      if (cmd === "concerto_rpc" && args.method === "Repositories.EstimateConeSize")
        return Promise.resolve({ file_count: 1, disk_size_bytes: 10 });
      if (cmd === "concerto_rpc" && args.method === "Repositories.EstimateRepoSize")
        return Promise.reject({ kind: "Rpc", message: "no probe" });
      if (cmd === "concerto_rpc" && args.method === "Workspaces.CreateWorkspace")
        return Promise.resolve({ id: "ws-new", name: "x", slug: "x" });
      return Promise.resolve(undefined);
    });

    renderWithClient(<NewWorkspaceModal />);
    await waitFor(() => expect(repoCheckboxes()).toHaveLength(3));

    // Add a repo via URL — the returned repo carries cone_defaults: ["pkg"].
    await userEvent.click(screen.getByRole("button", { name: /add by url/i }));
    await userEvent.type(
      screen.getByPlaceholderText(/git URL/i),
      "https://example.com/pkg-repo.git",
    );
    await userEvent.click(
      screen.getByRole("button", { name: /add repository/i }),
    );

    // Wait for the checkout row to appear (repo auto-selected).
    expect(
      await screen.findByRole("radio", { name: /Full working tree/i }),
    ).toBeInTheDocument();

    // Flip the new repo to Sparse — the RepoTreeBrowser should be pre-seeded
    // with "pkg" from the returned Repository's cone_defaults.
    const sparseRadios = screen.getAllByRole("radio", { name: /Sparse/i });
    await userEvent.click(sparseRadios[sparseRadios.length - 1]);

    expect(
      await screen.findByRole("button", { name: "Remove pkg" }),
    ).toBeInTheDocument();

    // Submit and verify sparse_cones includes "pkg".
    await userEvent.type(
      screen.getByPlaceholderText(/Payments revamp/i),
      "Cone seed WS",
    );
    await userEvent.click(
      screen.getByRole("button", { name: /create workspace/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Workspaces.CreateWorkspace",
        payload: expect.objectContaining({
          repos: expect.arrayContaining([
            { repository_id: "repo-new-cones", sparse_cones: ["pkg"] },
          ]),
        }),
      }),
    );
  });
});

describe("NewWorkspaceModal — add local folder", () => {
  it("opens the native picker and adopts the chosen folder", async () => {
    openDialog.mockResolvedValueOnce("/Users/me/code/widget");
    renderWithClient(<NewWorkspaceModal />);
    await waitFor(() => expect(repoCheckboxes()).toHaveLength(3));

    await userEvent.click(
      screen.getByRole("button", { name: /add local folder/i }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: /choose folder/i }),
    );

    expect(openDialog).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true }),
    );
    // The chosen path surfaces; the derived name pre-fills.
    expect(await screen.findByText("/Users/me/code/widget")).toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: /add repository/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.AddRepository",
        payload: {
          name: "widget",
          url: "",
          default_branch: "",
          clone_strategy: "",
          with_sparse: false,
          local_path: "/Users/me/code/widget",
        },
      }),
    );
  });
});
