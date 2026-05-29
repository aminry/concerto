# Desktop UI Redesign — Design Spec

**Date:** 2026-05-29
**Status:** Approved (design); pending implementation plan
**Scope:** `apps/desktop` (Tauri + React renderer). No Core/daemon changes.

---

## 1. Goal

Redesign the Concerto desktop app from its current high-contrast, monospace-everywhere
dark-only look into a modern, professional IDE aesthetic with **first-class light and
dark themes**. The layout structure (three-panel shell from `design/15 §3.4`) is sound and
stays; this is a visual + token-architecture overhaul plus targeted "IDE polish" gaps,
not a structural rewrite.

### Locked decisions (from brainstorming)

| Decision | Choice |
|----------|--------|
| Visual direction | **Modern IDE, own identity** — inspired by Conductor / Linear / VS Code, not a clone |
| Theme model | **Follow OS (`prefers-color-scheme`) by default + manual override toggle.** Both themes first-class |
| Accent color | **Indigo / violet** (`#6366f1` dark / `#5b5ef0` light) |
| Typography | **System sans for UI** (`-apple-system` stack), **monospace for code/terminal/IDs only** |
| Theming architecture | **Approach A** — semantic CSS-variable tokens + Tailwind theme extension. *Not* a full shadcn/Radix install |
| Scope | **Reskin + IDE polish** (icons, status bar, theme toggle, polished states). No new features (no top chat bar / command palette) |

### Non-goals

- No backend / Core / gRPC changes.
- No new product features (top Concerto chat bar, ⌘K palette, multi-session) — explicitly deferred.
- No full shadcn/ui CLI install or Radix dependency (token model adopts shadcn naming so a
  future migration stays trivial, but we keep the hand-rolled primitives).
- No Tailwind v4 upgrade (v3 is pinned for license reasons per `tailwind.config.ts`).

---

## 2. Theming architecture (Approach A)

### 2.1 Token layer

Define semantic design tokens as CSS custom properties in `src/index.css`, scoped by a
`data-theme` attribute (and/or `.dark` class) on `<html>`. A single attribute flip
re-themes the entire app.

```css
:root, :root[data-theme="light"] {
  --background: 247 248 250;     /* #f7f8fa  app canvas */
  --surface:    255 255 255;     /* #ffffff  panels, cards, sidebar */
  --surface-2:  246 247 249;     /* #f6f7f9  hover / raised */
  --raised:     236 238 242;     /* #eceef2  pressed / segmented bg */
  --border:     229 231 235;     /* #e5e7eb  subtle dividers */
  --border-strong: 211 215 222;  /* #d3d7de  stronger dividers / inputs */
  --foreground: 31 35 40;        /* #1f2328  primary text */
  --muted:      101 109 118;     /* #656d76  secondary text */
  --faint:      140 149 159;     /* #8c959f  tertiary / placeholder */
  --accent:     91 94 240;       /* #5b5ef0 */
  --accent-hover: 75 78 230;     /* #4b4ee6 */
  --accent-fg:  255 255 255;
  --accent-soft: 91 94 240;      /* used at low alpha for active fills */
  --ok:   26 127 55;             /* #1a7f37 */
  --warn: 154 103 0;             /* #9a6700 */
  --err:  207 34 46;             /* #cf222e */
  --run:  9 105 218;             /* #0969da */
}

:root[data-theme="dark"], .dark {
  --background: 13 17 23;        /* #0d1117 */
  --surface:    22 27 34;        /* #161b22 */
  --surface-2:  28 33 40;        /* #1c2128 */
  --raised:     33 38 45;        /* #21262d */
  --border:     42 47 55;        /* #2a2f37 */
  --border-strong: 55 62 71;     /* #373e47 */
  --foreground: 230 237 243;     /* #e6edf3 */
  --muted:      139 148 158;     /* #8b949e */
  --faint:      110 118 129;     /* #6e7681 */
  --accent:     99 102 241;      /* #6366f1 */
  --accent-hover: 124 127 244;   /* #7c7ff4 */
  --accent-fg:  255 255 255;
  --accent-soft: 99 102 241;
  --ok:   63 185 80;             /* #3fb950 */
  --warn: 210 153 34;            /* #d29922 */
  --err:  248 81 73;             /* #f85149 */
  --run:  88 166 255;            /* #58a6ff */
}
```

Tokens are stored as **space-separated RGB channels** (not hex) so Tailwind can apply
opacity modifiers (`bg-accent/15`, `text-muted/60`). The `--accent-soft` active-row fill
is expressed as `bg-accent/10`–`/15`.

### 2.2 Tailwind theme extension

`tailwind.config.ts` maps utilities to the tokens via the `rgb(var(--x) / <alpha-value>)`
pattern, and enables class-based dark mode for the toggle:

