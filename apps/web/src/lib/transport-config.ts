/**
 * Web client transport selection + LAN-direct TLS pinning config (Task 521).
 *
 * `design/17 §6.2` (transport selection) + `§3.3` (LAN-direct TLS) + `§3.4`
 * (remote WSS-via-relay). This module is intentionally **dependency-free** and
 * **standalone**: it computes *which transport to use* and *the base URL to dial*
 * from `window.location`, and carries the Core-identity TLS-cert fingerprint a
 * LAN client pins. It deliberately does NOT touch `data.ts`'s `makeDataClient`
 * signature — the data-client wires this config in separately.
 *
 * ## The two paths (`design/17 §3.3` / `§3.4`)
 *
 * - **LAN-direct** (`http(s)://*.local` or `http(s)://127.0.0.1:<port>`): the
 *   page is served by the user's own Core. When the Core enables LAN-direct TLS
 *   (Task 521, `CONCERTO_CONNECT_BRIDGE_TLS`), it serves `https://` with a
 *   **self-signed cert deterministically derived from its identity public key**
 *   and publishes the cert's **SPKI SHA-256 fingerprint**
 *   ({@link CoreTlsPin.spkiSha256Hex}). The Connect-Web client speaks gRPC-Web
 *   directly; no relay.
 *
 * - **Remote via relay** (any other origin, e.g. `https://app.concerto.app/...`):
 *   the browser opens a `wss://relay/.../<endpoint_id>` connection; the relay is
 *   a transparent **ciphertext-only** byte pump to the Core, inside which the
 *   browser runs **Noise IK** with its session pairing key (`design/17 §3.4`,
 *   `design/11 §3.9`). The outer `wss://` TLS is the relay operator's public-CA
 *   cert — NOT the Core-identity pin (the Core identity is authenticated by the
 *   inner Noise IK, not the outer TLS).
 *
 * ## Honest browser cert-pinning posture (`design/17 §3.3`, §8, R-1)
 *
 * A **native / LAN client** (Desktop split-host, mobile, CLI) can pin the
 * published SPKI fingerprint programmatically and refuse anything else — full
 * MITM resistance. A **browser cannot** be handed an SPKI pin for a self-signed
 * LAN cert at page-load time; instead the user clicks through the one-time
 * "self-signed certificate" interstitial and the browser stores a per-site
 * exception. The published fingerprint is a **verification aid** in the browser
 * (the user — or a Tray helper — confirms the cert they accept matches the Core
 * they paired with), not an enforced pin. Some browsers (HSTS-preloaded /
 * strict enterprise policy) refuse self-signed certs outright; those users must
 * use the relayed remote URL (`design/17 §8`). V1.5 (R-1) adds a one-click
 * mkcert-style local-CA trust from the Tray.
 */

/** The transport the web client uses to reach its Core (`design/17 §6.2`). */
export type TransportMode =
  /** Connect-Web / gRPC-Web straight to the Core's loopback/LAN bridge. */
  | "lan-direct"
  /** WSS through the relay; Noise IK runs inside the tunnel. */
  | "wss-relay";

/**
 * The Core-identity TLS pin a LAN client verifies (Task 521). The Core
 * publishes this alongside its bridge (it is the SHA-256 of the served cert's
 * SubjectPublicKeyInfo, lowercase hex).
 */
export interface CoreTlsPin {
  /** Lowercase-hex SHA-256 of the cert's SubjectPublicKeyInfo. 64 hex chars. */
  spkiSha256Hex: string;
}

/** Resolved transport configuration the data-client consumes. */
export interface TransportConfig {
  mode: TransportMode;
  /**
   * The base URL the Connect-Web client dials for LAN-direct (e.g.
   * `https://concerto.local:8443`), or the relay WSS base for remote.
   */
  baseUrl: string;
  /**
   * The Core endpoint id, present for the remote relay path (parsed from a
   * `/c/<endpoint_id>` route). `null` for LAN-direct.
   */
  endpointId: string | null;
  /**
   * The expected Core-identity TLS pin for LAN-direct, when one is known
   * (passed from pairing / a prior visit). `null` ⇒ no pin known yet (the user
   * accepts the cert on first visit and may verify it against the Core's
   * published fingerprint). Always `null` for the relay path (the outer TLS is
   * the relay's public-CA cert; the Core is authenticated by inner Noise IK).
   */
  tlsPin: CoreTlsPin | null;
}

