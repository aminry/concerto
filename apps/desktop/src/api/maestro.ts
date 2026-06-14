// Typed binding for the Maestro chat surface (Task 415) — the always-present
// "Concerto chat" top bar that dispatches, routes prompts to workareas, and
// renders a digest (design/08 §1/§3.6).
//
// This module is a WIRE-FROZEN CONSUMER, not a wire author. Every type below
// is a hand-mirror of the proto FROZEN by Task 401.5
// (`crates/proto/proto/concerto/v1/maestro.proto`, PHASE4_PLANNING §4.2). It
// adds NO proto, NO migration, NO `src-tauri` Rust. The live `Maestro.*` shell
// dispatch arm + real data are Task 414's job; until then the renderer drives
// these against a mocked `@tauri-apps/api` `invoke` double (Tier-2 — see
// `maestro.test.ts`).
//
// prost-serde keeps the proto's snake_case field names on the wire, so the
// mirror types use snake_case verbatim. All timestamps are `int64` unix-epoch
// MILLISECONDS as plain JS numbers (401.5 froze `generated_at_ms` /
// `created_at_ms` / `last_digest_at_ms` as `int64`) — NOT `[seconds, nanos]`
// tuples and NOT `google.protobuf.Timestamp`.

import { callRpc, onConcertoEvent } from "./client";
import { oneofVariant } from "./sessions";
import type { UnlistenFn } from "@tauri-apps/api/event";

// ── Wire mirror types (FROZEN by Task 401.5, maestro.proto) ──────────────────

/// Mirrors `concerto.v1.MaestroChip` (maestro.proto:67). 401.5's LOCAL copy of
/// `suggestions.proto`'s `Chip` (Task 07): same six fields + field numbers, but
/// RENAMED to `MaestroChip` because proto3's package namespace is flat — a
/// second `concerto.v1.Chip` would collide with the suggestions `Chip`. So
/// 409's digest chips map 1:1 onto this shape. `created_at_ms` is unix ms.
export type MaestroChip = {
  rule_id: string; // = 1
  workarea_id?: string; // = 2 (proto3 string default "")
  title: string; // = 3
  priority: number; // = 4 (int32)
  created_at_ms?: number; // = 5 (int64 unix ms)
  action?: string; // = 6 (free-form, mirrors Chip.action precedent)
};

/// Mirrors `concerto.v1.Digest` (maestro.proto:53) as FROZEN by Task 401.5.
/// The Finished/Blocked/Still-working grouping is TEXTUAL — it lives inside
/// `text` (the LLM-grouped prose, design/08 §3.6), NOT as wire sub-messages;
/// there is no `DigestGroup` on the frozen wire. `generated_at_ms` is
/// `int64 generated_at_ms = 3` (unix epoch ms; NO google.protobuf.Timestamp).
export type Digest = {
  text: string; // = 1, the 3-5 sentence grouped digest body + one-line next step
  chips: MaestroChip[]; // = 2, persisted on the digest chat_messages row (D11)
  generated_at_ms?: number; // = 3 (int64 unix ms)
  stale?: boolean; // = 4, R-7: last-good digest shown with a stale badge when inert
};

/// Mirrors `concerto.v1.MaestroVisibility` (maestro.proto:84). prost-serde
/// serializes a proto enum as its integer tag on the wire.
export const MaestroVisibility = {
  UNSPECIFIED: 0,
  FULL: 1, // summaries visible
  HARD_FACTS_ONLY: 2, // exclude_from_maestro — name + hard facts only
} as const;

export type MaestroVisibilityValue =
  (typeof MaestroVisibility)[keyof typeof MaestroVisibility];

/// Mirrors `concerto.v1.MaestroAttachment` (maestro.proto:43). The V1.0
/// text-only seam (design/08 R-9): empty in V1.0, populated in V1.5.
export type MaestroAttachment = {
  kind: string; // e.g. "diff" | "commit_url" (V1.5)
  ref: string; // opaque reference resolved by the consumer
};

/// Mirrors `concerto.v1.MaestroState` — the read-model returned by
/// `Maestro.GetState`, FROZEN by Task 416 (maestro.proto, field numbers 1-9).
/// 416 assembles it from `MaestroHandle::get_state` (enabled / daily counters /
/// last_digest) + 412's `TokenBudget` caps (200K in / 50K out) + the handle's
/// inert reason + the Maestro singleton session id. All timestamps are `int64`
/// unix-ms plain numbers — NOT [seconds, nanos] tuples.
///
/// Field contract (416 Handoff): `last_digest_at_ms = 0` ⇒ never;
/// `inert_reason` ∈ `"" | "budget_exhausted" | "disabled_by_policy"`;
/// `maestro_session_id = ""` ⇒ no live Maestro session. Task 417 binds this
/// shape via `getState`, feeds it into `<BudgetBanner>`/`<DigestPanel>`, and
/// subscribes to `maestro_session_id`'s `session.events` for the
/// confirmation-chip producer.
export type MaestroState = {
  enabled: boolean; // = 1
  daily_in_today: number; // = 2 (i64)
  daily_out_today: number; // = 3 (i64)
  in_cap: number; // = 4 (i64; 200000)
  out_cap: number; // = 5 (i64; 50000)
  last_digest_at_ms?: number | null; // = 6 (i64 unix-ms; 0 ⇒ never)
  inert: boolean; // = 7
  inert_reason: string; // = 8 ("" | "budget_exhausted" | "disabled_by_policy")
  maestro_session_id: string; // = 9 ("" ⇒ no live Maestro session)
};

