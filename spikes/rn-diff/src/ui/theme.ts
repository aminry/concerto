import type { TokenClass } from '@/diff/types';

/** Dark code theme — fixed colours, no runtime theming (spike). */
export const colors = {
  bg: '#0d1117',
  panel: '#161b22',
  border: '#30363d',
  text: '#c9d1d9',
  dim: '#8b949e',
  gutter: '#6e7681',
  addBg: '#10381f',
  addGutter: '#1f6f3f',
  delBg: '#3a1417',
  delGutter: '#8b2a30',
  hunkBg: '#161b22',
  hunkText: '#58a6ff',
  accent: '#58a6ff',
  add: '#3fb950',
  del: '#f85149',
} as const;

export const syntaxColors: Record<TokenClass, string> = {
  plain: colors.text,
  keyword: '#ff7b72',
  string: '#a5d6ff',
  comment: '#8b949e',
  number: '#79c0ff',
  punct: '#c9d1d9',
};

/** Fixed row height — required so the virtualized list can skip layout. */
export const ROW_HEIGHT = 20;
export const MONO_FONT_SIZE = 12;
