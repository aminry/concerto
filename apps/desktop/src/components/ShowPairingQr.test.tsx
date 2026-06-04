// @vitest-environment jsdom
//
// Component tests for the UDS-gated "Reveal pairing QR" affordance (Task 219,
// `design/15 §3.11`). The QR-show entry point is hidden (replaced by a hint)
// when the active Core's transport is not UDS; for a UDS Core it reveals the
// local payload from `start_pairing_show` and renders a QR.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

// Stub `qrcode` so no canvas is needed in jsdom.
vi.mock("qrcode", () => ({
  default: { toDataURL: vi.fn().mockResolvedValue("data:image/png;base64,ZZ") },
}));

import { ShowPairingQr } from "./ShowPairingQr";
import { TransportKind } from "../api/runtime";
import type { PairingPayload } from "../api/cores";

const payload: PairingPayload = {
  core_pubkey: "cHVi",
  pairing_token: "dG9r",
  lan_endpoint: "192.168.1.42:7777",
  iroh_endpoint_id: null,
  relay_hint: null,
};

beforeEach(() => {
  invoke.mockReset();
});

describe("ShowPairingQr", () => {
  it("renders the split-host hint and NO reveal button when not UDS", () => {
    render(<ShowPairingQr transportKind={TransportKind.Iroh} />);
    expect(screen.getByText(/concerto pair/i)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /reveal pairing qr/i }),
    ).not.toBeInTheDocument();
  });

  it("hides the QR-show for the WSS bridge transport too", () => {
    render(<ShowPairingQr transportKind={TransportKind.WssBridge} />);
    expect(
      screen.queryByRole("button", { name: /reveal pairing qr/i }),
    ).not.toBeInTheDocument();
  });

  it("reveals the local QR for a UDS Core", async () => {
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "start_pairing_show") return Promise.resolve(payload);
      return Promise.resolve(undefined);
    });

    render(<ShowPairingQr transportKind={TransportKind.Uds} />);
    await userEvent.click(
      screen.getByRole("button", { name: /reveal pairing qr/i }),
    );

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("start_pairing_show"),
    );
    expect(await screen.findByAltText(/pairing qr code/i)).toBeInTheDocument();
  });
});