```ts
const config: Config = {
  darkMode: ["class", '[data-theme="dark"]'],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        background: "rgb(var(--background) / <alpha-value>)",
        surface:    "rgb(var(--surface) / <alpha-value>)",
        "surface-2":"rgb(var(--surface-2) / <alpha-value>)",
        raised:     "rgb(var(--raised) / <alpha-value>)",
        border:     "rgb(var(--border) / <alpha-value>)",
        "border-strong": "rgb(var(--border-strong) / <alpha-value>)",
        foreground: "rgb(var(--foreground) / <alpha-value>)",
        muted:      "rgb(var(--muted) / <alpha-value>)",
        faint:      "rgb(var(--faint) / <alpha-value>)",
        accent: {
          DEFAULT: "rgb(var(--accent) / <alpha-value>)",
          hover:   "rgb(var(--accent-hover) / <alpha-value>)",
          fg:      "rgb(var(--accent-fg) / <alpha-value>)",
        },
        ok:   "rgb(var(--ok) / <alpha-value>)",
        warn: "rgb(var(--warn) / <alpha-value>)",
        err:  "rgb(var(--err) / <alpha-value>)",
        run:  "rgb(var(--run) / <alpha-value>)",
      },
      fontFamily: {
        sans: ['-apple-system','BlinkMacSystemFont','"SF Pro Text"','"Segoe UI"','system-ui','sans-serif'],
        mono: ['ui-monospace','"SF Mono"','"JetBrains Mono"','Menlo','monospace'],
      },
      borderRadius: { lg: "10px", md: "8px", sm: "6px" },
    },
  },
};
```

### 2.3 Migration rule

Replace hardcoded palette classes with semantic tokens across all renderer files:

| Old | New |
|-----|-----|
| `bg-slate-950` | `bg-background` |
| `bg-slate-900` | `bg-surface` |
| `bg-slate-800` (hover/raised) | `bg-surface-2` / `hover:bg-surface-2` |
| `border-slate-800/700` | `border-border` / `border-border-strong` |
| `text-slate-100/200` | `text-foreground` |
| `text-slate-300/400` | `text-muted` |
| `text-slate-500` | `text-faint` |
| `text-rose-400` | `text-err` |
| `border-emerald-500` (active rail) | `border-accent` / `text-accent` |
| `focus:ring-slate-500` | `focus:ring-accent` |
| root `font-mono` | `font-sans` (mono applied only to code/terminal/ID spans) |

30 files reference these palette classes today (audited list in §6).

---

## 3. Typography

- Root (`App.tsx`) switches from `font-mono` to `font-sans`.
- Monospace (`font-mono`) is applied explicitly only to: xterm terminal, Monaco diff,
  session/workarea IDs, branch slugs, code snippets, and the `/loop`-style command hints.
- No bundled web fonts — the system stack renders SF Pro on macOS (zero load cost).
- Establish a small type scale via existing Tailwind sizes: `text-xs` (11–12px) for labels/chrome,
  `text-sm` (13px) body, `text-base` for headings. Uppercase tracked labels keep `tracking-wide`.

---

## 4. Icon system

Add **`lucide-react`** (tree-shakeable SVG icon set, permissive ISC license — must pass
`cargo-deny`/license policy; verify in plan).

Replace text/unicode affordances:

- Sidebar expand/collapse `▸`/`▾` → `ChevronRight`/`ChevronDown`.
- Project / workspace / workarea rows → `Box` / `FolderGit2` / branch glyph.
- Header `Refresh` / `Settings` text buttons → `RefreshCw` / `Settings` icon buttons with tooltips.
- Right-rail `Sch`/`Skl`/`Tdo`/`MCP`/`Fil` abbreviations → `Clock` / `Sparkles` / `ListChecks`
  / `Blocks` / `Folder` icons with hover tooltips (label still in `title` + accessible name).
- Buttons gain leading icons where meaningful (Create PR → `GitPullRequest`, Send → `Send` or ⌘↵ hint).
- File-list rows → `FileText` + add/del counts colored with `text-ok` / `text-err`.

---

## 5. Component & layout work

### 5.1 UI primitives (`src/components/ui/`)

Upgrade existing hand-rolled primitives to consume tokens and gain variants:

- **`button.tsx`** — variants `default | primary | ghost | outline | danger`; sizes `sm | md | icon`.
  `primary` = `bg-accent text-accent-fg hover:bg-accent-hover`.
- **`card.tsx`**, **`input.tsx`**, **`dialog.tsx`**, **`progress.tsx`** — retoken to `surface`/`border`/`foreground`;
  inputs use `focus:ring-accent`.

New primitives (small, hand-rolled, same house style):

- **`tabs.tsx`** — underline-style sub-tabs (Chat/Terminal, Diff/Checks/PR).
- **`segmented.tsx`** — segmented control (Split/Unified, view modes).
- **`tooltip.tsx`** — lightweight hover tooltip for icon-only buttons (CSS/title-based or minimal JS; no Radix).
- **`status-dot.tsx`** — maps session/workarea/CI status → `ok/warn/err/run/faint` token + accessible label.
- **`icon-button.tsx`** — square icon button with hover bg + tooltip.
- **`badge.tsx` / `chip.tsx`** — branch chip, file counts, status pills.

### 5.2 Theme controller

