// Thin wrapper around the `concerto_rpc` Tauri command.
//
// Every server interaction goes through this module. The renderer
// has no permission to speak gRPC, HTTP, or filesystem APIs
// directly — Tauri capabilities (`src-tauri/capabilities/main.json`)
// enforce that boundary. Per `design/15 §3.2`, all renderer → Core
// traffic flows through the typed `<Service>.<Rpc>` method strings.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type RpcMethod =
  | "Runtime.GetServerCapabilities"
  | "Projects.ListProjects"
  | "Workspaces.ListWorkspaces"
  | "Workspaces.GetWorkspace"
  | "Workspaces.CreateWorkspace"
  | "Workareas.GetWorkarea"
  | "Workareas.ListWorkareas"
  | "Workareas.CreateWorkarea"
  | "Workareas.GetWorkareaRepoDiff"
  | "Repositories.AddRepository"
  | "Repositories.ListByProject"
  | "Sessions.ListSessions"
  | "Sessions.GetSession"
  | "Sessions.CreateSession"
  | "Sessions.SendMessage"
  | "Sessions.StopSession"
  | "Sessions.DeleteSession"
  | "Sessions.ListMcpServers"
  | "Schedules.ListSchedules"
  | "Skills.ListSkills";

export async function callRpc<TRequest, TResponse>(
  method: RpcMethod,
  payload: TRequest,
): Promise<TResponse> {
  return invoke<TResponse>("concerto_rpc", { method, payload });
}

/// Render an error from `callRpc`/`invoke` as a human-readable string.
/// The Tauri shell's `CoreClientError` derives `serde::Serialize`, so a
/// rejected `invoke` surfaces a tagged object (e.g. `{ Rpc: "rpc: …" }`,
/// `{ Transport: "…" }`, `{ NotImplemented: "…" }`) rather than a string —
/// `String(e)` on that yields the useless "[object Object]". Pull the
/// inner message out; fall back to JSON, then String.
export function errorMessage(e: unknown): string {
  if (e == null) return "Unknown error";
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (typeof e === "object") {
    const firstString = Object.values(e as Record<string, unknown>).find(
      (v) => typeof v === "string",
    );
    if (typeof firstString === "string") return firstString;
    try {
      return JSON.stringify(e);
    } catch {
      // fall through to String()
    }
  }
  return String(e);
}

export async function subscribe(
  subject: string,
  filter?: string,
): Promise<string> {
  return invoke<string>("concerto_subscribe", { subject, filter });
}

export async function unsubscribe(id: string): Promise<void> {
  await invoke<void>("concerto_unsubscribe", { id });
}

/// Map a Core subject to its Tauri event-channel name. Tauri v2 rejects
/// event names containing '.' (only alphanumerics and `-`, `/`, `:`, `_`
/// are permitted), but Core subjects are dotted (`session.io.<id>`). The
/// subject keeps its dots for the gRPC stream filter; only the Tauri
/// channel name is sanitized. The shell's `concerto_subscribe` forwarder
/// applies the identical '.' -> ':' mapping so `listen` and `emit` agree.
function eventChannel(subject: string): string {
  return `concerto/${subject.replace(/\./g, ":")}`;
}

export async function onConcertoEvent<T>(
  subject: string,
  callback: (payload: T) => void,
): Promise<UnlistenFn> {
  return listen<T>(eventChannel(subject), (event) => callback(event.payload));
}

/// Probe `PATH` for `name`. Returns the absolute path string when the
/// binary is found, or null when it isn't. The Tauri shell only runs
/// `which`; Windows fallback (`where`) lands when Desktop ships
/// cross-platform.
export async function checkCommand(name: string): Promise<string | null> {
  return invoke<string | null>("check_command", { name });
}

/// Trigger a server-streaming `Repositories.Clone` and forward each
/// `CloneProgress` frame to the renderer via the
/// `concerto/clone-progress/<id>` event bus. The promise resolves once
/// the stream terminates (cleanly or with `done: true`).
export async function cloneRepository(
  repositoryId: string,
): Promise<{ done: boolean }> {
  return invoke<{ done: boolean }>("clone_repository", {
    payload: { repository_id: repositoryId },
  });
}

export async function onCloneProgress(
  repositoryId: string,
  callback: (payload: CloneProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<CloneProgressEvent>(
    `concerto/clone-progress/${repositoryId}`,
    (event) => callback(event.payload),
  );
}

/// Mirrors `concerto.v1.CloneProgress`. Prost-serde keeps the proto's
/// snake_case naming on the wire.
export type CloneProgressEvent = {
  phase: string;
  objects_received: number;
  total_objects: number;
  bytes_received: number;
  done: boolean;
};
