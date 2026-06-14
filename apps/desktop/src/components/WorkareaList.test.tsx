// @vitest-environment jsdom
//
// Component test for the per-workarea Maestro-visibility toggle (Task 417,
// design/08 §3.3). Proves: an `exclude_from_maestro` workarea renders a
// "private" badge; the per-row menu calls `Maestro.SetWorkareaVisibility` with
// the correct `MaestroVisibility` enum tag (HARD_FACTS_ONLY = 2 / FULL = 1).
// The server-side blanking is Task 413's; this only drives the toggle.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { WorkareaList } from "./WorkareaList";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";

function workarea(
  id: string,
  composer_name: string,
  exclude_from_maestro = false,
) {
  return {
    id,
    workspace_id: "ws-1",
    composer_name,
    branch_name: `feat/${composer_name}`,
    worktree_root: `/tmp/${id}`,
    status: "active",
    exclude_from_maestro,
  };
}

function mockWorkareas(list: ReturnType<typeof workarea>[]): void {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd !== "concerto_rpc") return Promise.resolve(undefined);
    switch (args.method) {
      case "Workareas.ListWorkareas":
        return Promise.resolve({ workareas: list });
      case "Maestro.SetWorkareaVisibility":
        return Promise.resolve(null);
      default:
        return Promise.resolve(undefined);
    }
  });
}

beforeEach(() => {
  invoke.mockReset();
  useUiStore.setState({ selectedWorkareaId: null });
});

describe("WorkareaList — Maestro visibility toggle", () => {
  it("renders a private badge for an exclude_from_maestro workarea", async () => {
    mockWorkareas([workarea("wa-1", "alpha", true)]);
    renderWithClient(<WorkareaList workspaceId="ws-1" />);
    expect(await screen.findByTestId("private-badge-wa-1")).toBeInTheDocument();
  });

  it("calls SetWorkareaVisibility(HARD_FACTS_ONLY) when marking a workarea private", async () => {
    mockWorkareas([workarea("wa-1", "alpha", false)]);
    renderWithClient(<WorkareaList workspaceId="ws-1" />);

    await screen.findByText("alpha");
    await userEvent.click(
      screen.getByTestId("visibility-trigger-wa-1"),
    );
    await userEvent.click(screen.getByRole("menuitem", { name: /private/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Maestro.SetWorkareaVisibility",
        payload: { workarea_id: "wa-1", visibility: 2 },
      }),
    );
  });

  it("calls SetWorkareaVisibility(FULL) when making a private workarea visible", async () => {
    mockWorkareas([workarea("wa-1", "alpha", true)]);
    renderWithClient(<WorkareaList workspaceId="ws-1" />);

    await screen.findByText("alpha");
    await userEvent.click(
      screen.getByTestId("visibility-trigger-wa-1"),
    );
    await userEvent.click(
      screen.getByRole("menuitem", { name: /visible to concerto chat/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Maestro.SetWorkareaVisibility",
        payload: { workarea_id: "wa-1", visibility: 1 },
      }),
    );
  });
});
