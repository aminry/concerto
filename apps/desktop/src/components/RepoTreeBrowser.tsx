// Browsable repo-tree → editable per-repo sparse "default cone" picker
// (design/02 §3.2).
//
// A controlled, lazily-expanded directory tree of a repository. Each folder
// row carries an expand/collapse chevron + a checkbox; checking a folder
// ADDS its path to the cone (git cone mode includes the whole subtree
// recursively, so selecting `src` includes all of `src/**`). A folder whose
// ANCESTOR is already selected is shown checked+disabled ("included via
// parent"). Files (`is_dir=false`) are shown for context but are NOT
// checkable — cone mode selects directories.
//
// Children are fetched only when a folder is expanded (React Query keyed by
// `[repositoryId, path]`); the root loads on mount. The selection is kept
// MINIMAL: if a parent and a descendant are both selected, the redundant
// descendant is dropped (`normalizeConeSelection`).
//
// A live `EstimateConeSize` (Task 305, debounced) shows the file count + an
// order-of-magnitude size for the current selection. The selected paths are
// rendered as a removable summary.

import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, File, Folder } from "lucide-react";

import { listTree, type TreeEntry } from "../api/repositories";
import { formatError } from "../api/errors";
import { useConeEstimate, useDebouncedValue } from "../hooks/useConeEstimate";
import { formatBytes } from "./ConePicker";

// ── pure helpers (exported for unit tests) ─────────────────────────────

/// True iff `path` is implicitly included because a STRICT ancestor of it is
/// in `selected` (git cone mode includes the whole subtree). `path` itself
/// being selected is NOT "implied by an ancestor" — only a proper ancestor
/// counts.
export function isImpliedByAncestor(path: string, selected: string[]): boolean {
  return selected.some(
    (sel) => sel !== path && path.startsWith(`${sel}/`),
  );
}

/// Normalize a cone selection to the MINIMAL set: drop any path that is a
/// descendant of another selected path (cone mode already includes the
/// subtree), and de-duplicate. Order is preserved (first occurrence wins).
export function normalizeConeSelection(paths: string[]): string[] {
  const deduped = Array.from(new Set(paths));
  return deduped.filter((p) => !isImpliedByAncestor(p, deduped));
}

/// Add `path` to `selected` (then re-normalize so a newly-checked parent
/// drops its now-redundant already-checked children).
export function addToSelection(path: string, selected: string[]): string[] {
  return normalizeConeSelection([...selected, path]);
}

/// Remove `path` from `selected` (exact match only — removing a parent does
/// not re-expand its implicit children into explicit entries).
export function removeFromSelection(path: string, selected: string[]): string[] {
  return selected.filter((p) => p !== path);
}

// ── component ──────────────────────────────────────────────────────────

export type RepoTreeBrowserProps = {
  repositoryId: string;
  /// Selected cone directories (the minimal set). Controlled by the parent.
  value: string[];
  onChange: (next: string[]) => void;
  /// Optional ref to list against (empty ⇒ repo default branch / HEAD).
  gitRef?: string;
};

export function RepoTreeBrowser({
  repositoryId,
  value,
  onChange,
  gitRef = "",
}: RepoTreeBrowserProps): JSX.Element {
  // Debounce the selection feeding the live size estimate so rapid checking
  // doesn't fire an RPC per click.
  const debouncedSelection = useDebouncedValue(value.join("\n"), 300);
  const estimatePaths = useMemo(
    () => (debouncedSelection.length > 0 ? debouncedSelection.split("\n") : []),
    [debouncedSelection],
  );
  const estimate = useConeEstimate(repositoryId, estimatePaths);

  return (
    <div className="space-y-3">
      <div
        className="max-h-72 overflow-auto rounded-md border border-border p-1"
        role="tree"
        aria-label="Repository directories"
      >
        <TreeLevel
          repositoryId={repositoryId}
          gitRef={gitRef}
          path=""
          depth={0}
          selected={value}
          onChange={onChange}
        />
      </div>

      <SelectedSummary
        selected={value}
        onRemove={(p) => onChange(removeFromSelection(p, value))}
      />

      <div className="text-xs text-faint">
        {value.length === 0 ? (
          <span>
            No directories selected — the whole repository tree will be
            checked out.
          </span>
        ) : estimate.isFetching ? (
          <span>estimating…</span>
        ) : estimate.isError ? (
          <span role="alert" className="text-err">
            {formatError(estimate.error)}
          </span>
        ) : estimate.data ? (
          <span className="font-mono">
            {estimate.data.file_count.toLocaleString()} file
            {estimate.data.file_count === 1 ? "" : "s"} · ~
            {formatBytes(estimate.data.disk_size_bytes)}{" "}
            <span className="opacity-70">(est.)</span>
          </span>
        ) : (
          <span>—</span>
        )}
      </div>
    </div>
  );
}

