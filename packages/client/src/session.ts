//! Ephemeral browser pairing session (Task 522, design/17 §3.3 + D15/D8/210).
//!
//! The connect-web bridge is auth-less + loopback-only TODAY (D15/D8/210). Task
//! 522 layers a CLIENT-SIDE ephemeral session onto it: a short-lived (8h)
//! `web_ephemeral` device cert that the browser mints, stores in IndexedDB,
//! attaches as the FROZEN `concerto-device-cert` auth metadata header on every
//! rpc/subscribe, and clears on tab close (unless "remember browser" is on).
//!
//! This module is TRANSPORT-AGNOSTIC — it never imports connect-web; it produces
//! a connect-es `Interceptor` any transport accepts. Three pieces:
//!
//!   1. `mintEphemeralSession(signer)` — a stub-phone signer signs an 8h
//!      `web_ephemeral` cert; returns `{ cert, expiresAt }`.
//!   2. A `SessionStore` (put / get / clear) — an IndexedDB impl for the browser
//!      and an in-memory impl for tests; plus `isExpired`.
//!   3. `createSessionInterceptor(getCert)` — sets the auth header on each call.
//!
//! TIER-3: the Core actually TRUSTING this cert needs the real phone signer
//! (Task 511) to mediate pairing AND the bridge auth middleware to register the
//! ephemeral `web_ephemeral` device. `createStubPhoneSigner()` is a TEST helper
//! that STANDS IN for the phone so the full client-side session machinery is
//! verifiable at Tier-2. The signed-cert wire shape here is a Tier-2 stand-in
//! (JSON, NOT the Core's canonical-CBOR `DeviceCert`); the real CBOR encoding +
//! Core trust land with 511 + bridge middleware (Tier-3).

import type { Interceptor } from "@connectrpc/connect";

/**
 * The FROZEN auth metadata header the Core's auth middleware reads
 * (`crates/core/src/security/auth.rs::DEVICE_CERT_METADATA_KEY`). The value is
 * STANDARD base64 of the on-wire signed device cert.
 */
export const DEVICE_CERT_METADATA_KEY = "concerto-device-cert";

/** The device kind carried by a browser ephemeral cert (Task 522). */
export const WEB_EPHEMERAL_DEVICE_KIND = "web_ephemeral";

/** Ephemeral session lifetime: 8 hours, in milliseconds. */
export const EPHEMERAL_SESSION_TTL_MS = 8 * 60 * 60 * 1000;

/**
 * The unsigned `web_ephemeral` cert claims the stub phone signs (Task 522). A
 * Tier-2 stand-in for the Core's canonical-CBOR `DeviceCert` — same SEMANTIC
 * fields (device id / pubkey / issuer / validity), JSON-shaped so it round-trips
 * without a CBOR runtime dep. The real wire encoding lands Tier-3 (511).
 */
export interface EphemeralCertClaims {
  /** Cert schema version. */
  readonly version: 1;
  /** `web_ephemeral` (this is a browser pairing cert). */
  readonly deviceKind: typeof WEB_EPHEMERAL_DEVICE_KIND;
  /** Random per-session device id (base64url of 16 random bytes). */
  readonly deviceId: string;
  /** The session keypair's Ed25519 public key (base64, raw 32 bytes). */
  readonly devicePubkey: string;
  /** The signing phone's Ed25519 public key (base64, raw 32 bytes). */
  readonly issuerPubkey: string;
  /** Issued-at, epoch ms. */
  readonly issuedAtMs: number;
  /** Expiry, epoch ms (issuedAt + 8h). */
  readonly expiresAtMs: number;
}

/**
 * The on-wire signed cert: the JSON-encoded claims plus the issuer's Ed25519
 * signature over those exact bytes. STANDARD-base64'd whole, this is the value
 * of the `concerto-device-cert` header.
 */
export interface SignedEphemeralCert {
  /** The claims, as the exact JSON string that was signed (canonical-ish). */
  readonly claimsJson: string;
  /** Ed25519 signature (base64, raw 64 bytes) over `claimsJson` bytes. */
  readonly signature: string;
}

