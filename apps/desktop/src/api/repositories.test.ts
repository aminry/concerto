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
  suggestCones,
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

describe("suggestCones binding (Task 418 / 411 backend)", () => {
  it("dispatches Repositories.SuggestCones with the repo id + issue text", async () => {
    invoke.mockResolvedValueOnce({ cone_paths: ["src/auth", "packages/sso"] });

    const result = await suggestCones("repo-a", "add SSO to the API");

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.SuggestCones",
      payload: { repository_id: "repo-a", issue_text: "add SSO to the API" },
    });
    expect(result).toEqual(["src/auth", "packages/sso"]);
  });

  it("normalizes a missing cone_paths to []", async () => {
    invoke.mockResolvedValueOnce({});

    const result = await suggestCones("repo-a", "freeform");

    expect(result).toEqual([]);
  });

  it("propagates UNIMPLEMENTED from a suggester-less Core", async () => {
    invoke.mockRejectedValueOnce({
      kind: "not_implemented",
      message: "suggest_cones is wired in P4 (Maestro, Task 411)",
    });

    await expect(suggestCones("repo-a", "x")).rejects.toMatchObject({
      message: expect.stringContaining("suggest_cones"),
    });
  });
});

describe("addRepository strategy passthrough", () => {
  const repo = {
    id: "r1",
    name: "api",
    url: "u",
    local_path: "",
    clone_strategy: "blobless",
    default_branch: "main",
  };

  it("sends clone_strategy + with_sparse for Blobless + Sparse (no project_id)", async () => {
    invoke.mockResolvedValueOnce(repo);

    await addRepository({
      name: "api",
      url: "u",
      cloneStrategy: "blobless",
      withSparse: true,
    });

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.AddRepository",
      payload: {
        name: "api",
        url: "u",
        default_branch: "",
        clone_strategy: "blobless",
        with_sparse: true,
        local_path: "",
      },
    });
  });

  it("defaults to Full (empty clone_strategy, with_sparse=false) when omitted", async () => {
    invoke.mockResolvedValueOnce(repo);

    await addRepository({ name: "api", url: "u" });

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.AddRepository",
      payload: {
        name: "api",
        url: "u",
        default_branch: "",
        clone_strategy: "",
        with_sparse: false,
        local_path: "",
      },
    });
  });

  it("adopts a local folder (local_path set, url empty)", async () => {
    invoke.mockResolvedValueOnce({ ...repo, local_path: "/tmp/api" });

    await addRepository({ name: "api", localPath: "/tmp/api" });

    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Repositories.AddRepository",
      payload: {
        name: "api",
        url: "",
        default_branch: "",
        clone_strategy: "",
        with_sparse: false,
        local_path: "/tmp/api",
      },
    });
  });
});
