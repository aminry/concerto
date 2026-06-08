// @vitest-environment jsdom
//
// Tests for the "Choose directories for the sparse checkout" dialog (design/02
// §3.2). Saving calls `Repositories.SetRepoConeDefaults` with the selected
// paths (sent as `cone_defaults`); the RPC returns the updated `Repository`
// and the dialog surfaces a "Saved N directories" note. `invoke` is mocked.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { SparseConeDialog } from "./SparseConeDialog";
import { renderWithClient } from "./test-utils";
import type { Repository, TreeEntry } from "../api/repositories";

const ROOT: TreeEntry[] = [
  { name: "src", is_dir: true, path: "src" },
  { name: "docs", is_dir: true, path: "docs" },
];

const REPO: Repository = {
  id: "r1",
  name: "api",
  url: "u",
  local_path: "",
  clone_strategy: "blobless",
  default_branch: "main",
  // Pre-loaded existing default.
  cone_defaults: ["src"],
};

function mockInvoke(savedCones: string[] = ["src"]): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    if (args.method === "Repositories.ListTree")
      return Promise.resolve({ entries: ROOT });
    if (args.method === "Repositories.EstimateConeSize")
      return Promise.resolve({ file_count: 5, disk_size_bytes: 100 });
    if (args.method === "Repositories.SetRepoConeDefaults")
      return Promise.resolve({ ...REPO, cone_defaults: savedCones });
    return Promise.resolve(undefined);
  });
}

beforeEach(() => {
  invoke.mockReset();
});

describe("SparseConeDialog", () => {
  it("pre-loads the repo's existing cone_defaults as the initial selection", async () => {
    mockInvoke();
    renderWithClient(
      <SparseConeDialog open onClose={() => {}} repository={REPO} />,
    );
    // The summary shows the pre-loaded `src` chip.
    expect(
      await screen.findByRole("button", { name: "Remove src" }),
    ).toBeInTheDocument();
  });

  it("saving sends cone_defaults to SetRepoConeDefaults and shows the saved note", async () => {
    mockInvoke(["src"]);
    renderWithClient(
      <SparseConeDialog open onClose={() => {}} repository={REPO} />,
    );
    await screen.findByRole("button", { name: "Remove src" });

    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.SetRepoConeDefaults",
        payload: { repository_id: "r1", cone_defaults: ["src"] },
      }),
    );
    expect(
      await screen.findByText(/Saved 1 directory as the default/i),
    ).toBeInTheDocument();
  });

  it("notes when the default cone is cleared", async () => {
    mockInvoke([]);
    renderWithClient(
      <SparseConeDialog open onClose={() => {}} repository={REPO} />,
    );
    await screen.findByRole("button", { name: "Remove src" });
    // Drop the only selected directory, then save → cleared.
    await userEvent.click(screen.getByRole("button", { name: "Remove src" }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText(/Cleared the default cone/i),
    ).toBeInTheDocument();
  });
});