/** A minted session: the signed cert + a decoded view + its expiry. */
export interface EphemeralSession {
  /** The signed, on-wire cert (what the header carries). */
  readonly cert: SignedEphemeralCert;
  /** The decoded claims (convenience; equals JSON.parse(cert.claimsJson)). */
  readonly claims: EphemeralCertClaims;
  /** Expiry epoch ms (equals claims.expiresAtMs). */
  readonly expiresAt: number;
}

/**
 * A "phone" that signs `web_ephemeral` certs for the browser. The REAL one is
 * the paired mobile device (Task 511, Tier-3); `createStubPhoneSigner()` is a
 * Tier-2 test stand-in. `sign` returns an Ed25519 signature over the bytes.
 */
export interface StubPhoneSigner {
  /** The signer's Ed25519 public key (base64, raw 32 bytes). */
  readonly publicKeyB64: string;
  /** Ed25519-sign `bytes`, returning the raw 64-byte signature. */
  sign(bytes: Uint8Array): Promise<Uint8Array>;
}

// ── base64 (STANDARD) helpers — match the Core's STANDARD engine ──────────────

const textEncoder = new TextEncoder();

/** STANDARD base64-encode bytes (matches the Core's base64 STANDARD engine). */
export function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]!);
  return btoa(bin);
}

/** Decode STANDARD base64 to bytes. */
export function base64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Pick the ambient SubtleCrypto (browser `crypto.subtle` / node `globalThis`). */
function subtle(): SubtleCrypto {
  const c = (globalThis as { crypto?: Crypto }).crypto;
  if (!c?.subtle) {
    throw new Error("Web Crypto SubtleCrypto is unavailable in this environment");
  }
  return c.subtle;
}

function randomBytes(n: number): Uint8Array {
  const c = (globalThis as { crypto?: Crypto }).crypto;
  if (!c?.getRandomValues) throw new Error("crypto.getRandomValues is unavailable");
  return c.getRandomValues(new Uint8Array(n));
}

