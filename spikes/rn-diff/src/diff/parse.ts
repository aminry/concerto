/**
 * Unified-diff parser.
 *
 * Parses standard `git diff` unified output (`diff --git`, `@@` hunk headers,
 * `+`/`-`/` ` line prefixes) into the `ParsedDiff` tree. The tree is only used
 * for expand/collapse bookkeeping; the renderer consumes the *flattened* row
 * list produced by `flatten.ts`.
 */

import type {
  DiffFile,
  DiffHunk,
  DiffLine,
  LineKind,
  ParsedDiff,
} from './types';

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/;

interface FileAcc {
  path: string;
  addCount: number;
  delCount: number;
  hunks: DiffHunk[];
}

interface HunkAcc {
  header: string;
  section: string;
  lines: DiffLine[];
  oldNo: number;
  newNo: number;
}

function pathFromGitHeader(line: string): string {
  // `diff --git a/foo/bar.ts b/foo/bar.ts` → `foo/bar.ts`
  const m = /^diff --git a\/(.+?) b\/(.+)$/.exec(line);
  if (m && m[2]) {
    return m[2];
  }
  return line.replace(/^diff --git\s+/, '');
}

export function parseUnifiedDiff(raw: string): ParsedDiff {
  const lines = raw.split('\n');
  const files: DiffFile[] = [];
  let totalLines = 0;

  let file: FileAcc | null = null;
  let hunk: HunkAcc | null = null;

  const closeHunk = (): void => {
    if (file && hunk) {
      file.hunks.push({
        header: hunk.header,
        section: hunk.section,
        lines: hunk.lines,
      });
    }
    hunk = null;
  };

  const closeFile = (): void => {
    closeHunk();
    if (file) {
      files.push({
        path: file.path,
        addCount: file.addCount,
        delCount: file.delCount,
        hunks: file.hunks,
      });
    }
    file = null;
  };

  for (const line of lines) {
    if (line.startsWith('diff --git ')) {
      closeFile();
      file = {
        path: pathFromGitHeader(line),
        addCount: 0,
        delCount: 0,
        hunks: [],
      };
      continue;
    }

    // File-mode/index/`+++`/`---` metadata lines: skip but tolerate.
    if (
      line.startsWith('index ') ||
      line.startsWith('--- ') ||
      line.startsWith('+++ ') ||
      line.startsWith('new file') ||
      line.startsWith('deleted file') ||
      line.startsWith('old mode') ||
      line.startsWith('new mode') ||
      line.startsWith('rename ') ||
      line.startsWith('similarity ') ||
      line.startsWith('\\ No newline')
    ) {
      continue;
    }

    const hunkMatch = HUNK_RE.exec(line);
    if (hunkMatch && file) {
      closeHunk();
      hunk = {
        header: line.replace(/(@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@).*/, '$1'),
        section: (hunkMatch[3] ?? '').trim(),
        lines: [],
        oldNo: Number(hunkMatch[1]),
        newNo: Number(hunkMatch[2]),
      };
      continue;
    }

    if (!file || !hunk) {
      continue;
    }

    const prefix = line[0] ?? ' ';
    const body = line.slice(1);
    let kind: LineKind;
    let oldNo: number | null;
    let newNo: number | null;

    if (prefix === '+') {
      kind = 'add';
      oldNo = null;
      newNo = hunk.newNo++;
      file.addCount++;
    } else if (prefix === '-') {
      kind = 'del';
      oldNo = hunk.oldNo++;
      newNo = null;
      file.delCount++;
    } else {
      kind = 'context';
      oldNo = hunk.oldNo++;
      newNo = hunk.newNo++;
    }

    hunk.lines.push({ kind, oldNo, newNo, text: body });
    totalLines++;
  }

  closeFile();
  return { files, totalLines };
}
