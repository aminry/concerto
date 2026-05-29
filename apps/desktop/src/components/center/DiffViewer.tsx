// Monaco-backed diff viewer for the per-repo `Diff` sub-tab (Task 47).
//
// Layout: file list on the left, Monaco `DiffEditor` on the right. The
// view-mode toggle (split / unified) is mirrored into `useUiStore` so
// the choice persists with the rest of the layout state.
//
// V0.1 simplifications (see Task 47 Handoff Notes):
//
//   - Virtualization deferred. The file list renders as a plain list;
//     Monaco only mounts for the selected file, which keeps the cost
//     bounded.
//   - There's no `diff.<workarea>.<repo>` stream subject yet (Task 30
//     does not emit one), so refresh is driven by an explicit button +
//     React Query's `staleTime`. The Refresh button invalidates the
//     `diff` query so the next render re-fetches.
//   - Per-file `original`/`modified` content is synthesised from the
//     unified-diff body — we strip `+` lines to reconstruct the
//     before-side, and strip `-` lines to reconstruct the after-side.
//     This loses context outside hunk windows, but Monaco's diff
//     algorithm handles the windowed payload fine for V0.1 review.
//
// Monaco worker loading: the bundled `monaco-editor` package is wired
// into `@monaco-editor/react` via `loader.config({ monaco })`, so no
// CDN fetch is attempted (Tauri's CSP would block it anyway).

import { useEffect, useMemo, useState } from "react";
import { DiffEditor, loader } from "@monaco-editor/react";
import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import { useQueryClient } from "@tanstack/react-query";

import type { DiffHunk, DiffPayload, FileDiff } from "../../api/diff";
import { diffQueryKey, useDiff } from "../../hooks/useDiff";
import { useUiStore, type DiffViewMode } from "../../state/useUiStore";
import { FileListSidebar } from "./FileListSidebar";

// Wire Monaco's worker loader into Vite's worker pipeline. V0.1 only
// uses the generic editor worker (the diff viewer doesn't need TS /
// JSON / HTML language services to render unified diffs), so we ignore
// the `label` argument and always hand back the base worker.
// `MonacoEnvironment` is a global Monaco reads at editor-init time.
(self as unknown as { MonacoEnvironment: monaco.Environment }).MonacoEnvironment = {
  getWorker(): Worker {
    return new EditorWorker();
  },
};

// Route `@monaco-editor/react` at the bundled `monaco-editor` module —
// no CDN fetches, which matches the Tauri CSP. The call is idempotent
// across mounts; `loader.config` only takes effect once per session.
loader.config({ monaco });

export type DiffViewerProps = {
  workareaId: string;
  repositoryId: string | null;
};

