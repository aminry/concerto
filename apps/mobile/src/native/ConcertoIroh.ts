// The TYPED TS surface of the `ConcertoIroh` native module (Task 510; the Rust
// uniffi cdylib from Task 509 — `crates/concerto-iroh-ffi`). This interface
// MIRRORS the uniffi `#[uniffi::export]` functions 1:1 so the JS adapters
// (`native-data-client.ts`, `src/pairing/`) program against a stable contract
// regardless of where the binding comes from:
//
//   - On a dev/prod build the real binding loads via the Expo Modules /
//     uniffi-generated JS (`requireNativeModule("ConcertoIroh")`) — Tier-3, only
//     present once `expo prebuild` links the cdylib (see `plugins/withConcertoIroh.js`).
//   - In jest (Tier-2) the in-memory `mock-concerto-iroh.ts` implements this
//     same interface so the adapter + pairing logic are fully exercised with no
//     native code.
//
// CONTRACT (design/16 §3, the 509 FFI):
//   - `method` is the FULLY-QUALIFIED gRPC path "/concerto.v1.Service/Method".
//   - All payloads are OPAQUE bytes: the native side is an identity codec and
//     NEVER decodes them. Proto encode/decode is the JS adapter's job
//     (`@bufbuild/protobuf` toBinary/fromBinary).
//   - Sessions are referenced by an opaque numeric `handle`.

/** The connect-blob fields a paired device holds (mirrors `ConnectBlob` Record). */
export interface ConnectBlob {
  /** The Core endpoint's Iroh `EndpointId` (z-base-32 string). */
  endpointId: string;
  /** The relay URL the Core advertises (undefined/empty ⇒ no relay; loopback). */
  relayUrl?: string;
  /** Direct socket addresses (`ip:port`) for LAN / same-host / hole-punched reach. */
  directAddrs: string[];
  /** The Core's static Noise public key (hex, 32 bytes) — the IK responder identity. */
  coreNoisePub: string;
}

/** The one-shot pairing inputs for [`ConcertoIrohModule.pair`] (mirrors `PairingInputs`). */
export interface PairingInputs {
  /** The base connect-blob (endpoint id + relay + direct addrs + noise pub). */
  blob: ConnectBlob;
  /** The one-shot pairing token (hex, 32 bytes) — the Noise XX PSK. */
  pairingToken: string;
  /** A human-readable device name recorded in the cert (free text). */
  deviceName: string;
}

/** A freshly generated device identity (mirrors `DeviceKeypair` Record). */
export interface DeviceKeypair {
  /** The 32-byte Ed25519 seed (the PRIVATE key) — SECRET; persist securely. */
  seed: Uint8Array;
  /** The 32-byte Ed25519 public key. */
  publicKey: Uint8Array;
  /** The canonical `device_id` = BLAKE2b-256(public_key), 32 bytes. */
  deviceId: Uint8Array;
}

/** How this device's session reaches the Core (mirrors the `NatPath` enum). */
export type NatPath = "direct" | "relayed" | "lan";

/** Client-side NAT stats for this device's own live session(s) (mirrors `NatStats`). */
export interface NatStats {
  /** The path of the most-recently-opened live session (undefined ⇒ none / ambiguous). */
  path?: NatPath;
  /** Count of live sessions on a direct (hole-punched) path. */
  direct: number;
  /** Count of live sessions on a relayed path. */
  relayed: number;
  /** Count of live sessions on a LAN-direct path. */
  lan: number;
}

/**
 * Per-server-streamed-message callbacks (mirrors the `StreamEventCallback`
 * uniffi `callback_interface`). The Rust pump invokes `onEvent` per message and
 * exactly one of `onComplete` / `onError` at end-of-stream.
 */
export interface StreamEventCallback {
  /** One server-streamed message, RAW bytes, untouched (JS decodes). */
  onEvent(data: Uint8Array): void;
  /** The stream ended cleanly (end-of-stream from the server). */
  onComplete(): void;
  /** The stream ended with an error (gRPC status text). */
  onError(message: string): void;
}

/**
 * The native `ConcertoIroh` module surface (Task 510). One method per 509
 * `#[uniffi::export]` fn. The async functions block on the Rust tokio runtime
 * on-device; the mock resolves on a microtask. `rpcStream` returns a
 * subscription id synchronously and delivers messages via the callback.
 */
