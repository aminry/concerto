// @vitest-environment jsdom
//
// Component tests for the Concerto-chat composer (Task 415). Proves: the
// `@`-token routing preview + the workarea autocomplete (sourced from
// `Workareas.ListWorkareas`); the `/`-directive hint; and Cmd+Enter submits via
// `Maestro.SendToMaestro` (the mocked `invoke` double — live arm is 414). The
// composer AFFORDS routing only; it never parses/resolves (408 does).

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import {
  MaestroComposer,
  parseAtTokens,
  trailingAtFragment,
} from "./MaestroComposer";
import { renderWithClient } from "../test-utils";
import { useMaestroStore } from "../../state/useMaestroStore";

const workareas = [
  {
    id: "wa-bach",
    workspace_id: "ws-1",
    composer_name: "bach",
    branch_name: "feat/bach",
    worktree_root: "/x",
    status: "active",
  },
  {
    id: "wa-mozart",
    workspace_id: "ws-1",
    composer_name: "mozart",
    branch_name: "feat/mozart",
    worktree_root: "/y",
    status: "running",
  },
];

function mockListWorkareas() {
  invoke.mockImplementation((cmd: string, args: { method?: string }) => {
    if (cmd === "concerto_rpc" && args?.method === "Workareas.ListWorkareas") {
      return Promise.resolve({ workareas });
    }
    if (cmd === "concerto_rpc" && args?.method === "Maestro.SendToMaestro") {
      return Promise.resolve(null);
    }
    return Promise.resolve(null);
  });
}

beforeEach(() => {
  invoke.mockReset();
  useMaestroStore.setState({ composerDraft: "" });
  mockListWorkareas();
});

describe("composer parse helpers", () => {
  it("parseAtTokens extracts workarea segments (ignoring /session)", () => {
    expect(parseAtTokens("hi @bach and @mozart/claude")).toEqual([
      "bach",
      "mozart",
    ]);
  });

  it("trailingAtFragment returns the partial token under the caret", () => {
    expect(trailingAtFragment("route @ba")).toBe("ba");
    expect(trailingAtFragment("route @bach done")).toBeNull();
  });
});

describe("MaestroComposer", () => {
  it("renders the routing preview for an @-token matching a workarea", async () => {
    useMaestroStore.setState({ composerDraft: "@bach rebase" });
    renderWithClient(<MaestroComposer workspaceId="ws-1" />);
    await waitFor(() => {
      expect(screen.getByTestId("route-preview").textContent).toMatch(/@bach/);
    });
  });

  it("shows the workarea autocomplete while typing a trailing @fragment", async () => {
    useMaestroStore.setState({ composerDraft: "@mo" });
    renderWithClient(<MaestroComposer workspaceId="ws-1" />);
    await waitFor(() => {
      const list = screen.getByTestId("workarea-autocomplete");
      expect(list.textContent).toMatch(/@mozart/);
    });
  });

  it("shows the /-directive hint", async () => {
    useMaestroStore.setState({ composerDraft: "/dig" });
    renderWithClient(<MaestroComposer workspaceId="ws-1" />);
    await waitFor(() => {
      expect(screen.getByTestId("slash-hint").textContent).toMatch("/digest");
    });
  });

  it("Cmd+Enter submits via Maestro.SendToMaestro with the active workspace scope", async () => {
    useMaestroStore.setState({ composerDraft: "hello maestro" });
    renderWithClient(<MaestroComposer workspaceId="ws-1" />);
    const textarea = screen.getByLabelText("Message the Concerto chat");
    textarea.focus();
    await userEvent.keyboard("{Meta>}{Enter}{/Meta}");
    await waitFor(() => {
      // Task 8: the composer threads its active workspace as `workspace_id`.
      expect(invoke).toHaveBeenCalledWith("concerto_rpc", {
        method: "Maestro.SendToMaestro",
        payload: {
          text: "hello maestro",
          attachments: [],
          workspace_id: "ws-1",
        },
      });
    });
  });
});
