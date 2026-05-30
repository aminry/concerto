# Desktop UI Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reskin the Concerto desktop renderer into a modern professional IDE with first-class light + dark themes, driven by a semantic CSS-variable token system.

**Architecture:** Semantic design tokens are defined as CSS custom properties in `index.css` (one block per theme, switched by a `data-theme` attribute on `<html>`). `tailwind.config.ts` maps utility classes (`bg-surface`, `text-muted`, `border-border`, `bg-accent`…) to those variables via `rgb(var(--x) / <alpha-value>)`, so a single attribute flip re-themes everything. A pure `resolveTheme` module + `useTheme` hook own theme state (system/light/dark) and write the attribute; an inline pre-paint script in `index.html` prevents FOUC. All ~30 renderer files migrate from hardcoded `slate-*` classes to tokens. xterm and Monaco read the active theme dynamically.

**Tech Stack:** React 18 + TypeScript, Tailwind v3, Zustand, react-resizable-panels, `@monaco-editor/react`, `@xterm/xterm`, **new:** `lucide-react`. Package manager **pnpm**. Build/typecheck gate: `pnpm build`.

**Why no unit tests in this plan:** `apps/desktop` ships no JS test runner and zero JS tests. This is a visual reskin; the only non-trivial logic is `resolveTheme`, kept as a pure, obvious function. Per-task verification is therefore `pnpm build` (TypeScript typecheck + Vite build), targeted `grep` gates for residual hardcoded colors, and manual two-theme visual checks. Adding a test runner is explicitly out of scope (YAGNI).

