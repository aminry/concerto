# rn-diff-spike — throwaway React Native diff-viewer perf harness (Task 103)

A self-contained **Expo** app that renders a representative unified diff with
the touch interactions V1.0's mobile diff surface needs — **expand/collapse
hunks, smooth virtualized scroll, syntax-ish colouring** — so the operator can
measure it against the V1.0 bar on **real hardware**:

> **1000-line diff rendered in < 1.5 s, scrolling at 60 fps on iPhone 13+ /
> Pixel 6+** (`design/16 §10`, PRD §22.3, R-1/R-7).

This harness is **throwaway**. It is **not** the production `apps/mobile`
(Task 508) and not the production diff renderer (Task 514). It exists only to
produce the numbers in `design/spikes/rn-diff-findings.md`.

> **Why the verdict is deferred (operator decision: Option A).** The headline
> metric is *sustained 60 fps on a real iPhone 13+ / Pixel 6+*. That number
> cannot be credibly produced on a simulator/emulator (no real GPU/thermal
> envelope; the simulator does not render at a device's true frame rate). So
> this task **builds the runnable harness now** and **defers the real-device
> GO/NO-GO to the operator** at the Phase-1 gate. The findings doc carries a
> clearly-labelled **PENDING** verdict, not a fabricated GO/NO-GO.

---

## What it renders & how (the approach Task 514 would ship)

- **Parse** a unified diff (`src/diff/parse.ts`) into files → hunks → lines.
- **Flatten** (`src/diff/flatten.ts`) into a single flat `Row[]` — one
  fixed-height item per file header / hunk header / line. Expand/collapse is
  just *which line rows are in the flat array*; tokens are memoized so
  re-flatten on toggle is cheap.
- **Virtualize** (`src/ui/DiffViewer.tsx`) with **`@shopify/flash-list` v2**,
  which recycles row views and mounts only the visible window (+ overscan). A
  10k-line diff mounts ~30–50 row views at a time, **never 10k** — an
  un-virtualized render of that size would trivially fail and tell us nothing
  (`design/16 §3.7`, Task 103 implementation notes).
- **Syntax-ish colouring** (`src/diff/syntax.ts`) — a tiny one-pass tokenizer
  (keyword/string/comment/number/punct). Production uses
  `react-native-syntax-highlighter`; this is just enough to load the renderer
  with realistic per-line work. Toggle it in-app to isolate its cost.

## Fixtures

Generated programmatically, deterministically (seeded), at two sizes
(`src/fixtures/generate.ts`):

- **`~1k lines`** — the budget target.
- **`~10k lines`** — the large diff used to find the performance cliff.

Generated rather than committed-static so the cliff size is tunable and the
repo carries no megabyte of fixture text. (Verified off-device: 1000/10000
lines parse exactly to 1030/10292 flat rows; collapse-all reduces to just the
file rows.)

## The two numbers, in-app

A live HUD (top-right, `src/ui/PerfHud.tsx`) shows:

- **render** — wall-clock from the fixture button press to the list's first
  committed content frame (includes parse + flatten + tokenize). Green if
  ≤ 1.5 s, red otherwise.
- **draw** — FlashList's own reported native draw time (`onLoad`).
- **build** — parse + flatten + tokenize CPU time (the JS-side subset).
- **fps** + **min** — a `requestAnimationFrame` JS-frame counter and the
  worst-case 1-second dip. Green if ≥ ~60.

> ⚠️ The in-app **fps is the JS-thread frame rate** — a useful proxy on the New
> Architecture but **NOT** the authoritative scroll-smoothness number, which
> runs on the UI thread. For the real verdict, profile with the tools below.

---

## Run it on a real device (what the operator does)

Prerequisites: Node ≥ 20, `pnpm`, Xcode (iOS) / Android Studio + SDK (Android),
a real iPhone 13+ and/or Pixel 6+ in developer mode connected by cable, and the
Expo tooling (installed transitively).

```sh
pnpm -C spikes/rn-diff install
```

### iPhone 13 or newer (release build — required for a real fps number)

```sh
# Plug in the device, trust the Mac, then:
pnpm -C spikes/rn-diff exec expo run:ios --device --configuration Release
```

A **Release** build matters — a Debug JS bundle understates fps. In the app:

1. Tap **~1k lines**, read the **render** number (vs 1.5 s).
2. Flick-scroll hard top-to-bottom several times; read **fps** / **min**.
3. Repeat with **~10k lines** to find the cliff.
4. Toggle **syntax** and expand/collapse file headers to confirm interactions
   stay smooth.

**Authoritative fps:** attach **Xcode → Product → Profile → Instruments →
Core Animation FPS** (or the Animation Hitches template) while scrolling. That
Core-Animation fps, not the in-app counter, is the 60 fps verdict.

### Pixel 6 or newer

```sh
pnpm -C spikes/rn-diff exec expo run:android --device --variant release
```

**Authoritative fps:** Android Studio **Profiler** (frame timings) or
**Perfetto** / `adb shell dumpsys gfxinfo dev.concerto.rndiffspike` while
scrolling, or enable the on-device **GPU rendering profiler**
(Developer Options → Profile GPU rendering).

Record both devices' **render** ms and **scroll fps** into
`design/spikes/rn-diff-findings.md` (the `PENDING` rows), then set the GO/NO-GO.

### Quick look without a device (indicative only)

```sh
pnpm -C spikes/rn-diff start          # Metro; press i / a for sim/emulator
# or a native simulator build:
pnpm -C spikes/rn-diff exec expo run:ios   # boots an iOS Simulator
```

Simulator numbers are **indicative only** and must be labelled as such — they
are NOT the 60 fps device verdict.

---

## Verification (Task 103 tier: spike)

Reliable, automatable gates (the Tier-1-provable bar):

```sh
pnpm -C spikes/rn-diff install        # succeeds
pnpm -C spikes/rn-diff typecheck      # clean (tsc --noEmit, strict)
pnpm -C spikes/rn-diff lint           # clean (eslint-config-expo)
```

The numeric verdict — and the **PENDING** real-device rows — live in
`design/spikes/rn-diff-findings.md`.

## Layout

```
app/                 expo-router entry (_layout.tsx, index.tsx)
src/diff/            parse.ts · flatten.ts · syntax.ts · types.ts
src/fixtures/        generate.ts  (seeded ~1k / ~10k diff generator)
src/perf/            fps.ts (rAF meter) · timing.ts (budgets + helpers)
src/ui/              DiffViewer.tsx (FlashList) · DiffRow.tsx · PerfHud.tsx
                     HarnessScreen.tsx · theme.ts
```

`ios/`, `android/`, `node_modules/`, `.expo/` are git-ignored — they are
regenerated by `expo run:*` / `expo prebuild`. The committed artefact is the
source + `pnpm-lock.yaml`.