export function DiffViewer(props: DiffViewerProps): JSX.Element {
  const { workareaId, repositoryId } = props;
  const diffQuery = useDiff(workareaId, repositoryId);
  const queryClient = useQueryClient();
  const diffViewMode = useUiStore((s) => s.diffViewMode);
  const setDiffViewMode = useUiStore((s) => s.setDiffViewMode);
  const [selectedIndex, setSelectedIndex] = useState(0);

  const files: FileDiff[] = diffQuery.data?.files ?? [];

  // Clamp the selection if the file list shrinks under us (e.g. a
  // refresh removes the previously-selected file).
  useEffect(() => {
    if (selectedIndex >= files.length && files.length > 0) {
      setSelectedIndex(0);
    }
  }, [files.length, selectedIndex]);

  const selected: FileDiff | null =
    files.length > 0 ? files[Math.min(selectedIndex, files.length - 1)] : null;

  const { original, modified } = useMemo(
    () => synthesizeSides(selected),
    [selected],
  );

  const language = useMemo(
    () => (selected ? guessLanguage(selected.path) : "plaintext"),
    [selected],
  );

  function handleRefresh(): void {
    queryClient.invalidateQueries({
      queryKey: diffQueryKey(workareaId, repositoryId),
    });
  }

  if (!repositoryId) {
    return (
      <div className="h-full flex items-center justify-center text-xs text-slate-500 p-3">
        No repository linked to this workarea yet.
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col min-h-0">
      <DiffToolbar
        viewMode={diffViewMode}
        onViewModeChange={setDiffViewMode}
        onRefresh={handleRefresh}
        loading={diffQuery.isFetching}
        fileCount={files.length}
      />
      <div className="flex-1 min-h-0 grid grid-cols-[minmax(160px,_220px)_1fr] border-t border-slate-800">
        <div className="border-r border-slate-800 overflow-hidden">
          <FileListSidebar
            files={files}
            selectedIndex={selectedIndex}
            onSelect={setSelectedIndex}
          />
        </div>
        <div className="min-w-0 min-h-0">
          {diffQuery.isError ? (
            <ErrorBanner message={String(diffQuery.error)} />
          ) : selected ? (
            <DiffEditor
              key={`${selected.path}:${diffViewMode}`}
              original={original}
              modified={modified}
              language={language}
              theme="vs-dark"
              options={{
                renderSideBySide: diffViewMode === "split",
                readOnly: true,
                automaticLayout: true,
                minimap: { enabled: false },
                fontSize: 12,
                scrollBeyondLastLine: false,
                renderOverviewRuler: false,
              }}
            />
          ) : (
            <EmptyState loading={diffQuery.isLoading} />
          )}
        </div>
      </div>
    </div>
  );
}

function DiffToolbar(props: {
  viewMode: DiffViewMode;
  onViewModeChange: (mode: DiffViewMode) => void;
  onRefresh: () => void;
  loading: boolean;
  fileCount: number;
}): JSX.Element {
  const { viewMode, onViewModeChange, onRefresh, loading, fileCount } = props;
  return (
    <div className="shrink-0 flex items-center gap-2 px-2 py-1 text-xs text-slate-400">
      <span className="font-mono">{fileCount} file{fileCount === 1 ? "" : "s"}</span>
      <div className="flex items-center rounded border border-slate-800 overflow-hidden">
        <ToggleButton
          active={viewMode === "split"}
          onClick={() => onViewModeChange("split")}
          label="Split"
        />
        <ToggleButton
          active={viewMode === "unified"}
          onClick={() => onViewModeChange("unified")}
          label="Unified"
        />
      </div>
      <button
        type="button"
        className="ml-auto px-2 py-0.5 rounded bg-slate-800 text-slate-100 hover:bg-slate-700 disabled:opacity-60"
        onClick={onRefresh}
        disabled={loading}
      >
        {loading ? "Refreshing…" : "Refresh"}
      </button>
    </div>
  );
}

function ToggleButton(props: {
  active: boolean;
  onClick: () => void;
  label: string;
}): JSX.Element {
  const cls = props.active
    ? "px-2 py-0.5 bg-slate-800 text-slate-100"
    : "px-2 py-0.5 text-slate-400 hover:bg-slate-900";
  return (
    <button
      type="button"
      className={cls}
      onClick={props.onClick}
      aria-pressed={props.active}
    >
      {props.label}
    </button>
  );
}

function EmptyState({ loading }: { loading: boolean }): JSX.Element {
  return (
    <div className="h-full flex items-center justify-center text-xs text-slate-500 p-3">
      {loading ? "Loading diff…" : "No changes."}
    </div>
  );
}

function ErrorBanner({ message }: { message: string }): JSX.Element {
  return (
    <div className="h-full flex items-center justify-center text-xs text-rose-400 p-3 font-mono">
      Diff failed: {message}
    </div>
  );
}

/// Synthesize Monaco's `original` + `modified` sides from a `FileDiff`.
/// Walks each hunk's unified-diff body and:
///
///   - drops `+` lines for the `original` side,
///   - drops `-` lines for the `modified` side,
///   - keeps context (` ` prefix) on both sides,
///   - strips the leading prefix character so Monaco diffs the raw text.
///
/// Hunks are joined with newlines so Monaco shows a sensible separator
/// between disjoint hunk windows.
function synthesizeSides(file: FileDiff | null): {
  original: string;
  modified: string;
} {
  if (!file) return { original: "", modified: "" };
  const originalChunks: string[] = [];
  const modifiedChunks: string[] = [];
  for (const hunk of file.hunks) {
    const split = hunkToSides(hunk);
    originalChunks.push(split.original);
    modifiedChunks.push(split.modified);
  }
  return {
    original: originalChunks.join("\n"),
    modified: modifiedChunks.join("\n"),
  };
}

function hunkToSides(hunk: DiffHunk): { original: string; modified: string } {
  const original: string[] = [];
  const modified: string[] = [];
  for (const raw of hunk.body.split("\n")) {
    if (raw.length === 0) {
      original.push("");
      modified.push("");
      continue;
    }
    const prefix = raw[0];
    const rest = raw.slice(1);
    if (prefix === "+") {
      modified.push(rest);
    } else if (prefix === "-") {
      original.push(rest);
    } else if (prefix === " ") {
      original.push(rest);
      modified.push(rest);
    } else {
      // `\` (no-newline marker) and anything else — leave it out of
      // both sides; it isn't meaningful to Monaco.
    }
  }
  return { original: original.join("\n"), modified: modified.join("\n") };
}

/// Best-effort path → Monaco language id mapping. Monaco understands a
/// long tail of languages out of the box; for V0.1 we cover the common
/// extensions and fall back to `plaintext`.
function guessLanguage(path: string): string {
  const idx = path.lastIndexOf(".");
  if (idx < 0) return "plaintext";
  const ext = path.slice(idx + 1).toLowerCase();
  switch (ext) {
    case "ts":
    case "tsx":
      return "typescript";
    case "js":
    case "jsx":
    case "mjs":
    case "cjs":
      return "javascript";
    case "rs":
      return "rust";
    case "py":
      return "python";
    case "go":
      return "go";
    case "json":
      return "json";
    case "md":
    case "markdown":
      return "markdown";
    case "yml":
    case "yaml":
      return "yaml";
    case "toml":
      return "ini";
    case "sh":
    case "bash":
      return "shell";
    case "html":
    case "htm":
      return "html";
    case "css":
      return "css";
    case "scss":
      return "scss";
    case "sql":
      return "sql";
    case "proto":
      return "protobuf";
    default:
      return "plaintext";
  }
}

export type { DiffPayload };
