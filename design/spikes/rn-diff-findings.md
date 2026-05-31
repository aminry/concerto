# Spike findings — React Native diff-viewer performance (Task 103)

| Field | Value |
|---|---|
| Spike | Phase 1, #3 (`design/00 §11`, `design/16 §10`) |
| Task | `tasks/v1.0/103-spike-rn-diff-viewer-perf.md` |
| Harness | `spikes/rn-diff/` (throwaway Expo app, not `apps/mobile`) |
| Platform pin | **Expo SDK 54.0.35** · React Native 0.81.5 · React 19.1.0 · `@shopify/flash-list` 2.0.2 (New Architecture) |
| The bar | **1000-line diff render < 1.5 s · sustained 60 fps scroll on iPhone 13+ / Pixel 6+** (PRD §22.3, `design/16 §3.7`) |
| GO threshold | render ≤ 1.5 s **and** sustained ≥ 60 fps (≥ 58 fps tolerated for dips) → **GO** · marginal → re-tune then re-measure · clear miss → **NO-GO** (R-1/R-7 native escape hatch) |
| Verdict | **PENDING OPERATOR DEVICE MEASUREMENT** (see §2, §5) |
| Date | 2026-05-30 |

---

## 1. What this spike establishes

Whether a **custom React Native diff renderer** can hit the V1.0 mobile diff
budget — **a 1000-line unified diff rendered in < 1.5 s and scrolling at 60 fps
on an iPhone 13+ / Pixel 6+** — before Phase 5 (Task 514) commits to RN for the
diff surface. A GO confirms the `design/16 §3.7` plan (custom RN diff, *not*
Monaco-in-WebView per R-7). A **NO-GO** points at the **V1.5 native escape
hatch** (R-1): drop a SwiftUI / Jetpack-Compose diff component into just this
one view while the rest of the app stays RN.

The headline metric — **sustained 60 fps under fast scroll on real hardware** —
is, by construction, **physical**. A simulator/emulator has no real GPU,
thermal envelope, or device display pipeline; it does not render at a device's
true frame rate. The 60 fps verdict can only be produced on a real
iPhone 13+ / Pixel 6+ with a device profiler (Xcode Instruments Core Animation
FPS / Android GPU profiler / Perfetto). **It cannot be fabricated from this
environment.**

## 2. Operator decision in force: Option A (build now, device verdict deferred)

This spike was executed in an automated environment with **iOS Simulators only
and no real iPhone/Pixel hardware**. Per the operator's explicit **Option A**:

- the **full runnable harness was built** — clean TypeScript, the virtualized
  diff renderer, expand/collapse, the two fixtures, and an in-app HUD that
  reports time-to-first-render and a live JS-fps reading — so the operator can
  install it on real devices and read the two numbers directly (see
  `spikes/rn-diff/README.md`);
- the **reliably-automatable gates were nailed** (§3): `install`, `typecheck`,
  `lint`, and a full Metro bundle all pass;
- an **indicative iOS-Simulator reading** was attempted (§4), **explicitly
  labelled “Simulator — indicative only, NOT a 60 fps device measurement”**;
- the **real-device rows are `PENDING — operator field measurement`** (§5).
  **No device numbers were invented.**
- the final GO/NO-GO is **deferred to the operator at the Phase-1 gate**; this
  doc carries a clearly-labelled **PENDING provisional verdict**, not a faked
  GO/NO-GO.

This deferral is the correct, honest outcome for a device-gated spike.

## 3. Rendering approach (what was built, and what Task 514 should ship)

**Parse → flatten → virtualize.** The renderer never holds a nested tree at
scroll time; it renders one **flat array of fixed-height rows**:

1. **Parse** (`src/diff/parse.ts`) — standard `git diff` unified output →
   files → hunks → lines, tracking old/new line numbers and add/del/context
   kind.
2. **Flatten** (`src/diff/flatten.ts`) — the parsed tree becomes a single
   `Row[]`: one row per **file header**, **hunk header**, and **line**.
   - **Expand/collapse** is purely *which line rows are present in the flat
     array* — a collapsed file contributes only its header row. Toggling
     re-flattens; **tokens are memoized by line text** (identical source lines
     — braces, imports, blanks — share one token array), so re-flatten is
     cheap.
