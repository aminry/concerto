// @vitest-environment jsdom
//
// Component tests for the parallel-workareas summary (Task 323, design/15
// §3.4). Proves: the summary renders the workspace's workareas as rows with
// status dots (replacing the V0.1 JSON dump); a "+ new workarea" affordance
// invokes the parent's create flow; clicking a row selects the workarea
// (`setSelectedWorkarea`); the cross-workarea PR-set placeholder slot
// renders (Task 324 fills it). Status colors come from the shared
// `workareaStatusToDot` mapper — including Task 307's `finished`/`partial`.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// The summary mounts a live `workarea.events` subscription via
// `useEventSubscription`; stub the event bridge so the subscription is a
// no-op in jsdom (no Tauri runtime).
vi.mock("../api/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api/client")>();
  return {
    ...actual,
    onConcertoEvent: vi.fn(async () => () => {}),
    subscribe: vi.fn(async () => "sub-1"),
    unsubscribe: vi.fn(async () => {}),
  };
});

import { WorkspaceSummary } from "./WorkspaceSummary";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";

const workareas = [
  {
    id: "wa-1",
    workspace_id: "ws-1",
    composer_name: "bach",
    branch_name: "concerto/bach",
    worktree_root: "/tmp/wt1",
    status: "running",
  },
  {
    id: "wa-2",
    workspace_id: "ws-1",
    composer_name: "mozart",
    branch_name: "concerto/mozart",
    worktree_root: "/tmp/wt2",
    status: "partial",
  },
];

function mockWorkareas(list = workareas): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    if (args.method === "Workareas.ListWorkareas") {
      return Promise.resolve({ workareas: list });
    }
    return Promise.resolve(undefined);
  });
}

beforeEach(() => {
  invoke.mockReset();
  mockWorkareas();
  useUiStore.setState({ selectedWorkareaId: null });
});

describe("WorkspaceSummary", () => {
  it("renders the workareas as rows with status dots and branch chips", async () => {
    renderWithClient(
      <WorkspaceSummary workspaceId="ws-1" onNewWorkarea={() => {}} />,
    );

    expect(await screen.findByText("bach")).toBeInTheDocument();
    expect(screen.getByText("mozart")).toBeInTheDocument();
    expect(screen.getByText("concerto/bach")).toBeInTheDocument();
    expect(screen.getByText("concerto/mozart")).toBeInTheDocument();
    // Status dots carry an accessible label from the shared mapper:
    // running → "Running", partial → "Warning".
    expect(screen.getByLabelText("Running")).toBeInTheDocument();
    expect(screen.getByLabelText("Warning")).toBeInTheDocument();
    // No JSON dump remains.
    expect(screen.queryByText(/worktree_root/)).not.toBeInTheDocument();
  });

  it("renders the cross-workarea PR-set placeholder slot", async () => {
    renderWithClient(
      <WorkspaceSummary workspaceId="ws-1" onNewWorkarea={() => {}} />,
    );
    await screen.findByText("bach");
    // One em-dash placeholder per workarea row (Task 324 fills it).
    expect(screen.getAllByText("—")).toHaveLength(2);
  });

  it("'+ new workarea' invokes the parent's create flow", async () => {
    const onNewWorkarea = vi.fn();
    renderWithClient(
      <WorkspaceSummary workspaceId="ws-1" onNewWorkarea={onNewWorkarea} />,
    );
    await screen.findByText("bach");
    await userEvent.click(
      screen.getByRole("button", { name: /new workarea/i }),
    );
    expect(onNewWorkarea).toHaveBeenCalledTimes(1);
  });

  it("clicking a workarea row selects it", async () => {
    renderWithClient(
      <WorkspaceSummary workspaceId="ws-1" onNewWorkarea={() => {}} />,
    );
    await userEvent.click(await screen.findByText("mozart"));
    await waitFor(() =>
      expect(useUiStore.getState().selectedWorkareaId).toBe("wa-2"),
    );
  });

  it("shows an empty state when the workspace has no workareas", async () => {
    mockWorkareas([]);
    renderWithClient(
      <WorkspaceSummary workspaceId="ws-1" onNewWorkarea={() => {}} />,
    );
    expect(await screen.findByText(/no workareas yet/i)).toBeInTheDocument();
  });
});
