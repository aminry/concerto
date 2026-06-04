// @vitest-environment jsdom
//
// Component tests for Settings → Connected Cores (Task 219). Drives the
// switch/rename/remove rows against a stub CoreClient (mocked `invoke`) and
// asserts each invokes the right frozen command. `location.reload` is stubbed
// so the switch-active cache-clear is assertable.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { ConnectedCoresList } from "./ConnectedCoresList";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";
import type { PairedCore } from "../api/cores";

const active: PairedCore = {
  core_id: "active-1",
  display_name: "This machine",
  transport_kind: "uds",
  iroh_endpoint_id: null,
  last_connected_at: 1717459200,
  is_active: true,
};
const other: PairedCore = {
  core_id: "other-2",
  display_name: "Home workstation",
  transport_kind: "iroh",
  iroh_endpoint_id: "ep-2",
  last_connected_at: 1717459200,
  is_active: false,
};

beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation((cmd: string) => {
    if (cmd === "list_paired_cores") return Promise.resolve([active, other]);
    return Promise.resolve(undefined);
  });
});

describe("ConnectedCoresList", () => {
  it("lists paired Cores with the active marker", async () => {
    renderWithClient(<ConnectedCoresList />);
    expect(await screen.findByText("This machine")).toBeInTheDocument();
    expect(screen.getByText("Home workstation")).toBeInTheDocument();
    expect(screen.getByText("active")).toBeInTheDocument();
  });

  it("Switch active invokes set_active_core + reloads to clear cache", async () => {
    const reload = vi.fn();
    Object.defineProperty(window, "location", {
      value: { reload },
      writable: true,
    });

    renderWithClient(<ConnectedCoresList />);
    await screen.findByText("Home workstation");
    // Only the non-active row exposes "Switch active".
    await userEvent.click(
      screen.getByRole("button", { name: /switch active/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_active_core", {
        coreId: "other-2",
      }),
    );
    await waitFor(() => expect(reload).toHaveBeenCalled());
  });

  it("Rename invokes rename_paired_core with the edited name", async () => {
    renderWithClient(<ConnectedCoresList />);
    await screen.findByText("Home workstation");
    // Click the Rename button on the second row.
    const renameButtons = screen.getAllByRole("button", { name: /^rename$/i });
    await userEvent.click(renameButtons[1]);

    const input = screen.getByLabelText(/rename home workstation/i);
    await userEvent.clear(input);
    await userEvent.type(input, "Cloud VM");
    await userEvent.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("rename_paired_core", {
        coreId: "other-2",
        displayName: "Cloud VM",
      }),
    );
  });

  it("Remove invokes remove_paired_core (best-effort revoke)", async () => {
    renderWithClient(<ConnectedCoresList />);
    await screen.findByText("Home workstation");
    const removeButtons = screen.getAllByRole("button", { name: /^remove$/i });
    await userEvent.click(removeButtons[1]);

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("remove_paired_core", {
        coreId: "other-2",
      }),
    );
  });

  it("Add another opens the pairing modal", async () => {
    useUiStore.setState({ pairingOpen: false });
    renderWithClient(<ConnectedCoresList />);
    await screen.findByText("This machine");
    await userEvent.click(screen.getByRole("button", { name: /add another/i }));
    expect(useUiStore.getState().pairingOpen).toBe(true);
  });
});
