// Cross-device handoff state (Task 518; design/16 §3.12 — cross-device handoff).
// When you pick up another device you should land where you left off: the same
// screen, the same workarea/session, the same active Core. This module
// SERIALIZES the current navigation/session state into a compact, versioned
// "handoff token" (a base64url string) and RESTORES it on the other device.
//
// Tier-2 (this task) = the state ROUND-TRIP: serialize → token → restore →
// equal state, with version + corruption guards. Tier-3 = the real transport that
// carries the token between devices (Handoff/Continuity, a relayed channel, or a
// QR) — out of scope here; this module is transport-agnostic and only produces /
// consumes the token string.
import { base64ToBytes, bytesToBase64 } from "../pairing/secure-store";

/** The current handoff state — what the other device needs to resume. */
export interface HandoffState {
  /** The active Core id (so the other device opens the same session). */
  coreId: string;
  /** The route the user is on, e.g. "workspace/[id]" or "(tabs)/inbox". */
  route: string;
  /** Route params (e.g. `{ id: "ws_123" }`). String-valued (URL-safe). */
  params?: Record<string, string>;
  /** The focused session id, if a live session view is open. */
  sessionId?: string;
  /** The highest stream offset observed, so the resumed device replays from it. */
  sinceOffset?: string;
  /** When the state was captured (epoch ms) — lets the consumer drop stale tokens. */
  capturedAtMs: number;
}

/** The on-wire envelope (versioned for forward-compat). */
interface HandoffEnvelope {
  v: 1;
  s: HandoffState;
}

const HANDOFF_VERSION = 1 as const;

/** Thrown when a handoff token cannot be parsed / is the wrong version. */
export class HandoffParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "HandoffParseError";
  }
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** base64 → base64url (URL/QR-safe: no `+`, `/`, `=`). */
function toBase64Url(b64: string): string {
  return b64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** base64url → base64 (re-pad for the decoder). */
function fromBase64Url(b64url: string): string {
  const b64 = b64url.replace(/-/g, "+").replace(/_/g, "/");
  const pad = b64.length % 4 === 0 ? "" : "=".repeat(4 - (b64.length % 4));
  return b64 + pad;
}

/**
 * Serialize a [`HandoffState`] into a compact, URL/QR-safe handoff token. The
 * inverse of [`restoreHandoff`]. Pure — no IO.
 */
export function serializeHandoff(state: HandoffState): string {
  const envelope: HandoffEnvelope = { v: HANDOFF_VERSION, s: state };
  const json = JSON.stringify(envelope);
  return toBase64Url(bytesToBase64(encoder.encode(json)));
}

/**
 * Restore a [`HandoffState`] from a handoff token produced by
 * [`serializeHandoff`]. Throws [`HandoffParseError`] on a corrupt / wrong-version
 * / missing-field token (never returns a partial state).
 */
export function restoreHandoff(token: string): HandoffState {
  let json: string;
  try {
    json = decoder.decode(base64ToBytes(fromBase64Url(token.trim())));
  } catch {
    throw new HandoffParseError("handoff token is not valid base64url");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    throw new HandoffParseError("handoff token is not valid JSON");
  }
  if (typeof parsed !== "object" || parsed === null) {
    throw new HandoffParseError("handoff token is not an object");
  }
  const env = parsed as Partial<HandoffEnvelope>;
  if (env.v !== HANDOFF_VERSION) {
    throw new HandoffParseError(`unsupported handoff version: ${String(env.v)}`);
  }
  const s = env.s;
  if (
    typeof s !== "object" ||
    s === null ||
    typeof s.coreId !== "string" ||
    typeof s.route !== "string" ||
    typeof s.capturedAtMs !== "number"
  ) {
    throw new HandoffParseError("handoff state is missing required fields");
  }
  // Re-narrow optional fields defensively.
  const state: HandoffState = {
    coreId: s.coreId,
    route: s.route,
    capturedAtMs: s.capturedAtMs,
    // params is Tier-3 (untrusted): a corrupt token can carry arrays or
    // non-string values. Keep only string-valued entries; drop arrays entirely
    // so HandoffState.params honors its Record<string,string> contract.
    ...((() => {
      const p = s.params;
      if (!p || typeof p !== "object" || Array.isArray(p)) return {};
      const entries = Object.entries(p).filter(([, v]) => typeof v === "string");
      return entries.length ? { params: Object.fromEntries(entries) } : {};
    })()),
    ...(typeof s.sessionId === "string" ? { sessionId: s.sessionId } : {}),
    ...(typeof s.sinceOffset === "string" ? { sinceOffset: s.sinceOffset } : {}),
  };
  return state;
}

/** Options for [`isHandoffFresh`]. */
export interface FreshnessOptions {
  /** Max age in ms before a token is considered stale (default 5 min). */
  maxAgeMs?: number;
  /** `now` override (tests). Default `Date.now()`. */
  now?: () => number;
}

/** Whether a restored handoff state is recent enough to act on (default 5 min). */
export function isHandoffFresh(state: HandoffState, opts: FreshnessOptions = {}): boolean {
  const maxAgeMs = opts.maxAgeMs ?? 5 * 60 * 1000;
  const now = (opts.now ?? Date.now)();
  return now - state.capturedAtMs <= maxAgeMs && state.capturedAtMs <= now;
}
