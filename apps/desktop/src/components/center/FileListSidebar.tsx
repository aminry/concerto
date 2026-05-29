// File list rendered alongside the Monaco diff editor.
//
// Task 47 keeps this dead-simple for V0.1: a vertical scrolling list of
// files with a kind chip + per-file `+`/`-` counts. Virtualization is
// deferred (see Task 47 Handoff Notes) — Monaco only mounts for the
// selected file, so the cost of the list itself is bounded by DOM nodes
// (one per changed file).

import { FileText } from "lucide-react";

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
      <ul className="text-xs text-faint px-2 py-2 h-full flex items-center justify-center text-center">
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
          ? "w-full text-left px-2 py-1 rounded-md bg-accent/10 text-foreground flex items-center gap-2"
          : "w-full text-left px-2 py-1 rounded-md text-muted hover:bg-surface-2 flex items-center gap-2";
        return (
          <li key={`${file.path}:${idx}`}>
            <button
              type="button"
              className={cls}
              onClick={() => onSelect(idx)}
              aria-pressed={active}
              title={file.path}
            >
              <FileText size={14} className="shrink-0" />
              <KindChip kind={file.kind} />
              <span className="font-mono truncate flex-1">{file.path}</span>
              <span className="font-mono text-ok">
                +{stats.added}
              </span>
              <span className="font-mono text-err">-{stats.removed}</span>
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
      return "bg-ok/15 text-ok";
    case "DIFF_KIND_DELETED":
      return "bg-err/15 text-err";
    case "DIFF_KIND_MODIFIED":
      return "bg-warn/15 text-warn";
    case "DIFF_KIND_RENAMED":
      return "bg-accent/15 text-accent";
    default:
      return "bg-surface-2 text-muted";
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
