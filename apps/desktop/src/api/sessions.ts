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

/// Convert a `number[]` (or `Uint8Array`) carried in a stream payload
/// into a `Uint8Array` so xterm.js can write the raw bytes.
export function chunkToBytes(data: number[] | Uint8Array): Uint8Array {
  if (data instanceof Uint8Array) return data;
  return Uint8Array.from(data);
}
