// Typed wrappers around the `Sessions.*` RPCs and the `Streams`
// subscription subjects (`session.events.<sid>` and
// `session.io.<sid>`) used by the terminal panel from Task 26.
//
// Bytes encoding: prost-serde serializes `bytes` fields as a JSON
// array of u8 (serde's default). Renderer-side code sends `payload`
// as `number[]` and receives `data` as `number[]`. No base64 hop.

import { callRpc } from "./client";

/// Mirrors `concerto.v1.Session`. `agent_kind` and `status` are
/// stringly-typed at the wire per the proto (Task 23 locked the
/// shape). Timestamps land as `[seconds, nanos]` tuples per the
/// shared serde shim.
export type Session = {
  id: string;
  workarea_id: string;
  chat_id: string;
  agent_kind: string;
  agent_version?: string | null;
  model?: string | null;
  // status ∈ { starting | running | awaiting | finished | crashed }
  status: string;
  permission_mode?: number | null;
  started_at?: [number, number] | null;
  ended_at?: [number, number] | null;
};

export type ListSessionsResponse = {
  sessions: Session[];
};

export async function listSessions(
  workareaId: string,
): Promise<ListSessionsResponse> {
  return callRpc<{ workarea_id: string }, ListSessionsResponse>(
    "Sessions.ListSessions",
    { workarea_id: workareaId },
  );
}

export async function getSession(id: string): Promise<Session> {
  return callRpc<{ id: string }, Session>("Sessions.GetSession", { id });
}

export async function createSession(input: {
  workareaId: string;
  agentKind: string;
  model?: string;
  permissionMode?: number;
}): Promise<Session> {
  return callRpc<
    {
      workarea_id: string;
      agent_kind: string;
      model?: string;
      permission_mode?: number;
    },
    Session
  >("Sessions.CreateSession", {
    workarea_id: input.workareaId,
    agent_kind: input.agentKind,
    model: input.model,
    permission_mode: input.permissionMode,
  });
}

/// Send raw bytes to the agent's stdin via `Sessions.SendMessage`.
/// The Tauri shell forwards verbatim — V0.1 does no parsing.
export async function sendMessage(
  sessionId: string,
  payload: Uint8Array,
): Promise<void> {
  await callRpc<
    { session_id: string; payload: number[] },
    null
  >("Sessions.SendMessage", {
    session_id: sessionId,
    // serde's default `Vec<u8>` deserialiser accepts a JSON array of
    // small integers. Passing `Array.from(uint8)` keeps the wire
    // shape symmetric with the array-of-u8 we receive on
    // `session.io.<sid>`.
    payload: Array.from(payload),
  });
}

export async function stopSession(
  sessionId: string,
  reason = "user_request",
): Promise<void> {
  await callRpc<{ session_id: string; reason: string }, null>(
    "Sessions.StopSession",
    { session_id: sessionId, reason },
  );
}

/// Destructive: stops the session if running, then permanently deletes
/// it server-side (chat thread, approvals, checkpoints). The Core's
/// `DeleteSession` takes a `SessionId { value }`; the shell maps the
/// `{ id }` payload to it (same convention as GetSession).
export async function deleteSession(sessionId: string): Promise<void> {
  await callRpc<{ id: string }, null>("Sessions.DeleteSession", {
    id: sessionId,
  });
}

/// Relay the xterm pane geometry to the agent's PTY so full-screen TUIs
/// render at the size the user sees. Sent on mount and on every resize.
export async function resizeSession(
  sessionId: string,
  rows: number,
  cols: number,
): Promise<void> {
  await callRpc<
    { session_id: string; rows: number; cols: number },
    null
  >("Sessions.ResizeSession", { session_id: sessionId, rows, cols });
}

/// Task 33: the four legal user-initiated values for a pending tool-approval
/// gate. Mirrors `concerto.v1.ApprovalDecision` (sessions.proto:96) — the
/// `auto_*` resolver values are server-written and never sent by a client.
/// prost-serde serializes a proto enum as its integer tag on the wire.
export const ApprovalDecision = {
  UNSPECIFIED: 0,
  APPROVE: 1,
  APPROVE_ONCE: 2,
  DENY: 3,
} as const;

