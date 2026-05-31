/**
 * Flatten a parsed diff into the single flat row array the virtualized list
 * renders.
 *
 * Expand/collapse is modelled here, not in the renderer: a collapsed file
 * contributes only its `FileHeaderRow`; an expanded file contributes its file
 * header + every hunk header + every (tokenized) line row. Because the list is
 * virtualized, the cost of a large diff is dominated by *flattening + token
 * memoization*, not by mounting rows — only the visible window mounts. The
 * flatten pass is therefore the main CPU cost on first render, which is exactly
 * what the harness times.
 */

import { tokenizeLine } from './syntax';
import type { ParsedDiff, Row, Token } from './types';

export interface FlattenOptions {
  /** Set of fileIndex values that are collapsed (header only). */
  readonly collapsed: ReadonlySet<number>;
  /** When false, skip tokenization (used to isolate tokenize cost). */
  readonly syntax: boolean;
}

// Module-level memo so re-flattening on expand/collapse does not re-tokenize
// unchanged lines. Keyed by raw line text; identical source lines (very common
// in real diffs — braces, imports, blank lines) share one token array.
const tokenCache = new Map<string, readonly Token[]>();
const PLAIN: readonly Token[] = [{ text: '', cls: 'plain' }];

function tokensFor(text: string, syntax: boolean): readonly Token[] {
  if (!syntax) {
    return [{ text, cls: 'plain' }];
  }
  if (text.length === 0) {
    return PLAIN;
  }
  const hit = tokenCache.get(text);
  if (hit) {
    return hit;
  }
  const toks = tokenizeLine(text);
  tokenCache.set(text, toks);
  return toks;
}

export function flattenDiff(diff: ParsedDiff, opts: FlattenOptions): Row[] {
  const rows: Row[] = [];
  for (let f = 0; f < diff.files.length; f++) {
    const file = diff.files[f];
    if (!file) {
      continue;
    }
    rows.push({
      type: 'file',
      key: `f${f}`,
      path: file.path,
      fileIndex: f,
      addCount: file.addCount,
      delCount: file.delCount,
    });
    if (opts.collapsed.has(f)) {
      continue;
    }
    for (let h = 0; h < file.hunks.length; h++) {
      const hunk = file.hunks[h];
      if (!hunk) {
        continue;
      }
      rows.push({
        type: 'hunk',
        key: `f${f}h${h}`,
        fileIndex: f,
        hunkIndex: h,
        header: hunk.header,
        section: hunk.section,
      });
      for (let l = 0; l < hunk.lines.length; l++) {
        const line = hunk.lines[l];
        if (!line) {
          continue;
        }
        rows.push({
          type: 'line',
          key: `f${f}h${h}l${l}`,
          fileIndex: f,
          hunkIndex: h,
          kind: line.kind,
          oldNo: line.oldNo,
          newNo: line.newNo,
          tokens: tokensFor(line.text, opts.syntax),
        });
      }
    }
  }
  return rows;
}

/** Test/diagnostic hook: clear the shared token memo. */
export function clearTokenCache(): void {
  tokenCache.clear();
}
