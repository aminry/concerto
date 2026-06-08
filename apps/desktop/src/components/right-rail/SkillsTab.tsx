// Right-rail Skills tab — lists discovered skills for the active workspace
// (Task 39 surface). V0.1 is read-only; enable/disable + refresh controls
// land in the settings polish task.

import { formatError } from "../../api/errors";
import { useUiStore } from "../../state/useUiStore";
import { useSkills } from "../../hooks/useSkills";

export function SkillsTab(): JSX.Element {
  const workspaceId = useUiStore((s) => s.selectedWorkspaceId);
  const query = useSkills(workspaceId);

  if (!workspaceId) {
    return (
      <p className="text-xs text-faint p-3">
        Select a workspace to see its skills.
      </p>
    );
  }
  if (query.isLoading) {
    return <p className="text-xs text-faint p-3">Loading…</p>;
  }
  if (query.isError) {
    return (
      <p className="text-xs text-err p-3">
        Failed to load skills: {formatError(query.error)}
      </p>
    );
  }
  const skills = query.data?.skills ?? [];
  if (skills.length === 0) {
    return <p className="text-xs text-faint p-3">No skills discovered.</p>;
  }
  return (
    <ul className="p-2 space-y-1">
      {skills.map((s) => (
        <li
          key={s.id}
          className="rounded border border-border bg-surface-2 px-2 py-1.5"
        >
          <div className="flex items-center gap-2">
            <span
              className={`inline-block h-2 w-2 rounded-full ${
                s.enabled ? "bg-accent" : "bg-faint"
              }`}
              aria-label={s.enabled ? "enabled" : "disabled"}
            />
            <span className="text-xs font-mono text-foreground truncate">
              {s.name}
            </span>
            <span className="ml-auto text-xs text-faint">{s.scope}</span>
          </div>
          {s.description && (
            <p className="mt-1 text-xs text-muted truncate">
              {s.description}
            </p>
          )}
        </li>
      ))}
    </ul>
  );
}
