/**
 * Diff data model for the spike renderer.
 *
 * The whole point of the spike is the *rendering* strategy, so the parser
 * produces the representation a virtualized list actually consumes: a single
 * flat array of "rows", where every row is one fixed-height item the list can
 * render or skip independently. Files and hunks become marker rows in that
 * same flat list; collapsing a hunk is just dropping its line rows from the
 * flattened view. There is no nested tree to walk at scroll time.
 */

export type LineKind = 'add' | 'del' | 'context' | 'meta';

/** A token of a syntax-ish highlighted line. */
export interface Token {
  readonly text: string;
  /** A coarse syntax class; mapped to a colour at render time. */
  readonly cls: TokenClass;
}

export type TokenClass =
  | 'plain'
  | 'keyword'
  | 'string'
  | 'comment'
  | 'number'
  | 'punct';

/** Row kinds in the flattened render list. */
export type Row = FileHeaderRow | HunkHeaderRow | LineRow;

export interface FileHeaderRow {
  readonly type: 'file';
  /** Stable key for the virtualized list. */
  readonly key: string;
  readonly path: string;
  /** Index into the parsed `DiffFile[]`, for expand/collapse toggling. */
  readonly fileIndex: number;
  readonly addCount: number;
  readonly delCount: number;
}

export interface HunkHeaderRow {
  readonly type: 'hunk';
  readonly key: string;
  readonly fileIndex: number;
  readonly hunkIndex: number;
  /** The `@@ -a,b +c,d @@` header text. */
  readonly header: string;
  /** Trailing section context after the second `@@`, if any. */
  readonly section: string;
}

export interface LineRow {
  readonly type: 'line';
  readonly key: string;
  readonly fileIndex: number;
  readonly hunkIndex: number;
  readonly kind: LineKind;
  /** Old-file line number, or null for added lines. */
  readonly oldNo: number | null;
  /** New-file line number, or null for deleted lines. */
  readonly newNo: number | null;
  /** Pre-tokenized content (syntax-ish). */
  readonly tokens: readonly Token[];
}

/** Parsed structure (kept around for expand/collapse bookkeeping). */
export interface DiffFile {
  readonly path: string;
  readonly addCount: number;
  readonly delCount: number;
  readonly hunks: readonly DiffHunk[];
}

export interface DiffHunk {
  readonly header: string;
  readonly section: string;
  readonly lines: readonly DiffLine[];
}

export interface DiffLine {
  readonly kind: LineKind;
  readonly oldNo: number | null;
  readonly newNo: number | null;
  readonly text: string;
}

export interface ParsedDiff {
  readonly files: readonly DiffFile[];
  readonly totalLines: number;
}