3. **Virtualize** (`src/ui/DiffViewer.tsx`) — **`@shopify/flash-list` v2**
   recycles row views and mounts only the visible window plus a small overscan
   (`drawDistance`). A 10k-line diff mounts **~30–50 row views at a time, never
   10k**. `getItemType` gives file/hunk/line rows separate recycle pools.
   - Chosen over RN's `FlatList`: FlashList v2 is purpose-built for long,
     uniform lists on the New Architecture and holds frame budget far better
     under fast flings — exactly the 60 fps stressor. (An un-virtualized 10k
     render would trivially fail and measure nothing, per the Task 103 notes.)
4. **Syntax-ish colouring** (`src/diff/syntax.ts`) — a tiny one-pass tokenizer
   (keyword / string / comment / number / punct) so the renderer carries
   realistic per-line work. Production (Task 514) swaps in
   `react-native-syntax-highlighter` with the `design/16 §3.7` language
   whitelist; the harness can toggle the tokenizer off in-app to isolate its
   cost.

**Fixtures** (`src/fixtures/generate.ts`) — deterministic, seeded, two sizes:
`~1k` (budget target) and `~10k` (cliff finder). Generated, not committed
static, so the cliff size is tunable.

**The two numbers, in-app** (`src/ui/PerfHud.tsx`): **render** (request→first
content frame, vs 1.5 s), **draw** (FlashList native `onLoad`), **build**
(parse+flatten+tokenize CPU), and **fps** + worst-case 1-second **min**.

## 4. Measured numbers

### 4a. Off-device build-cost (host Node, indicative)

The pure JS pipeline (parse + flatten + tokenize) was run on the host to bound
the **JS-side** cost — the part independent of the native render layer:

| Fixture | Raw bytes | Files | Parsed lines | Flat rows | parse + flatten + tokenize |
|---|---|---|---|---|---|
| `~1k`  | 64.6 KB  |  5 | 1000  | 1030 rows  | **~1.2 ms** |
| `~10k` | 640 KB   | 42 | 10000 | 10292 rows | **~4.0 ms** |

Collapse-all correctly reduces to just the file rows (5 / 42). **Takeaway:**
parse + flatten + tokenize is **negligible** (single-digit ms even at 10k lines
on a laptop; a phone is slower but this stays well inside budget). Therefore the
1.5 s render budget and any 60 fps cliff are dominated by the **native
render/commit + scroll pipeline**, not by the JS data prep — which is exactly
why the verdict must come from a device profiler, not a JS benchmark.

> Host figures are a MacBook-class CPU; they are an **upper bound on the JS
> portion**, not a device render time.

### 4b. Metro bundle (automatable, passed)

`expo export --platform ios` bundles the entire harness (router + FlashList +
all `src/`, **1027 modules**) into a 2.68 MB Hermes bytecode bundle with **zero
errors** — the harness is genuinely runnable, not just type-correct.

### 4c. iOS Simulator — indicative only, NOT a 60 fps device measurement

**What was achieved in this environment:** `expo run:ios` produced a **clean
native build** (`Build Succeeded`, 0 errors / 0 warnings), **installed** the app
on an iPhone 17 Pro Simulator (SDK 54, New Architecture, Xcode 26.5), and the
harness **launched and rendered live** — the toolbar, fixture buttons, and the
perf HUD all draw, with the **idle JS-fps reading holding a steady 60 (min
60/60)**. A screenshot confirms the running app.

**What could NOT be captured here:** the *fixture-loaded* render-ms and
*under-scroll* fps. Driving a fixture-load tap and a scroll fling needs
synthetic UI input (idb / Accessibility automation), which this sandbox does not
grant. The numbers below are therefore left for the device run; the simulator
result is "**builds, installs, runs, idle-60fps**" — a runnable-harness
confirmation, **not** a perf measurement.

| Device | render (~1k) | scroll fps (~1k) | render (~10k) | scroll fps (~10k) | Notes |
|---|---|---|---|---|---|
| **iOS Simulator (iPhone 17 Pro, SDK 54)** | runs (idle) | 60 idle | runs (idle) | not captured | **Indicative only — runnable confirmation, not a perf measurement.** Simulator fps is not a device fps; loaded/scroll numbers need device + Instruments. |