**Conventions for every task:**
- Run all `pnpm` commands from `apps/desktop` (`cd apps/desktop` first; the tool's working dir resets between calls, so use the `cd … && …` form shown).
- If `node_modules` is missing, run `pnpm install --frozen-lockfile` once before the first build.
- Commit messages end with the `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` trailer (omitted from snippets below for brevity — add it).
- We are on branch `redesign-desktop-app-ui`; commit directly to it.

---

## File Structure

**New files**
- `apps/desktop/src/theme/tokens.ts` — TypeScript mirror of the token hex values (single source for xterm/Monaco; documents the palette).
- `apps/desktop/src/theme/resolveTheme.ts` — pure theme-resolution logic (`ThemePreference` → effective `"light" | "dark"`).
- `apps/desktop/src/hooks/useTheme.ts` — React hook: reads/writes preference, applies `data-theme`, listens to OS changes.
- `apps/desktop/src/components/ui/icon-button.tsx` — square icon button + tooltip.
- `apps/desktop/src/components/ui/tooltip.tsx` — lightweight title-based tooltip wrapper.
- `apps/desktop/src/components/ui/status-dot.tsx` — status → token color dot + a11y label.
- `apps/desktop/src/components/ui/badge.tsx` — chip/badge (branch slug, file counts).
- `apps/desktop/src/components/ui/tabs.tsx` — underline sub-tabs.
- `apps/desktop/src/components/ui/segmented.tsx` — segmented control.
- `apps/desktop/src/components/StatusBar.tsx` — bottom status bar incl. theme toggle.

**Modified files**
- `apps/desktop/tailwind.config.ts` — token color + font + radius extension, `darkMode`.
- `apps/desktop/index.html` — pre-paint theme script.
- `apps/desktop/src/index.css` — token CSS variable blocks.
- `apps/desktop/src/App.tsx` — `font-sans` root, `bg-background text-foreground`, mount `useTheme`.
- `apps/desktop/src/components/AppLayout.tsx` — wrap in a column flex with `StatusBar`; retoken resize handles.
- UI primitives: `ui/button.tsx`, `ui/card.tsx`, `ui/input.tsx`, `ui/dialog.tsx`, `ui/progress.tsx`.
- Sidebar group: `Sidebar.tsx`, `WorkareaList.tsx`.
- Center group: `CenterPanel.tsx`, `WorkspaceDetail.tsx`, `center/SessionRegion.tsx`, `SessionTab.tsx`, `SessionComposer.tsx`, `center/CodePrRegion.tsx`, `center/FileListSidebar.tsx`, `center/DiffViewer.tsx`.
- Right-rail group: `RightRail.tsx`, `right-rail/SchedulerTab.tsx`, `right-rail/SkillsTab.tsx`, `right-rail/TodosTab.tsx`, `right-rail/McpTab.tsx`, `right-rail/FilesTab.tsx`.
- Modals/misc: `NewWorkspaceModal.tsx`, `SettingsPanel.tsx`, `AddRepositoryForm.tsx`, `StartSessionPicker.tsx`, `Toast.tsx`.
- Theme-aware third-party: `SessionTerminal.tsx` (xterm), `center/DiffViewer.tsx` (Monaco).

---

## Canonical class-migration map

Apply this mapping wherever the old class appears (used by Tasks 6–10). This is the single source — later tasks reference "the migration map" instead of repeating it.

| Old class | New class |
|-----------|-----------|
| `bg-slate-950` | `bg-background` |
| `bg-slate-900` | `bg-surface` |
| `bg-slate-800` | `bg-surface-2` |
| `bg-slate-700` (raised/pressed) | `bg-raised` |
| `hover:bg-slate-900` | `hover:bg-surface-2` |
| `hover:bg-slate-800` | `hover:bg-surface-2` |
| `hover:bg-slate-700` | `hover:bg-accent-hover` (only on accent buttons) / `hover:bg-raised` |
| `border-slate-800` | `border-border` |
| `border-slate-700` | `border-border-strong` |
| `text-slate-100` | `text-foreground` |
| `text-slate-200` | `text-foreground` |
| `text-slate-300` | `text-muted` |
| `text-slate-400` | `text-muted` |
| `text-slate-500` | `text-faint` |
| `placeholder:text-slate-500` | `placeholder:text-faint` |
| `text-rose-400` | `text-err` |
| `bg-emerald-500` / `border-emerald-500` | `bg-accent` / `border-accent` |
| `text-emerald-*` | `text-accent` |
| `focus:ring-slate-500` | `focus:ring-accent` |
| `divide-slate-800` | `divide-border` |

Status **dots/indicators** (session state, CI) do NOT map to `accent`; they use the `<StatusDot>` component (Task 4) → `ok/warn/err/run/faint`.

---

## Task 1: Token foundation — CSS variables, Tailwind extension, root + FOUC script

**Files:**
- Modify: `apps/desktop/src/index.css`
- Modify: `apps/desktop/tailwind.config.ts`
- Modify: `apps/desktop/index.html`
- Modify: `apps/desktop/src/App.tsx:41`

- [ ] **Step 1: Add token blocks to `index.css`**

Append after the existing `@tailwind utilities;` line (keep the xterm import + `.xterm-viewport` rule intact):

```css
/* --- Design tokens (Task: UI redesign). Channels are space-separated
   RGB so Tailwind's `rgb(var(--x) / <alpha-value>)` opacity modifiers
   work (e.g. bg-accent/15). Light is the :root default; dark overrides
   under [data-theme="dark"]. The .dark class mirror keeps Tailwind's
   class-based darkMode working. */
:root,
:root[data-theme="light"] {
  --background: 247 248 250;
  --surface: 255 255 255;
  --surface-2: 246 247 249;
  --raised: 236 238 242;
  --border: 229 231 235;
  --border-strong: 211 215 222;
  --foreground: 31 35 40;
  --muted: 101 109 118;
  --faint: 140 149 159;
  --accent: 91 94 240;
  --accent-hover: 75 78 230;
  --accent-fg: 255 255 255;
  --ok: 26 127 55;
  --warn: 154 103 0;
  --err: 207 34 46;
  --run: 9 105 218;
}

:root[data-theme="dark"],
.dark {
  --background: 13 17 23;
  --surface: 22 27 34;
  --surface-2: 28 33 40;
  --raised: 33 38 45;
  --border: 42 47 55;
  --border-strong: 55 62 71;
  --foreground: 230 237 243;
  --muted: 139 148 158;
  --faint: 110 118 129;
  --accent: 99 102 241;
  --accent-hover: 124 127 244;
  --accent-fg: 255 255 255;
  --ok: 63 185 80;
  --warn: 210 153 34;
  --err: 248 81 73;
  --run: 88 166 255;
}

/* App canvas + native text smoothing for the system sans stack. */
html, body, #root { height: 100%; }
body {
  background-color: rgb(var(--background));
  color: rgb(var(--foreground));
  -webkit-font-smoothing: antialiased;
}
```

- [ ] **Step 2: Extend `tailwind.config.ts`**

Replace the file's `const config` with:

```ts
import type { Config } from "tailwindcss";

// Tailwind v3 — pinned (v4's LightningCSS dep trips cargo-deny). Theme
// tokens resolve to the CSS variables defined in src/index.css so the
// whole app re-themes by flipping `data-theme` on <html>.
const config: Config = {
  darkMode: ["class", '[data-theme="dark"]'],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        background: "rgb(var(--background) / <alpha-value>)",
        surface: "rgb(var(--surface) / <alpha-value>)",
        "surface-2": "rgb(var(--surface-2) / <alpha-value>)",
        raised: "rgb(var(--raised) / <alpha-value>)",
        border: "rgb(var(--border) / <alpha-value>)",
        "border-strong": "rgb(var(--border-strong) / <alpha-value>)",
        foreground: "rgb(var(--foreground) / <alpha-value>)",
        muted: "rgb(var(--muted) / <alpha-value>)",
        faint: "rgb(var(--faint) / <alpha-value>)",
        accent: {
          DEFAULT: "rgb(var(--accent) / <alpha-value>)",
          hover: "rgb(var(--accent-hover) / <alpha-value>)",
          fg: "rgb(var(--accent-fg) / <alpha-value>)",
        },
        ok: "rgb(var(--ok) / <alpha-value>)",
        warn: "rgb(var(--warn) / <alpha-value>)",
        err: "rgb(var(--err) / <alpha-value>)",
        run: "rgb(var(--run) / <alpha-value>)",
      },
      fontFamily: {
        sans: [
          "-apple-system", "BlinkMacSystemFont", '"SF Pro Text"',
          '"Segoe UI"', "system-ui", "sans-serif",
        ],
        mono: [
          "ui-monospace", '"SF Mono"', '"JetBrains Mono"', "Menlo", "monospace",
        ],
      },
      borderRadius: { lg: "10px", md: "8px", sm: "6px" },
    },
  },
  plugins: [],
};

export default config;
```

- [ ] **Step 3: Add the pre-paint FOUC guard to `index.html`**

Insert this `<script>` inside `<head>`, before the `</head>` close (above the body):

```html
    <script>
      // Pre-paint theme: read saved preference (system|light|dark) and
      // set data-theme before first paint to avoid a flash of the wrong
      // theme. Mirrors src/theme/resolveTheme.ts.
      (function () {
        try {
          var pref = localStorage.getItem("concerto.theme.v1") || "system";
          var dark =
            pref === "dark" ||
            (pref === "system" &&
              window.matchMedia("(prefers-color-scheme: dark)").matches);
          document.documentElement.setAttribute(
            "data-theme",
            dark ? "dark" : "light"
          );
        } catch (e) {
          document.documentElement.setAttribute("data-theme", "light");
        }
      })();
    </script>
```

- [ ] **Step 4: Switch the App root to tokens + sans**

In `apps/desktop/src/App.tsx` change the root div (line ~41):

```tsx
      <div className="h-screen w-screen bg-background text-foreground font-sans">
```
(was `bg-slate-950 text-slate-100 font-mono`)

- [ ] **Step 5: Build**

Run: `cd apps/desktop && pnpm install --frozen-lockfile && pnpm build`
Expected: PASS (no TS errors; Vite build completes). The app will look mostly unstyled-correct because child components still use `slate-*` — that's fine; later tasks migrate them.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/index.css apps/desktop/tailwind.config.ts apps/desktop/index.html apps/desktop/src/App.tsx
git commit -m "feat(desktop): add semantic theme token system + tailwind extension"
```

---

## Task 2: Theme resolution module + `useTheme` hook

**Files:**
- Create: `apps/desktop/src/theme/resolveTheme.ts`
- Create: `apps/desktop/src/theme/tokens.ts`
- Create: `apps/desktop/src/hooks/useTheme.ts`

- [ ] **Step 1: Create `resolveTheme.ts`**

```ts
// Pure theme-resolution logic. No React, no DOM mutation — kept pure so
// it is trivially correct and reused by the index.html pre-paint guard's
// mental model. `ThemePreference` is what the user picks; `EffectiveTheme`
// is what actually renders.

export type ThemePreference = "system" | "light" | "dark";
export type EffectiveTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "concerto.theme.v1";

/** Resolve the user's preference + the OS signal into the theme to render. */
export function resolveTheme(
  pref: ThemePreference,
  systemPrefersDark: boolean,
): EffectiveTheme {
  if (pref === "dark") return "dark";
  if (pref === "light") return "light";
  return systemPrefersDark ? "dark" : "light";
}

/** Narrow an untrusted localStorage string back to a ThemePreference. */
export function isThemePreference(v: unknown): v is ThemePreference {
  return v === "system" || v === "light" || v === "dark";
}
```

- [ ] **Step 2: Create `tokens.ts`**

```ts
// Hex mirror of the CSS-variable tokens in src/index.css. Single source
// for surfaces that cannot read CSS variables directly — xterm's ITheme
// and Monaco's editor theme (Tasks 9–10). Keep in sync with index.css.

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
```

- [ ] **Step 3: Create `useTheme.ts`**

```ts
// Theme controller hook. Owns the user preference (persisted), watches
// the OS color-scheme, and writes `data-theme` onto <html>. Returns the
// preference, the effective theme, and a cycle helper for the toggle.

import { useCallback, useEffect, useState } from "react";
import {
  isThemePreference,
  resolveTheme,
  THEME_STORAGE_KEY,
  type EffectiveTheme,
  type ThemePreference,
} from "../theme/resolveTheme";

function loadPreference(): ThemePreference {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    return isThemePreference(raw) ? raw : "system";
  } catch {
    return "system";
  }
}

