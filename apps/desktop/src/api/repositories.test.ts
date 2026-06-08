// Data-layer tests for the repositories bindings (DS-1). Mocks
// `@tauri-apps/api/core`'s `invoke` so the `<Service>.<Rpc>` method strings +
// the snake_case payload shapes are pinned without a running Tauri shell (the
// Task 218 pattern). NOTE: `invoke` is mocked, so these prove the binding
// shape only — that the shell actually dispatches `Repositories.EstimateRepoSize`
// is verified by hand against `src-tauri/src/rpc.rs` + `cargo check`.

import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  addRepository,
  estimateRepoSize,
  type SizeReport,
} from "./repositories";

beforeEach(() => {
  invoke.mockReset();
});

describe("estimateRepoSize binding", () => {
  it("dispatches Repositories.EstimateRepoSize with the url payload", async () => {
    const report: SizeReport = {
      size_bytes: 4_200_000_000,
      object_count: 1_100_000,
      branch_count: 37,
      recommended_strategy: "blobless",
      recommend_sparse: false,
    };
    invoke.mockResolvedValueOnce(report);

    const result = await estimateRepoSize("https://example.com/repo.git");

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.EstimateRepoSize",
      payload: { url: "https://example.com/repo.git" },
    });
    // uint64 lands as a JS number under prost-serde.
    expect(result.size_bytes).toBe(4_200_000_000);
    expect(result.recommended_strategy).toBe("blobless");
    expect(result.recommend_sparse).toBe(false);
  });

  it("propagates a Core probe failure (private/offline remote)", async () => {
    invoke.mockRejectedValueOnce({
      kind: "Rpc",
      message: "could not read remote: authentication required",
    });

    await expect(
      estimateRepoSize("git@private:repo.git"),
    ).rejects.toMatchObject({
      message: expect.stringContaining("authentication required"),
    });
  });
});

describe("addRepository strategy passthrough", () => {
  const repo = {
    id: "r1",
    project_id: "p1",
    name: "api",
    url: "u",
    local_path: "",
    clone_strategy: "blobless",
    default_branch: "main",
  };

  it("sends clone_strategy + with_sparse for Blobless + Sparse", async () => {
    invoke.mockResolvedValueOnce(repo);

    await addRepository({
      projectId: "p1",
      name: "api",
      url: "u",
      cloneStrategy: "blobless",
      withSparse: true,
    });

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.AddRepository",
      payload: {
        project_id: "p1",
        name: "api",
        url: "u",
        default_branch: "",
        clone_strategy: "blobless",
        with_sparse: true,
      },
    });
  });

  it("defaults to Full (empty clone_strategy, with_sparse=false) when omitted", async () => {
    invoke.mockResolvedValueOnce(repo);

    await addRepository({ projectId: "p1", name: "api", url: "u" });

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.AddRepository",
      payload: {
        project_id: "p1",
        name: "api",
        url: "u",
        default_branch: "",
        clone_strategy: "",
        with_sparse: false,
      },
    });
  });
});
