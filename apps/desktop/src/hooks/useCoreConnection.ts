// Polls the Core's GetServerCapabilities as a liveness probe so the
// status bar can show real connection state. A failed poll (Transport
// error → the daemon is down/unreachable) flips the state to
// "disconnected"; it recovers automatically on the next successful poll.
import { useQuery } from "@tanstack/react-query";
import { getServerCapabilities } from "../api/runtime";

/// "connecting" only on the very first probe (before any result), so the
/// indicator doesn't flash a false "unreachable" on launch.
export type CoreConnectionState = "connecting" | "connected" | "disconnected";

export function useCoreConnection(): { state: CoreConnectionState } {
  const query = useQuery({
    queryKey: ["core-connection"],
    queryFn: getServerCapabilities,
    refetchInterval: 5000,
    refetchOnWindowFocus: true,
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
  // pending = first probe in flight (no result yet); success = last poll
  // reached the Core; error = last poll failed (daemon down/unreachable).
  const state: CoreConnectionState =
    query.status === "pending"
      ? "connecting"
      : query.status === "success"
        ? "connected"
        : "disconnected";
  return { state };
}
