// Multi-Core registry storage-ordering tests (Task 511, Tier-2). Focused on the
// CRASH-SAFETY ordering of `removeCore`: the per-Core secret keys (the Ed25519
// device seed + signed cert) must be deleted BEFORE the trimmed index is written,
// so a process kill mid-op can never orphan secret material in the Keychain/
// Keystore with no index entry referencing it. The complementary `addCore`
// ordering (secrets BEFORE index) is asserted as the safe baseline.
//
// The native module is the in-memory mock; expo-secure-store is the jest mock
// (see jest.setup.ts) whose backing functions are `jest.fn()`, so we read
// `mock.invocationCallOrder` to assert the relative ordering of the underlying
// setItemAsync / deleteItemAsync calls.
import * as SecureStore from "expo-secure-store";

import { addCore, loadCore, removeCore } from "./core-store";
import type { ConnectBlob } from "../native/ConcertoIroh";

const NOISE = "b".repeat(64);
const INDEX_KEY = "concerto.cores";
const seedKey = (id: string) => `concerto.core.${id}.seed`;
const certKey = (id: string) => `concerto.core.${id}.cert`;

const blobA: ConnectBlob = {
  endpointId: "core-a",
  directAddrs: ["1.2.3.4:1"],
  coreNoisePub: NOISE,
};

type MockFn = jest.Mock & { mock: { invocationCallOrder: number[]; calls: unknown[][] } };
const setItem = SecureStore.setItemAsync as unknown as MockFn;
const deleteItem = SecureStore.deleteItemAsync as unknown as MockFn;

/** The invocation order of the first call whose first arg === `key`, or -1. */
function orderOfCall(fn: MockFn, key: string): number {
  const i = fn.mock.calls.findIndex((args) => args[0] === key);
  return i === -1 ? -1 : fn.mock.invocationCallOrder[i];
}

beforeEach(() => {
  (SecureStore as unknown as { __resetSecureStore: () => void }).__resetSecureStore();
  setItem.mockClear();
  deleteItem.mockClear();
});

async function addCoreA(): Promise<void> {
  await addCore({
    id: "core-a",
    label: "Core A",
    blob: blobA,
    deviceIdHex: "aa",
    deviceSeed: new Uint8Array(32).fill(1),
    signedCert: new Uint8Array([1]),
  });
}

describe("addCore storage ordering", () => {
  it("writes the secret keys BEFORE the index (safe-on-crash baseline)", async () => {
    await addCoreA();
    const seedAt = orderOfCall(setItem, seedKey("core-a"));
    const certAt = orderOfCall(setItem, certKey("core-a"));
    const indexAt = orderOfCall(setItem, INDEX_KEY);
    expect(seedAt).toBeGreaterThan(0);
    expect(certAt).toBeGreaterThan(0);
    expect(indexAt).toBeGreaterThan(0);
    // Both secrets persisted before the index references them.
    expect(seedAt).toBeLessThan(indexAt);
    expect(certAt).toBeLessThan(indexAt);
  });
});

describe("removeCore storage ordering", () => {
  it("deletes the secret keys BEFORE writing the trimmed index", async () => {
    await addCoreA();
    setItem.mockClear();
    deleteItem.mockClear();

    await removeCore("core-a");

    // Both secret keys must have been deleted before the index was trimmed, so a
    // crash mid-remove can never leave the seed/cert orphaned with no index entry.
    const seedDeleteAt = orderOfCall(deleteItem, seedKey("core-a"));
    const certDeleteAt = orderOfCall(deleteItem, certKey("core-a"));
    const indexWriteAt = orderOfCall(setItem, INDEX_KEY);
    expect(seedDeleteAt).toBeGreaterThan(0);
    expect(certDeleteAt).toBeGreaterThan(0);
    expect(indexWriteAt).toBeGreaterThan(0);
    expect(seedDeleteAt).toBeLessThan(indexWriteAt);
    expect(certDeleteAt).toBeLessThan(indexWriteAt);
  });

  it("leaves no secret material behind after removal", async () => {
    await addCoreA();
    await removeCore("core-a");
    expect(await SecureStore.getItemAsync(seedKey("core-a"))).toBeNull();
    expect(await SecureStore.getItemAsync(certKey("core-a"))).toBeNull();
    expect(await loadCore("core-a")).toBeNull();
  });
});
