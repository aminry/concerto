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
- [ ] Throwaway Expo diff renderer runs on at least one real device
- [ ] 1000-line and large-diff fixtures measured (time-to-render + scroll fps)
- [ ] Findings doc committed with numbers and GO/NO-GO vs the §10 bar
- [ ] Unmeasured platform (if any) explicitly marked, not extrapolated
- [ ] Single commit created with the message below

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
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
