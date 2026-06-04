// @vitest-environment jsdom
//
// Component tests for the split-host pairing modal (Task 219). Drives the
// scan/paste flow against a stub CoreClient (mocked `invoke`) + a stub QR
// reader. Proves: paste-token decodes a fixture payload and invokes
// `complete_pairing_from_payload`; the 60s-TTL expiry surfaces an error; the
// name-the-pairing step defaults to the Core hostname and persists via
// rename + set-active.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Stub the webcam QR reader so the scan path never touches `getUserMedia`.
const decodeFromVideoDevice = vi.fn();
vi.mock("@zxing/browser", () => ({
  BrowserQRCodeReader: class {
    decodeFromVideoDevice = decodeFromVideoDevice;
  },
}));

import { PairCoreModal } from "./PairCoreModal";
import { renderWithClient } from "./test-utils";
import { useUiStore } from "../state/useUiStore";
import { encodePairingPayload, type PairingPayload } from "../api/cores";

const fixturePayload: PairingPayload = {
  core_pubkey: "Y29yZS1wdWJrZXk=",
  pairing_token: "dG9rZW4tMzItYnl0ZXM=",
  lan_endpoint: "192.168.1.42:7777",
  iroh_endpoint_id: null,
  relay_hint: null,
};
const fixtureToken = encodePairingPayload(fixturePayload);

beforeEach(() => {
  invoke.mockReset();
  decodeFromVideoDevice.mockReset();
  decodeFromVideoDevice.mockResolvedValue({ stop: () => {} });
  useUiStore.setState({ pairingOpen: true });
});

async function openPasteStep(): Promise<void> {
  renderWithClient(<PairCoreModal />);
  await userEvent.click(screen.getByRole("button", { name: /paste token/i }));
}

describe("PairCoreModal", () => {
  it("paste-token decodes a fixture payload and invokes complete-pairing", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "complete_pairing_from_payload")
        return Promise.resolve({
          core_id: "newcore",
          suggested_name: "workstation.local",
        });
      return Promise.resolve(undefined);
    });

    await openPasteStep();
    await userEvent.type(
      screen.getByLabelText(/pairing token/i),
      fixtureToken,
    );
    await userEvent.click(screen.getByRole("button", { name: /^pair$/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("complete_pairing_from_payload", {
        token: fixtureToken,
      }),
    );
  });

  it("surfaces an invalid-token error before calling the shell", async () => {
    await openPasteStep();
    await userEvent.type(
      screen.getByLabelText(/pairing token/i),
      "garbage-not-base64!!!",
    );
    await userEvent.click(screen.getByRole("button", { name: /^pair$/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/base64/i);
    expect(invoke).not.toHaveBeenCalledWith(
      "complete_pairing_from_payload",
      expect.anything(),
    );
  });

  it("surfaces the 60s-TTL expiry error from the shell", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "complete_pairing_from_payload")
        return Promise.reject({ kind: "Rpc", message: "pairing token expired" });
      return Promise.resolve(undefined);
    });

    await openPasteStep();
    await userEvent.type(
      screen.getByLabelText(/pairing token/i),
      fixtureToken,
    );
    await userEvent.click(screen.getByRole("button", { name: /^pair$/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/expired/i);
  });

  it("name step defaults to the Core hostname and persists rename + set-active", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "complete_pairing_from_payload")
        return Promise.resolve({
          core_id: "newcore",
          suggested_name: "workstation.local",
        });
      return Promise.resolve(undefined);
    });

    await openPasteStep();
    await userEvent.type(
      screen.getByLabelText(/pairing token/i),
      fixtureToken,
    );
    await userEvent.click(screen.getByRole("button", { name: /^pair$/i }));

    // Naming step: the input pre-fills with the suggested hostname.
    const nameInput = (await screen.findByLabelText(
      /^name$/i,
    )) as HTMLInputElement;
    expect(nameInput.value).toBe("workstation.local");

    await userEvent.click(screen.getByRole("button", { name: /connect/i }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("rename_paired_core", {
        coreId: "newcore",
        displayName: "workstation.local",
      }),
    );
    expect(invoke).toHaveBeenCalledWith("set_active_core", {
      coreId: "newcore",
    });
    // Modal closes on success.
    expect(useUiStore.getState().pairingOpen).toBe(false);
  });

  it("scan step falls back to paste when the camera is unavailable", async () => {
    decodeFromVideoDevice.mockRejectedValue(new Error("Permission denied"));

    renderWithClient(<PairCoreModal />);
    await userEvent.click(
      screen.getByRole("button", { name: /scan qr with camera/i }),
    );

    expect(await screen.findByText(/camera unavailable/i)).toBeInTheDocument();
    await userEvent.click(
      screen.getByRole("button", { name: /paste a token instead/i }),
    );
    expect(screen.getByLabelText(/pairing token/i)).toBeInTheDocument();
  });
});
