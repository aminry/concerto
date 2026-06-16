// Pairing flow tests (Task 511, Tier-2). Proves:
//   - parseConnectBlob decodes the base64(JSON) QR shape (and rejects bad input),
//   - pairWithQr: parse → generateDeviceKeypair → pair (mock module) → secure-store
//     persists the seed + cert + index,
//   - the multi-Core registry: add / list / switch / remove.
//
// The native module is the in-memory mock; expo-secure-store is the jest mock
// (see jest.setup.ts). The REAL camera / Keychain / native module are Tier-3.
import * as SecureStore from "expo-secure-store";

import { ConnectBlobParseError, parseConnectBlob } from "./connect-blob";
import { pairWithQr } from "./pair";
import {
  activeCore,
  activeCoreId,
  addCore,
  listCores,
  loadCore,
  removeCore,
  switchCore,
} from "./core-store";
import { createMockConcertoIroh } from "../native/mock-concerto-iroh";
import type { ConnectBlob } from "../native/ConcertoIroh";

const TOKEN = "a".repeat(64); // 32-byte hex pairing token
const NOISE = "b".repeat(64); // 32-byte hex core noise pub

/** Build a connect-blob QR string (base64(JSON) snake_case, the pair-serve shape). */
function makeQr(over: Partial<Record<string, unknown>> = {}): string {
  const obj = {
    endpoint_id: "ep-abcdefgh",
    relay_url: "https://relay.example",
    direct_addrs: ["127.0.0.1:4433"],
    pairing_token: TOKEN,
    core_noise_pub: NOISE,
    ...over,
  };
  return globalThis.btoa(JSON.stringify(obj));
}

// Reset the in-memory secure-store between tests.
beforeEach(() => {
  (SecureStore as unknown as { __resetSecureStore: () => void }).__resetSecureStore();
});

describe("parseConnectBlob", () => {
  it("decodes a valid base64(JSON) connect blob", () => {
    const { blob, pairingToken } = parseConnectBlob(makeQr());
    expect(blob.endpointId).toBe("ep-abcdefgh");
    expect(blob.relayUrl).toBe("https://relay.example");
    expect(blob.directAddrs).toEqual(["127.0.0.1:4433"]);
    expect(blob.coreNoisePub).toBe(NOISE);
    expect(pairingToken).toBe(TOKEN);
  });

  it("accepts a PAIR-BLOB: prefixed payload (CLI form)", () => {
    const { blob } = parseConnectBlob(`PAIR-BLOB: ${makeQr()}`);
    expect(blob.endpointId).toBe("ep-abcdefgh");
  });

  it("omits relayUrl for a loopback (relay-less) blob", () => {
    const { blob } = parseConnectBlob(makeQr({ relay_url: undefined }));
    expect(blob.relayUrl).toBeUndefined();
    expect(blob.directAddrs).toEqual(["127.0.0.1:4433"]);
  });

  it("rejects a non-base64 / non-JSON payload", () => {
    expect(() => parseConnectBlob("not a blob!!!")).toThrow(ConnectBlobParseError);
  });

  it("rejects a blob missing the pairing token", () => {
    expect(() => parseConnectBlob(makeQr({ pairing_token: undefined }))).toThrow(
      ConnectBlobParseError,
    );
  });

  it("rejects a malformed (non-hex) pairing token", () => {
    expect(() => parseConnectBlob(makeQr({ pairing_token: "zz" }))).toThrow(
      /32-byte hex/,
    );
  });

  it("rejects a blob with no reachable address", () => {
    expect(() =>
      parseConnectBlob(makeQr({ relay_url: undefined, direct_addrs: [] })),
    ).toThrow(/reachable/);
  });
});