export type ApprovalDecisionValue =
  (typeof ApprovalDecision)[keyof typeof ApprovalDecision];

/// Mirrors `concerto.v1.AwaitingApproval` (streams.proto:286, Task 33/43).
/// Surfaced on `session.events.<sid>` as the `awaiting_approval` oneof variant
/// when an agent pauses for a write-tool gate. `urgent`/`destructive_label`
/// (fields 5/6, FROZEN at Task 43) drive the red-urgent rendering. Task 415's
/// Maestro confirmation chip renders this exact shape.
export type AwaitingApproval = {
  approval_id: string;
  tool: string;
  summary: string;
  payload_json: string;
  urgent?: boolean;
  destructive_label?: string | null;
};

/// Resolve a pending tool-approval gate via `Sessions.ResolveApproval`
/// (Task 33). The server validates the approval is still pending
/// (first-write-wins), persists the decision, and injects the matching
/// accept/deny bytes into the agent's stdin. Task 415's Maestro write-tool
/// confirmation chip resolves through this same path — no new RPC, no bypass
/// (design/08 R-2).
export async function resolveApproval(
  sessionId: string,
  approvalId: string,
  decision: ApprovalDecisionValue,
): Promise<void> {
  await callRpc<
    { session_id: string; approval_id: string; decision: number },
    null
  >("Sessions.ResolveApproval", {
    session_id: sessionId,
    approval_id: approvalId,
    decision,
  });
}

/// Shape of an `Event` frame emitted under `concerto/session.io.<sid>`.
/// Prost-serde's oneof representation puts the variant under `body`
/// keyed by the proto field name. `session_io` carries
/// `SessionIoChunk { session_id, stream, data }`. `data` is a JSON
/// array of u8 per serde's default `Vec<u8>` serialisation.
export type StreamEvent =
  | {
      offset: number;
      at?: [number, number] | null;
      body?: { session_io: SessionIoChunkPayload };
    }
  | {
      offset: number;
      at?: [number, number] | null;
      body?: { session: SessionEventPayload };
    }
  | {
      offset: number;
      at?: [number, number] | null;
      body?: { workarea: WorkareaEventPayload };
    }
  | {
      offset: number;
      at?: [number, number] | null;
      body?: { workspace: WorkspaceEventPayload };
    };

export type SessionIoChunkPayload = {
  session_id: string;
  stream: string;
  data: number[];
};

/// `SessionEvent.kind` is a oneof of three V0.1 variants; the proto
/// field name keys the payload object.
export type SessionEventPayload = {
  session_id: string;
  kind?:
    | { started: { model: string; mode: string } }
    | { message: { role: string; content: number[] } }
    | { exited: { exit_code?: number | null } };
};

export type WorkareaEventPayload = { workarea_id: string; kind: string };
export type WorkspaceEventPayload = { workspace_id: string; kind: string };

/// Read the active variant of a proto `oneof` from its JSON form.
///
/// prost's serde derive serializes a oneof variant under a key named
/// after the Rust enum variant — i.e. PascalCase (`SessionIo`, `Exited`)
/// — NOT the snake_case proto field name. The renderer was written
/// against snake_case, which silently dropped every live event (no agent
/// output in the terminal, no live status). Accept both spellings so we
/// match the actual wire and stay resilient if the proto serde config is
/// ever changed to rename. Returns the first present key's value.
export function oneofVariant<T>(obj: unknown, ...keys: string[]): T | undefined {
  if (obj && typeof obj === "object") {
    const rec = obj as Record<string, unknown>;
    for (const k of keys) {
      if (k in rec) return rec[k] as T;
    }
  }
  return undefined;
}

/// Convert a `number[]` (or `Uint8Array`) carried in a stream payload
/// into a `Uint8Array` so xterm.js can write the raw bytes.
export function chunkToBytes(data: number[] | Uint8Array): Uint8Array {
  if (data instanceof Uint8Array) return data;
  return Uint8Array.from(data);
}