function systemPrefersDark(): boolean {
  return (
    typeof window !== "undefined" &&
    !!window.matchMedia &&
    window.matchMedia("(prefers-color-scheme: dark)").matches
  );
}

export type UseThemeResult = {
  preference: ThemePreference;
  effective: EffectiveTheme;
  setPreference: (p: ThemePreference) => void;
  /** system → light → dark → system */
  cycle: () => void;
};

export function useTheme(): UseThemeResult {
  const [preference, setPreferenceState] =
    useState<ThemePreference>(loadPreference);
  const [systemDark, setSystemDark] = useState<boolean>(systemPrefersDark);

  // Track OS changes so `system` preference stays live.
  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  const effective = resolveTheme(preference, systemDark);

  // Apply to <html> whenever the effective theme changes.
  useEffect(() => {
    document.documentElement.setAttribute("data-theme", effective);
  }, [effective]);

  const setPreference = useCallback((p: ThemePreference) => {
    setPreferenceState(p);
    try {
      localStorage.setItem(THEME_STORAGE_KEY, p);
    } catch {
      // Persistence is best-effort; in-memory state still applies.
    }
  }, []);

  const cycle = useCallback(() => {
    setPreference(
      preference === "system" ? "light" : preference === "light" ? "dark" : "system",
    );
  }, [preference, setPreference]);

  return { preference, effective, setPreference, cycle };
}
```

- [ ] **Step 4: Build**

Run: `cd apps/desktop && pnpm build`
Expected: PASS. (Hook/modules are not imported yet — Task 5 wires them via StatusBar. TypeScript only checks compilation here.)

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src/theme apps/desktop/src/hooks/useTheme.ts
git commit -m "feat(desktop): theme resolution module + useTheme hook"
```

---

## Task 3: Add lucide-react + upgrade existing UI primitives

**Files:**
- Modify: `apps/desktop/package.json` (via pnpm add)
- Modify: `apps/desktop/src/components/ui/button.tsx`
- Modify: `apps/desktop/src/components/ui/card.tsx`
- Modify: `apps/desktop/src/components/ui/input.tsx`
- Modify: `apps/desktop/src/components/ui/dialog.tsx`
- Modify: `apps/desktop/src/components/ui/progress.tsx`

- [ ] **Step 1: Add the icon dependency**

Run: `cd apps/desktop && pnpm add lucide-react`
Expected: adds `lucide-react` to `dependencies` and updates `pnpm-lock.yaml`. (License: ISC — permissive. `cargo-deny` governs Rust crates only, so this npm dep is not gated by it; if a separate JS license check exists in CI, ISC passes.)

- [ ] **Step 2: Rewrite `button.tsx` with variants + sizes**

```tsx
// Button primitive. Token-driven; variants cover the app's needs.
// `primary` is the indigo accent CTA; `icon` size is for icon-only buttons.

import { forwardRef, type ButtonHTMLAttributes } from "react";

export type ButtonVariant =
  | "default" | "primary" | "ghost" | "outline" | "danger";
export type ButtonSize = "sm" | "md" | "icon";

export type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
};

const VARIANTS: Record<ButtonVariant, string> = {
  default: "bg-surface-2 hover:bg-raised text-foreground disabled:opacity-50",
  primary: "bg-accent hover:bg-accent-hover text-accent-fg disabled:opacity-50",
  ghost: "bg-transparent hover:bg-surface-2 text-muted hover:text-foreground",
  outline:
    "border border-border-strong hover:bg-surface-2 text-foreground disabled:opacity-50",
  danger: "bg-err/10 hover:bg-err/20 text-err disabled:opacity-50",
};

const SIZES: Record<ButtonSize, string> = {
  sm: "px-2 py-1 text-xs",
  md: "px-3 py-1.5 text-sm",
  icon: "h-8 w-8 p-0",
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ variant = "default", size = "md", className, ...props }, ref) => {
    const base =
      "inline-flex items-center justify-center gap-1.5 rounded-md font-medium transition-colors disabled:cursor-not-allowed focus:outline-none focus-visible:ring-2 focus-visible:ring-accent";
    const combined = [base, VARIANTS[variant], SIZES[size], className ?? ""]
      .join(" ")
      .trim();
    return <button ref={ref} className={combined} {...props} />;
  },
);
Button.displayName = "Button";
```

- [ ] **Step 3: Retoken `card.tsx`**

Replace the three class strings:
- `Card`: `"rounded-md border border-border bg-surface"`
- `CardHeader`: `"px-4 py-3 border-b border-border"`
- `CardTitle`: `"text-sm font-semibold uppercase tracking-wide text-muted"`
- `CardContent`: `"px-4 py-3 text-sm text-foreground"`

- [ ] **Step 4: Retoken `input.tsx`**

Replace the `base` string with:

```tsx
    const base =
      "w-full rounded-md border border-border-strong bg-background px-2.5 py-1.5 text-sm text-foreground placeholder:text-faint focus:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50";
```

- [ ] **Step 5: Retoken `dialog.tsx`**

Apply the migration map to its class strings and use a lucide close icon:
- overlay: `"fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm"`
- panel: `"w-[28rem] max-w-[90vw] rounded-lg border border-border bg-surface shadow-2xl"`
- header: `"flex items-center justify-between px-4 py-3 border-b border-border"`
- title: `"text-sm font-semibold text-foreground"`
- body: `"px-4 py-3 text-sm text-foreground"`
- Replace the `×` close button content with `<X size={16} />` (import `{ X } from "lucide-react"`) and class `"text-faint hover:text-foreground transition-colors"`.

