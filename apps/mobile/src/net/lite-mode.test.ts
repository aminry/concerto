// Lite-mode cellular-detection tests (Task 518, Tier-2; design/16 §3.12). Cellular
// -> lite mode ON; wifi/ethernet -> OFF. The flag tracks live connectivity changes
// and dedups (only emits on a flip). The `expo-network` seam is a fake.
import {
  type NetworkApi,
  type NetworkSnapshot,
  isLiteMode,
  watchLiteMode,
} from "./lite-mode";

/** A drivable fake of the expo-network seam. */
function fakeNetwork(initial: NetworkSnapshot) {
  let listener: ((s: NetworkSnapshot) => void) | undefined;
  const api: NetworkApi = {
    getNetworkStateAsync: jest.fn(async () => initial),
    addNetworkStateListener: jest.fn((l) => {
      listener = l;
      return { remove: jest.fn(() => (listener = undefined)) };
    }),
  };
  return { api, change: (s: NetworkSnapshot) => listener?.(s) };
}

const wifi: NetworkSnapshot = { type: "wifi", isConnected: true };
const cell: NetworkSnapshot = { type: "cellular", isConnected: true };
const eth: NetworkSnapshot = { type: "ethernet", isConnected: true };
const none: NetworkSnapshot = { type: "none", isConnected: false };

const flush = () => new Promise<void>((r) => setTimeout(r, 0));

describe("isLiteMode", () => {
  it("is ON only for cellular", () => {
    expect(isLiteMode(cell)).toBe(true);
    expect(isLiteMode(wifi)).toBe(false);
    expect(isLiteMode(eth)).toBe(false);
    expect(isLiteMode(none)).toBe(false);
  });
});

describe("watchLiteMode", () => {
  it("emits the initial value: cellular -> lite ON", async () => {
    const net = fakeNetwork(cell);
    const flips: boolean[] = [];
    watchLiteMode({ api: net.api, onChange: (lite) => flips.push(lite) });
    await flush();
    expect(flips).toEqual([true]);
  });

  it("emits the initial value: wifi -> lite OFF", async () => {
    const net = fakeNetwork(wifi);
    const flips: boolean[] = [];
    watchLiteMode({ api: net.api, onChange: (lite) => flips.push(lite) });
    await flush();
    expect(flips).toEqual([false]);
  });

  it("flips ON when moving wifi -> cellular and OFF on the return", async () => {
    const net = fakeNetwork(wifi);
    const flips: boolean[] = [];
    watchLiteMode({ api: net.api, onChange: (lite) => flips.push(lite) });
    await flush();
    net.change(cell); // -> ON
    net.change(cell); // no-op (dedup)
    net.change(wifi); // -> OFF
    expect(flips).toEqual([false, true, false]);
  });

  it("a listener event before the initial probe resolves wins over the stale probe", async () => {
    // Initial probe says wifi (lite OFF), but a real change to cellular arrives
    // through the listener BEFORE that probe resolves. The live value must win:
    // the final state is lite ON, not flipped back to the stale wifi snapshot.
    const net = fakeNetwork(wifi);
    const flips: boolean[] = [];
    const snaps: NetworkSnapshot[] = [];
    watchLiteMode({
      api: net.api,
      onChange: (lite, snap) => {
        flips.push(lite);
        snaps.push(snap);
      },
    });
    // Listener is registered synchronously; fire a change before the probe's
    // microtask resolves.
    net.change(cell); // -> ON (live)
    await flush(); // now the stale wifi probe resolves — must NOT re-apply
    expect(flips).toEqual([true]);
    expect(snaps).toEqual([cell]);
  });

  it("stop() removes the listener and silences further changes", async () => {
    const net = fakeNetwork(wifi);
    const flips: boolean[] = [];
    const w = watchLiteMode({ api: net.api, onChange: (lite) => flips.push(lite) });
    await flush();
    w.stop();
    net.change(cell);
    expect(flips).toEqual([false]);
  });
});
