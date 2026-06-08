// @vitest-environment jsdom
//
// Covers the add/remove-anytime sparse-cone entry point on the Repositories
// list (design/02 §3.2): every Blobless repo row gets a "Sparse directories"
// button that opens the `SparseConeDialog`, and saving from it calls
// `Repositories.SetRepoConeDefaults`. `invoke` is mocked; clone side-effects
// are stubbed (mirrors `AddRepositoryForm.test.tsx`).

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

vi.mock("../api/client", async (importActual) => {
  const actual = await importActual<typeof import("../api/client")>();
  return {
    ...actual,
    cloneRepository: vi.fn().mockResolvedValue(undefined),
    onCloneProgress: vi.fn().mockResolvedValue(() => {}),
  };
});

import { AddRepositoryForm } from "./AddRepositoryForm";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";

const BLOBLESS_REPO = {
  id: "r1",
  project_id: "p1",
  name: "api",
  url: "https://example.com/api.git",
  local_path: "",
  clone_strategy: "blobless",
  default_branch: "main",
  cone_defaults: ["src"],
};
const FULL_REPO = {
  id: "r2",
  project_id: "p1",
  name: "web",
  url: "https://example.com/web.git",
  local_path: "",
  clone_strategy: "full",
  default_branch: "main",
  cone_defaults: [],
};

function mockInvoke(): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    switch (args.method) {
      case "Repositories.ListByProject":
        return Promise.resolve({ repositories: [BLOBLESS_REPO, FULL_REPO] });
      case "Repositories.ListTree":
        return Promise.resolve({
          entries: [{ name: "src", is_dir: true, path: "src" }],
        });
      case "Repositories.EstimateConeSize":
        return Promise.resolve({ file_count: 1, disk_size_bytes: 10 });
      case "Repositories.SetRepoConeDefaults":
        return Promise.resolve({ cone_paths: ["src"], workareas_updated: 2 });
      case "Repositories.EstimateRepoSize":
        return Promise.reject({ kind: "Rpc", message: "no probe" });
      default:
        return Promise.resolve(undefined);
    }
  });
}

beforeEach(() => {
  invoke.mockReset();
  useUiStore.setState({ selectedProjectId: "p1" });
});

describe("AddRepositoryForm sparse-directories entry point", () => {
  it("shows the button only for blobless repos and opens the dialog", async () => {
    mockInvoke();
    renderWithClient(<AddRepositoryForm />);

    // Both repo rows render; only the blobless one gets the button.
    await screen.findByText("api");
    const buttons = screen.getAllByRole("button", {
      name: /Sparse directories/i,
    });
    expect(buttons).toHaveLength(1);

    await userEvent.click(buttons[0]);
    expect(
      await screen.findByText(/Choose directories for the sparse checkout/i),
    ).toBeInTheDocument();
  });

  it("saving from the row dialog calls SetRepoConeDefaults", async () => {
    mockInvoke();
    renderWithClient(<AddRepositoryForm />);
    await screen.findByText("api");

    await userEvent.click(
      screen.getByRole("button", { name: /Sparse directories/i }),
    );
    await screen.findByText(/Choose directories for the sparse checkout/i);
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.SetRepoConeDefaults",
        payload: { repository_id: "r1", cone_paths: ["src"] },
      }),
    );
    expect(await screen.findByText(/Updated 2 workareas/i)).toBeInTheDocument();
  });
});
