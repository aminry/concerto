// The multi-Core paired-Core registry (Task 511; design/16 §3.6 "multi-Core",
// the sibling of split-host Desktop's registry in design/15).
//
// A phone can be paired with more than one Core (personal Mac + work Mac, …).
// Each paired Core stores: the SESSION connect blob (endpoint id + relay +
// direct addrs + noise pub), the on-wire signed device cert, and the per-Core
// device seed (the Ed25519 PRIVATE key — a fresh identity per Core). All of it
// lives in `expo-secure-store` (Keychain/Keystore). One Core is "active" at a
// time — the one `appDataClient()` opens a session to.
//
// Storage layout (secure-store keys):
//   concerto.cores            -> JSON { activeId, cores: StoredCore[] }  (no secrets)
//   concerto.core.<id>.seed   -> base64(device seed)        (SECRET)
//   concerto.core.<id>.cert   -> base64(signed device cert)
//
// Secrets are kept in SEPARATE keys (not in the index blob) so listing the Cores
// never pulls private key material into memory unnecessarily.
import type { ConnectBlob } from "../native/ConcertoIroh";
import { getBytes, getJson, remove, setBytes, setJson } from "./secure-store";

const INDEX_KEY = "concerto.cores";
const seedKey = (id: string) => `concerto.core.${id}.seed`;
const certKey = (id: string) => `concerto.core.${id}.cert`;

/** A paired Core as stored in the index (NO secret material). */
export interface StoredCore {
  /** Stable id (the Core's endpoint id — unique per Core). */
  id: string;
  /** Human label shown in the picker (the Core's device name, editable). */
  label: string;
  /** The session connect blob (endpoint id + relay + direct addrs + noise pub). */
  blob: ConnectBlob;
  /** This device's id (BLAKE2b-256(pubkey), hex) recorded at pair time. */
  deviceIdHex: string;
  /** When this Core was paired (epoch ms). */
  pairedAtMs: number;
}

/** A paired Core hydrated with its secret material (for opening a session). */
export interface PairedCore extends StoredCore {
  /** The 32-byte device seed (SECRET). */
  deviceSeed: Uint8Array;
  /** The on-wire signed device cert. */
  signedCert: Uint8Array;
}

interface CoreIndex {
  activeId: string | null;
  cores: StoredCore[];
}

async function readIndex(): Promise<CoreIndex> {
  return (await getJson<CoreIndex>(INDEX_KEY)) ?? { activeId: null, cores: [] };
}

async function writeIndex(idx: CoreIndex): Promise<void> {
  await setJson(INDEX_KEY, idx);
}

/** List the paired Cores (index only, no secrets), in pair order. */
export async function listCores(): Promise<StoredCore[]> {
  return (await readIndex()).cores;
}

/** The active Core's id, or `null` if none is paired/selected. */
export async function activeCoreId(): Promise<string | null> {
  return (await readIndex()).activeId;
}

/** The active Core hydrated with its secrets, or `null` if none. */
export async function activeCore(): Promise<PairedCore | null> {
  const idx = await readIndex();
  if (!idx.activeId) return null;
  return loadCore(idx.activeId);
}

/** Load one Core (index + secrets), or `null` if unknown / missing material. */
export async function loadCore(id: string): Promise<PairedCore | null> {
  const idx = await readIndex();
  const stored = idx.cores.find((c) => c.id === id);
  if (!stored) return null;
  const deviceSeed = await getBytes(seedKey(id));
  const signedCert = await getBytes(certKey(id));
  if (!deviceSeed || !signedCert) return null;
  return { ...stored, deviceSeed, signedCert };
}

/** Inputs to [`addCore`]. */
export interface AddCoreInput {
  id: string;
  label: string;
  blob: ConnectBlob;
  deviceIdHex: string;
  deviceSeed: Uint8Array;
  signedCert: Uint8Array;
}

/**
 * Persist a freshly-paired Core and make it active. If a Core with the same id
 * already exists it is REPLACED (re-pairing a Core updates its blob/cert). The
 * secrets go to their own keys; the index carries no key material.
 */
export async function addCore(input: AddCoreInput): Promise<StoredCore> {
  const stored: StoredCore = {
    id: input.id,
    label: input.label,
    blob: input.blob,
    deviceIdHex: input.deviceIdHex,
    pairedAtMs: Date.now(),
  };
  await setBytes(seedKey(input.id), input.deviceSeed);
  await setBytes(certKey(input.id), input.signedCert);

  const idx = await readIndex();
  const cores = idx.cores.filter((c) => c.id !== input.id);
  cores.push(stored);
  await writeIndex({ activeId: input.id, cores });
  return stored;
}

/** Switch the active Core. Throws if `id` is not a paired Core. */
export async function switchCore(id: string): Promise<void> {
  const idx = await readIndex();
  if (!idx.cores.some((c) => c.id === id)) {
    throw new Error(`switchCore: unknown Core "${id}"`);
  }
  await writeIndex({ ...idx, activeId: id });
}

/** Remove a paired Core (and its secrets). Re-points `activeId` if it was active. */
export async function removeCore(id: string): Promise<void> {
  const idx = await readIndex();
  const cores = idx.cores.filter((c) => c.id !== id);
  const activeId = idx.activeId === id ? (cores[0]?.id ?? null) : idx.activeId;
  // Delete the secret keys BEFORE writing the trimmed index. `remove` is a plain
  // non-transactional `deleteItemAsync`, so a process kill mid-op has no
  // atomicity to rely on — ordering decides what an interrupted remove leaves
  // behind. Index-last means the worst case is a stale index entry whose secrets
  // are already gone, which `loadCore` already handles (returns null). The
  // reverse order would instead orphan the device seed (Ed25519 PRIVATE key) +
  // signed cert in the Keychain/Keystore with no index entry referencing them,
  // silently persisting secret material for an un-paired Core.
  await remove(seedKey(id));
  await remove(certKey(id));
  await writeIndex({ activeId, cores });
}
