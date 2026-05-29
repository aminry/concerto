// Typed wrapper around `Sessions.ListMcpServers`. The MCP tab in the
// Task 46 right rail consumes this; the V1.0 write path
// (`UpsertProjectMcp`) is not wired here because the V0.1 surface is
// read-only.

import { callRpc } from "./client";

/// Mirrors `concerto.v1.McpServer`.
export type McpServer = {
  name: string;
  scope: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  source_path: string;
};

export type ListMcpResponse = {
  servers: McpServer[];
};

export async function listMcpServers(input?: {
  scope?: string;
  repositoryId?: string;
}): Promise<ListMcpResponse> {
  return callRpc<
    { scope?: string; repository_id?: string },
    ListMcpResponse
  >("Sessions.ListMcpServers", {
    scope: input?.scope,
    repository_id: input?.repositoryId,
  });
}
