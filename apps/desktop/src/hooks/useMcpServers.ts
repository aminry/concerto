// React Query hook for `Sessions.ListMcpServers`.
//
// Used by the right-rail MCP tab from Task 46. V0.1 queries the
// personal scope only (no repository_id); project-scope listings need a
// repository handle that the right rail doesn't have wired yet.

import { useQuery } from "@tanstack/react-query";

import { listMcpServers } from "../api/mcp";

export function useMcpServers() {
  return useQuery({
    queryKey: ["mcp-servers"] as const,
    queryFn: async () => listMcpServers(),
  });
}
