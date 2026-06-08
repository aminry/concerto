// @vitest-environment jsdom
//
// Component tests for the New-Project modal's inline first-repository step.
// Proves: creating a project with no repo URL only calls
// `Projects.CreateProject` (unchanged behaviour); supplying a URL + choosing
// "Blobless + Sparse" creates the project then adds + clones the repo into it
// via `Repositories.AddRepository` (clone_strategy=blobless, with_sparse=true)
// and `clone_repository`; and the repo name defaults to the URL's basename
// when left blank.
//
// `invoke` is mocked, so this pins the binding shape + method strings only —
// the shell's actual dispatch is confirmed by hand against
// `src-tauri/src/rpc.rs`.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Keep `callRpc` real (routes through the mocked `invoke`) but stub the clone
// side-effect so a successful submit doesn't try to drive a stream.
const cloneRepository = vi.fn().mockResolvedValue(undefined);
vi.mock("../api/client", async (importActual) => {
  const actual = await importActual<typeof import("../api/client")>();
  return { ...actual, cloneRepository: (id: string) => cloneRepository(id) };
});

import { NewProjectModal } from "./NewProjectModal";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";

function mockInvoke(): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    switch (args.method) {
      case "Projects.CreateProject":
        return Promise.resolve({ id: "p1", name: "Concerto" });
      case "Repositories.EstimateRepoSize":
        return Promise.reject({ kind: "Rpc", message: "offline" });
      case "Repositories.AddRepository":
        return Promise.resolve({
          id: "r1",
          project_id: "p1",
          name: "web",
          url: "u",
          local_path: "",
          clone_strategy: "blobless",
          default_branch: "main",
        });
      default:
        return Promise.resolve(undefined);
    }
  });
}

beforeEach(() => {
  invoke.mockReset();
  cloneRepository.mockClear();
  useUiStore.setState({ newProjectModalOpen: true });
});

describe("NewProjectModal first-repository step", () => {
  it("creates an empty project when no repo URL is given", async () => {
    mockInvoke();
    renderWithClient(<NewProjectModal />);

    await userEvent.type(screen.getByPlaceholderText(/e\.g\. Concerto/i), "Concerto");
    await userEvent.click(screen.getByRole("button", { name: /^Create$/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Projects.CreateProject",
        payload: { name: "Concerto" },
      }),
    );
    // No repo was added.
    const addCalls = invoke.mock.calls.filter(
      ([, a]) => (a as { method?: string }).method === "Repositories.AddRepository",
    );
    expect(addCalls).toHaveLength(0);
  });

  it("adds + clones a Blobless+Sparse repo into the new project", async () => {
    mockInvoke();
    renderWithClient(<NewProjectModal />);

    await userEvent.type(screen.getByPlaceholderText(/e\.g\. Concerto/i), "Concerto");
    await userEvent.type(
      screen.getByPlaceholderText(/git URL/i),
      "https://example.com/acme/web.git",
    );
    // Repo name left blank ⇒ defaults to the URL basename ("web").
    await userEvent.click(
      screen.getByRole("button", { name: "Blobless + Sparse" }),
    );
    await userEvent.click(screen.getByRole("button", { name: /Create \+ Clone/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.AddRepository",
        payload: {
          project_id: "p1",
          name: "web",
          url: "https://example.com/acme/web.git",
          default_branch: "",
          clone_strategy: "blobless",
          with_sparse: true,
        },
      }),
    );
    await waitFor(() => expect(cloneRepository).toHaveBeenCalledWith("r1"));
  });
});
