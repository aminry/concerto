// @vitest-environment jsdom
//
// Component tests for the Connect-to-Core picker (Task 219). Drives the picker
// against a stub CoreClient (mocked `@tauri-apps/api/core` `invoke`): it renders
// the paired-Core list with status dots + the "Start local" / "Pair remote"
// entry points, and wires Connect → `set_active_core`.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { ConnectCorePicker } from "./ConnectCorePicker";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";
import type { PairedCore } from "../api/cores";

const remoteCore: PairedCore = {
  core_id: "remote-1",
  display_name: "Home workstation",
  transport_kind: "iroh",
  iroh_endpoint_id: "ep-1",
  last_connected_at: 1717459200,
  is_active: false,
};
const localCore: PairedCore = {
  core_id: "local-machine",
  display_name: "This machine",
  transport_kind: "uds",
  iroh_endpoint_id: null,
  last_connected_at: null,
  is_active: true,
};

beforeEach(() => {
  invoke.mockReset();
  useUiStore.setState({ connectCoreOpen: true, pairingOpen: false });
});

describe("ConnectCorePicker", () => {
  it("renders the paired-Core list with the entry points", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_paired_cores")
        return Promise.resolve([remoteCore, localCore]);
      return Promise.resolve(undefined);
    });

    renderWithClient(<ConnectCorePicker />);

    expect(await screen.findByText("Home workstation")).toBeInTheDocument();
    expect(screen.getByText("This machine")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /start a local core/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /pair with a remote core/i }),
    ).toBeInTheDocument();
  });

  it("renders nothing when closed", () => {
    useUiStore.setState({ connectCoreOpen: false });
    invoke.mockResolvedValue([]);
    const { container } = renderWithClient(<ConnectCorePicker />);
    expect(container).toBeEmptyDOMElement();
  });

  it("Connect invokes set_active_core for the chosen Core", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_paired_cores") return Promise.resolve([remoteCore]);
      return Promise.resolve(undefined);
    });

    renderWithClient(<ConnectCorePicker />);
    const row = await screen.findByText("Home workstation");
    await userEvent.click(row);

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_active_core", {
        coreId: "remote-1",
      }),
    );
  });

  it("Pair-remote button opens the pairing modal + closes the picker", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_paired_cores") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });

    renderWithClient(<ConnectCorePicker />);
    await screen.findByText(/no paired cores yet/i);
    await userEvent.click(
      screen.getByRole("button", { name: /pair with a remote core/i }),
    );

    expect(useUiStore.getState().pairingOpen).toBe(true);
    expect(useUiStore.getState().connectCoreOpen).toBe(false);
  });

  it("Start-local invokes the frozen start_local_core command", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "list_paired_cores") return Promise.resolve([]);
      return Promise.resolve(undefined);
    });

    renderWithClient(<ConnectCorePicker />);
    await screen.findByText(/no paired cores yet/i);
    await userEvent.click(
      screen.getByRole("button", { name: /start a local core/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_local_core"),
    );
  });
});
