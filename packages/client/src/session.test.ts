import { describe, expect, it } from "vitest";

import { createClient, createRouterTransport } from "@connectrpc/connect";

import { Notifications } from "./gen/concerto/v1/notifications_pb";
import {
  base64ToBytes,
  bytesToBase64,
  createMemorySessionStore,
  createSessionInterceptor,
  createStubPhoneSigner,
  decodeCertHeader,
  DEVICE_CERT_METADATA_KEY,
  encodeCertHeader,
  EPHEMERAL_SESSION_TTL_MS,
  isExpired,
  loadValidSession,
  mintEphemeralSession,
  sessionFromStored,
  type StoredSession,
  verifyEphemeralCert,
  WEB_EPHEMERAL_DEVICE_KIND,
} from "./session";

describe("base64 (STANDARD)", () => {
  it("round-trips arbitrary bytes", () => {
    const bytes = new Uint8Array([0, 1, 2, 250, 251, 255, 64, 128]);
    expect(Array.from(base64ToBytes(bytesToBase64(bytes)))).toEqual(Array.from(bytes));
  });

  it("uses STANDARD alphabet (+ and /), matching the Core's engine", () => {
    // 0xfb,0xff,0xfe → 6-bit groups 62,63,63,62 → "+//+" under STANDARD
    // (url-safe would render these as "-__-").
    expect(bytesToBase64(new Uint8Array([0xfb, 0xff, 0xfe]))).toBe("+//+");
  });
});

describe("createStubPhoneSigner", () => {
  it("exposes a 32-byte Ed25519 public key and signs to 64 bytes", async () => {
    const signer = await createStubPhoneSigner();
    expect(base64ToBytes(signer.publicKeyB64).length).toBe(32);
    const sig = await signer.sign(new TextEncoder().encode("hello"));
    expect(sig.length).toBe(64);
  });
});

describe("mintEphemeralSession", () => {
  it("mints a web_ephemeral cert that expires in 8h", async () => {
    const signer = await createStubPhoneSigner();
    const now = 1_700_000_000_000;
    const session = await mintEphemeralSession(signer, { nowMs: now });

    expect(session.claims.deviceKind).toBe(WEB_EPHEMERAL_DEVICE_KIND);
    expect(session.claims.version).toBe(1);
    expect(session.claims.issuedAtMs).toBe(now);
    expect(session.expiresAt).toBe(now + EPHEMERAL_SESSION_TTL_MS);
    expect(session.expiresAt - session.claims.issuedAtMs).toBe(8 * 60 * 60 * 1000);
    // The decoded view equals the parsed signed-cert claims.
    expect(JSON.parse(session.cert.claimsJson)).toEqual(session.claims);
    // The device + issuer keys are present and 32 bytes.
    expect(base64ToBytes(session.claims.devicePubkey).length).toBe(32);
    expect(session.claims.issuerPubkey).toBe(signer.publicKeyB64);
  });

  it("mints distinct device ids / keys per call", async () => {
    const signer = await createStubPhoneSigner();
    const a = await mintEphemeralSession(signer);
    const b = await mintEphemeralSession(signer);
    expect(a.claims.deviceId).not.toBe(b.claims.deviceId);
    expect(a.claims.devicePubkey).not.toBe(b.claims.devicePubkey);
  });

  it("produces a signature the issuer key verifies (tamper → false)", async () => {
    const signer = await createStubPhoneSigner();
    const session = await mintEphemeralSession(signer);
    expect(await verifyEphemeralCert(session.cert, signer.publicKeyB64)).toBe(true);

    const tampered = {
      ...session.cert,
      claimsJson: session.cert.claimsJson.replace(WEB_EPHEMERAL_DEVICE_KIND, "desktop"),
    };
    expect(await verifyEphemeralCert(tampered, signer.publicKeyB64)).toBe(false);

    // A different issuer key does not verify it.
    const other = await createStubPhoneSigner();
    expect(await verifyEphemeralCert(session.cert, other.publicKeyB64)).toBe(false);
  });
});

describe("isExpired (8h boundary)", () => {
  it("is false before expiry and true at/after it", async () => {
    const signer = await createStubPhoneSigner();
    const now = 1_700_000_000_000;
    const session = await mintEphemeralSession(signer, { nowMs: now });

    expect(isExpired(session, now)).toBe(false);
    expect(isExpired(session, now + EPHEMERAL_SESSION_TTL_MS - 1)).toBe(false);
    // Exactly 8h later: expired.
    expect(isExpired(session, now + EPHEMERAL_SESSION_TTL_MS)).toBe(true);
    expect(isExpired(session, now + EPHEMERAL_SESSION_TTL_MS + 1)).toBe(true);
  });
});

describe("cert header encoding", () => {
  it("encode/decode round-trips the signed cert through STANDARD base64", async () => {
    const signer = await createStubPhoneSigner();
    const session = await mintEphemeralSession(signer);
    const header = encodeCertHeader(session.cert);
    // It is base64 of JSON, so it decodes back exactly.
    expect(decodeCertHeader(header)).toEqual(session.cert);
    // Sanity: decoding the outer base64 yields parseable JSON.
    expect(() => JSON.parse(new TextDecoder().decode(base64ToBytes(header)))).not.toThrow();
  });
});

