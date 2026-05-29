// File list rendered alongside the Monaco diff editor.
//
// Task 47 keeps this dead-simple for V0.1: a vertical scrolling list of
// files with a kind chip + per-file `+`/`-` counts. Virtualization is
// deferred (see Task 47 Handoff Notes) — Monaco only mounts for the
// selected file, so the cost of the list itself is bounded by DOM nodes
// (one per changed file).

import type { FileDiff } from "../../api/diff";

export type FileListSidebarProps = {
  files: FileDiff[];
  selectedIndex: number;
  onSelect: (index: number) => void;
};

export function FileListSidebar(props: FileListSidebarProps): JSX.Element {
  const { files, selectedIndex, onSelect } = props;
  if (files.length === 0) {
    return (
      <ul className="text-xs text-slate-500 px-2 py-2">
        <li>No changed files.</li>
      </ul>
    );
  }
  return (
    <ul className="text-xs overflow-auto h-full">
      {files.map((file, idx) => {
        const active = idx === selectedIndex;
        const stats = countAddDel(file);
        const cls = active
          ? "w-full text-left px-2 py-1 bg-slate-800 text-slate-100 flex items-center gap-2"
          : "w-full text-left px-2 py-1 text-slate-300 hover:bg-slate-900 flex items-center gap-2";
        return (
          <li key={`${file.path}:${idx}`}>
            <button
              type="button"
              className={cls}
              onClick={() => onSelect(idx)}
              aria-pressed={active}
              title={file.path}
            >
              <KindChip kind={file.kind} />
              <span className="font-mono truncate flex-1">{file.path}</span>
              <span className="font-mono text-emerald-400">
                +{stats.added}
              </span>
              <span className="font-mono text-rose-400">-{stats.removed}</span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}

function KindChip({ kind }: { kind: FileDiff["kind"] }): JSX.Element {
  const label = kindLabel(kind);
  const cls = kindClass(kind);
  return (
    <span
      className={`shrink-0 inline-block w-4 text-center text-[10px] rounded ${cls}`}
      aria-label={label}
    >
      {label}
    </span>
  );
}

function kindLabel(kind: FileDiff["kind"]): string {
  switch (kind) {
    case "DIFF_KIND_ADDED":
      return "A";
    case "DIFF_KIND_DELETED":
      return "D";
    case "DIFF_KIND_MODIFIED":
      return "M";
    case "DIFF_KIND_RENAMED":
      return "R";
    default:
      return "?";
  }
}

function kindClass(kind: FileDiff["kind"]): string {
  switch (kind) {
    case "DIFF_KIND_ADDED":
      return "bg-emerald-900 text-emerald-200";
    case "DIFF_KIND_DELETED":
      return "bg-rose-900 text-rose-200";
    case "DIFF_KIND_MODIFIED":
      return "bg-amber-900 text-amber-200";
    case "DIFF_KIND_RENAMED":
      return "bg-sky-900 text-sky-200";
    default:
      return "bg-slate-800 text-slate-300";
  }
}

function countAddDel(file: FileDiff): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const hunk of file.hunks) {
    for (const line of hunk.body.split("\n")) {
      if (line.startsWith("+") && !line.startsWith("+++")) added += 1;
      else if (line.startsWith("-") && !line.startsWith("---")) removed += 1;
    }
  }
  return { added, removed };
}