export interface ConcertoIrohModule {
  /**
   * Generate a fresh Ed25519 device keypair from OS randomness. The caller (511)
   * persists [`DeviceKeypair.seed`] to secure-store and re-derives on next launch.
   */
  generateDeviceKeypair(): Promise<DeviceKeypair>;

  /**
   * Pair this device with the Core over the Noise-XX `0x03` channel and return
   * the on-wire `SignedDeviceCert` bytes (`cert_bytes || signature`). The caller
   * persists the cert and presents it on every subsequent session.
   *
   * @param deviceSeed the 32-byte Ed25519 seed from `generateDeviceKeypair`.
   */
  pair(inputs: PairingInputs, deviceSeed: Uint8Array): Promise<Uint8Array>;

  /**
   * Open an authenticated session to the Core (Noise-IK API channel) and return
   * an opaque session handle. `signedCert` is the on-wire device cert from `pair`.
   */
  openSession(blob: ConnectBlob, signedCert: Uint8Array): Promise<number>;

  /**
   * Drive a unary RPC as a pure byte passthrough. `method` is the
   * fully-qualified gRPC path; `payload` is the raw request body; the raw
   * response body is resolved. The bytes are NEVER decoded native-side.
   */
  rpcUnary(handle: number, method: string, payload: Uint8Array): Promise<Uint8Array>;

  /**
   * Drive a server-streaming RPC as a pure byte passthrough. Each message's raw
   * bytes go to `onEvent`; the returned subscription id cancels the stream (see
   * `cancelSubscription`) or it is dropped on `closeSession`.
   */
  rpcStream(
    handle: number,
    method: string,
    payload: Uint8Array,
    onEvent: StreamEventCallback,
  ): Promise<number>;

  /** Cancel a live subscription (drops the stream task). No-op if unknown. */
  cancelSubscription(handle: number, subscriptionId: number): void;

  /** Close a session: drop the channel + endpoint and deregister the handle. */
  closeSession(handle: number): void;

  /** Client-side NAT stats for this device's own live session(s) (not a Core RPC). */
  natStats(): NatStats;
}

/**
 * Resolve the REAL native binding (Tier-3). The uniffi cdylib is registered as
 * an Expo module via the config plugin (`plugins/withConcertoIroh.js`) and only
 * present in a dev-client / prod build after `expo prebuild`. In Expo Go / jest
 * the module is absent and this throws — callers fall back to the mock (tests)
 * or surface a "needs a dev build" message (the app shell).
 *
 * Kept as a lazy `require` so that merely importing this file in jest (which has
 * no `expo-modules-core` native binding) does NOT crash: the throw happens only
 * if a caller actually asks for the native module.
 */
export function getNativeConcertoIroh(): ConcertoIrohModule {
  // Lazy require: avoids a hard top-level dependency on expo-modules-core's
  // native side, which is unavailable in the jest (node) runtime.
  // eslint-disable-next-line @typescript-eslint/no-var-requires, @typescript-eslint/no-require-imports
  const mod = require("expo-modules-core") as {
    requireOptionalNativeModule?: (name: string) => ConcertoIrohModule | null;
    requireNativeModule?: (name: string) => ConcertoIrohModule;
  };
  const native =
    mod.requireOptionalNativeModule?.("ConcertoIroh") ??
    mod.requireNativeModule?.("ConcertoIroh") ??
    null;
  if (!native) {
    throw new Error(
      "ConcertoIroh native module is not available. It links only in a dev-client " +
        "or production build after `expo prebuild` (see plugins/withConcertoIroh.js); " +
        "it is absent in Expo Go and the jest test runtime.",
    );
  }
  return native;
}

/** True iff the real native `ConcertoIroh` module is linked (a dev/prod build). */
export function hasNativeConcertoIroh(): boolean {
  try {
    // eslint-disable-next-line @typescript-eslint/no-var-requires, @typescript-eslint/no-require-imports
    const mod = require("expo-modules-core") as {
      requireOptionalNativeModule?: (name: string) => ConcertoIrohModule | null;
    };
    return Boolean(mod.requireOptionalNativeModule?.("ConcertoIroh"));
  } catch {
    return false;
  }
}