// ── RPC request/response shapes (FROZEN field names, maestro.proto) ──────────

/// Mirrors `concerto.v1.MaestroMessageRequest` (maestro.proto:36).
export type MaestroMessageRequest = {
  text: string; // = 1
  attachments: MaestroAttachment[]; // = 2 (empty in V1.0, R-9 seam)
  workspace_id?: string; // = 3 (Task 8 scope hint; absent ⇒ default workspace)
};

/// Mirrors `concerto.v1.VisibilityRequest` (maestro.proto:79).
export type VisibilityRequest = {
  workarea_id: string; // = 1
  visibility: number; // = 2 (MaestroVisibility enum tag)
};

// ── Bindings ─────────────────────────────────────────────────────────────────

/// Send the user's chat input to the Maestro (`Maestro.SendToMaestro`).
/// V1.0 is text-only (design/08 R-9); `attachments` is a frozen-but-empty
/// seam. `workspaceId` is the Task 8 scope hint — the active workspace the
/// composer is showing — so a bare `@composer` resolves there first; absent
/// ⇒ the Core falls back to the default (most-recent) workspace. Returns
/// `google.protobuf.Empty` (null on the wire).
export async function sendToMaestro(
  text: string,
  attachments: MaestroAttachment[] = [],
  workspaceId?: string,
): Promise<void> {
  await callRpc<MaestroMessageRequest, null>("Maestro.SendToMaestro", {
    text,
    attachments,
    // Omit the key entirely when there's no active workspace so the wire
    // stays `optional`-absent (Core default scope) rather than empty-string.
    ...(workspaceId ? { workspace_id: workspaceId } : {}),
  });
}

/// Fetch the digest rendered above the chat composer (`Maestro.GetDigest`,
/// design/08 §3.6). Task 414 wires the live handle; Task 409 generates the
/// content. The request message `GetDigestRequest` is empty.
export async function getDigest(): Promise<Digest> {
  return callRpc<Record<string, never>, Digest>("Maestro.GetDigest", {});
}

/// Read the live Maestro state (`Maestro.GetState`, FROZEN by Task 416): the
/// budget counters + caps (for `<BudgetBanner>`'s 80%-amber / 100%-red meter),
/// the inert state + reason (the stale badge + policy-disabled banner), the
/// last-digest cursor, and the Maestro singleton session id (which
/// `useMaestroConfirmations` subscribes to for write-tool `AwaitingApproval`
/// frames). The request message `GetStateRequest` is empty. When the Maestro
/// handle is policy-disabled at boot the Core rejects with
/// `failed_precondition("maestro.disabled_by_policy")` — the caller degrades to
/// the policy banner via the event path.
export async function getState(): Promise<MaestroState> {
  return callRpc<Record<string, never>, MaestroState>("Maestro.GetState", {});
}

/// Toggle a workarea's Maestro visibility (`Maestro.SetWorkareaVisibility`,
/// design/08 §3.3 privacy toggle). Task 413 enforces the summary blanking this
/// drives. Returns `google.protobuf.Empty`.
export async function setWorkareaVisibility(
  workareaId: string,
  visibility: MaestroVisibilityValue,
): Promise<void> {
  await callRpc<VisibilityRequest, null>("Maestro.SetWorkareaVisibility", {
    workarea_id: workareaId,
    visibility,
  });
}

// ── maestro.events subscription + decode ─────────────────────────────────────

/// The pub/sub subject the Maestro lifecycle events ride. Payloads arrive on
/// the opaque `Event.checks_opaque = 17` carrier (401.5 / D7 — NOT a new
/// `body` oneof arm; the oneof is frozen through field 16). The subject is
/// UNSCOPED (`maestro.events`, no `<wa>` segment), unlike `checks.<wa>.<repo>`.
export const MAESTRO_EVENTS_SUBJECT = "maestro.events";

/// The five Maestro lifecycle event kinds Task 414 publishes (design/08 §5.4).
/// `maestro.message` carries a chat line; `routing_executed` a routed-target
/// confirmation; `digest_generated` a fresh digest; `budget_exhausted` /
/// `disabled_by_policy` the banner triggers.
export type MaestroEvent =
  | { kind: "message"; text: string; role?: string }
  | { kind: "routing_executed"; targets: string[]; summary?: string }
  | { kind: "digest_generated"; digest?: Digest }
  | { kind: "budget_exhausted" }
  | { kind: "disabled_by_policy"; reason?: string }
  | { kind: "unknown"; raw: unknown };

