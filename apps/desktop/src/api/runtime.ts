// Typed wrapper for the Runtime service RPCs.
import { callRpc } from "./client";

/// Opaque for now — we only use a successful response as a liveness
/// signal for the connection indicator. Shape can be typed later.
export type ServerCapabilities = Record<string, unknown>;

export async function getServerCapabilities(): Promise<ServerCapabilities> {
  return callRpc<Record<string, never>, ServerCapabilities>(
    "Runtime.GetServerCapabilities",
    {},
  );
}
