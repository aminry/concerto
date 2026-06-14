// @vitest-environment jsdom
//
// Component tests for the §3.8 create-workspace-from-description stepper
// (Task 418). Proves the flow wiring against a mocked `invoke`:
//   detect repos → `Repositories.SuggestCones` seeds the reused ConePicker →
//   the user edits the repo set / cones → the confirmation slate drives the
//   create ONLY on an explicit confirm (the never-silent R-2 invariant).
//
// `invoke` (and the bootstrap side-effect) are mocked; nothing here round-trips
// a real Tauri shell or a real issue — that is the Phase-4 Tier-3 line.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Auto-bootstrap (first workarea + session) for the "create + first workarea"
// confirm option.
vi.mock("./bootstrapWorkspace", () => ({
  bootstrapWorkspace: vi
    .fn()
    .mockResolvedValue({ workareaId: "wa1", sessionId: "s1" }),
  DEFAULT_FIRST_AGENT: "claude",
}));

import {
  CreateFromDescription,
  detectIssueUrl,
  detectRepos,
} from "./CreateFromDescription";
import { renderWithClient } from "./test-utils";
import { bootstrapWorkspace } from "./bootstrapWorkspace";
import type { Repository } from "../api/repositories";

const repos: Repository[] = [
  { id: "repo-a", name: "api", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
  { id: "repo-b", name: "ios", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
  { id: "repo-c", name: "android", url: "", local_path: "", clone_strategy: "full", default_branch: "main" },
];

// Cone suggestions keyed by repo id (the mocked SuggestCones response).
const suggestions: Record<string, string[]> = {
  "repo-a": ["src/auth"],
  "repo-b": ["Sources/SSO"],
  "repo-c": ["app/sso"],
};

function mockInvoke(): void {
  invoke.mockImplementation((cmd: string, args: { method?: string; payload?: { repository_id?: string } }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    switch (args.method) {
      case "Repositories.ListRepositories":
        return Promise.resolve({ repositories: repos });
      case "Repositories.SuggestCones": {
        const id = args.payload?.repository_id ?? "";
        return Promise.resolve({ cone_paths: suggestions[id] ?? [] });
      }
      // The ConePicker fires EstimateConeSize as the user reviews cones.
      case "Repositories.EstimateConeSize":
        return Promise.resolve({ file_count: 3, disk_size_bytes: 99 });
      case "Workspaces.CreateWorkspace":
        return Promise.resolve({ id: "ws-new", name: "x", slug: "x" });
      case "Workareas.CreateWorkarea":
        return Promise.resolve({ id: "wa1" });
      default:
        return Promise.resolve(undefined);
    }
  });
}

beforeEach(() => {
  invoke.mockReset();
  vi.mocked(bootstrapWorkspace).mockClear();
  mockInvoke();
});

function createCalls(): unknown[] {
  return invoke.mock.calls.filter(
    (c) => c[0] === "concerto_rpc" && (c[1] as { method?: string }).method === "Workspaces.CreateWorkspace",
  );
}

describe("detectIssueUrl", () => {
  it("extracts a Linear / GitHub issue URL", () => {
    expect(
      detectIssueUrl("add SSO — https://linear.app/acme/issue/ABC-1 please"),
    ).toBe("https://linear.app/acme/issue/ABC-1");
    expect(
      detectIssueUrl("see https://github.com/acme/api/issues/42"),
    ).toBe("https://github.com/acme/api/issues/42");
  });
  it("returns null for a pure freeform description", () => {
    expect(detectIssueUrl("just refactor the auth module")).toBeNull();
  });
});

describe("detectRepos", () => {
  it("proposes the named repo subset, not unrelated repos", () => {
    const ids = detectRepos("add SSO to the API and the iOS app", repos);
    expect(ids).toContain("repo-a"); // api
    expect(ids).toContain("repo-b"); // ios
    expect(ids).not.toContain("repo-c"); // android — not named
  });
  it("does not match a repo name embedded mid-word", () => {
    // "api" must not match inside "rapidly".
    expect(detectRepos("ship this rapidly", repos)).toEqual([]);
  });
});

describe("CreateFromDescription — detect → suggest → confirm → create", () => {
  it("seeds the ConePicker from SuggestCones and creates only on explicit confirm", async () => {
    const onCreated = vi.fn();
    renderWithClient(
      <CreateFromDescription onCreated={onCreated} onCancel={() => {}} />,
    );

    // Step 1 — describe (names api + ios).
    await userEvent.type(
      screen.getByLabelText(/Description or issue link/i),
      "add SSO to the API and the iOS app — https://linear.app/acme/issue/L-1",
    );
    // Detected issue link surfaces.
    expect(screen.getByText(/Detected issue link/i)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /^Next/i }));

    // Step 2 — repos pre-checked by the detector (api + ios), android not.
    const group = await screen.findByRole("group", {
      name: "Detected repositories",
    });
    const boxes = within(group).getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes[0].checked).toBe(true); // api
    expect(boxes[1].checked).toBe(true); // ios
    expect(boxes[2].checked).toBe(false); // android

    // NOTHING created yet (never-silent).
    expect(createCalls()).toHaveLength(0);

    await userEvent.click(
      screen.getByRole("button", { name: /Suggest cones/i }),
    );

    // Step 3 — ConePicker seeded from SuggestCones for the two selected repos.
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Repositories.SuggestCones",
        payload: { repository_id: "repo-a", issue_text: expect.stringContaining("SSO") },
      }),
    );
    const apiCone = (await screen.findByLabelText(
      "Cone paths for api",
    )) as HTMLTextAreaElement;
    await waitFor(() => expect(apiCone.value).toBe("src/auth"));

    await userEvent.click(screen.getByRole("button", { name: /Review/i }));

    // Step 4 — confirmation slate. Still nothing created.
    expect(
      screen.getByRole("button", {
        name: /Create workspace \+ first workarea/i,
      }),
    ).toBeInTheDocument();
    expect(createCalls()).toHaveLength(0);

    // Explicit confirm → create + bootstrap.
    await userEvent.click(
      screen.getByRole("button", {
        name: /Create workspace \+ first workarea/i,
      }),
    );

    await waitFor(() => expect(createCalls()).toHaveLength(1));
    // The create carries the two selected repos with the seeded cones.
    expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
      method: "Workspaces.CreateWorkspace",
      payload: expect.objectContaining({
        repos: [
          { repository_id: "repo-a", sparse_cones: ["src/auth"] },
          { repository_id: "repo-b", sparse_cones: ["Sources/SSO"] },
        ],
      }),
    });
    await waitFor(() => expect(bootstrapWorkspace).toHaveBeenCalledWith("ws-new"));
    expect(onCreated).toHaveBeenCalledWith("ws-new", "s1");
  });

  it("'Just the workspace, no workarea' creates without bootstrapping", async () => {
    const onCreated = vi.fn();
    renderWithClient(
      <CreateFromDescription onCreated={onCreated} onCancel={() => {}} />,
    );

    await userEvent.type(
      screen.getByLabelText(/Description or issue link/i),
      "work on the api",
    );
    await userEvent.click(screen.getByRole("button", { name: /^Next/i }));
    await userEvent.click(
      screen.getByRole("button", { name: /Suggest cones/i }),
    );
    await screen.findByLabelText("Cone paths for api");
    await userEvent.click(screen.getByRole("button", { name: /Review/i }));

    await userEvent.click(
      screen.getByRole("button", { name: /Just the workspace/i }),
    );

    await waitFor(() => expect(createCalls()).toHaveLength(1));
    expect(bootstrapWorkspace).not.toHaveBeenCalled();
    expect(onCreated).toHaveBeenCalledWith("ws-new", null);
  });

  it("'Edit repo set / cones' returns to the editable repo step without creating", async () => {
    renderWithClient(
      <CreateFromDescription onCreated={() => {}} onCancel={() => {}} />,
    );

    await userEvent.type(
      screen.getByLabelText(/Description or issue link/i),
      "work on the api",
    );
    await userEvent.click(screen.getByRole("button", { name: /^Next/i }));
    await userEvent.click(
      screen.getByRole("button", { name: /Suggest cones/i }),
    );
    await screen.findByLabelText("Cone paths for api");
    await userEvent.click(screen.getByRole("button", { name: /Review/i }));

    // Edit → back to step 2, and the user adds android.
    await userEvent.click(
      screen.getByRole("button", { name: /Edit repo set/i }),
    );
    const group = await screen.findByRole("group", {
      name: "Detected repositories",
    });
    const boxes = within(group).getAllByRole("checkbox");
    await userEvent.click(boxes[2]); // android
    expect(createCalls()).toHaveLength(0); // still nothing created
  });

  it("a freeform description with NO issue URL still reaches the confirm step (no silent create)", async () => {
    renderWithClient(
      <CreateFromDescription onCreated={() => {}} onCancel={() => {}} />,
    );

    await userEvent.type(
      screen.getByLabelText(/Description or issue link/i),
      "refactor the api auth layer",
    );
    // No detected-issue line for a freeform description.
    expect(screen.queryByText(/Detected issue link/i)).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: /^Next/i }));
    await userEvent.click(
      screen.getByRole("button", { name: /Suggest cones/i }),
    );
    await screen.findByLabelText("Cone paths for api");
    await userEvent.click(screen.getByRole("button", { name: /Review/i }));

    // Reached the confirm slate; create has NOT fired.
    expect(
      screen.getByRole("button", {
        name: /Create workspace \+ first workarea/i,
      }),
    ).toBeInTheDocument();
    expect(createCalls()).toHaveLength(0);
  });

  it("degrades to an empty cone seed when SuggestCones rejects (UNIMPLEMENTED)", async () => {
    invoke.mockImplementation((cmd: string, args: { method?: string }) => {
      if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
      if (args.method === "Repositories.ListRepositories")
        return Promise.resolve({ repositories: repos });
      if (args.method === "Repositories.SuggestCones")
        return Promise.reject({ kind: "not_implemented", message: "unwired" });
      if (args.method === "Repositories.EstimateConeSize")
        return Promise.resolve({ file_count: 0, disk_size_bytes: 0 });
      if (args.method === "Workspaces.CreateWorkspace")
        return Promise.resolve({ id: "ws-new", name: "x", slug: "x" });
      return Promise.resolve(undefined);
    });

    renderWithClient(
      <CreateFromDescription onCreated={() => {}} onCancel={() => {}} />,
    );

    await userEvent.type(
      screen.getByLabelText(/Description or issue link/i),
      "work on the api",
    );
    await userEvent.click(screen.getByRole("button", { name: /^Next/i }));
    await userEvent.click(
      screen.getByRole("button", { name: /Suggest cones/i }),
    );

    // The flow still advances to the cone step with an empty (manual) seed.
    const apiCone = (await screen.findByLabelText(
      "Cone paths for api",
    )) as HTMLTextAreaElement;
    expect(apiCone.value).toBe("");
  });
});
