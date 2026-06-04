// Typed wrapper for the Runtime service RPCs.
import { callRpc } from "./client";

/// The transport the active Core is reached over, as carried on
/// `ServerCapabilities.transport_kind` (Task 201's `TransportKind` proto enum).
/// The wire value is the proto enum **ordinal** (an integer), so this is a
/// numeric enum matching `crates/proto/.../runtime.proto`:
///
///   TRANSPORT_KIND_UNSPECIFIED = 0
///   TRANSPORT_KIND_UDS         = 1  (co-located)
///   TRANSPORT_KIND_IROH        = 2  (split-host)
///   TRANSPORT_KIND_WSS_BRIDGE  = 3  (web via relay)
///
/// The renderer branches remote-mode affordances (Task 602) on this without
/// learning the transport mechanics. **FROZEN** — Task 219's UI + Task 601/602
/// consume this typing.
export enum TransportKind {
  Unspecified = 0,
  Uds = 1,
  Iroh = 2,
  WssBridge = 3,
}

/// Whether the active Core is reached over a remote (non-co-located) transport.
/// Co-located = UDS (or unspecified, which only occurs before the first
/// successful capability read). Split-host (Iroh) and web (WSS bridge) are
/// remote — the leaf where Task 602 hides local-only affordances.
export function isRemoteTransport(kind: TransportKind): boolean {
  return kind === TransportKind.Iroh || kind === TransportKind.WssBridge;
}

/// Typed `Runtime.GetServerCapabilities` response. Previously an opaque
/// `Record<string, unknown>` used only as a liveness signal; Task 218 types
/// `transport_kind` (and the adjacent host fields) so the renderer can branch.
/// Other fields stay loosely typed — they are not load-bearing for this task.
export type ServerCapabilities = {
  server_version?: string;
  schema_version?: string;
  optional_services?: string[];
  limits?: {
    max_concurrent_streams?: number;
    max_payload_bytes?: number;
  } | null;
  /// The transport the Core answered this connection over (Task 201).
  transport_kind?: TransportKind;
  /// The Core host OS (e.g. "macos" / "linux" / "windows").
  core_host_os?: string;
  /// The Core hostname (for the status bar's split-host label).
  core_hostname?: string;
};

export async function getServerCapabilities(): Promise<ServerCapabilities> {
  return callRpc<Record<string, never>, ServerCapabilities>(
    "Runtime.GetServerCapabilities",
    {},
  );
}
