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
  | "Projects.CreateProject"
  | "Workspaces.ListWorkspaces"
  | "Workspaces.GetWorkspace"
  | "Workspaces.CreateWorkspace"
  | "Workareas.GetWorkarea"
  | "Workareas.ListWorkareas"
  | "Workareas.CreateWorkarea"
  | "Workareas.GetWorkareaRepoDiff"
  | "Repositories.AddRepository"
  | "Repositories.ListByProject"
  // Task 322 — sparse-cone picker (Task 305 EstimateConeSize telemetry +
  // Task 302 SetCones per-(workarea, repo) cone setter). The strings match
  // the Rust shell dispatch table (`<Service>.<Rpc>`) exactly.
  | "Repositories.EstimateConeSize"
  | "Repositories.SetCones"
  | "Sessions.ListSessions"
  | "Sessions.GetSession"
  | "Sessions.CreateSession"
  | "Sessions.SendMessage"
  | "Sessions.StopSession"
  | "Sessions.DeleteSession"
  | "Sessions.ResizeSession"
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
/// The Tauri shell's `CoreClientError` derives `serde::Serialize` with
/// `#[serde(tag = "kind", content = "message")]`, so a rejected `invoke`
/// surfaces an adjacently-tagged object: `{ kind: "Rpc", message: "rpc: …" }`
/// (likewise `Transport` / `NotImplemented`). `String(e)` on that yields the
/// useless "[object Object]".
///
/// Read the `message` field directly. The previous implementation grabbed
/// the FIRST string value via `Object.values().find()`, which is `kind`
/// ("Rpc") — discarding the actual error text and surfacing a bare,
/// undebuggable "Rpc" to the user. Fall back to JSON, then String.
export function errorMessage(e: unknown): string {
  if (e == null) return "Unknown error";
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (typeof e === "object") {
    const obj = e as { message?: unknown };
    if (typeof obj.message === "string") return obj.message;
    // Pre-tagging fallback: some errors may still be a flat
    // `{ Variant: "msg" }` map — return the first string value.
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

/// Map a gRPC subject (which uses dots, e.g. `session.io.<sid>`) to the
/// Tauri event-bus name. Tauri 2 rejects event names containing '.', so
/// dots become slashes (`concerto/session/io/<sid>`). MUST stay in sync
/// with the Rust side in `src-tauri/src/commands.rs`
/// (`concerto_subscribe`). `split`/`join` avoids relying on
/// `String.replaceAll` (ES2021) for broad target compatibility.
function eventNameForSubject(subject: string): string {
  return `concerto/${subject.split(".").join("/")}`;
}

export async function onConcertoEvent<T>(
  subject: string,
  callback: (payload: T) => void,
): Promise<UnlistenFn> {
  return listen<T>(eventNameForSubject(subject), (event) =>
    callback(event.payload),
  );
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
