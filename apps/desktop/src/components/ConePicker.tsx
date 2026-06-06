// Sparse-cone picker for workarea creation (Task 322, design/02 §3.2/§3.5,
// design/15 §3.4).
//
// For each repo in the workspace, the user enters cone paths (one per
// line, or comma-separated — both accepted). As they type, a debounced
// `Repositories.EstimateConeSize` call (Task 305) shows live
// `(file_count, disk_size_bytes)` feedback. The size is labelled an
// ESTIMATE: 305's `disk_size_bytes` is a lower bound for a blobless clone
// (not-yet-fetched blobs read as size 0). A cone path the Core rejects
// (INVALID_ARGUMENT, "path not found in repo") surfaces inline next to
// that repo's input and does NOT block the other repos.
//
// Empty cone input for a repo ⇒ "use the inherited workspace/repo
// defaults" (the three-layer resolver, Task 302) — the picker sends
// nothing for that repo so the Core's seeded defaults stand. This is
// labelled so the user knows empty inherits.
//
// The picker only COLLECTS the choices; `WorkspaceDetail` threads them
// into `createWorkarea` (which applies them via `Repositories.SetCones`
// after create — see `api/workareas.ts`). `suggest_cones` / Maestro-driven
// auto-suggestion is P4 (Task 411) — this is the manual picker only.

import { useMemo } from "react";

import type { Repository } from "../api/repositories";
import { useConeEstimate, useDebouncedValue } from "../hooks/useConeEstimate";
import { formatError } from "../api/errors";

/// Parse a free-text cone field into normalized cone paths. Accepts
/// newline- and/or comma-separated entries; trims whitespace and drops
/// empties. Forward-slash, repo-root-relative paths (git normalizes
/// separators) — no client-side validation of existence (the Core's
/// `EstimateConeSize`/`SetCones` is authoritative, surfacing a clean
/// reject for a missing path).
export function parseConePaths(raw: string): string[] {
  return raw
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/// The picker's output: one entry per repo whose cone field is non-empty.
export type ConeSelection = {
  repository_id: string;
  cone_paths: string[];
};

export type ConePickerProps = {
  repos: Repository[];
  /// Current raw text per repo id (controlled by the parent so the values
  /// survive a parent re-render / dialog state).
  values: Record<string, string>;
  onChange: (repositoryId: string, raw: string) => void;
};

/// Collect the non-empty cone selections from the raw values. Repos with
/// an empty/blank field are omitted (⇒ inherit defaults).
export function coneSelections(
  repos: Repository[],
  values: Record<string, string>,
): ConeSelection[] {
  const out: ConeSelection[] = [];
  for (const r of repos) {
    const paths = parseConePaths(values[r.id] ?? "");
    if (paths.length > 0) {
      out.push({ repository_id: r.id, cone_paths: paths });
    }
  }
  return out;
}

export function ConePicker({
  repos,
  values,
  onChange,
}: ConePickerProps): JSX.Element {
  return (
    <div className="space-y-3">
      <p className="text-xs text-faint">
        Set a sparse cone per repository (one path per line). Leave a repo
        blank to inherit the workspace/repo defaults.
      </p>
      {repos.map((repo) => (
        <RepoConeRow
          key={repo.id}
          repo={repo}
          value={values[repo.id] ?? ""}
          onChange={(raw) => onChange(repo.id, raw)}
        />
      ))}
    </div>
  );
}

function RepoConeRow({
  repo,
  value,
  onChange,
}: {
  repo: Repository;
  value: string;
  onChange: (raw: string) => void;
}): JSX.Element {
  const conePaths = useMemo(() => parseConePaths(value), [value]);
  // Debounce the parsed paths so each keystroke doesn't fire an RPC. The
  // join/split round-trip keeps the dependency a stable primitive for the
  // debounce + query key.
  const debouncedKey = useDebouncedValue(conePaths.join("\n"), 300);
  const debouncedPaths = useMemo(
    () => (debouncedKey.length > 0 ? debouncedKey.split("\n") : []),
    [debouncedKey],
  );

  const estimate = useConeEstimate(repo.id, debouncedPaths);

  return (
    <div className="rounded-md border border-border p-2 space-y-1.5">
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-mono text-foreground truncate">
          {repo.name}
        </span>
        <ConeStatsLabel
          loading={estimate.isFetching}
          error={estimate.isError ? formatError(estimate.error) : null}
          fileCount={estimate.data?.file_count ?? null}
          diskBytes={estimate.data?.disk_size_bytes ?? null}
        />
      </div>
      <textarea
        aria-label={`Cone paths for ${repo.name}`}
        rows={2}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="e.g. src/, packages/api  (blank = inherit defaults)"
        className="w-full rounded-md border border-border-strong bg-background px-2 py-1 text-xs font-mono text-foreground focus:outline-none focus-visible:ring-2 focus-visible:ring-accent resize-y"
      />
      {estimate.isError && (
        <p role="alert" className="text-xs text-err">
          {formatError(estimate.error)}
        </p>
      )}
    </div>
  );
}

function ConeStatsLabel({
  loading,
  error,
  fileCount,
  diskBytes,
}: {
  loading: boolean;
  error: string | null;
  fileCount: number | null;
  diskBytes: number | null;
}): JSX.Element {
  if (loading) {
    return <span className="text-xs text-faint">estimating…</span>;
  }
  if (error) {
    // The inline <p role="alert"> below carries the detail; keep the
    // header label terse.
    return <span className="text-xs text-err">invalid cone</span>;
  }
  if (fileCount == null) {
    return <span className="text-xs text-faint">—</span>;
  }
  return (
    <span className="text-xs text-faint font-mono whitespace-nowrap">
      {fileCount.toLocaleString()} file{fileCount === 1 ? "" : "s"} ·{" "}
      ~{formatBytes(diskBytes ?? 0)} <span className="opacity-70">(est.)</span>
    </span>
  );
}

/// Order-of-magnitude byte formatter for the cone-size estimate.
export function formatBytes(bytes: number): string {
  if (bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const exp = Math.min(
    units.length - 1,
    Math.floor(Math.log(bytes) / Math.log(1024)),
  );
  const value = bytes / Math.pow(1024, exp);
  return `${value >= 10 || exp === 0 ? Math.round(value) : value.toFixed(1)} ${units[exp]}`;
}