/** A minimal view of `window.location` so this module is unit-testable. */
export interface LocationLike {
  protocol: string; // e.g. "https:"
  hostname: string; // e.g. "concerto.local" / "127.0.0.1" / "app.concerto.app"
  host: string; // hostname[:port]
  pathname: string; // e.g. "/c/<endpoint_id>" or "/workspace/123"
  search?: string; // e.g. "?force=lan&endpoint=..."
}

/**
 * Is this a LAN-direct origin (`design/17 §6.2` step 1): `*.local` or a loopback
 * host (`127.0.0.1`, `localhost`, `::1`)? These are served by the user's own
 * Core, so the Connect-Web client speaks to it directly.
 */
export function isLanDirectHost(hostname: string): boolean {
  const h = hostname.toLowerCase().replace(/^\[|\]$/g, ""); // strip IPv6 brackets
  return (
    h === "localhost" ||
    h === "127.0.0.1" ||
    h === "::1" ||
    h.endsWith(".local")
  );
}

/**
 * Parse a `/c/<endpoint_id>` (or `/wss/<endpoint_id>`) route into the Core
 * endpoint id, or `null` if the path is not an endpoint-addressed route. Only
 * the first non-empty segment after the prefix is taken (the FROZEN relay route
 * shape, `design/11 §3.4`).
 */
export function parseEndpointId(pathname: string): string | null {
  const m = pathname.match(/^\/(?:c|wss)\/([^/?#]+)/);
  return m ? decodeURIComponent(m[1]) : null;
}

/**
 * Decide the transport from the current location (`design/17 §6.2`).
 *
 * Selection order:
 *   1. `?force=lan` (with optional `&endpoint=<host>`): IT-restricted networks
 *      where mDNS is blocked but the user can type the Core host directly.
 *   2. LAN-direct host (`*.local` / loopback) → `lan-direct`.
 *   3. otherwise → `wss-relay` (endpoint id parsed from `/c/<id>` when present).
 *
 * `knownPin` is the Core-identity TLS pin already known from pairing / a prior
 * visit (or `null`); it is attached only to the LAN-direct result.
 */
export function selectTransport(
  loc: LocationLike,
  knownPin: CoreTlsPin | null = null,
): TransportConfig {
  const params = new URLSearchParams(loc.search ?? "");
  const forceLan = (params.get("force") ?? "").toLowerCase() === "lan";

  if (forceLan) {
    const endpointHost = params.get("endpoint");
    const host = endpointHost && endpointHost.length > 0 ? endpointHost : loc.host;
    // Honor an explicit scheme on the forced endpoint; default to the page's.
    const scheme = host.includes("://") ? "" : `${loc.protocol}//`;
    return {
      mode: "lan-direct",
      baseUrl: `${scheme}${host}`,
      endpointId: null,
      tlsPin: knownPin,
    };
  }

  if (isLanDirectHost(loc.hostname)) {
    return {
      mode: "lan-direct",
      baseUrl: `${loc.protocol}//${loc.host}`,
      endpointId: null,
      tlsPin: knownPin,
    };
  }

  // Remote: WSS through the relay. Inner Noise IK authenticates the Core, so the
  // Core-identity TLS pin does NOT apply to the relay's outer TLS.
  return {
    mode: "wss-relay",
    baseUrl: `${loc.protocol}//${loc.host}`,
    endpointId: parseEndpointId(loc.pathname),
    tlsPin: null,
  };
}

/**
 * Whether the page is being served over a secure context. LAN-direct over plain
 * HTTP (loopback) is fine; LAN-direct over a non-loopback LAN host needs TLS
 * (the Task-521 reason the Core can serve `https://` on the LAN). Surfaced so
 * the UI can prompt the user to enable LAN-direct TLS when needed.
 */
export function lanDirectNeedsTls(loc: LocationLike): boolean {
  if (!isLanDirectHost(loc.hostname)) return false;
  const isLoopback =
    loc.hostname === "127.0.0.1" ||
    loc.hostname === "localhost" ||
    loc.hostname === "::1";
  // A non-loopback LAN host (e.g. concerto.local / 192.168.x.y) served over
  // plain http: would be a non-secure context → needs the Task-521 TLS bridge.
  return !isLoopback && loc.protocol === "http:";
}

/**
 * Compare a server-presented pin against the expected one (constant-ish string
 * compare; pins are public, non-secret). Returns `true` when they match. The
 * caller decides what to do on mismatch — for a native client, refuse; for a
 * browser, warn ("Core identity mismatch", `design/17 §8`).
 */
export function pinMatches(expected: CoreTlsPin, presentedSpkiSha256Hex: string): boolean {
  return (
    expected.spkiSha256Hex.toLowerCase() === presentedSpkiSha256Hex.toLowerCase()
  );
}
