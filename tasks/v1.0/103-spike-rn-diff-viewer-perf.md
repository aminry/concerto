# Task 103 — Spike: React Native Diff-Viewer Performance

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | spike |
| Verification tier | spike |
| Size | spike (~2–3 engineer-days) |
| Depends on | — |
| Touches subsystem(s) | 16 (Mobile Clients) |
| Smoke gate | unchanged |

## Goal
Determine whether a React Native diff renderer can hit the V1.0 mobile bar — **a 1000-line diff rendered in <1.5 s and scrolling at 60 fps on an iPhone 13+ / Pixel 6+** (`design/16 §10`, R-1/R-7) — before Phase 5 commits to RN for the diff surface. A NO-GO points to the V1.5 native-diff escape hatch and reshapes Task 514.

## Inputs to read before starting
- `design/16_Mobile_Clients.md` §3.7 (touch-first custom RN diff renderer, not Monaco), §10 (perf budget + device matrix), R-1/R-7 (native fallback if RN can't meet it).
- `design/00_Architecture_Overview.md` §11 (validation spikes — this is the RN-diff spike).
- `tasks/v1.0/README.md` §5.2 (spike deliverables) and §4 V8 (`apps/mobile` is an Expo project).

## Scope — in
- A throwaway Expo app at `spikes/rn-diff/` rendering a representative unified diff with the touch interactions V1.0 needs (expand/collapse hunks, smooth scroll, syntax-ish coloring) using the rendering approach you'd actually ship (e.g. virtualized list of line rows).
- Fixtures: a ~1000-line diff and a large (~10k-line) diff to find the cliff.
- Measurements on **at least one real recent iPhone and one real recent Android device** (simulators do not measure 60 fps credibly — say so if you only have one platform): time-to-first-render and sustained scroll fps.
- Findings doc `design/spikes/rn-diff-findings.md`: approach, per-device numbers, where it falls over, and an explicit **GO / NO-GO** vs the <1.5 s / 60 fps bar, with the native-escape-hatch recommendation if NO-GO or marginal.

## Scope — out
- Wiring to real Core data (use static fixtures).
- The production `apps/mobile` project (Task 508) — this is throwaway.
- Voice, pairing, navigation — only the diff renderer.

## Public interface this task locks
- None (throwaway spike).

## Implementation notes
- Virtualization is almost certainly required; render only visible line rows. Measure with the virtualization in place — an un-virtualized 10k-line render will trivially fail and tells you nothing.
- Use a real device profiler (Xcode Instruments / Android GPU profiler or the RN perf monitor) for the fps number; don't eyeball it.
- If you can only test one platform's hardware, run it there and explicitly mark the other platform "unmeasured — needs device" rather than guessing.

## Verification
Tier: **spike**.
1. The Expo app builds and runs on a device/simulator: `pnpm -C spikes/rn-diff install && pnpm -C spikes/rn-diff exec expo run:ios` (or `run:android`).
2. `design/spikes/rn-diff-findings.md` exists with the rendering approach, per-device time-to-render + fps numbers, the failure cliff, and a clear **GO / NO-GO** with the fallback recommendation.
3. `pnpm -C spikes/rn-diff typecheck` clean (if TS configured) and `pnpm -C spikes/rn-diff lint` clean.

## Definition of Done
- [~] Throwaway Expo diff renderer runs on at least one real device
      — Option A: runs on the **iOS Simulator** (clean native build, installs,
      launches, idle 60 fps HUD); real-device run is the operator's Phase-1-gate
      step (`spikes/rn-diff/README.md`). No real iPhone/Pixel in this env.
- [~] 1000-line and large-diff fixtures measured (time-to-render + scroll fps)
      — both fixtures built; JS build-cost measured off-device (~1.2 ms / ~4 ms);
      on-device render-ms + scroll-fps are **PENDING operator field measurement**
      (cannot be synthesized without device input here).
- [x] Findings doc committed with numbers and GO/NO-GO vs the §10 bar
      — `design/spikes/rn-diff-findings.md`; verdict is the labelled **PENDING
      OPERATOR DEVICE MEASUREMENT** per Option A (60 fps is device-only).
- [x] Unmeasured platform (if any) explicitly marked, not extrapolated
      — both real-device rows marked `PENDING`; simulator row labelled
      "indicative only, runnable confirmation, not a perf measurement".
- [x] Single commit created with the message below

## Outputs
- `spikes/rn-diff/` (new — throwaway Expo app)
- `design/spikes/rn-diff-findings.md` (new)

## Commit message
```
phase-1 spike: react-native diff-viewer perf findings

Throwaway Expo diff renderer measuring time-to-render and scroll fps on
real devices against the <1.5s / 60fps V1.0 bar. Findings doc records
the numbers and a GO/NO-GO with the native-escape-hatch recommendation.

Refs: tasks/v1.0/103-spike-rn-diff-viewer-perf.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** Executed under operator **Option A** (device-gated spike):
  built a genuinely runnable harness now, deferred the real-device 60 fps verdict
  to the operator at the Phase-1 gate. **Rendering approach chosen:** parse →
  flatten to a single flat `Row[]` (one fixed-height row per file/hunk/line) →
  virtualize with **`@shopify/flash-list` v2** (New Architecture), with
  **per-line-text-memoized** syntax-ish tokenization; expand/collapse is just
  which line rows are in the flat array. Pinned **Expo SDK 54.0.35 / RN 0.81.5 /
  React 19.1.0 / FlashList 2.0.2**. Indicative simulator result (clearly
  caveated): `expo run:ios` builds clean, installs, and **runs live on an
  iPhone 17 Pro Simulator with an idle 60 fps HUD** — a runnable confirmation,
  **not** a perf measurement; fixture-loaded render-ms and under-scroll fps were
  **not** captured because synthetic tap/scroll input isn't available in this
  sandbox. Off-device JS build cost measured: parse+flatten+tokenize ≈ 1.2 ms
  (1k) / 4 ms (10k) — negligible, so any cliff lives in the native render/scroll
  layer, not the JS prep.
- **Open questions for next task:** The **real-device 60 fps GO/NO-GO is PENDING
  operator field measurement** — a **Phase-1 Tier-3 checklist line** (feeds Task
  514). Operator runs Release builds on iPhone 13+ / Pixel 6+ and reads fps via
  Xcode Instruments (Core Animation) / Android GPU profiler per
  `spikes/rn-diff/README.md`, then sets the verdict. Open device questions:
  per-row `<Text>`-child cost once real `react-native-syntax-highlighter` is in;
  very-long-line layout; the 10k memory cliff feeding the `design/16 §8`
  "too large → open on desktop" guard.
- **Deliberate debt:** Throwaway harness — tokenizer is a placeholder (Task 514
  uses `react-native-syntax-highlighter` + the `§3.7` whitelist and must
  re-measure); only tap + expand/collapse wired (no pager / pinch-zoom /
  long-press-comment); fixtures generated, not real `GetWorkareaRepoDiff` data.
  All intentional and scoped out. Architectural carry-forward for Task 514 is in
  findings §6.
- **Smoke-gate state:** unchanged (spike; no product smoke gate). Automatable
  gates green: `pnpm -C spikes/rn-diff install` / `typecheck` / `lint` all pass;
  full Metro bundle (`expo export`, 1027 modules) succeeds; native iOS sim build
  `Build Succeeded` (0 errors/0 warnings).