describe("pairWithQr", () => {
  it("parses, generates a keypair, pairs, and persists the Core", async () => {
    const module = createMockConcertoIroh({
      signedCert: new Uint8Array([0xaa, 0xbb, 0xcc]),
    });

    const { core } = await pairWithQr(module, makeQr(), {
      coreLabel: "My MacBook",
      deviceName: "Amin's iPhone",
    });

    // The native module was driven correctly.
    expect(module.generateCount()).toBe(1);
    expect(module.pairCalls).toHaveLength(1);
    expect(module.pairCalls[0].inputs.pairingToken).toBe(TOKEN);
    expect(module.pairCalls[0].inputs.deviceName).toBe("Amin's iPhone");
    expect(module.pairCalls[0].inputs.blob.endpointId).toBe("ep-abcdefgh");

    // The Core is persisted + active.
    expect(core.id).toBe("ep-abcdefgh");
    expect(core.label).toBe("My MacBook");
    expect(await activeCoreId()).toBe("ep-abcdefgh");

    // Secrets are hydrated from secure-store.
    const loaded = await loadCore("ep-abcdefgh");
    expect(loaded).not.toBeNull();
    expect(Array.from(loaded!.signedCert)).toEqual([0xaa, 0xbb, 0xcc]);
    expect(loaded!.deviceSeed.length).toBe(32);

    // The seed is in secure-store (and NOT in the plaintext index).
    expect(await SecureStore.getItemAsync("concerto.core.ep-abcdefgh.seed")).not.toBeNull();
    const index = await SecureStore.getItemAsync("concerto.cores");
    expect(index).not.toContain("seed");
  });

  it("uses a default label + device name when omitted", async () => {
    const module = createMockConcertoIroh();
    const { core } = await pairWithQr(module, makeQr());
    expect(core.label).toMatch(/^Core /);
    expect(module.pairCalls[0].inputs.deviceName).toBe("Concerto Mobile");
  });

  it("propagates a connect-blob parse error before touching the module", async () => {
    const module = createMockConcertoIroh();
    await expect(pairWithQr(module, "garbage")).rejects.toBeInstanceOf(ConnectBlobParseError);
    expect(module.generateCount()).toBe(0);
    expect(module.pairCalls).toHaveLength(0);
  });
});

describe("multi-Core registry", () => {
  const blobA: ConnectBlob = { endpointId: "core-a", directAddrs: ["1.2.3.4:1"], coreNoisePub: NOISE };
  const blobB: ConnectBlob = { endpointId: "core-b", directAddrs: ["5.6.7.8:1"], coreNoisePub: NOISE };

  async function seedTwo() {
    await addCore({
      id: "core-a",
      label: "Core A",
      blob: blobA,
      deviceIdHex: "aa",
      deviceSeed: new Uint8Array(32).fill(1),
      signedCert: new Uint8Array([1]),
    });
    await addCore({
      id: "core-b",
      label: "Core B",
      blob: blobB,
      deviceIdHex: "bb",
      deviceSeed: new Uint8Array(32).fill(2),
      signedCert: new Uint8Array([2]),
    });
  }

  it("adds and lists multiple Cores (newest active)", async () => {
    await seedTwo();
    const cores = await listCores();
    expect(cores.map((c) => c.id)).toEqual(["core-a", "core-b"]);
    expect(await activeCoreId()).toBe("core-b"); // last added is active
  });

  it("switches the active Core", async () => {
    await seedTwo();
    await switchCore("core-a");
    expect(await activeCoreId()).toBe("core-a");
    const active = await activeCore();
    expect(active?.id).toBe("core-a");
    expect(Array.from(active!.deviceSeed)).toEqual(Array(32).fill(1));
  });

  it("rejects switching to an unknown Core", async () => {
    await seedTwo();
    await expect(switchCore("core-x")).rejects.toThrow(/unknown Core/);
  });

  it("replaces a Core on re-pair (same id)", async () => {
    await seedTwo();
    await addCore({
      id: "core-a",
      label: "Core A (renamed)",
      blob: blobA,
      deviceIdHex: "aa2",
      deviceSeed: new Uint8Array(32).fill(9),
      signedCert: new Uint8Array([9]),
    });
    const cores = await listCores();
    expect(cores).toHaveLength(2);
    expect(cores.find((c) => c.id === "core-a")?.label).toBe("Core A (renamed)");
    expect(await activeCoreId()).toBe("core-a");
  });

  it("removes a Core and re-points the active id", async () => {
    await seedTwo();
    expect(await activeCoreId()).toBe("core-b");
    await removeCore("core-b");
    const cores = await listCores();
    expect(cores.map((c) => c.id)).toEqual(["core-a"]);
    expect(await activeCoreId()).toBe("core-a");
    // Its secrets are gone.
    expect(await loadCore("core-b")).toBeNull();
  });
});