### 4d. Real devices — PENDING operator field measurement

| Device | render (~1k) | scroll fps (~1k) | render (~10k) | scroll fps (~10k) | Verdict |
|---|---|---|---|---|---|
| **iPhone 13+ (Release build)** | `PENDING` | `PENDING` | `PENDING` | `PENDING` | `PENDING` |
| **Pixel 6+ (release variant)** | `PENDING` | `PENDING` | `PENDING` | `PENDING` | `PENDING` |

**What's needed to fill these:** a real iPhone 13+ and Pixel 6+, cabled, in a
**Release** build (Debug understates fps):
`expo run:ios --device --configuration Release` /
`expo run:android --device --variant release`; tap `~1k`, read **render**;
flick-scroll hard and read sustained fps with **Xcode Instruments → Core
Animation FPS** (iOS) and **Android Studio Profiler / Perfetto /
`dumpsys gfxinfo`** (Android); repeat with `~10k`. Record above and set the
GO/NO-GO. Full steps: `spikes/rn-diff/README.md`.

## 5. Provisional verdict — **PENDING OPERATOR DEVICE MEASUREMENT**

The 60 fps GO/NO-GO **can only be decided on real iPhone 13+ / Pixel 6+
hardware** and is therefore a **Phase-1 Tier-3 checklist line** for the
operator (feeding Task 514). What this spike *can* assert:

- **The architecture is sound and runnable.** Virtualized flat-row rendering
  with FlashList + memoized tokenization keeps the mounted view count tiny and
  the JS prep cost negligible (§4a). This is the right approach to ship and the
  one most likely to clear the bar.
- **The likely outcome is GO or near-GO**, on the strength of: negligible JS
  build cost, a virtualized list that mounts ~tens of rows regardless of diff
  size, and FlashList v2's New-Architecture scroll performance. But *likely* is
  not *measured* — the spike does not pre-empt the device run.
- **The cliff to watch** is not row count (virtualization makes 1k vs 10k
  near-identical in mounted-view terms) but: (a) **per-row complexity** — many
  syntax tokens per line means many `<Text>` children, the usual RN scroll
  cost; (b) **very long individual lines** (minified/no-newline) forcing wide
  off-screen layout; (c) **memory** on the 10k fixture (`design/16 §8` already
  specifies a "diff too large → open on desktop" guard). Probe these on device.

### Native escape hatch (R-1 / R-7) — for the operator's reference

If the device run is a **NO-GO or marginal**: per `design/16 §12` R-1 and
`design/00 §11`, the V1.5 contingency is to **replace only the diff view** with
native **SwiftUI (iOS)** + **Jetpack Compose (Android)** embedded components,
leaving the rest of the app in RN. R-7 stays firm regardless: **do not** fall
back to Monaco-in-WebView (too heavy, wrong touch UX). A NO-GO reshapes Task
514 toward a native-module diff view behind the existing RN screen.

## 6. Handoff to Task 514 (production RN diff renderer)

- **Keep the parse → flatten → virtualize architecture** and the flat-`Row[]`
  model; it is the load-bearing decision and it is clean.
- **Use FlashList v2** (`getItemType` per row kind, fixed row height,
  `maintainVisibleContentPosition` disabled for the long uniform list).
- **Memoize tokenization by line text** — real diffs repeat lines heavily;
  this is a large, cheap win.
- Swap the placeholder tokenizer for `react-native-syntax-highlighter` with the
  `design/16 §3.7` language whitelist, and **re-measure** — real highlighting
  adds `<Text>` children per line, the most likely fps cost.
- Add the **per-file pager / pinch-zoom / long-press-to-comment** interactions
  (`design/16 §3.7`); the harness wires only tap + expand/collapse.
- Implement the **"diff too large → open on desktop"** guard (`design/16 §8`)
  using the on-device memory cliff found above as the threshold.
- The **real-device 60 fps GO/NO-GO is PENDING** (§5) — a Phase-1 Tier-3 line
  the operator signs off before Task 514 starts.

---

*End of `rn-diff-findings.md`. The harness, gates, and architecture are done;
the 60 fps GO/NO-GO stays PENDING until the operator measures on real
iPhone 13+ / Pixel 6+ hardware at the Phase-1 gate.*
