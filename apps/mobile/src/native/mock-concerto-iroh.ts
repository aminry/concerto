// In-memory mock of the `ConcertoIroh` native module (Task 510, Tier-2). Used by
// jest to exercise BOTH the native `DataClient` adapter (encode → rpcUnary →
// decode round-trips) and the pairing flow (generateDeviceKeypair + pair +
// secure-store persistence) with NO native code — the real binding is Tier-3.
//
// It stays an HONEST opaque-bytes seam: every RPC handler receives + returns raw
// `Uint8Array`, exactly like the 509 identity codec — the adapter owns proto
// encode/decode, so a test that round-trips a real proto type through this mock
// proves the adapter, not the mock.
import type {
  ConcertoIrohModule,
  ConnectBlob,
  DeviceKeypair,
  NatStats,
  PairingInputs,
  StreamEventCallback,
} from "./ConcertoIroh";

/** A unary handler: given the request bytes, return the response bytes. */
export type UnaryHandler = (payload: Uint8Array) => Uint8Array | Promise<Uint8Array>;

/**
 * A stream handler: push messages via `cb.onEvent` and end with
 * `cb.onComplete()` / `cb.onError(msg)`. Return an optional teardown invoked on
 * `cancelSubscription` / `closeSession` (e.g. to clear a timer).
 */
export type StreamHandler = (
  payload: Uint8Array,
  cb: StreamEventCallback,
) => void | (() => void);

/** A recorded pair() call (for assertions). */
export interface PairCall {
  inputs: PairingInputs;
  deviceSeed: Uint8Array;
}

/** A recorded openSession() call (for assertions). */
export interface OpenSessionCall {
  blob: ConnectBlob;
  signedCert: Uint8Array;
}

/** Options controlling the mock's behaviour. */
export interface MockConcertoIrohOptions {
  /** Per-gRPC-path unary responders (keyed by the FULL "/svc/Method" path). */
  unary?: Record<string, UnaryHandler>;
  /** Per-gRPC-path streaming responders. */
  stream?: Record<string, StreamHandler>;
  /** Cert bytes `pair()` resolves to (default: a deterministic 8-byte tag). */
  signedCert?: Uint8Array;
  /** The keypair `generateDeviceKeypair()` resolves to (default: deterministic). */
  keypair?: DeviceKeypair;
  /** NAT stats `natStats()` returns (default: one direct session). */
  natStats?: NatStats;
}

/** The mock module plus its recorded interactions (for test assertions). */
export interface MockConcertoIroh extends ConcertoIrohModule {
  /** Every `pair()` call, in order. */
  readonly pairCalls: PairCall[];
  /** Every `openSession()` call, in order. */
  readonly openSessionCalls: OpenSessionCall[];
  /** Every `generateDeviceKeypair()` call count. */
  readonly generateCount: () => number;
  /** Handles closed via `closeSession`. */
  readonly closedHandles: number[];
}

const DEFAULT_KEYPAIR: DeviceKeypair = {
  // Deterministic, NON-secret test vectors (clearly fake — not real key material).
  seed: new Uint8Array(32).fill(7),
  publicKey: new Uint8Array(32).fill(8),
  deviceId: new Uint8Array(32).fill(9),
};

const DEFAULT_CERT = new Uint8Array([0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7]);

const DEFAULT_NAT: NatStats = { path: "direct", direct: 1, relayed: 0, lan: 0 };

/**
 * Build an in-memory [`ConcertoIrohModule`] mock for jest. Wire `unary` / `stream`
 * responders keyed by gRPC path; the adapter encodes/decodes around them.
 */
export function createMockConcertoIroh(
  opts: MockConcertoIrohOptions = {},
): MockConcertoIroh {
  const pairCalls: PairCall[] = [];
  const openSessionCalls: OpenSessionCall[] = [];
  const closedHandles: number[] = [];
  let generateCalls = 0;
  let nextHandle = 1;
  let nextSubId = 1;

  // Live subscriptions: handle → (subId → teardown).
  const subs = new Map<number, Map<number, () => void>>();

  const module: MockConcertoIroh = {
    pairCalls,
    openSessionCalls,
    closedHandles,
    generateCount: () => generateCalls,

    async generateDeviceKeypair() {
      generateCalls += 1;
      return opts.keypair ?? DEFAULT_KEYPAIR;
    },

    async pair(inputs, deviceSeed) {
      pairCalls.push({ inputs, deviceSeed });
      return opts.signedCert ?? DEFAULT_CERT;
    },

    async openSession(blob, signedCert) {
      openSessionCalls.push({ blob, signedCert });
      const handle = nextHandle++;
      subs.set(handle, new Map());
      return handle;
    },

    async rpcUnary(_handle, method, payload) {
      const handler = opts.unary?.[method];
      if (!handler) {
        throw new Error(`mock ConcertoIroh: no unary handler for "${method}"`);
      }
      return handler(payload);
    },

    async rpcStream(handle, method, payload, onEvent) {
      const handler = opts.stream?.[method];
      if (!handler) {
        throw new Error(`mock ConcertoIroh: no stream handler for "${method}"`);
      }
      const subId = nextSubId++;
      const teardown = handler(payload, onEvent);
      const map = subs.get(handle) ?? new Map();
      map.set(subId, teardown ?? (() => {}));
      subs.set(handle, map);
      return subId;
    },

    cancelSubscription(handle, subscriptionId) {
      const map = subs.get(handle);
      const teardown = map?.get(subscriptionId);
      if (teardown) {
        teardown();
        map?.delete(subscriptionId);
      }
    },

    closeSession(handle) {
      closedHandles.push(handle);
      const map = subs.get(handle);
      if (map) {
        for (const teardown of map.values()) teardown();
        subs.delete(handle);
      }
    },

    natStats() {
      return opts.natStats ?? DEFAULT_NAT;
    },
  };

  return module;
}
