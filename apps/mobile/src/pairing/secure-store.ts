// A thin typed wrapper over `expo-secure-store` (Task 511; design/16 §3.8 / §3.6
// — Keychain on iOS, Keystore on Android). The pairing flow persists the device
// seed (the Ed25519 PRIVATE key) and the signed device cert here; the multi-Core
// registry (`core-store.ts`) persists the paired-Core list here too.
//
// expo-secure-store stores STRINGS, so byte values (the seed / cert) are
// base64-encoded across the boundary. The real module is a NATIVE module
// (Tier-3: real Keychain/Keystore); jest mocks it with an in-memory map (see
// `jest.setup.ts`).
import * as SecureStore from "expo-secure-store";

/** Persist a JSON-serialisable value under `key` (Keychain/Keystore). */
export async function setJson(key: string, value: unknown): Promise<void> {
  await SecureStore.setItemAsync(key, JSON.stringify(value));
}

/** Read + parse a JSON value, or `null` if absent / unparseable. */
export async function getJson<T>(key: string): Promise<T | null> {
  const raw = await SecureStore.getItemAsync(key);
  if (raw == null) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

/** Delete a key (idempotent). */
export async function remove(key: string): Promise<void> {
  await SecureStore.deleteItemAsync(key);
}

/** Persist raw bytes (base64-encoded under the hood). */
export async function setBytes(key: string, bytes: Uint8Array): Promise<void> {
  await SecureStore.setItemAsync(key, bytesToBase64(bytes));
}

/** Read raw bytes, or `null` if absent. */
export async function getBytes(key: string): Promise<Uint8Array | null> {
  const raw = await SecureStore.getItemAsync(key);
  if (raw == null) return null;
  return base64ToBytes(raw);
}

/** Encode bytes as base64 (RN/Hermes-safe; no Buffer). */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return globalThis.btoa(binary);
}

/** Decode base64 to bytes (RN/Hermes-safe; no Buffer). */
export function base64ToBytes(b64: string): Uint8Array {
  const binary = globalThis.atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}
