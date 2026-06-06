// Vitest unit tests for the sparse-cone bindings (Task 322). Mocks
// `@tauri-apps/api/core`'s `invoke` so the binding shape + the
// `<Service>.<Rpc>` method strings + the snake_case payload are pinned
// without a running Tauri shell (the Task 218 pattern).

import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { estimateConeSize, setCones, type ConeStats } from "./cones";
import { createWorkarea } from "./workareas";

beforeEach(() => {
  invoke.mockReset();
});

describe("cones bindings", () => {
  it("estimateConeSize dispatches Repositories.EstimateConeSize with snake_case payload", async () => {
    const stats: ConeStats = { file_count: 1234, disk_size_bytes: 567_890 };
    invoke.mockResolvedValueOnce(stats);

    const result = await estimateConeSize("repo-1", ["src/", "packages/api"]);

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.EstimateConeSize",
      payload: { repository_id: "repo-1", cone_paths: ["src/", "packages/api"] },
    });
    // uint64 lands as a JS number under prost-serde.
    expect(result.file_count).toBe(1234);
    expect(result.disk_size_bytes).toBe(567_890);
  });

  it("estimateConeSize propagates a Core rejection (bad cone path)", async () => {
    invoke.mockRejectedValueOnce({
      kind: "Rpc",
      message: "path not found in repo: bogus/",
    });

    await expect(estimateConeSize("repo-1", ["bogus/"])).rejects.toMatchObject({
      message: expect.stringContaining("path not found"),
    });
  });

  it("setCones dispatches Repositories.SetCones with the (workarea, repo) pair", async () => {
    invoke.mockResolvedValueOnce({ cone_paths: ["src/"] });

    const res = await setCones("wa-1", "repo-1", ["src/"]);

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.SetCones",
      payload: {
        workarea_id: "wa-1",
        repository_id: "repo-1",
        cone_paths: ["src/"],
      },
    });
    expect(res.cone_paths).toEqual(["src/"]);
  });
});

describe("createWorkarea cone threading (Task 322)", () => {
  const workarea = {
    id: "wa-new",
    workspace_id: "ws-1",
    composer_name: "bach",
    branch_name: "concerto/bach",
    worktree_root: "/tmp/wt",
    status: "active",
  };

  it("creates the workarea, then applies each non-empty cone via SetCones", async () => {
    invoke.mockImplementation((cmd: string, args: { method?: string }) => {
      if (cmd === "concerto_rpc" && args.method === "Workareas.CreateWorkarea")
        return Promise.resolve(workarea);
      return Promise.resolve({ cone_paths: [] }); // SetCones echo
    });

    await createWorkarea("ws-1", {
      cones: [
        { repository_id: "repo-a", cone_paths: ["src/"] },
        { repository_id: "repo-b", cone_paths: [] }, // empty ⇒ inherit, skipped
        { repository_id: "repo-c", cone_paths: ["lib/", "pkg/"] },
      ],
    });

    // Create fires first.
    expect(invoke).toHaveBeenNthCalledWith(1, "concerto_rpc", {
      method: "Workareas.CreateWorkarea",
      payload: { workspace_id: "ws-1", permission_mode: undefined },
    });
    // SetCones fires only for the two non-empty repos, keyed to the new id.
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.SetCones",
      payload: {
        workarea_id: "wa-new",
        repository_id: "repo-a",
        cone_paths: ["src/"],
      },
    });
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.SetCones",
      payload: {
        workarea_id: "wa-new",
        repository_id: "repo-c",
        cone_paths: ["lib/", "pkg/"],
      },
    });
    // repo-b (empty) is never sent — the Core's inherited defaults stand.
    const setConesCalls = invoke.mock.calls.filter(
      (c) => c[1]?.method === "Repositories.SetCones",
    );
    expect(setConesCalls).toHaveLength(2);
  });

  it("sends no SetCones when no cones are chosen", async () => {
    invoke.mockResolvedValueOnce(workarea);
    await createWorkarea("ws-1");
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Workareas.CreateWorkarea",
      payload: { workspace_id: "ws-1", permission_mode: undefined },
    });
  });
});
