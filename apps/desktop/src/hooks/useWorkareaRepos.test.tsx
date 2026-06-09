// Hook test for `useWorkareaRepos`. Regression guard for the Desktop
// Diff-panel bug: the hook must source the workarea's repos from the
// workarea-SCOPED `Workareas.ListWorkareaRepos` RPC (only the repos the
// workarea materialized — every one diff-able), NOT the unscoped global
// `Repositories.ListRepositories` registry (which would offer repos from
// OTHER workspaces that the backend rejects as "not attached").
//
// The `workareas` / `repositories` api modules are mocked so the test pins
// which binding the hook calls, without a running Tauri shell.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Repository } from "../api/repositories";

const listWorkareaRepos = vi.fn();
const listRepositories = vi.fn();

vi.mock("../api/workareas", () => ({
  listWorkareaRepos: (...args: unknown[]) => listWorkareaRepos(...args),
}));
vi.mock("../api/repositories", () => ({
  listRepositories: (...args: unknown[]) => listRepositories(...args),
}));

import { useWorkareaRepos, workareaReposQueryKey } from "./useWorkareaRepos";

function repo(id: string): Repository {
  return {
    id,
    name: id,
    url: "u",
    local_path: "",
    clone_strategy: "full",
    default_branch: "main",
  };
}

function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

beforeEach(() => {
  listWorkareaRepos.mockReset();
  listRepositories.mockReset();
});

describe("useWorkareaRepos", () => {
  it("returns the workarea-scoped repos from Workareas.ListWorkareaRepos", async () => {
    // The workarea only materialized repo A; the global registry would also
    // hold repo B (another workspace's). The hook must surface only A.
    listWorkareaRepos.mockResolvedValueOnce({ repositories: [repo("repo-a")] });

    const { result } = renderHook(() => useWorkareaRepos("wa-1"), {
      wrapper: wrapper(),
    });

    await waitFor(() => expect(result.current.isSuccess).toBe(true));

    expect(listWorkareaRepos).toHaveBeenCalledWith("wa-1");
    expect(listRepositories).not.toHaveBeenCalled();
    expect(result.current.data?.map((r) => r.id)).toEqual(["repo-a"]);
  });

  it("short-circuits to no fetch when workareaId is null", () => {
    const { result } = renderHook(() => useWorkareaRepos(null), {
      wrapper: wrapper(),
    });

    // `enabled: false` ⇒ the query never fires.
    expect(listWorkareaRepos).not.toHaveBeenCalled();
    expect(result.current.fetchStatus).toBe("idle");
  });

  it("keys the cache per workarea", () => {
    expect(workareaReposQueryKey("wa-9")).toEqual(["workareaRepos", "wa-9"]);
  });
});