/// Decode the bytes of an `Event.checks_opaque = 17` opaque frame into a typed
/// `MaestroEvent`. The frame is a JSON object 414 emits; we parse it
/// DEFENSIVELY (the live emitter is a sibling task not yet merged):
///   - the carrier may surface the opaque bytes as `checks_opaque` (PascalCase
///     `ChecksOpaque`), or the shell may already have decoded it to an object;
///   - the inner discriminator may be PascalCase (prost serde default,
///     `Message`/`RoutingExecuted`) or snake_case (`message`/`routing_executed`),
///     read via the `oneofVariant` dual-spelling helper;
///   - any unrecognized shape degrades to `{ kind: "unknown", raw }` rather
///     than throwing, so a future 414 frame addition never crashes the chat.
///
/// 414 must emit a frame whose decoded JSON matches one of the discriminated
/// shapes below (the `kind`-bearing object). This is the contract 414's live
/// emitter satisfies; see Handoff.
export function decodeMaestroEvent(payload: unknown): MaestroEvent {
  const frame = extractMaestroFrame(payload);
  if (frame == null || typeof frame !== "object") {
    return { kind: "unknown", raw: payload };
  }
  const obj = frame as Record<string, unknown>;

  const message = oneofVariant<{ text?: unknown; role?: unknown }>(
    obj,
    "Message",
    "message",
    "maestro_message",
    "MaestroMessage",
  );
  if (message) {
    return {
      kind: "message",
      text: asString(message.text),
      role: optString(message.role),
    };
  }

  const routing = oneofVariant<{ targets?: unknown; summary?: unknown }>(
    obj,
    "RoutingExecuted",
    "routing_executed",
  );
  if (routing) {
    return {
      kind: "routing_executed",
      targets: asStringArray(routing.targets),
      summary: optString(routing.summary),
    };
  }

  const digestGen = oneofVariant<{ digest?: unknown }>(
    obj,
    "DigestGenerated",
    "digest_generated",
  );
  if (digestGen) {
    return {
      kind: "digest_generated",
      digest: isDigest(digestGen.digest) ? digestGen.digest : undefined,
    };
  }

  if ("budget_exhausted" in obj || "BudgetExhausted" in obj) {
    return { kind: "budget_exhausted" };
  }

  const disabled = oneofVariant<{ reason?: unknown }>(
    obj,
    "DisabledByPolicy",
    "disabled_by_policy",
  );
  if (disabled) {
    return { kind: "disabled_by_policy", reason: optString(disabled.reason) };
  }

  return { kind: "unknown", raw: payload };
}

/// Pull the inner Maestro frame out of an `Event` carrier. The opaque payload
/// rides `Event.checks_opaque = 17` (dual-spelled `checks_opaque` /
/// `ChecksOpaque`); the shell may forward it either as already-decoded JSON or
/// as a `number[]`/string of bytes the renderer JSON-parses. If the payload is
/// already a flat `kind`-bearing object (e.g. a test feeds the inner frame
/// directly), return it as-is.
function extractMaestroFrame(payload: unknown): unknown {
  if (payload == null) return null;

  // An `Event` envelope carrying the opaque field.
  const carrier = oneofVariant<unknown>(payload, "checks_opaque", "ChecksOpaque");
  const raw = carrier ?? payload;

  // Bytes as a `number[]` (serde's default `Vec<u8>`): decode to a UTF-8 string
  // then JSON-parse.
  if (Array.isArray(raw) && raw.every((n) => typeof n === "number")) {
    return parseJsonBytes(raw as number[]);
  }
  // Bytes as a base64/utf8 string.
  if (typeof raw === "string") {
    return safeJsonParse(raw);
  }
  return raw;
}

function parseJsonBytes(bytes: number[]): unknown {
  try {
    const text =
      typeof TextDecoder !== "undefined"
        ? new TextDecoder().decode(Uint8Array.from(bytes))
        : String.fromCharCode(...bytes);
    return safeJsonParse(text);
  } catch {
    return null;
  }
}

function safeJsonParse(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

function isDigest(value: unknown): value is Digest {
  return (
    !!value &&
    typeof value === "object" &&
    typeof (value as Record<string, unknown>).text === "string"
  );
}

function asString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function optString(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((v): v is string => typeof v === "string");
}

/// Subscribe to `maestro.events` and forward each decoded `MaestroEvent` to
/// `callback`. Thin wrapper over `onConcertoEvent` (the dot→slash subject
/// mapping lives in `client.ts`). Components typically use the
/// `useEventSubscription` hook instead; this helper is the decode seam the
/// hook callback runs each frame through.
export async function onMaestroEvent(
  callback: (event: MaestroEvent) => void,
): Promise<UnlistenFn> {
  return onConcertoEvent<unknown>(MAESTRO_EVENTS_SUBJECT, (payload) =>
    callback(decodeMaestroEvent(payload)),
  );
}
