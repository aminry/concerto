// Data-layer test for the `listWorkareaRepos` binding. Mocks
// `@tauri-apps/api/core`'s `invoke` so the `Workareas.ListWorkareaRepos`
// method string + the snake_case `{ workarea_id }` payload are pinned without
// a running Tauri shell (the Task 218 pattern). That the shell actually
// dispatches the RPC is verified against `src-tauri/src/rpc.rs` + `cargo check`.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { listWorkareaRepos } from "./workareas";

beforeEach(() => {
  invoke.mockReset();
});

describe("listWorkareaRepos binding", () => {
  it("dispatches Workareas.ListWorkareaRepos with the workarea_id payload", async () => {
    invoke.mockResolvedValueOnce({
      repositories: [
        {
          id: "repo-a",
          name: "alpha",
          url: "u",
          local_path: "",
          clone_strategy: "full",
          default_branch: "main",
        },
      ],
    });

    const result = await listWorkareaRepos("wa-1");

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Workareas.ListWorkareaRepos",
      payload: { workarea_id: "wa-1" },
    });
    expect(result.repositories.map((r) => r.id)).toEqual(["repo-a"]);
  });

  it("does NOT call the global Repositories.ListRepositories", async () => {
    invoke.mockResolvedValueOnce({ repositories: [] });

    await listWorkareaRepos("wa-1");

    expect(invoke).not.toHaveBeenCalledWith(
      "concerto_rpc",
      expect.objectContaining({ method: "Repositories.ListRepositories" }),
    );
  });
});
