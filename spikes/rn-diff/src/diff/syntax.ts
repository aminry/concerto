/**
 * Deliberately tiny, allocation-light "syntax-ish" tokenizer.
 *
 * This is NOT a real highlighter (production Task 514 uses
 * `react-native-syntax-highlighter`, `design/16 §3.7`). The spike only needs
 * *representative per-line tokenization cost* so the measured render/scroll
 * numbers reflect coloured rows, not plain text. A single regex pass with a
 * handful of classes is enough to load the renderer realistically while
 * staying fast enough that it is not itself the bottleneck.
 */

import type { Token, TokenClass } from './types';

const KEYWORDS = new Set([
  'const', 'let', 'var', 'function', 'return', 'if', 'else', 'for', 'while',
  'import', 'export', 'from', 'class', 'extends', 'interface', 'type', 'enum',
  'async', 'await', 'new', 'fn', 'pub', 'struct', 'impl', 'match', 'use',
  'def', 'self', 'None', 'True', 'False', 'null', 'true', 'false', 'public',
  'private', 'static', 'void', 'int', 'string', 'bool', 'package', 'func',
]);

// One pass, ordered alternation: comment | string | number | word | punct | ws.
const TOKEN_RE =
  /(\/\/[^\n]*|#[^\n]*)|("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|`(?:[^`\\]|\\.)*`)|(\b\d[\d_.eExXa-fA-F]*\b)|([A-Za-z_$][A-Za-z0-9_$]*)|([{}()[\];:,.<>=+\-*/&|!?%^~]+)|(\s+)/g;

/**
 * Tokenize a single source line. Pure and synchronous; safe to memoize per
 * line. Returns at least one token (possibly an empty plain token).
 */
export function tokenizeLine(text: string): Token[] {
  if (text.length === 0) {
    return [{ text: '', cls: 'plain' }];
  }
  const tokens: Token[] = [];
  TOKEN_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  let lastEnd = 0;
  while ((match = TOKEN_RE.exec(text)) !== null) {
    // Preserve any gap the regex skipped (should not happen, but be safe).
    if (match.index > lastEnd) {
      tokens.push({ text: text.slice(lastEnd, match.index), cls: 'plain' });
    }
    lastEnd = TOKEN_RE.lastIndex;

    let cls: TokenClass;
    if (match[1] !== undefined) {
      cls = 'comment';
    } else if (match[2] !== undefined) {
      cls = 'string';
    } else if (match[3] !== undefined) {
      cls = 'number';
    } else if (match[4] !== undefined) {
      cls = KEYWORDS.has(match[4]) ? 'keyword' : 'plain';
    } else if (match[5] !== undefined) {
      cls = 'punct';
    } else {
      cls = 'plain';
    }
    tokens.push({ text: match[0], cls });
  }
  if (lastEnd < text.length) {
    tokens.push({ text: text.slice(lastEnd), cls: 'plain' });
  }
  return tokens.length > 0 ? tokens : [{ text, cls: 'plain' }];
}
