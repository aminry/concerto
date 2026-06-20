// Unified-diff parser (Task 514). Turns unified-diff TEXT (the format `git diff`
// / GitHub PR patches emit) into a flat list of typed `DiffRow`s the pure-RN
// renderer (`DiffView`) virtualizes through a FlatList.
//
// We parse to a FLAT row list (not a nested file->hunk->line tree) because the
// renderer is a single virtualized FlatList: file headers, hunk headers, and the
// add/remove/context lines all become rows with a stable `key`, and the renderer
// decides collapse/indent from `row.kind` + `row.hunkId`. Keeping the parser
// pure (text in, rows out — no RN imports) makes it trivially Tier-2 testable.
//
// Supported grammar (a permissive subset of `git diff` unified output):
//   diff --git a/path b/path        -> file boundary (starts a new file group)
//   --- a/path        | +++ b/path  -> old/new path lines (folded into the file header)
//   @@ -l,s +l,s @@ section          -> hunk header (resets per-side line counters)
//   " context"  "+added"  "-removed" -> body lines (leading marker char)
//   "\ No newline at end of file"    -> ignored (annotation, not a content line)
// Anything before the first hunk that isn't a recognized header is treated as
// preamble and dropped (e.g. `index abc..def 100644`, `similarity index`, …).

/** A single rendered row of a unified diff. */
export type DiffRow =
  | FileHeaderRow
  | HunkHeaderRow
  | AddRow
  | RemoveRow
  | ContextRow;

/** Discriminant for [`DiffRow`]. */
export type DiffRowKind = DiffRow["kind"];

/** A file boundary — the start of a new file's hunks. */
export interface FileHeaderRow {
  kind: "file";
  /** Stable row key (unique within the parse). */
  key: string;
  /** The "after" path (`b/…`), falling back to the "before" path for deletions. */
  path: string;
  /** The "before" path (`a/…`), present for renames/deletions. */
  oldPath?: string;
  /** Index of the owning file group (0-based). */
  fileIndex: number;
}

/** A `@@ … @@` hunk header. */
export interface HunkHeaderRow {
  kind: "hunk";
  key: string;
  /** The raw `@@ -a,b +c,d @@` text (without the trailing section heading). */
  header: string;
  /** Optional section heading after the second `@@` (the function/context hint). */
  section?: string;
  /** Stable id grouping this hunk's body rows (for collapse/expand). */
  hunkId: string;
  fileIndex: number;
}

interface BodyRowBase {
  key: string;
  /** The line content WITHOUT the leading +/-/space marker. */
  content: string;
  /** Id of the owning hunk (so the renderer can collapse a whole hunk). */
  hunkId: string;
  fileIndex: number;
}

/** An added (`+`) line. */
export interface AddRow extends BodyRowBase {
  kind: "add";
  /** 1-based line number in the NEW file. */
  newLine: number;
}

/** A removed (`-`) line. */
export interface RemoveRow extends BodyRowBase {
  kind: "remove";
  /** 1-based line number in the OLD file. */
  oldLine: number;
}

/** An unchanged context line (present in both sides). */
export interface ContextRow extends BodyRowBase {
  kind: "context";
  oldLine: number;
  newLine: number;
}

/** Parsed `@@ -oldStart,oldCount +newStart,newCount @@ section` numbers. */
interface HunkRange {
  oldStart: number;
  newStart: number;
  section?: string;
}

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/;

function parseHunkHeader(line: string): HunkRange | null {
  const m = HUNK_RE.exec(line);
  if (!m) return null;
  const section = m[3]?.trim();
  return {
    oldStart: Number(m[1]),
    newStart: Number(m[2]),
    ...(section ? { section } : {}),
  };
}

