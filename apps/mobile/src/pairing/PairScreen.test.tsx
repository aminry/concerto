// PairScreen tests (Task 511, Tier-2). Drives the QR scan + manual-entry flows
// against the in-memory ConcertoIroh mock + the mocked secure-store, proving the
// screen runs the real `pairWithQr` (parse → generateDeviceKeypair → pair →
// persist) and renders the loading / done / error states. The REAL camera is
// mocked (jest.setup.ts); we trigger scans by invoking the mocked CameraView's
// `onBarcodeScanned` prop. RN-TL v13.3.3.
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import * as SecureStore from "expo-secure-store";

import { PairScreen } from "./PairScreen";
import { activeCoreId, loadCore } from "./core-store";
import { createMockConcertoIroh } from "../native/mock-concerto-iroh";

const TOKEN = "a".repeat(64);
const NOISE = "b".repeat(64);

function makeQr(over: Partial<Record<string, unknown>> = {}): string {
  return globalThis.btoa(
    JSON.stringify({
      endpoint_id: "ep-test",
      relay_url: "https://relay.example",
      direct_addrs: ["127.0.0.1:4433"],
      pairing_token: TOKEN,
      core_noise_pub: NOISE,
      ...over,
    }),
  );
}

/** Pull the `onBarcodeScanned` prop off the last-rendered mocked CameraView. */
async function fireScan(data: string) {
  const calls = (CameraView as unknown as jest.Mock).mock.calls;
  const props = calls[calls.length - 1][0] as {
    onBarcodeScanned?: (r: { data: string; type: string }) => void;
  };
  // The handler kicks off async state updates; wrap so React's act() is satisfied.
  await act(async () => {
    props.onBarcodeScanned?.({ data, type: "qr" });
  });
}

beforeEach(() => {
  (SecureStore as unknown as { __resetSecureStore: () => void }).__resetSecureStore();
  (CameraView as unknown as jest.Mock).mockClear();
  (useCameraPermissions as unknown as jest.Mock).mockReturnValue([
    { granted: true, canAskAgain: true, status: "granted" },
    jest.fn(),
  ]);
});

describe("PairScreen", () => {
  it("renders the scanner when camera permission is granted", () => {
    render(<PairScreen onPair={async () => ({ core: anyCore() })} />);
    expect(screen.getByTestId("pair-scanning")).toBeOnTheScreen();
    expect(screen.getByTestId("pair-camera")).toBeOnTheScreen();
  });

  it("scans a QR, runs the real pair flow, and persists the Core", async () => {
    const module = createMockConcertoIroh({ signedCert: new Uint8Array([7, 8, 9]) });
    const onPaired = jest.fn();
    // Use the screen's default-style flow but inject the module via onPair.
    render(
      <PairScreen
        onPair={(qr) => realPair(module, qr)}
        onPaired={onPaired}
      />,
    );

    await fireScan(makeQr());

    expect(await screen.findByTestId("pair-done")).toBeOnTheScreen();
    await waitFor(() => expect(onPaired).toHaveBeenCalledTimes(1));

    // Persisted + active.
    expect(await activeCoreId()).toBe("ep-test");
    const loaded = await loadCore("ep-test");
    expect(Array.from(loaded!.signedCert)).toEqual([7, 8, 9]);
    // The native module was actually driven.
    expect(module.generateCount()).toBe(1);
    expect(module.pairCalls).toHaveLength(1);
  });

  it("shows an error for a malformed QR without invoking pair", async () => {
    const onPair = jest.fn();
    render(<PairScreen onPair={onPair} />);
    await fireScan("not-a-blob");
    expect(await screen.findByTestId("pair-error")).toBeOnTheScreen();
    expect(onPair).not.toHaveBeenCalled();
  });

  it("surfaces a handshake failure and offers retry", async () => {
    const onPair = jest.fn().mockRejectedValue(new Error("noise handshake rejected"));
    render(<PairScreen onPair={onPair} />);
    await fireScan(makeQr());
    expect(await screen.findByTestId("pair-error")).toBeOnTheScreen();
    expect(screen.getByText("noise handshake rejected")).toBeOnTheScreen();
    fireEvent.press(screen.getByTestId("pair-retry"));
    expect(await screen.findByTestId("pair-scanning")).toBeOnTheScreen();
  });

  it("pairs via the manual-code fallback", async () => {
    const module = createMockConcertoIroh();
    render(<PairScreen onPair={(qr) => realPair(module, qr)} />);
    fireEvent.press(screen.getByTestId("pair-manual-toggle"));
    fireEvent.changeText(screen.getByTestId("pair-manual-input"), makeQr());
    fireEvent.press(screen.getByTestId("pair-manual-submit"));
    expect(await screen.findByTestId("pair-done")).toBeOnTheScreen();
    expect(module.pairCalls).toHaveLength(1);
  });

  it("prompts for camera access when permission is denied", () => {
    const requestPermission = jest.fn();
    (useCameraPermissions as unknown as jest.Mock).mockReturnValue([
      { granted: false, canAskAgain: true, status: "denied" },
      requestPermission,
    ]);
    render(<PairScreen onPair={jest.fn()} />);
    expect(screen.getByTestId("pair-permission-denied")).toBeOnTheScreen();
    fireEvent.press(screen.getByTestId("pair-grant"));
    expect(requestPermission).toHaveBeenCalled();
  });
});

// Helpers --------------------------------------------------------------------

import { pairWithQr } from "./pair";
import type { ConcertoIrohModule } from "../native/ConcertoIroh";
import type { PairResult } from "./pair";
import type { StoredCore } from "./core-store";

function realPair(module: ConcertoIrohModule, qr: string): Promise<PairResult> {
  return pairWithQr(module, qr);
}

function anyCore(): StoredCore {
  return {
    id: "x",
    label: "X",
    blob: { endpointId: "x", directAddrs: [], coreNoisePub: NOISE },
    deviceIdHex: "00",
    pairedAtMs: 0,
  };
}
