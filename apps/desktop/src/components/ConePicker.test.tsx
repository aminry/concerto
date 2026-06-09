// @vitest-environment jsdom
//
// Component tests for the sparse-cone picker (Task 322). Proves: typing a
// cone debounce-calls `Repositories.EstimateConeSize` and renders the
// `(file_count, disk_size_bytes)` feedback; a Core rejection of a bad cone
// path surfaces inline for that repo and does NOT block the other repos
// (their estimate still renders). Uses a controlled wrapper so the picker's
// raw values persist across re-renders (the real `WorkspaceDetail` owns
// that state).

import { beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  ConePicker,
  coneSelections,
  parseConePaths,
  formatBytes,
} from "./ConePicker";
import { renderWithClient } from "./test-utils";
import type { Repository } from "../api/repositories";

const repos: Repository[] = [
  { id: "repo-a", name: "api", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
  { id: "repo-b", name: "web", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
];

function ControlledPicker(): JSX.Element {
  const [values, setValues] = useState<Record<string, string>>({});
  return (
    <ConePicker
      repos={repos}
      values={values}
      onChange={(id, raw) => setValues((p) => ({ ...p, [id]: raw }))}
    />
  );
}

beforeEach(() => {
  invoke.mockReset();
});

describe("ConePicker helpers", () => {
  it("parseConePaths splits on newlines/commas and trims", () => {
    expect(parseConePaths("src/\n packages/api , lib/")).toEqual([
      "src/",
      "packages/api",
      "lib/",
    ]);
    expect(parseConePaths("  \n , ")).toEqual([]);
  });

  it("coneSelections omits repos with an empty field (inherit defaults)", () => {
    expect(coneSelections(repos, { "repo-a": "src/", "repo-b": "  " })).toEqual([
      { repository_id: "repo-a", cone_paths: ["src/"] },
    ]);
  });

  it("formatBytes is order-of-magnitude", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2_500_000)).toBe("2.4 MB");
  });
});

describe("ConePicker live estimate", () => {
  it("debounce-calls EstimateConeSize and renders (file_count, disk_size)", async () => {
    invoke.mockImplementation((cmd: string, args: { method?: string }) => {
      if (cmd === "concerto_rpc" && args.method === "Repositories.EstimateConeSize")
        return Promise.resolve({ file_count: 42, disk_size_bytes: 1_048_576 });
      return Promise.resolve(undefined);
    });

    renderWithClient(<ControlledPicker />);
    const apiInput = screen.getByLabelText(/Cone paths for api/i);
    await userEvent.type(apiInput, "src/");

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.EstimateConeSize",
        payload: { repository_id: "repo-a", cone_paths: ["src/"] },
      }),
    );
    // Scope to the api repo's row (every empty-field repo also estimates,
    // so the same numbers appear twice — assert within the api row).
    const apiRow = apiInput.closest("div.rounded-md") as HTMLElement;
    expect(await within(apiRow).findByText(/42 files/)).toBeInTheDocument();
    expect(within(apiRow).getByText(/1\.0 MB/)).toBeInTheDocument();
  });

  it("renders an inline reject for a bad cone path without blocking other repos", async () => {
    invoke.mockImplementation(
      (cmd: string, args: { method?: string; payload?: { repository_id?: string } }) => {
        if (cmd === "concerto_rpc" && args.method === "Repositories.EstimateConeSize") {
          if (args.payload?.repository_id === "repo-a")
            return Promise.reject({ kind: "Rpc", message: "path not found in repo: bogus/" });
          return Promise.resolve({ file_count: 7, disk_size_bytes: 2048 });
        }
        return Promise.resolve(undefined);
      },
    );

    renderWithClient(<ControlledPicker />);

    // repo-a gets a bad path; repo-b a good one.
    await userEvent.type(screen.getByLabelText(/Cone paths for api/i), "bogus/");
    await userEvent.type(screen.getByLabelText(/Cone paths for web/i), "src/");

    // repo-a surfaces the inline reject.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/path not found/i);

    // repo-b's estimate still renders — the bad repo didn't block it.
    expect(await screen.findByText(/7 files/)).toBeInTheDocument();
  });
});