- **`useTheme` hook + `theme.ts`** — resolves effective theme: `system | light | dark`
  (default `system`). Reads `window.matchMedia('(prefers-color-scheme: dark)')`, listens for
  OS changes, writes `data-theme` to `document.documentElement`, persists the user override in
  `localStorage`. Persisted alongside layout under a small `concerto.theme.v1` key (or folded
  into the existing `concerto.layout.v1` shape — decided in plan).
- An inline `<script>` in `index.html` sets `data-theme` **before first paint** to avoid a
  flash of the wrong theme (FOUC) on launch.
- Toggle UI lives in the new **status bar** (cycles system → light → dark, or simple light/dark
  switch — finalized in plan).

### 5.3 Status bar (new — `StatusBar.tsx`)

Currently missing; specified in `design/15 §3.4`. A 26px bottom bar showing:
Core connection state (color-coded dot), current branch, active session count,
permission mode, and the theme toggle on the right. Wired to existing query/event data
where available; static placeholders where the data isn't surfaced yet (noted in plan).

### 5.4 Layout polish (`AppLayout.tsx` + panels)

- Resize handles: thinner, `bg-border hover:bg-accent/40 transition-colors`.
- Sidebar tree: icon + chevron rows, indigo soft-fill active state, tighter row rhythm.
- Right rail: icon nav strip + indigo active indicator bar.
- Session tabs: rounded token-bordered tabs, active = `border-accent bg-accent/10`.
- Empty states ("No sessions yet", "No changed files", "No changes"): centered, icon + muted
  copy, dashed `border-border` — consistent treatment via a small `EmptyState` helper.

### 5.5 Third-party surface theming

- **xterm** (`SessionTerminal.tsx::XTERM_OPTIONS`) — replace hardcoded `#0f172a/#e2e8f0`
  theme object with values derived from the active theme; recompute the xterm `ITheme`
  (background/foreground/cursor/selection/ANSI) on theme change and call `terminal.options`.
  Background reads `--surface`.
- **Monaco** (`DiffViewer.tsx`) — swap hardcoded `theme="vs-dark"` for an effective-theme
  binding: `vs` in light, `vs-dark` in dark (optionally a lightly-customized theme so the
  editor background matches `--surface`). Re-set theme on toggle.

---

## 6. File-level scope (audited)

30 renderer files reference palette/mono/hardcoded-theme tokens today and will be migrated:

```
index.css, App.tsx, AppLayout.tsx, CenterPanel.tsx, Sidebar.tsx, RightRail.tsx,
WorkareaList.tsx, WorkspaceDetail.tsx, SessionTab.tsx, SessionComposer.tsx,
SessionTerminal.tsx, StartSessionPicker.tsx, NewWorkspaceModal.tsx, SettingsPanel.tsx,
AddRepositoryForm.tsx, Toast.tsx,
center/CodePrRegion.tsx, center/DiffViewer.tsx, center/FileListSidebar.tsx, center/SessionRegion.tsx,
right-rail/SchedulerTab.tsx, right-rail/SkillsTab.tsx, right-rail/TodosTab.tsx,
right-rail/McpTab.tsx, right-rail/FilesTab.tsx,
ui/button.tsx, ui/card.tsx, ui/input.tsx, ui/dialog.tsx, ui/progress.tsx
```

New files: `tailwind.config.ts` (extend), `src/theme/theme.ts`, `src/hooks/useTheme.ts`,
`src/components/StatusBar.tsx`, plus new `ui/` primitives (tabs, segmented, tooltip,
status-dot, icon-button, badge). `index.html` gets the pre-paint theme script.

---

## 7. Verification

- `pnpm build` (tsc `--noEmit` + vite) passes; no TypeScript errors.
- Manual: launch app, confirm both themes render correctly, toggle flips instantly with no
  FOUC on reload, OS-preference change is respected when set to `system`.
- xterm and Monaco both re-theme on toggle (no stale dark editor in light mode).
- Existing smoke gate (`scripts/` smoke v3) still passes — redesign must not break the
  V0.1 coverage referenced in recent commits.
- Spot-check every panel/empty-state/modal in both themes for contrast and missed
  hardcoded colors (grep for residual `slate-`/`rose-`/`emerald-`/`#0f172a` returns nothing).

---

## 8. Risks & mitigations

- **`lucide-react` license** — must clear the repo's `cargo-deny`/license policy. *Mitigation:*
  verify license (ISC, expected fine) in the plan's first step; fall back to a tiny hand-rolled
  SVG icon module if it's blocked.
- **Missed hardcoded colors** — a stray `slate-*` ruins a theme. *Mitigation:* final grep gate
  in verification; consider an ESLint rule banning raw palette classes (optional).
- **FOUC on launch** — *Mitigation:* pre-paint inline theme script in `index.html`.
- **Token churn across 30 files** — large but mechanical diff. *Mitigation:* land the token
  layer + primitives first, then migrate panels in reviewable batches (sidebar / center /
  right-rail / modals).
- **xterm/Monaco re-theming** — easy to forget the dynamic update path. *Mitigation:* explicit
  verification step; theme value flows from a single `useTheme` source.