function base64url(bytes: Uint8Array): string {
  return bytesToBase64(bytes).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/**
 * Create a Tier-2 stub-phone signer backed by a fresh Web Crypto Ed25519 key
 * pair. NO new runtime dep — pure SubtleCrypto. In production the phone (Task
 * 511) plays this role; here it lets the browser mint a self-consistent,
 * signature-verifiable `web_ephemeral` cert for client-side testing.
 */
export async function createStubPhoneSigner(): Promise<StubPhoneSigner> {
  const keyPair = (await subtle().generateKey({ name: "Ed25519" }, true, [
    "sign",
    "verify",
  ])) as CryptoKeyPair;
  const rawPub = new Uint8Array(await subtle().exportKey("raw", keyPair.publicKey));
  const publicKeyB64 = bytesToBase64(rawPub);
  return {
    publicKeyB64,
    async sign(bytes) {
      const sig = await subtle().sign({ name: "Ed25519" }, keyPair.privateKey, bytes as BufferSource);
      return new Uint8Array(sig);
    },
  };
}

/**
 * Verify a [`SignedEphemeralCert`] against an issuer public key (base64 raw 32
 * bytes). Exposed for tests + a future bridge-side analog; the Core's real
 * verification (canonical-CBOR + revocation) is Tier-3.
 */
export async function verifyEphemeralCert(
  cert: SignedEphemeralCert,
  issuerPubkeyB64: string,
): Promise<boolean> {
  try {
    const key = await subtle().importKey(
      "raw",
      base64ToBytes(issuerPubkeyB64) as BufferSource,
      { name: "Ed25519" },
      false,
      ["verify"],
    );
    return await subtle().verify(
      { name: "Ed25519" },
      key,
      base64ToBytes(cert.signature) as BufferSource,
      textEncoder.encode(cert.claimsJson) as BufferSource,
    );
  } catch {
    return false;
  }
}

/** Options for [`mintEphemeralSession`]. */
export interface MintOptions {
  /** Clock override (epoch ms); defaults to `Date.now()`. */
  nowMs?: number;
  /** TTL override (ms); defaults to 8h ([`EPHEMERAL_SESSION_TTL_MS`]). */
  ttlMs?: number;
}

/**
 * Mint an 8h `web_ephemeral` session: generate a fresh per-session device key
 * pair, build the cert claims, and have the (stub-phone) signer sign the
 * JSON-encoded claims. Returns the signed cert + decoded claims + expiry.
 */
export async function mintEphemeralSession(
  signer: StubPhoneSigner,
  opts: MintOptions = {},
): Promise<EphemeralSession> {
  const now = opts.nowMs ?? Date.now();
  const ttl = opts.ttlMs ?? EPHEMERAL_SESSION_TTL_MS;

  const deviceKeyPair = (await subtle().generateKey({ name: "Ed25519" }, true, [
    "sign",
    "verify",
  ])) as CryptoKeyPair;
  const devicePub = new Uint8Array(await subtle().exportKey("raw", deviceKeyPair.publicKey));

  const claims: EphemeralCertClaims = {
    version: 1,
    deviceKind: WEB_EPHEMERAL_DEVICE_KIND,
    deviceId: base64url(randomBytes(16)),
    devicePubkey: bytesToBase64(devicePub),
    issuerPubkey: signer.publicKeyB64,
    issuedAtMs: now,
    expiresAtMs: now + ttl,
  };
  const claimsJson = JSON.stringify(claims);
  const signature = await signer.sign(textEncoder.encode(claimsJson));
  const cert: SignedEphemeralCert = { claimsJson, signature: bytesToBase64(signature) };

  return { cert, claims, expiresAt: claims.expiresAtMs };
}

/** True if `session` is at/after its expiry (`nowMs` defaults to `Date.now()`). */
export function isExpired(session: EphemeralSession, nowMs: number = Date.now()): boolean {
  return nowMs >= session.expiresAt;
}

/**
 * Encode a signed cert into the STANDARD-base64 string the
 * `concerto-device-cert` header carries. The Core base64-DECODES this to get the
 * on-wire signed cert bytes (Tier-2 stand-in: JSON; Tier-3: canonical-CBOR).
 */
export function encodeCertHeader(cert: SignedEphemeralCert): string {
  return bytesToBase64(textEncoder.encode(JSON.stringify(cert)));
}

/** Inverse of [`encodeCertHeader`] — decode a header value back to a signed cert. */
export function decodeCertHeader(headerValue: string): SignedEphemeralCert {
  const obj = JSON.parse(new TextDecoder().decode(base64ToBytes(headerValue)));
  return obj as SignedEphemeralCert;
}

// ── Persistence: a small SessionStore seam (IndexedDB for the browser, ────────
//    in-memory for tests). Stores the SIGNED cert (the header source of truth).

/** The persisted shape: the signed cert + a "remember past tab close" flag. */
export interface StoredSession {
  /** The signed cert (what the header carries / mint() produced). */
  readonly cert: SignedEphemeralCert;
  /** Whether the user opted to keep the session past tab close. */
  readonly remember: boolean;
}

/** Async put / get / clear over the persisted ephemeral session. */
export interface SessionStore {
  put(stored: StoredSession): Promise<void>;
  get(): Promise<StoredSession | null>;
  clear(): Promise<void>;
}

/** Rebuild an [`EphemeralSession`] view from a stored signed cert. */
export function sessionFromStored(stored: StoredSession): EphemeralSession {
  const claims = JSON.parse(stored.cert.claimsJson) as EphemeralCertClaims;
  return { cert: stored.cert, claims, expiresAt: claims.expiresAtMs };
}

/**
 * Load a stored session ONLY if it is still valid (not expired). Expired or
 * absent → returns null AND clears the store (housekeeping). `nowMs` overrides
 * the clock for tests.
 */
export async function loadValidSession(
  store: SessionStore,
  nowMs: number = Date.now(),
): Promise<EphemeralSession | null> {
  const stored = await store.get();
  if (!stored) return null;
  const session = sessionFromStored(stored);
  if (isExpired(session, nowMs)) {
    await store.clear();
    return null;
  }
  return session;
}

/** An in-memory [`SessionStore`] for tests (and SSR/no-IndexedDB fallback). */
export function createMemorySessionStore(): SessionStore {
  let current: StoredSession | null = null;
  return {
    async put(stored) {
      current = stored;
    },
    async get() {
      return current;
    },
    async clear() {
      current = null;
    },
  };
}

const IDB_DB_NAME = "concerto-session";
const IDB_STORE_NAME = "session";
const IDB_KEY = "ephemeral";

/** Options for [`createIndexedDbSessionStore`]. */
export interface IndexedDbStoreOptions {
  /** `IDBFactory` override (tests inject a fake); defaults to `globalThis.indexedDB`. */
  factory?: IDBFactory;
  /** DB name override (default `concerto-session`). */
  dbName?: string;
}

/**
 * A real IndexedDB-backed [`SessionStore`]. Used by the browser app (and
 * exercised end-to-end by the Playwright reload tests). One object store keyed
 * by a constant; values are the [`StoredSession`] (signed cert + remember flag).
 */
export function createIndexedDbSessionStore(opts: IndexedDbStoreOptions = {}): SessionStore {
  const factory = opts.factory ?? (globalThis as { indexedDB?: IDBFactory }).indexedDB;
  if (!factory) throw new Error("IndexedDB is unavailable in this environment");
  const dbName = opts.dbName ?? IDB_DB_NAME;

  const open = (): Promise<IDBDatabase> =>
    new Promise((resolve, reject) => {
      const req = factory.open(dbName, 1);
      req.onupgradeneeded = () => {
        const db = req.result;
        if (!db.objectStoreNames.contains(IDB_STORE_NAME)) {
          db.createObjectStore(IDB_STORE_NAME);
        }
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => reject(req.error ?? new Error("IndexedDB open failed"));
    });

  const tx = <T>(
    mode: IDBTransactionMode,
    run: (store: IDBObjectStore) => IDBRequest<T> | null,
  ): Promise<T | undefined> =>
    open().then(
      (db) =>
        new Promise<T | undefined>((resolve, reject) => {
          const transaction = db.transaction(IDB_STORE_NAME, mode);
          const store = transaction.objectStore(IDB_STORE_NAME);
          const request = run(store);
          transaction.oncomplete = () => {
            db.close();
            resolve(request ? (request.result as T) : undefined);
          };
          transaction.onerror = () => {
            db.close();
            reject(transaction.error ?? new Error("IndexedDB transaction failed"));
          };
        }),
    );

  return {
    async put(stored) {
      await tx("readwrite", (store) => {
        store.put(stored, IDB_KEY);
        return null;
      });
    },
    async get() {
      const v = await tx<StoredSession>("readonly", (store) => store.get(IDB_KEY));
      return v ?? null;
    },
    async clear() {
      await tx("readwrite", (store) => {
        store.delete(IDB_KEY);
        return null;
      });
    },
  };
}

// ── The connect interceptor: attach the cert header to every call ─────────────

/**
 * Build a connect-es [`Interceptor`] that attaches the current ephemeral cert as
 * the FROZEN `concerto-device-cert` auth metadata header on EVERY rpc AND
 * subscribe (interceptors wrap unary + streaming alike — both carry `req.header:
 * Headers`). `getCert` is called per request so a freshly minted / cleared
 * session is reflected immediately; when it returns null/undefined no header is
 * attached (the bridge is auth-less today — Tier-2 — so calls still succeed).
 */
export function createSessionInterceptor(
  getCert: () => SignedEphemeralCert | null | undefined,
): Interceptor {
  return (next) => async (req) => {
    const cert = getCert();
    if (cert) {
      req.header.set(DEVICE_CERT_METADATA_KEY, encodeCertHeader(cert));
    }
    return next(req);
  };
}