describe("SessionStore (memory) round-trip", () => {
  it("put → get returns the stored session; clear empties it", async () => {
    const signer = await createStubPhoneSigner();
    const session = await mintEphemeralSession(signer);
    const store = createMemorySessionStore();

    expect(await store.get()).toBeNull();

    const stored: StoredSession = { cert: session.cert, remember: false };
    await store.put(stored);
    expect(await store.get()).toEqual(stored);

    // Rebuilding the session view from storage matches the original.
    const rebuilt = sessionFromStored((await store.get())!);
    expect(rebuilt.expiresAt).toBe(session.expiresAt);
    expect(rebuilt.claims).toEqual(session.claims);

    await store.clear();
    expect(await store.get()).toBeNull();
  });

  it("persists the remember flag (remember-browser opt-out)", async () => {
    const signer = await createStubPhoneSigner();
    const session = await mintEphemeralSession(signer);
    const store = createMemorySessionStore();
    await store.put({ cert: session.cert, remember: true });
    const got = await store.get();
    expect(got?.remember).toBe(true);
  });
});

describe("loadValidSession", () => {
  it("returns the session when not expired", async () => {
    const signer = await createStubPhoneSigner();
    const now = 1_700_000_000_000;
    const session = await mintEphemeralSession(signer, { nowMs: now });
    const store = createMemorySessionStore();
    await store.put({ cert: session.cert, remember: true });

    const loaded = await loadValidSession(store, now + 1000);
    expect(loaded?.expiresAt).toBe(session.expiresAt);
    // Still present (not cleared).
    expect(await store.get()).not.toBeNull();
  });

  it("returns null AND clears the store when expired", async () => {
    const signer = await createStubPhoneSigner();
    const now = 1_700_000_000_000;
    const session = await mintEphemeralSession(signer, { nowMs: now });
    const store = createMemorySessionStore();
    await store.put({ cert: session.cert, remember: true });

    const loaded = await loadValidSession(store, now + EPHEMERAL_SESSION_TTL_MS);
    expect(loaded).toBeNull();
    // Housekeeping: the expired entry is gone.
    expect(await store.get()).toBeNull();
  });

  it("returns null for an empty store", async () => {
    expect(await loadValidSession(createMemorySessionStore())).toBeNull();
  });
});

describe("createSessionInterceptor", () => {
  // A router transport that captures the inbound header the interceptor set, by
  // routing GetInbox through a handler that echoes nothing but lets the
  // interceptor mutate req.header first. We assert on the header via a custom
  // interceptor placed AFTER ours that reads the final value.
  it("attaches the cert header on a unary rpc", async () => {
    const signer = await createStubPhoneSigner();
    const session = await mintEphemeralSession(signer);

    let seen: string | null = null;
    const transport = createRouterTransport(
      (router) => {
        router.service(Notifications, {
          getInbox() {
            return { notifications: [] };
          },
        });
      },
      {
        transport: {
          interceptors: [
            createSessionInterceptor(() => session.cert),
            // Inner interceptor (runs after ours mutates the header) captures it.
            (next) => async (req) => {
              seen = req.header.get(DEVICE_CERT_METADATA_KEY);
              return next(req);
            },
          ],
        },
      },
    );

    const client = createClient(Notifications, transport);
    await client.getInbox({ unreadOnly: false, limit: 0 });

    expect(seen).not.toBeNull();
    // The captured header decodes back to the signed cert.
    expect(decodeCertHeader(seen!)).toEqual(session.cert);
  });

  it("attaches no header when there is no session", async () => {
    let seen: string | null | undefined = "unset";
    const transport = createRouterTransport(
      (router) => {
        router.service(Notifications, {
          getInbox() {
            return { notifications: [] };
          },
        });
      },
      {
        transport: {
          interceptors: [
            createSessionInterceptor(() => null),
            (next) => async (req) => {
              seen = req.header.get(DEVICE_CERT_METADATA_KEY);
              return next(req);
            },
          ],
        },
      },
    );

    const client = createClient(Notifications, transport);
    await client.getInbox({ unreadOnly: false, limit: 0 });
    expect(seen).toBeNull();
  });

  it("reflects a freshly minted cert (getCert is read per call)", async () => {
    const signer = await createStubPhoneSigner();
    let current: Awaited<ReturnType<typeof mintEphemeralSession>> | null = null;

    let seen: string | null = null;
    const transport = createRouterTransport(
      (router) => {
        router.service(Notifications, {
          getInbox() {
            return { notifications: [] };
          },
        });
      },
      {
        transport: {
          interceptors: [
            createSessionInterceptor(() => current?.cert ?? null),
            (next) => async (req) => {
              seen = req.header.get(DEVICE_CERT_METADATA_KEY);
              return next(req);
            },
          ],
        },
      },
    );
    const client = createClient(Notifications, transport);

    // First call: no session yet → no header.
    await client.getInbox({ unreadOnly: false, limit: 0 });
    expect(seen).toBeNull();

    // Mint, then a second call picks it up with no transport rebuild.
    current = await mintEphemeralSession(signer);
    await client.getInbox({ unreadOnly: false, limit: 0 });
    expect(decodeCertHeader(seen!)).toEqual(current.cert);
  });
});
