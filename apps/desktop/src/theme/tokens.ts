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

// Per-theme ANSI 16-color palette for the xterm terminal. Without this,
// xterm falls back to its built-in dark-tuned defaults, which are
// near-invisible on the light surface. Values are GitHub's terminal
// palette (light + dark), chosen for legibility on each background.
export type TerminalAnsi = {
  black: string; red: string; green: string; yellow: string;
  blue: string; magenta: string; cyan: string; white: string;
  brightBlack: string; brightRed: string; brightGreen: string;
  brightYellow: string; brightBlue: string; brightMagenta: string;
  brightCyan: string; brightWhite: string;
};

export const TERMINAL_ANSI: Record<EffectiveTheme, TerminalAnsi> = {
  light: {
    black: "#24292f", red: "#cf222e", green: "#116329", yellow: "#7d4e00",
    blue: "#0969da", magenta: "#8250df", cyan: "#1b7c83", white: "#6e7781",
    brightBlack: "#57606a", brightRed: "#a40e26", brightGreen: "#1a7f37",
    brightYellow: "#633c01", brightBlue: "#218bff", brightMagenta: "#a475f9",
    brightCyan: "#3192aa", brightWhite: "#8c959f",
  },
  dark: {
    black: "#484f58", red: "#ff7b72", green: "#3fb950", yellow: "#d29922",
    blue: "#58a6ff", magenta: "#bc8cff", cyan: "#39c5cf", white: "#b1bac4",
    brightBlack: "#6e7681", brightRed: "#ffa198", brightGreen: "#56d364",
    brightYellow: "#e3b341", brightBlue: "#79c0ff", brightMagenta: "#d2a8ff",
    brightCyan: "#56d4dd", brightWhite: "#ffffff",
  },
};
