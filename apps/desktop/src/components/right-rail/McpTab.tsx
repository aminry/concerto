// Right-rail MCP tab — lists MCP servers discovered for the personal
// scope (Task 35 surface). V0.1 keeps the panel read-only and limits
// the query to the personal scope; project-scope listings need a
// repository_id, which the right rail does not have wired here yet.

import { useMcpServers } from "../../hooks/useMcpServers";

export function McpTab(): JSX.Element {
  const query = useMcpServers();

  if (query.isLoading) {
    return <p className="text-xs text-faint p-3">Loading…</p>;
  }
  if (query.isError) {
    return (
      <p className="text-xs text-err p-3">
        Failed to load MCP servers: {String(query.error)}
      </p>
    );
  }
  const servers = query.data?.servers ?? [];
  if (servers.length === 0) {
    return (
      <p className="text-xs text-faint p-3">
        No MCP servers in the personal scope.
      </p>
    );
  }
  return (
    <ul className="p-2 space-y-1">
      {servers.map((s) => (
        <li
          key={`${s.scope}/${s.name}`}
          className="rounded border border-border bg-surface-2 px-2 py-1.5"
        >
          <div className="flex items-center gap-2">
            <span className="text-xs font-mono text-foreground truncate">
              {s.name}
            </span>
            <span className="ml-auto text-xs text-faint">{s.scope}</span>
          </div>
          <p className="mt-1 text-xs text-muted truncate font-mono">
            {s.command} {s.args.join(" ")}
          </p>
        </li>
      ))}
    </ul>
  );
}
