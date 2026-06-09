// @vitest-environment jsdom
//
// Component tests for the add-repo clone-strategy picker (DS-1, design/02
// §3.5 + design/15 §7.1). Proves: entering a URL probes
// `Repositories.EstimateRepoSize` and renders a size→strategy recommendation;
// the selector defaults to the recommendation; the user can override it and
// the override is what `AddRepository` receives; an omitted/Full choice sends
// the empty-strategy default; and a probe failure degrades to a manual pick
// (no block). Treeless is never offered (design/02 §12 R-1).
//
// `invoke` is mocked, so this pins the binding shape + method strings only —
// the shell's actual dispatch of `Repositories.EstimateRepoSize` is confirmed
// by hand against `src-tauri/src/rpc.rs`.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Keep `callRpc` real (it routes through the mocked `invoke`) but stub the
// clone side-effects so a successful submit doesn't try to drive a stream.
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
import type { SizeReport } from "../api/repositories";

const BLOBLESS_REPORT: SizeReport = {
  size_bytes: 4_200_000_000,
  object_count: 1_100_000,
  branch_count: 12,
  recommended_strategy: "blobless",
  recommend_sparse: false,
};

function mockInvoke(opts: {
  size?: SizeReport | "fail";
}): void {
  invoke.mockImplementation(
    (cmd: string, args: { method?: string }) => {
      if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
      if (args.method === "Repositories.EstimateRepoSize") {
        if (opts.size === "fail")
          return Promise.reject({ kind: "Rpc", message: "could not reach remote" });
        return Promise.resolve(opts.size ?? BLOBLESS_REPORT);
      }
      if (args.method === "Repositories.ListRepositories")
        return Promise.resolve({ repositories: [] });
      if (args.method === "Repositories.AddRepository")
        return Promise.resolve({
          id: "r1",
          name: "api",
          url: "u",
          local_path: "",
          clone_strategy: "blobless",
          default_branch: "main",
        });
      return Promise.resolve(undefined);
    },
  );
}

beforeEach(() => {
  invoke.mockReset();
});

describe("AddRepositoryForm clone-strategy picker", () => {
  it("probes the URL and renders the size→strategy recommendation", async () => {
    mockInvoke({ size: BLOBLESS_REPORT });
    renderWithClient(<AddRepositoryForm />);

    await userEvent.type(
      screen.getByPlaceholderText(/git URL/i),
      "https://example.com/repo.git",
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.EstimateRepoSize",
        payload: { url: "https://example.com/repo.git" },
      }),
    );
    // The recommendation line shows the size + recommended strategy.
    expect(
      await screen.findByText(/recommended:/i),
    ).toHaveTextContent(/Blobless/);
    // Treeless is never offered.
    expect(screen.queryByText(/treeless/i)).not.toBeInTheDocument();
    // The selector defaulted to the recommendation.
    expect(screen.getByRole("button", { name: "Blobless" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  it("submits the recommended strategy by default", async () => {
    mockInvoke({ size: BLOBLESS_REPORT });
    renderWithClient(<AddRepositoryForm />);

    await userEvent.type(
      screen.getByPlaceholderText(/git URL/i),
      "https://example.com/repo.git",
    );
    await screen.findByText(/recommended:/i);
    await userEvent.type(screen.getByPlaceholderText(/short label/i), "api");

    await userEvent.click(screen.getByRole("button", { name: /Add \+ Clone/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.AddRepository",
        payload: {
          name: "api",
          url: "https://example.com/repo.git",
          default_branch: "",
          clone_strategy: "blobless",
          with_sparse: false,
          local_path: "",
        },
      }),
    );
  });

  it("lets the user override the recommendation (Blobless + Sparse)", async () => {
    mockInvoke({ size: BLOBLESS_REPORT });
    renderWithClient(<AddRepositoryForm />);

    await userEvent.type(
      screen.getByPlaceholderText(/git URL/i),
      "https://example.com/repo.git",
    );
    await screen.findByText(/recommended:/i);
    await userEvent.type(screen.getByPlaceholderText(/short label/i), "api");

    // Override to Blobless + Sparse.
    await userEvent.click(
      screen.getByRole("button", { name: "Blobless + Sparse" }),
    );
    await userEvent.click(screen.getByRole("button", { name: /Add \+ Clone/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.AddRepository",
        payload: {
          name: "api",
          url: "https://example.com/repo.git",
          default_branch: "",
          clone_strategy: "blobless",
          with_sparse: true,
          local_path: "",
        },
      }),
    );
  });

  it("degrades to a manual pick on probe failure (defaults to Full)", async () => {
    mockInvoke({ size: "fail" });
    renderWithClient(<AddRepositoryForm />);

    await userEvent.type(screen.getByPlaceholderText(/short label/i), "api");
    await userEvent.type(
      screen.getByPlaceholderText(/git URL/i),
      "git@private:repo.git",
    );

    // The failure note shows; the selector stays usable defaulting to Full.
    expect(await screen.findByText(/Couldn.t reach the remote/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Full" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    await userEvent.click(screen.getByRole("button", { name: /Add \+ Clone/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.AddRepository",
        payload: {
          name: "api",
          url: "git@private:repo.git",
          default_branch: "",
          // Picker visible + Full selected ⇒ explicit "full" (which the
          // Core treats identically to the empty-string default).
          clone_strategy: "full",
          with_sparse: false,
          local_path: "",
        },
      }),
    );
  });
});
