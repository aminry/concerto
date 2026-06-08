// @vitest-environment jsdom
//
// Tests for the browsable repo-tree → per-repo default-cone picker (design/02
// §3.2). `invoke` is mocked, so this pins the binding shape + method strings
// and the pure selection helpers; the shell's actual dispatch of
// `Repositories.ListTree` is confirmed by hand against `src-tauri/src/rpc.rs`.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  RepoTreeBrowser,
  addToSelection,
  isImpliedByAncestor,
  normalizeConeSelection,
  removeFromSelection,
} from "./RepoTreeBrowser";
import { renderWithClient } from "./test-utils";
import type { TreeEntry } from "../api/repositories";

const ROOT: TreeEntry[] = [
  { name: "src", is_dir: true, path: "src" },
  { name: "docs", is_dir: true, path: "docs" },
  { name: "README.md", is_dir: false, path: "README.md" },
];
const SRC_CHILDREN: TreeEntry[] = [
  { name: "api", is_dir: true, path: "src/api" },
  { name: "lib.rs", is_dir: false, path: "src/lib.rs" },
];

function mockTree(): void {
  invoke.mockImplementation(
    (cmd: string, args: { method?: string; payload?: { path?: string } }) => {
      if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
      if (args.method === "Repositories.ListTree") {
        const path = args.payload?.path ?? "";
        if (path === "") return Promise.resolve({ entries: ROOT });
        if (path === "src") return Promise.resolve({ entries: SRC_CHILDREN });
        return Promise.resolve({ entries: [] });
      }
      if (args.method === "Repositories.EstimateConeSize")
        return Promise.resolve({ file_count: 12, disk_size_bytes: 4096 });
      return Promise.resolve(undefined);
    },
  );
}

beforeEach(() => {
  invoke.mockReset();
});

describe("normalizeConeSelection / ancestor helpers", () => {
  it("drops a child redundant with a selected parent", () => {
    expect(normalizeConeSelection(["src", "src/api"])).toEqual(["src"]);
  });
  it("keeps siblings + de-duplicates", () => {
    expect(normalizeConeSelection(["src", "docs", "src"])).toEqual([
      "src",
      "docs",
    ]);
  });
  it("isImpliedByAncestor is true only for a strict descendant", () => {
    expect(isImpliedByAncestor("src/api", ["src"])).toBe(true);
    expect(isImpliedByAncestor("src", ["src"])).toBe(false);
    expect(isImpliedByAncestor("srcfoo", ["src"])).toBe(false); // boundary
  });
  it("addToSelection re-normalizes; removeFromSelection is exact", () => {
    expect(addToSelection("src", ["src/api"])).toEqual(["src"]);
    expect(removeFromSelection("src", ["src", "docs"])).toEqual(["docs"]);
  });
});

describe("RepoTreeBrowser", () => {
  it("loads the root on mount and lists directories", async () => {
    mockTree();
    renderWithClient(
      <RepoTreeBrowser repositoryId="r1" value={[]} onChange={() => {}} />,
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.ListTree",
        payload: { repository_id: "r1", path: "", git_ref: "" },
      }),
    );
    expect(await screen.findByText("src")).toBeInTheDocument();
    expect(screen.getByText("docs")).toBeInTheDocument();
    expect(screen.getByText("README.md")).toBeInTheDocument();
  });

  it("expanding a folder calls ListTree with that path", async () => {
    mockTree();
    renderWithClient(
      <RepoTreeBrowser repositoryId="r1" value={[]} onChange={() => {}} />,
    );
    await screen.findByText("src");

    await userEvent.click(screen.getByRole("button", { name: /Expand src/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.ListTree",
        payload: { repository_id: "r1", path: "src", git_ref: "" },
      }),
    );
    expect(await screen.findByText("api")).toBeInTheDocument();
    expect(screen.getByText("lib.rs")).toBeInTheDocument();
  });

  it("checking a folder adds its path to the cone", async () => {
    mockTree();
    const onChange = vi.fn();
    renderWithClient(
      <RepoTreeBrowser repositoryId="r1" value={[]} onChange={onChange} />,
    );
    await screen.findByText("src");

    await userEvent.click(screen.getByRole("checkbox", { name: "src" }));
    expect(onChange).toHaveBeenCalledWith(["src"]);
  });

  it("files are not checkable", async () => {
    mockTree();
    renderWithClient(
      <RepoTreeBrowser repositoryId="r1" value={[]} onChange={() => {}} />,
    );
    await screen.findByText("README.md");
    expect(screen.getByRole("checkbox", { name: "README.md" })).toBeDisabled();
  });

  it("a child implied by a selected ancestor is checked + disabled", async () => {
    mockTree();
    renderWithClient(
      <RepoTreeBrowser repositoryId="r1" value={["src"]} onChange={() => {}} />,
    );
    // Wait for the lazily-loaded tree row's expand control (the summary chip
    // "src" exists immediately from the value prop, so wait on the chevron).
    await userEvent.click(
      await screen.findByRole("button", { name: /Expand src/i }),
    );
    await screen.findByText("api");

    const apiCheckbox = screen.getByRole("checkbox", {
      name: /src\/api \(included via parent\)/i,
    });
    expect(apiCheckbox).toBeChecked();
    expect(apiCheckbox).toBeDisabled();
  });

  it("renders the selected-directory summary with removable chips", async () => {
    mockTree();
    const onChange = vi.fn();
    renderWithClient(
      <RepoTreeBrowser
        repositoryId="r1"
        value={["src", "docs"]}
        onChange={onChange}
      />,
    );
    await screen.findByText("Selected directories");
    await userEvent.click(screen.getByRole("button", { name: "Remove src" }));
    expect(onChange).toHaveBeenCalledWith(["docs"]);
  });

  it("shows a live size estimate for the current selection", async () => {
    mockTree();
    renderWithClient(
      <RepoTreeBrowser repositoryId="r1" value={["src"]} onChange={() => {}} />,
    );
    // The debounced EstimateConeSize fires and the file count renders.
    expect(await screen.findByText(/12 files/i)).toBeInTheDocument();
  });
});