- [ ] **Step 6: Retoken `progress.tsx`**

Apply the migration map: track → `bg-surface-2`, fill → `bg-accent`. (Read the file and swap the two color classes; keep all sizing/markup.)

- [ ] **Step 7: Build**

Run: `cd apps/desktop && pnpm build`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/package.json apps/desktop/pnpm-lock.yaml apps/desktop/src/components/ui
git commit -m "feat(desktop): add lucide-react; retoken + extend ui primitives"
```

---

## Task 4: New UI primitives — status-dot, tooltip, icon-button, badge

**Files:**
- Create: `apps/desktop/src/components/ui/status-dot.tsx`
- Create: `apps/desktop/src/components/ui/tooltip.tsx`
- Create: `apps/desktop/src/components/ui/icon-button.tsx`
- Create: `apps/desktop/src/components/ui/badge.tsx`

- [ ] **Step 1: Create `status-dot.tsx`**

```tsx
// Status dot. Maps a semantic status to a token color and exposes an
// accessible label. Used by session tabs, the sidebar tree, the workarea
// header, CI checks, and the status bar.

export type DotStatus = "ok" | "running" | "warning" | "error" | "idle";

const COLOR: Record<DotStatus, string> = {
  ok: "bg-ok",
  running: "bg-run",
  warning: "bg-warn",
  error: "bg-err",
  idle: "bg-faint",
};

const LABEL: Record<DotStatus, string> = {
  ok: "Active", running: "Running", warning: "Warning",
  error: "Error", idle: "Idle",
};

export function StatusDot({
  status,
  className = "",
}: {
  status: DotStatus;
  className?: string;
}) {
  return (
    <span
      className={`inline-block h-2 w-2 shrink-0 rounded-full ${COLOR[status]} ${className}`}
      role="img"
      aria-label={LABEL[status]}
      title={LABEL[status]}
    />
  );
}
```

- [ ] **Step 2: Create `tooltip.tsx`**

```tsx
// Minimal tooltip — a hover/focus bubble with no external deps (no Radix).
// Wrap any trigger; the label appears on hover and focus-visible.

import { useState, type ReactNode } from "react";

