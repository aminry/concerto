// Hex mirror of the CSS-variable tokens in src/index.css. Single source
// for surfaces that cannot read CSS variables directly — xterm's ITheme
// and Monaco's editor theme (added in later tasks). Keep in sync with
// src/index.css.

import type { EffectiveTheme } from "./resolveTheme";

export type ThemeColors = {
  background: string;
  surface: string;
  foreground: string;
  muted: string;
  faint: string;
  accent: string;
  border: string;
  ok: string;
  warn: string;
  err: string;
  run: string;
};

export const THEME_COLORS: Record<EffectiveTheme, ThemeColors> = {
  light: {
    background: "#f7f8fa", surface: "#ffffff", foreground: "#1f2328",
    muted: "#656d76", faint: "#8c959f", accent: "#5b5ef0", border: "#e5e7eb",
    ok: "#1a7f37", warn: "#9a6700", err: "#cf222e", run: "#0969da",
  },
  dark: {
    background: "#0d1117", surface: "#161b22", foreground: "#e6edf3",
    muted: "#8b949e", faint: "#6e7681", accent: "#6366f1", border: "#2a2f37",
    ok: "#3fb950", warn: "#d29922", err: "#f85149", run: "#58a6ff",
  },
};