/** Strip a leading `a/` or `b/` git path prefix (but leave `/dev/null` alone). */
function stripGitPrefix(p: string): string {
  if (p === "/dev/null") return p;
  return p.replace(/^[ab]\//, "");
}

/**
 * Parse unified-diff TEXT into a flat [`DiffRow`] list. Tolerant of missing file
 * headers (a bare `@@ … @@` body, as some patch fragments ship) — a synthetic
 * file group is opened so every body row still has a `fileIndex`/`hunkId`.
 */
export function parseUnifiedDiff(text: string): DiffRow[] {
  const rows: DiffRow[] = [];
  // Split on \n; tolerate \r\n by trimming a trailing \r per line. A trailing
  // newline at end of the diff yields a final empty element — drop it so it
  // isn't mistaken for a blank context line (git emits blank context as " ").
  const lines = text.split("\n");
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();

  let fileIndex = -1;
  let hunkIndex = -1;
  let oldLine = 0;
  let newLine = 0;
  let inHunk = false;
  // Pending paths gathered from `diff --git` / `---` / `+++` before we emit the
  // file header (we emit lazily so `+++ b/path` can supply the canonical path).
  let pendingOldPath: string | undefined;
  let pendingNewPath: string | undefined;
  let fileHeaderEmitted = false;

  const emitFileHeaderIfNeeded = () => {
    if (fileHeaderEmitted) return;
    // For a deletion the new side is /dev/null; fall back to the old path.
    const newReal = pendingNewPath && pendingNewPath !== "/dev/null" ? pendingNewPath : undefined;
    const oldReal = pendingOldPath && pendingOldPath !== "/dev/null" ? pendingOldPath : undefined;
    const path = newReal ?? oldReal ?? "(unknown)";
    const row: FileHeaderRow = {
      kind: "file",
      key: `f${fileIndex}`,
      path,
      fileIndex,
    };
    // Only surface oldPath as a rename source when BOTH sides are real files and
    // they differ (a new file's /dev/null old side is not a rename).
    if (oldReal && newReal && oldReal !== newReal) row.oldPath = oldReal;
    rows.push(row);
    fileHeaderEmitted = true;
  };

  const startFile = () => {
    // A previous file whose `diff --git` produced no `@@` hunk (a pure rename, a
    // file-mode-only change, or a binary file) never flushed its captured paths.
    // Flush a header-only row for it now so it isn't silently dropped.
    if (fileIndex >= 0 && !fileHeaderEmitted) emitFileHeaderIfNeeded();
    fileIndex += 1;
    inHunk = false;
    pendingOldPath = undefined;
    pendingNewPath = undefined;
    fileHeaderEmitted = false;
  };

  for (const raw of lines) {
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;

    if (line.startsWith("diff --git ")) {
      startFile();
      // `diff --git a/x b/y` — capture both as a fallback (--- / +++ refine them).
      const m = /^diff --git (\S+) (\S+)$/.exec(line);
      if (m && m[1] && m[2]) {
        pendingOldPath = stripGitPrefix(m[1]);
        pendingNewPath = stripGitPrefix(m[2]);
      }
      continue;
    }

    if (line.startsWith("--- ")) {
      // A `---` with no preceding `diff --git` still opens a file group.
      if (fileHeaderEmitted || fileIndex < 0) startFile();
      pendingOldPath = stripGitPrefix(line.slice(4).trim());
      continue;
    }

    if (line.startsWith("+++ ")) {
      if (fileIndex < 0) startFile();
      pendingNewPath = stripGitPrefix(line.slice(4).trim());
      continue;
    }

    const range = parseHunkHeader(line);
    if (range) {
      if (fileIndex < 0) startFile(); // bare hunk with no file header
      emitFileHeaderIfNeeded();
      hunkIndex += 1;
      const hunkId = `h${hunkIndex}`;
      inHunk = true;
      oldLine = range.oldStart;
      newLine = range.newStart;
      const atatEnd = line.indexOf("@@", 2);
      const header = atatEnd >= 0 ? line.slice(0, atatEnd + 2) : line;
      const hunkRow: HunkHeaderRow = {
        kind: "hunk",
        key: hunkId,
        header,
        hunkId,
        fileIndex,
      };
      if (range.section) hunkRow.section = range.section;
      rows.push(hunkRow);
      continue;
    }

    if (!inHunk) {
      // Preamble (index/mode lines, etc.) — ignored.
      continue;
    }

    // "\ No newline at end of file" is an annotation, not content.
    if (line.startsWith("\\")) continue;

    const marker = line[0];
    const content = line.slice(1);
    const hunkId = `h${hunkIndex}`;
    const keyBase = `${hunkId}-${rows.length}`;

    if (marker === "+") {
      rows.push({
        kind: "add",
        key: keyBase,
        content,
        hunkId,
        fileIndex,
        newLine,
      });
      newLine += 1;
    } else if (marker === "-") {
      rows.push({
        kind: "remove",
        key: keyBase,
        content,
        hunkId,
        fileIndex,
        oldLine,
      });
      oldLine += 1;
    } else if (marker === " " || marker === undefined) {
      // A space-prefixed context line, or a fully-empty line (treated as blank
      // context — git emits a bare empty line for a blank context line).
      rows.push({
        kind: "context",
        key: keyBase,
        content,
        hunkId,
        fileIndex,
        oldLine,
        newLine,
      });
      oldLine += 1;
      newLine += 1;
    }
    // Any other leading char inside a hunk is unexpected; skip it defensively.
  }

  // Flush the final file's header if it had no `@@` hunk (trailing pure rename /
  // mode-only / binary file), so it renders a header-only row instead of vanishing.
  if (fileIndex >= 0) emitFileHeaderIfNeeded();

  return rows;
}

/** Count rows by kind — handy for tests + a renderer summary chip. */
export function summarizeRows(rows: DiffRow[]): {
  files: number;
  hunks: number;
  added: number;
  removed: number;
  context: number;
} {
  let files = 0;
  let hunks = 0;
  let added = 0;
  let removed = 0;
  let context = 0;
  for (const r of rows) {
    if (r.kind === "file") files += 1;
    else if (r.kind === "hunk") hunks += 1;
    else if (r.kind === "add") added += 1;
    else if (r.kind === "remove") removed += 1;
    else context += 1;
  }
  return { files, hunks, added, removed, context };
}