export function Tooltip({
  label,
  side = "right",
  children,
}: {
  label: string;
  side?: "top" | "right" | "bottom" | "left";
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const pos: Record<string, string> = {
    top: "bottom-full left-1/2 -translate-x-1/2 mb-1.5",
    right: "left-full top-1/2 -translate-y-1/2 ml-1.5",
    bottom: "top-full left-1/2 -translate-x-1/2 mt-1.5",
    left: "right-full top-1/2 -translate-y-1/2 mr-1.5",
  };
  return (
    <span
      className="relative inline-flex"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
      onFocus={() => setOpen(true)}
      onBlur={() => setOpen(false)}
    >
      {children}
      {open && (
        <span
          role="tooltip"
          className={`pointer-events-none absolute z-50 whitespace-nowrap rounded-md border border-border bg-surface px-2 py-1 text-xs text-foreground shadow-lg ${pos[side]}`}
        >
          {label}
        </span>
      )}
    </span>
  );
}
```

- [ ] **Step 3: Create `icon-button.tsx`**

```tsx
// Icon button = square Button(size=icon, variant=ghost) wrapped in a
// Tooltip. `label` is both the tooltip text and the accessible name.

import type { ReactNode } from "react";
import { Button, type ButtonProps } from "./button";
import { Tooltip } from "./tooltip";

export type IconButtonProps = Omit<ButtonProps, "size" | "children"> & {
  label: string;
  side?: "top" | "right" | "bottom" | "left";
  children: ReactNode; // the icon
};

export function IconButton({
  label,
  side = "bottom",
  variant = "ghost",
  children,
  ...props
}: IconButtonProps) {
  return (
    <Tooltip label={label} side={side}>
      <Button size="icon" variant={variant} aria-label={label} {...props}>
        {children}
      </Button>
    </Tooltip>
  );
}
```

- [ ] **Step 4: Create `badge.tsx`**

```tsx
// Badge / chip. `neutral` for branch slugs & metadata, `accent` for the
// brand-tinted variant. Defaults to mono since most uses are slugs/IDs.

import type { HTMLAttributes } from "react";

export type BadgeVariant = "neutral" | "accent";

const VARIANTS: Record<BadgeVariant, string> = {
  neutral: "bg-surface-2 text-muted border-border",
  accent: "bg-accent/10 text-accent border-accent/30",
};

export function Badge({
  variant = "neutral",
  className = "",
  ...props
}: HTMLAttributes<HTMLSpanElement> & { variant?: BadgeVariant }) {
  return (
    <span
      className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-xs font-mono ${VARIANTS[variant]} ${className}`}
      {...props}
    />
  );
}
```

- [ ] **Step 5: Build**

Run: `cd apps/desktop && pnpm build`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/components/ui/status-dot.tsx apps/desktop/src/components/ui/tooltip.tsx apps/desktop/src/components/ui/icon-button.tsx apps/desktop/src/components/ui/badge.tsx
git commit -m "feat(desktop): add status-dot, tooltip, icon-button, badge primitives"
```

---

## Task 5: Tabs + Segmented primitives, StatusBar, and wire theme into the layout

**Files:**
- Create: `apps/desktop/src/components/ui/tabs.tsx`
- Create: `apps/desktop/src/components/ui/segmented.tsx`
- Create: `apps/desktop/src/components/StatusBar.tsx`
- Modify: `apps/desktop/src/components/AppLayout.tsx`

- [ ] **Step 1: Create `tabs.tsx` (underline sub-tabs)**

```tsx
// Underline tab strip (Chat/Terminal, Diff/Checks/PR). Generic over the
// tab id string. Active tab gets the accent underline.

export type TabItem<T extends string> = {
  id: T;
  label: string;
  disabled?: boolean;
  title?: string;
};

export function Tabs<T extends string>({
  items,
  active,
  onSelect,
}: {
  items: ReadonlyArray<TabItem<T>>;
  active: T;
  onSelect: (id: T) => void;
}) {
  return (
    <div className="flex items-center gap-1 border-b border-border">
      {items.map((t) => {
        const isActive = t.id === active;
        const cls = isActive
          ? "border-accent text-foreground"
          : t.disabled
            ? "border-transparent text-faint cursor-not-allowed"
            : "border-transparent text-muted hover:text-foreground";
        return (
          <button
            key={t.id}
            type="button"
            disabled={t.disabled}
            title={t.title}
            aria-pressed={isActive}
            onClick={() => !t.disabled && onSelect(t.id)}
            className={`-mb-px border-b-2 px-3 py-1.5 text-xs font-medium transition-colors ${cls}`}
          >
            {t.label}
          </button>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 2: Create `segmented.tsx`**

```tsx
// Segmented control (Split/Unified). Pill background with a raised active
// segment.

export function Segmented<T extends string>({
  items,
  active,
  onSelect,
}: {
  items: ReadonlyArray<{ id: T; label: string }>;
  active: T;
  onSelect: (id: T) => void;
}) {
  return (
    <div className="inline-flex gap-0.5 rounded-md bg-surface-2 p-0.5">
      {items.map((it) => {
        const isActive = it.id === active;
        const cls = isActive
          ? "bg-surface text-foreground shadow-sm"
          : "text-muted hover:text-foreground";
        return (
          <button
            key={it.id}
            type="button"
            aria-pressed={isActive}
            onClick={() => onSelect(it.id)}
            className={`rounded px-2.5 py-1 text-xs font-medium transition-colors ${cls}`}
          >
            {it.label}
          </button>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 3: Create `StatusBar.tsx`**

```tsx
// Bottom status bar (design/15 §3.4). Shows Core connection, current
// branch + active session count for the selected workarea, the permission
// mode, and the theme toggle. Data that isn't surfaced by the Core yet
// shows a static placeholder — wiring those is out of scope for the
// redesign (noted inline).

import { Moon, Sun, MonitorSmartphone, GitBranch } from "lucide-react";
import { useTheme } from "../hooks/useTheme";
import { StatusDot } from "./ui/status-dot";

export function StatusBar(): JSX.Element {
  const { preference, cycle } = useTheme();

  const ThemeIcon =
    preference === "dark" ? Moon : preference === "light" ? Sun : MonitorSmartphone;
  const themeLabel =
    preference === "dark" ? "Dark" : preference === "light" ? "Light" : "System";

  return (
    <footer className="flex h-6 shrink-0 items-center gap-4 border-t border-border bg-surface px-3 text-xs text-muted">
      {/* Connection state: placeholder until the renderer surfaces the
          transport status. Kept green/idle to match the connected default. */}
      <span className="flex items-center gap-1.5">
        <StatusDot status="ok" />
        Core connected
      </span>
      <span className="flex items-center gap-1.5 font-mono">
        <GitBranch size={12} />
        {/* TODO-data: replace with selected workarea branch when surfaced */}
        —
      </span>
      <div className="ml-auto flex items-center gap-4">
        <span>
          Permission: <span className="text-foreground">plan</span>
        </span>
        <button
          type="button"
          onClick={cycle}
          className="flex items-center gap-1.5 text-muted transition-colors hover:text-foreground"
          title={`Theme: ${themeLabel} (click to change)`}
        >
          <ThemeIcon size={13} />
          {themeLabel}
        </button>
      </div>
    </footer>
  );
}
```

> Note: the `—` branch placeholder and static "Core connected"/"plan" are intentional — the renderer doesn't expose transport/permission/branch state to a global component today. They are visually correct placeholders, not logic gaps. A follow-up can wire them to real state.

- [ ] **Step 4: Wrap the layout with the status bar**

Rewrite `AppLayout.tsx`'s return so the `PanelGroup` sits above `StatusBar` in a full-height column, and retoken the resize handles:

```tsx
  return (
    <div className="flex h-full flex-col">
      <PanelGroup
        direction="horizontal"
        className="min-h-0 flex-1"
        onLayout={(sizes) => {
          if (sizes[0] !== undefined) setSidebarWidth(sizes[0]);
        }}
      >
        <Panel defaultSize={sidebarWidth} minSize={12} maxSize={40}>
          <Sidebar />
        </Panel>
        <PanelResizeHandle className="w-px bg-border transition-colors hover:bg-accent/40" />
        <Panel minSize={30}>
          {selectedWorkareaId ? <CenterPanel /> : <WorkspaceDetail />}
        </Panel>
        {!rightRailCollapsed && (
          <PanelResizeHandle className="w-px bg-border transition-colors hover:bg-accent/40" />
        )}
        <Panel
          defaultSize={rightRailCollapsed ? 3 : RIGHT_RAIL_WIDTH}
          minSize={rightRailCollapsed ? 3 : 12}
          maxSize={rightRailCollapsed ? 3 : 40}
        >
          <RightRail />
        </Panel>
      </PanelGroup>
      <StatusBar />
    </div>
  );
```

Add the import: `import { StatusBar } from "./StatusBar";`

- [ ] **Step 5: Build + manual check**

Run: `cd apps/desktop && pnpm build`
Expected: PASS.
Then `pnpm dev`, open the app: a status bar appears at the bottom; clicking the theme button cycles System → Light → Dark and the token-migrated surfaces (App root, primitives) flip. Reload — no flash of the wrong theme (FOUC guard works). Set macOS appearance while on "System" — app follows.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/components/ui/tabs.tsx apps/desktop/src/components/ui/segmented.tsx apps/desktop/src/components/StatusBar.tsx apps/desktop/src/components/AppLayout.tsx
git commit -m "feat(desktop): tabs + segmented primitives, status bar with theme toggle"
```

---

## Task 6: Migrate the Sidebar group (tokens + icons)

**Files:**
- Modify: `apps/desktop/src/components/Sidebar.tsx`
- Modify: `apps/desktop/src/components/WorkareaList.tsx`

- [ ] **Step 1: Retoken + iconify `Sidebar.tsx`**

Apply the migration map to every class in the file, plus these structural edits:
- Imports: `import { ChevronDown, ChevronRight, FolderGit2, Plus, RefreshCw, Settings } from "lucide-react";` and `import { IconButton } from "./ui/icon-button";`
- Header buttons → icon buttons:

```tsx
        <div className="flex gap-0.5">
          <IconButton label="Refresh" onClick={onRefresh}>
            <RefreshCw size={15} />
          </IconButton>
          <IconButton label="Settings" onClick={() => setSettingsOpen(true)}>
            <Settings size={15} />
          </IconButton>
        </div>
```
- The `aside` becomes `className="h-full border-r border-border bg-surface flex flex-col min-h-0"`.
- The brand `h1`: `className="text-sm font-semibold tracking-wide text-foreground"`.
- "Workspaces" header `+` button → `<IconButton label="New workspace" onClick={() => setNewWorkspaceModalOpen(true)}><Plus size={14} /></IconButton>`.
- Section labels (`Project`, `Workspaces`): `className="px-2 text-xs uppercase tracking-wide text-faint mb-1"`.
- In `WorkspaceNode`, replace the `▾`/`▸` text button content with `{expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}` and class `"text-faint hover:text-foreground"`; add a `<FolderGit2 size={14} className="text-faint" />` before the name. Active row: `"flex-1 text-left px-2 py-1 rounded-md text-sm bg-accent/10 text-foreground"`; inactive: `"flex-1 text-left px-2 py-1 rounded-md text-sm text-muted hover:bg-surface-2"`. Slug line: `text-faint`. Wrap the name in `<span className="font-mono">`.
- Error lines `text-rose-400` → `text-err`; loading/empty `text-slate-500` → `text-faint`.

- [ ] **Step 2: Retoken + iconify `WorkareaList.tsx`**

Read the file, apply the migration map, and add a `<StatusDot>` for each workarea's status. Import `{ StatusDot } from "./ui/status-dot";`. Map the workarea status field to a `DotStatus` (e.g. `active`/`running` → `running`, `idle` → `idle`, failures → `error`). Selected workarea row uses `bg-accent/10 text-foreground`; others `text-muted hover:bg-surface-2`. Replace any `+ new workarea` text button styling per the button primitive.

- [ ] **Step 3: Build + visual check**

Run: `cd apps/desktop && pnpm build`
Expected: PASS. In `pnpm dev`, the sidebar shows icons + chevrons, the active workspace/workarea has the indigo soft-fill, in both themes.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/components/Sidebar.tsx apps/desktop/src/components/WorkareaList.tsx
git commit -m "feat(desktop): retoken + iconify sidebar tree"
```

---

## Task 7: Migrate the Center group (tokens, icons, tabs/segmented, empty states)

**Files:**
- Modify: `apps/desktop/src/components/CenterPanel.tsx`
- Modify: `apps/desktop/src/components/WorkspaceDetail.tsx`
- Modify: `apps/desktop/src/components/center/SessionRegion.tsx`
- Modify: `apps/desktop/src/components/SessionTab.tsx`
- Modify: `apps/desktop/src/components/SessionComposer.tsx`
- Modify: `apps/desktop/src/components/center/CodePrRegion.tsx`
- Modify: `apps/desktop/src/components/center/FileListSidebar.tsx`

- [ ] **Step 1: Retoken `CenterPanel.tsx` + `WorkspaceDetail.tsx`**

Apply the migration map to all classes in both files. For `WorkspaceDetail` (the JSON workspace view), keep behavior; retoken surfaces/borders/text and wrap any code/JSON in `font-mono`.

- [ ] **Step 2: Migrate `SessionRegion.tsx`**

- Apply the migration map.
- Replace the inline `SubTabHeader` with the `Tabs` primitive:

```tsx
import { Tabs } from "../ui/tabs";
// ...
function SubTabHeader(): JSX.Element {
  return (
    <Tabs
      items={[
        { id: "terminal", label: "Terminal" },
        { id: "chat", label: "Chat", disabled: true, title: "Chat view comes in V1.0" },
      ]}
      active="terminal"
      onSelect={() => {}}
    />
  );
}
```
- "Sessions:" label → `text-xs uppercase tracking-wide text-faint`.
- Empty state: replace the dashed box with an icon + copy:

```tsx
import { TerminalSquare } from "lucide-react";
// ...
          <div className="flex-1 min-h-0 flex flex-col items-center justify-center gap-2 text-faint text-sm border border-dashed border-border rounded-lg">
            <TerminalSquare size={28} />
            No sessions yet. Click “+ Start Session”.
          </div>
```
- The `+ Start Session` button → `<Button variant="outline" size="sm">`; `Stop Session` → `<Button variant="ghost" size="sm">`. Error text → `text-err`.

- [ ] **Step 3: Migrate `SessionTab.tsx` with StatusDot**

Read the file. Replace the hand-rolled status dot (the colored span, likely `bg-rose-*`/emerald) with `<StatusDot status={...} />` mapping the session `status` field: `running`/`starting` → `running`, `awaiting` → `warning`, `exited`/`stopped` → `idle`, errors → `error`. Tab container active: `"border-accent bg-accent/10 text-foreground"`, inactive: `"border-border bg-surface text-muted hover:bg-surface-2"`, all `rounded-md`. Session id span → `font-mono text-faint`. Import `{ StatusDot } from "./ui/status-dot";`.

- [ ] **Step 4: Migrate `SessionComposer.tsx`**

Apply the migration map. Textarea/input → token classes with `focus-visible:ring-accent`; placeholder → `text-faint`. Send button → `<Button variant="primary">` with a `<Send size={14} />` from lucide. The `⌘+Enter` hint text → `text-faint`.

- [ ] **Step 5: Migrate `CodePrRegion.tsx`**

Apply the migration map. Replace the `Diff / Checks / PR` sub-tab strip with `Tabs`, and the `Split / Unified` toggle with `Segmented` driven by `diffViewMode` from the store:

```tsx
import { Segmented } from "../ui/segmented";
// ...
<Segmented
  items={[{ id: "split", label: "Split" }, { id: "unified", label: "Unified" }]}
  active={diffViewMode}
  onSelect={setDiffViewMode}
/>
```
`Refresh` → `<Button variant="outline" size="sm">` with `<RefreshCw size={13} />`. "N files" label → `text-faint`. Per-repo tab dot → `<StatusDot>`. Add a `Create PR` primary button placeholder if the file has one; keep existing handlers.

- [ ] **Step 6: Migrate `FileListSidebar.tsx`**

Apply the migration map. File rows: `<FileText size={14} />` (lucide) before the name; active row `bg-accent/10 text-foreground`, others `text-muted hover:bg-surface-2`; add/del counts colored `text-ok` / `text-err`. "No changed files." empty state → centered `text-faint`.

- [ ] **Step 7: Build + visual check**

Run: `cd apps/desktop && pnpm build`
Expected: PASS. In dev, the center panel shows underline sub-tabs, segmented Split/Unified, status-dotted session tabs, indigo Send, and polished empty states in both themes.

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src/components/CenterPanel.tsx apps/desktop/src/components/WorkspaceDetail.tsx apps/desktop/src/components/center/SessionRegion.tsx apps/desktop/src/components/SessionTab.tsx apps/desktop/src/components/SessionComposer.tsx apps/desktop/src/components/center/CodePrRegion.tsx apps/desktop/src/components/center/FileListSidebar.tsx
git commit -m "feat(desktop): retoken center panel — tabs, segmented, status dots, empty states"
```

---

## Task 8: Migrate the Right Rail group (icon nav strip + tokens)

**Files:**
- Modify: `apps/desktop/src/components/RightRail.tsx`
- Modify: `apps/desktop/src/components/right-rail/SchedulerTab.tsx`
- Modify: `apps/desktop/src/components/right-rail/SkillsTab.tsx`
- Modify: `apps/desktop/src/components/right-rail/TodosTab.tsx`
- Modify: `apps/desktop/src/components/right-rail/McpTab.tsx`
- Modify: `apps/desktop/src/components/right-rail/FilesTab.tsx`

- [ ] **Step 1: Replace the abbreviation nav with icons in `RightRail.tsx`**

- Extend `TabSpec` to carry an icon component and drop `short`:

```tsx
import { Clock, Sparkles, ListChecks, Blocks, Folder } from "lucide-react";
import { Tooltip } from "./ui/tooltip";
import type { ComponentType } from "react";

type TabSpec = { id: RightRailTab; label: string; Icon: ComponentType<{ size?: number }> };

const TABS: readonly TabSpec[] = [
  { id: "scheduler", label: "Scheduler", Icon: Clock },
  { id: "skills", label: "Skills", Icon: Sparkles },
  { id: "todos", label: "Todos", Icon: ListChecks },
  { id: "mcp", label: "MCP", Icon: Blocks },
  { id: "files", label: "Files", Icon: Folder },
];
```
- Render each nav button as an icon inside a `Tooltip`, active = accent:

```tsx
        {TABS.map((t) => {
          const isActive = t.id === activeTab && !collapsed;
          const cls = isActive
            ? "relative grid h-9 w-11 place-items-center text-accent bg-accent/10"
            : "relative grid h-9 w-11 place-items-center text-muted hover:bg-surface-2 hover:text-foreground";
          return (
            <Tooltip key={t.id} label={t.label} side="left">
              <button type="button" className={cls} onClick={() => onTabClick(t.id)} aria-pressed={isActive} aria-label={t.label}>
                {isActive && <span className="absolute left-0 top-1.5 bottom-1.5 w-0.5 rounded-full bg-accent" />}
                <t.Icon size={17} />
              </button>
            </Tooltip>
          );
        })}
```
- Apply the migration map to the `aside`, drawer, and header (`border-l border-border bg-surface`; header title `text-xs uppercase tracking-wide text-muted`).

- [ ] **Step 2: Retoken the five tab bodies**

For each of `SchedulerTab.tsx`, `SkillsTab.tsx`, `TodosTab.tsx`, `McpTab.tsx`, `FilesTab.tsx`: read the file, apply the migration map to all classes, wrap any command hints (e.g. `/loop`) or IDs in `font-mono text-accent`/`font-mono`, and give empty-state copy `text-faint`.

- [ ] **Step 3: Build + visual check**

Run: `cd apps/desktop && pnpm build`
Expected: PASS. The right rail nav shows five icons with tooltips; the active one has the indigo bar + soft fill; bodies are themed.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src/components/RightRail.tsx apps/desktop/src/components/right-rail
git commit -m "feat(desktop): icon nav strip + retoken right rail tabs"
```

---

## Task 9: Migrate Modals & misc (tokens)

**Files:**
- Modify: `apps/desktop/src/components/NewWorkspaceModal.tsx`
- Modify: `apps/desktop/src/components/SettingsPanel.tsx`
- Modify: `apps/desktop/src/components/AddRepositoryForm.tsx`
- Modify: `apps/desktop/src/components/StartSessionPicker.tsx`
- Modify: `apps/desktop/src/components/Toast.tsx`

- [ ] **Step 1: Retoken each file**

For all five files: read, apply the migration map to every class. Use the upgraded `Button` variants for actions (primary for confirm/submit, outline/ghost for cancel). Inputs/selects → token border + `focus-visible:ring-accent` + `placeholder:text-faint`. Toasts: success → `border-ok/40 bg-ok/10 text-foreground`, error → `border-err/40 bg-err/10 text-foreground`, info → `bg-surface border-border`. Wrap repo paths / branch names / IDs in `font-mono`.

- [ ] **Step 2: Build + visual check**

Run: `cd apps/desktop && pnpm build`
Expected: PASS. Open New Workspace, Settings/Add Repository, Start Session picker, and trigger a toast — all themed in light + dark.

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/components/NewWorkspaceModal.tsx apps/desktop/src/components/SettingsPanel.tsx apps/desktop/src/components/AddRepositoryForm.tsx apps/desktop/src/components/StartSessionPicker.tsx apps/desktop/src/components/Toast.tsx
git commit -m "feat(desktop): retoken modals, settings, session picker, toasts"
```

---

## Task 10: Theme-aware xterm terminal

**Files:**
- Modify: `apps/desktop/src/components/SessionTerminal.tsx`

- [ ] **Step 1: Drive the xterm theme from `useTheme` + `THEME_COLORS`**

`SessionTerminal.tsx::XTERM_OPTIONS` hardcodes `background: "#0f172a"`, `foreground: "#e2e8f0"`. Replace with theme-derived values and update live on theme change.

- Add imports: `import { useTheme } from "../hooks/useTheme";` and `import { THEME_COLORS } from "../theme/tokens";`.
- Build the xterm `ITheme` from the active theme:

```tsx
function xtermTheme(effective: "light" | "dark") {
  const c = THEME_COLORS[effective];
  return {
    background: c.surface,
    foreground: c.foreground,
    cursor: c.accent,
    cursorAccent: c.surface,
    selectionBackground: effective === "dark" ? "#33415580" : "#c7d2fe80",
  };
}
```
- In the component, read `const { effective } = useTheme();`. Pass `theme: xtermTheme(effective)` into the `XTERM_OPTIONS` used at construction (make `XTERM_OPTIONS` a function of `effective`, or spread the theme in at `new Terminal({ ...XTERM_OPTIONS, theme: xtermTheme(effective) })`).
- Add an effect that re-applies the theme when `effective` changes without remounting the terminal:

```tsx
  useEffect(() => {
    const term = terminalRef.current; // whatever the existing ref is named
    if (term) term.options.theme = xtermTheme(effective);
  }, [effective]);
```
(Use the file's existing terminal instance ref; if the instance is in a local variable inside an effect, store it in a ref so this effect can reach it.)

- The `.xterm-viewport { background-color: transparent !important; }` rule in `index.css` already lets the panel's `bg-surface` show through — keep it; ensure the wrapping panel div uses `bg-surface`.

- [ ] **Step 2: Build + visual check**

Run: `cd apps/desktop && pnpm build`
Expected: PASS. Start/open a session, toggle theme: the terminal background + text follow the theme with no remount/flicker.

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/components/SessionTerminal.tsx
git commit -m "feat(desktop): theme-aware xterm terminal colors"
```

---

## Task 11: Theme-aware Monaco diff viewer

**Files:**
- Modify: `apps/desktop/src/components/center/DiffViewer.tsx`

- [ ] **Step 1: Bind the Monaco theme to the effective theme**

`DiffViewer.tsx` hardcodes `theme="vs-dark"`. Make it follow the app theme.

- Add `import { useTheme } from "../../hooks/useTheme";`.
- In the component: `const { effective } = useTheme();`.
- Change the Monaco editor's `theme` prop to `theme={effective === "dark" ? "vs-dark" : "vs"}`.
- Optional (only if the default `vs` white clashes with `--surface`): define a custom theme on mount via the `beforeMount`/`onMount` handler so the editor background matches `--surface`:

```tsx
  function handleMount(_editor: unknown, monaco: typeof import("monaco-editor")) {
    monaco.editor.defineTheme("concerto-light", {
      base: "vs", inherit: true, rules: [],
      colors: { "editor.background": "#ffffff" },
    });
    monaco.editor.defineTheme("concerto-dark", {
      base: "vs-dark", inherit: true, rules: [],
      colors: { "editor.background": "#161b22" },
    });
  }
```
…and set `theme={effective === "dark" ? "concerto-dark" : "concerto-light"}`, wiring `onMount={handleMount}` (compose with any existing mount handler — do not drop existing logic). If the file uses `@monaco-editor/react`'s `DiffEditor`, the same `theme` prop and `onMount(editor, monaco)` signature apply.

- [ ] **Step 2: Build + visual check**

Run: `cd apps/desktop && pnpm build`
Expected: PASS. Open a workarea with a diff, toggle theme: Monaco flips between light/dark, background matches the surrounding panel.

- [ ] **Step 3: Commit**

```bash
git add apps/desktop/src/components/center/DiffViewer.tsx
git commit -m "feat(desktop): theme-aware Monaco diff viewer"
```

---

## Task 12: Final sweep — residual-color grep gate + full two-theme verification

**Files:** none (verification + any stragglers found)

- [ ] **Step 1: Grep for residual hardcoded palette usage**

Run:
```bash
cd apps/desktop && grep -rn "slate-\|rose-\|emerald-\|sky-\|zinc-\|gray-\|#0f172a\|#e2e8f0\|vs-dark\b" src || echo "CLEAN"
```
Expected: `CLEAN`, OR only intentional hits (e.g. the `vs-dark` inside the `effective === "dark" ? "vs-dark"` ternary in DiffViewer, and `#161b22` inside `tokens.ts`/Monaco custom theme). Any other hit → fix that file using the migration map, rebuild, and re-run.

- [ ] **Step 2: Confirm root no longer forces mono**

Run: `cd apps/desktop && grep -rn "font-mono" src/App.tsx || echo "ROOT IS SANS"`
Expected: `ROOT IS SANS` (mono now only appears on code/terminal/ID spans in components).

- [ ] **Step 3: Full build**

Run: `cd apps/desktop && pnpm build`
Expected: PASS (tsc `--noEmit` + Vite build clean).

- [ ] **Step 4: Manual two-theme walkthrough**

`pnpm dev`, then for **each** of light, dark, and system: sidebar tree, workarea center (session tabs, sub-tabs, terminal, composer), Code & PRs (diff list + Monaco), right rail (all five tabs), every modal (New Workspace, Settings, Start Session), a toast, and the empty states. Confirm: no unreadable/low-contrast text, no leftover dark-only surface, accent is indigo everywhere interactive, status dots use status colors (not accent), toggle persists across reload with no FOUC.

- [ ] **Step 5: Smoke gate (don't regress V0.1)**

Run: `cd /Users/amin/conductor/workspaces/concerto/curitiba && ./scripts/smoke.sh` (if it builds the desktop frontend it must still pass). Expected: PASS. If the script doesn't cover the desktop UI, note that and rely on `pnpm build`.

- [ ] **Step 6: Commit any stragglers**

```bash
git add -A apps/desktop/src
git commit -m "fix(desktop): final token sweep — remove residual hardcoded colors"
```

---

## Self-Review

**Spec coverage:**
- §2.1 token CSS vars → Task 1. §2.2 Tailwind extension → Task 1. §2.3 migration rule → migration map + Tasks 6–9.
- §3 typography (sans root, mono for code) → Task 1 Step 4 + per-component mono spans + Task 12 Step 2 gate.
- §4 icons (lucide, chevrons, rail icons, button icons, file icons) → Tasks 3,6,7,8.
- §5.1 primitives upgrade + new primitives → Tasks 3,4,5.
- §5.2 theme controller + FOUC + toggle → Tasks 1 (FOUC), 2 (hook), 5 (toggle UI).
- §5.3 status bar → Task 5.
- §5.4 layout polish (handles, sidebar, rail, tabs, empty states) → Tasks 5,6,7,8.
- §5.5 xterm + Monaco → Tasks 10, 11.
- §6 file scope → covered across Tasks 1,3,6–11 (all 30 listed files touched).
- §7 verification → Task 12. §8 risks (lucide license, residual colors, FOUC, batching, xterm/Monaco) → addressed in Tasks 3,12,1, batching structure, 10–11.

**Placeholder scan:** The only "TODO"/placeholder is the StatusBar branch/connection data, which is explicitly called out as an intentional visual placeholder (renderer doesn't expose that state), not a missing implementation. No "implement later"/"add error handling"-style gaps.

**Type consistency:** `ThemePreference`/`EffectiveTheme`/`resolveTheme`/`THEME_STORAGE_KEY` (Task 2) are used identically in `useTheme` (Task 2), `StatusBar` (Task 5), `SessionTerminal` (Task 10), `DiffViewer` (Task 11). `THEME_COLORS` shape (Task 2) is consumed by `xtermTheme` (Task 10). `DotStatus` (Task 4) is the type mapped in Tasks 6,7. `Button` `variant`/`size` (Task 3) used consistently. `Tabs`/`Segmented` generic signatures (Task 5) match call sites in Task 7. No name drift found.
