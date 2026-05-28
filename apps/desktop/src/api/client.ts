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
  | "Workareas.GetWorkarea"
  | "Sessions.ListSessions";

export async function callRpc<TRequest, TResponse>(
  method: RpcMethod,
  payload: TRequest,
): Promise<TResponse> {
  return invoke<TResponse>("concerto_rpc", { method, payload });
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

export async function onConcertoEvent<T>(
  subject: string,
  callback: (payload: T) => void,
): Promise<UnlistenFn> {
  return listen<T>(`concerto/${subject}`, (event) => callback(event.payload));
}