/// One level of the lazy tree: lists the children of `path` and renders a
/// row per entry. Expanding a folder mounts a nested `<TreeLevel>` for it.
function TreeLevel({
  repositoryId,
  gitRef,
  path,
  depth,
  selected,
  onChange,
}: {
  repositoryId: string;
  gitRef: string;
  path: string;
  depth: number;
  selected: string[];
  onChange: (next: string[]) => void;
}): JSX.Element {
  const query = useQuery({
    queryKey: ["repoTree", repositoryId, gitRef, path] as const,
    queryFn: () => listTree(repositoryId, path, gitRef),
    retry: false,
  });

  if (query.isLoading) {
    return <p className="px-2 py-1 text-xs text-faint">loading…</p>;
  }
  if (query.isError) {
    return (
      <p role="alert" className="px-2 py-1 text-xs text-err">
        {formatError(query.error)}
      </p>
    );
  }
  const entries = query.data?.entries ?? [];
  if (entries.length === 0) {
    return <p className="px-2 py-1 text-xs text-faint">empty</p>;
  }

  return (
    <ul className="space-y-0.5">
      {entries.map((entry) => (
        <TreeRow
          key={entry.path}
          repositoryId={repositoryId}
          gitRef={gitRef}
          entry={entry}
          depth={depth}
          selected={selected}
          onChange={onChange}
        />
      ))}
    </ul>
  );
}

function TreeRow({
  repositoryId,
  gitRef,
  entry,
  depth,
  selected,
  onChange,
}: {
  repositoryId: string;
  gitRef: string;
  entry: TreeEntry;
  depth: number;
  selected: string[];
  onChange: (next: string[]) => void;
}): JSX.Element {
  const [expanded, setExpanded] = useState(false);

  const isSelected = selected.includes(entry.path);
  const impliedByAncestor = isImpliedByAncestor(entry.path, selected);
  const checked = isSelected || impliedByAncestor;
  // A folder included via a selected ancestor is checked + locked (you
  // uncheck the ancestor to drop it).
  const disabled = !entry.is_dir || impliedByAncestor;

  const indent = { paddingLeft: `${depth * 1}rem` };

  function toggleCheck(): void {
    if (disabled) return;
    if (isSelected) {
      onChange(removeFromSelection(entry.path, selected));
    } else {
      onChange(addToSelection(entry.path, selected));
    }
  }

  return (
    <li role="treeitem" aria-expanded={entry.is_dir ? expanded : undefined}>
      <div className="flex items-center gap-1.5 py-0.5" style={indent}>
        {entry.is_dir ? (
          <button
            type="button"
            onClick={() => setExpanded((e) => !e)}
            className="text-faint hover:text-foreground"
            aria-label={expanded ? `Collapse ${entry.name}` : `Expand ${entry.name}`}
          >
            {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          </button>
        ) : (
          <span className="inline-block w-[14px]" />
        )}

        <input
          type="checkbox"
          checked={checked}
          disabled={disabled}
          onChange={toggleCheck}
          aria-label={
            impliedByAncestor
              ? `${entry.path} (included via parent)`
              : entry.path
          }
          title={impliedByAncestor ? "included via parent directory" : undefined}
          className="accent-accent disabled:opacity-50"
        />

        {entry.is_dir ? (
          <Folder size={14} className="text-faint shrink-0" />
        ) : (
          <File size={14} className="text-faint shrink-0" />
        )}
        <span
          className={`font-mono text-xs truncate ${
            entry.is_dir ? "text-foreground" : "text-faint"
          }`}
        >
          {entry.name}
        </span>
        {impliedByAncestor && (
          <span className="text-[10px] uppercase tracking-wide text-faint">
            via parent
          </span>
        )}
      </div>

      {entry.is_dir && expanded && (
        <TreeLevel
          repositoryId={repositoryId}
          gitRef={gitRef}
          path={entry.path}
          depth={depth + 1}
          selected={selected}
          onChange={onChange}
        />
      )}
    </li>
  );
}

function SelectedSummary({
  selected,
  onRemove,
}: {
  selected: string[];
  onRemove: (path: string) => void;
}): JSX.Element {
  return (
    <div className="space-y-1">
      <p className="text-xs uppercase tracking-wider text-faint">
        Selected directories
      </p>
      {selected.length === 0 ? (
        <p className="text-xs text-faint">None — whole repository.</p>
      ) : (
        <ul className="flex flex-wrap gap-1.5">
          {selected.map((p) => (
            <li
              key={p}
              className="inline-flex items-center gap-1 rounded border border-border px-1.5 py-0.5 text-xs font-mono text-foreground"
            >
              {p}
              <button
                type="button"
                onClick={() => onRemove(p)}
                aria-label={`Remove ${p}`}
                className="text-faint hover:text-err"
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
