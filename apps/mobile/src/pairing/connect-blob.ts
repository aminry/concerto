// The connect-blob QR payload parser (Task 511; design/12 §3.3 / design/16 §3.8,
// the `pair-serve` blob shape in `tools/pair-serve`).
//
// The Core's tray (and `tools/pair-serve`) emit the connect blob as
// `base64(JSON)` with snake_case keys:
//   { endpoint_id, relay_url?, direct_addrs[], pairing_token, core_noise_pub }
// QR codes carry that base64 string directly (optionally `PAIR-BLOB:`-prefixed
// from the CLI). We accept either form, decode + validate, and return camelCase
// fields ready for the native module's `ConnectBlob` / `PairingInputs`.

import type { ConnectBlob } from "../native/ConcertoIroh";

/** The decoded QR payload: a [`ConnectBlob`] plus the one-shot pairing token. */
export interface ParsedConnectBlob {
  /** The session-time blob (endpoint id + relay + direct addrs + noise pub). */
  blob: ConnectBlob;
  /** The one-shot pairing token (hex, 32 bytes) — the Noise XX PSK. */
  pairingToken: string;
}

/** Raised when a scanned QR is not a valid Concerto connect blob. */
export class ConnectBlobParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ConnectBlobParseError";
  }
}

const HEX32 = /^[0-9a-fA-F]{64}$/;

/** Decode a base64 string to its UTF-8 text (RN/Hermes-safe, no Buffer). */
function base64ToUtf8(b64: string): string {
  // `atob` exists on Hermes (RN 0.71+) and in jsdom; decode to bytes then UTF-8.
  // (We avoid `Buffer` so this works unchanged on-device.)
  const binary = globalThis.atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

function asString(v: unknown, field: string): string {
  if (typeof v !== "string" || v.length === 0) {
    throw new ConnectBlobParseError(`connect blob: "${field}" must be a non-empty string`);
  }
  return v;
}

/**
 * Parse a scanned QR string into a [`ParsedConnectBlob`]. Accepts the raw
 * base64(JSON) or a `PAIR-BLOB: <base64>` line (the CLI form). Throws
 * [`ConnectBlobParseError`] on any malformed input.
 */
export function parseConnectBlob(raw: string): ParsedConnectBlob {
  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    throw new ConnectBlobParseError("connect blob: empty QR payload");
  }
  // Strip an optional `PAIR-BLOB:` prefix (tools/pair-serve prints this).
  const b64 = trimmed.replace(/^PAIR-BLOB:\s*/i, "").trim();

  let json: unknown;
  try {
    json = JSON.parse(base64ToUtf8(b64));
  } catch (err) {
    throw new ConnectBlobParseError(
      `connect blob: not base64(JSON) (${err instanceof Error ? err.message : String(err)})`,
    );
  }
  if (typeof json !== "object" || json === null) {
    throw new ConnectBlobParseError("connect blob: decoded payload is not an object");
  }
  const o = json as Record<string, unknown>;

  const endpointId = asString(o.endpoint_id, "endpoint_id");
  const pairingToken = asString(o.pairing_token, "pairing_token");
  const coreNoisePub = asString(o.core_noise_pub, "core_noise_pub");

  if (!HEX32.test(pairingToken)) {
    throw new ConnectBlobParseError("connect blob: pairing_token must be 32-byte hex");
  }
  if (!HEX32.test(coreNoisePub)) {
    throw new ConnectBlobParseError("connect blob: core_noise_pub must be 32-byte hex");
  }

  const directAddrsRaw = o.direct_addrs;
  if (directAddrsRaw !== undefined && !Array.isArray(directAddrsRaw)) {
    throw new ConnectBlobParseError("connect blob: direct_addrs must be an array");
  }
  const directAddrs: string[] = Array.isArray(directAddrsRaw)
    ? directAddrsRaw.map((a, i) => asString(a, `direct_addrs[${i}]`))
    : [];

  let relayUrl: string | undefined;
  if (o.relay_url !== undefined && o.relay_url !== null) {
    relayUrl = asString(o.relay_url, "relay_url");
  }

  if (directAddrs.length === 0 && !relayUrl) {
    throw new ConnectBlobParseError(
      "connect blob: needs at least one of relay_url or direct_addrs to be reachable",
    );
  }

  const blob: ConnectBlob = {
    endpointId,
    directAddrs,
    coreNoisePub,
    ...(relayUrl !== undefined ? { relayUrl } : {}),
  };
  return { blob, pairingToken };
}
