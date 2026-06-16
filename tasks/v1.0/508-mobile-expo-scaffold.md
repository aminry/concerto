# Task 508 — `apps/mobile` Expo scaffold + jest/RN-TL harness + mobile CI lane

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rn-mobile |
| Verification tier | 2 |
| Size | medium |
| Depends on | 507.5 |
| Touches subsystem(s) | 16 (Mobile Clients) |
| Smoke gate | unchanged |

## Goal
Stand up the Concerto Mobile Client (Expo managed + React Native + TypeScript app) as a pnpm
workspace member consuming **only** `@concerto/client` (PHASE5_PLANNING D11), plus the **jest +
@testing-library/react-native** harness and a **mobile CI lane** — the first mobile surface and the
durable unit-test gate for Track C (508–518). The native Iroh module (509), packaging (509.5), and
`expo prebuild` / on-device builds are explicitly OUT OF SCOPE here (Tier-3, no toolchain).

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` D11 (mobile consumes ONLY `@concerto/client`, NOT `@concerto/ui`),
  D13 (jest + RN-TL + mobile CI job), D14 ("Concerto" naming; no project tier).
- `design/16_Mobile_Clients.md` — read the **Amendment block** + §3.1 (Expo SDK) + §3.4 (tab order
  Concerto / Workspaces / Inbox).
- `packages/client` (507.5: `DataClient` + generated proto under `./gen/concerto/v1/*_pb`).
- `apps/web` + `spikes/rn-diff/package.json` — the existing Expo SDK 54 version matrix to align to.

## Scope — in
- `apps/mobile` (`@concerto/mobile`): Expo (managed) + RN + TS workspace member, registered in
  `pnpm-workspace.yaml`; depends on `@concerto/client` (`workspace:*`). Aligned to the repo's
  Expo SDK 54 matrix (expo 54, react 19.1, react-native 0.81, expo-router 6) — the same matrix the
  validated `spikes/rn-diff` harness uses.
- App shell: **expo-router** file-system router (`app/_layout.tsx`, `app/(tabs)/_layout.tsx`) with a
  **bottom-tab nav in the FROZEN order Concerto (default) / Workspaces / Inbox** (D14, design/16 §3.4).
  Concerto + Workspaces are placeholder screens; **Inbox renders a fresh RN component tree**
  (`src/inbox/InboxScreen.tsx`, a `FlatList` of severity-coded cards) wired to `@concerto/client`'s
  generated `Notification` type + `NotificationKind` — NOT a port of `@concerto/ui` (D11).
- Config: `app.json` (Expo app config + `expo-router` plugin), `eas.json` (EAS build/submit profiles),
  `tsconfig.json` (extends `expo/tsconfig.base`), `babel.config.js`, `metro.config.js` (pnpm monorepo
  resolution), `expo-env.d.ts`.
- Test harness: `jest.config.js` (`jest-expo` preset; pnpm-aware `transformIgnorePatterns`),
  `jest.setup.ts`, a sample passing spec (`src/inbox/InboxScreen.test.tsx`: empty-state renders +
  a notification card renders from a `@concerto/client` `Notification`).
- `.github/workflows/mobile.yml`: `pnpm install` + `pnpm -C apps/mobile typecheck` + `... test`.

## Scope — out
- The native `ConcertoIroh` module (509), XCFramework/.aar packaging + cross-compile lane (509.5),
  the native `DataClient` (510), pairing (511), and screens 512–518. The live transport: `InboxScreen`
  takes an optional `items` prop (deterministic in tests) and defaults to the empty state until 510
  wires a live `DataClient`.
- **`expo prebuild` / EAS native builds / simulator / Detox** — need Xcode / Android-NDK toolchains
  not assumed present. Recorded as a **Tier-3 blocker** (the phase-gate checklist), not run here.

## Public interface this task locks
- `apps/mobile` workspace member + the expo-router route tree (`app/(tabs)/{index,workspaces,inbox}`),
  the tab order, the `InboxScreen` RN component (`items?: Notification[]`) + `src/theme/tokens`, and
  the mobile jest harness conventions (`testID` hooks, `jest-expo` preset, pnpm `transformIgnorePatterns`).

## Verification
**Tier 2 (achievable without native toolchains).**
- `pnpm install` (root) — adds `apps/mobile`; `--frozen-lockfile` parity confirmed.
- `pnpm -C apps/mobile typecheck` (`tsc --noEmit`) — clean.
- `pnpm -C apps/mobile test` (jest) — 2/2 green (Inbox empty-state + a card from a `@concerto/client`
  `Notification`).
- Regression: `@concerto/client` + `apps/web` typecheck/test still green after the workspace add.

**NOT run (Tier-3, no toolchain):** `expo prebuild`, EAS build, simulator/Detox, on-device push /
biometric / 60fps. These are the README Phase-5 operator checklist lines.

## Definition of Done
- [x] `apps/mobile` Expo + RN + TS app; registered in `pnpm-workspace.yaml`; depends on `@concerto/client`
- [x] Bottom-tab nav Concerto / Workspaces / Inbox; Inbox is a fresh RN tree wired to `@concerto/client` notif types
- [x] `app.json` + `eas.json` present
- [x] jest + `@testing-library/react-native` (`jest-expo` preset) harness + a passing sample test
- [x] `.github/workflows/mobile.yml` (install + typecheck + jest)
- [x] `pnpm -C apps/mobile typecheck` clean · `pnpm -C apps/mobile test` green

## Outputs
- `apps/mobile/{package.json,app.json,eas.json,tsconfig.json,babel.config.js,metro.config.js,jest.config.js,jest.setup.ts,expo-env.d.ts}`
- `apps/mobile/app/_layout.tsx` + `apps/mobile/app/(tabs)/{_layout,index,workspaces,inbox}.tsx`
- `apps/mobile/src/{Placeholder.tsx,theme/tokens.ts,inbox/{InboxScreen.tsx,kind-label.ts,InboxScreen.test.tsx}}`
- `pnpm-workspace.yaml` (+ `apps/mobile`) · `.github/workflows/mobile.yml` · `pnpm-lock.yaml`

## Commit message
```
phase-5: apps/mobile Expo scaffold + jest/RN-TL harness + mobile CI (508)

Stands up apps/mobile as an Expo (managed) + React Native + TypeScript pnpm
workspace member consuming only @concerto/client (D11). expo-router bottom-tab
shell in the frozen order Concerto/Workspaces/Inbox (D14); Inbox is a fresh RN
component tree wired to @concerto/client's generated Notification types. Adds
app.json + eas.json, the jest + @testing-library/react-native (jest-expo)
harness with a passing sample test, and .github/workflows/mobile.yml (install +
typecheck + jest). Native module (509), prebuild + EAS native builds are Tier-3
(no toolchain) and out of scope.

Refs: tasks/v1.0/508-mobile-expo-scaffold.md
```

## Handoff Notes
- **Expo SDK 54 matrix:** aligned to the validated `spikes/rn-diff` versions (expo 54.0.35, react
  19.1.0, react-native 0.81.5, expo-router ~6.0.24) rather than the bleeding-edge SDK 56, for repo
  consistency.
- **`@testing-library/react-native` pinned to v13, not v14:** RTL v14 introduced a new `test-renderer`
  dep that peers on `react@^19.2`, but the SDK-54 / jest-expo-54 stack ships `react-test-renderer@19.1`
  (we run React 19.1). On v14 `render()` returned an empty object (silent mount bail-out, "render
  function has not been called"); v13.3.3 (which uses `react-test-renderer`) is the compatible pin.
- **pnpm `transformIgnorePatterns`:** the default jest-expo pattern assumes a hoisted `node_modules`;
  pnpm stores deps under `node_modules/.pnpm/<pkg>@<ver>/node_modules/<pkg>`, so the pattern allows an
  optional `.pnpm/...@.../node_modules/` prefix before each whitelisted package name — otherwise
  untranspiled RN ESM reaches jest's CJS runtime ("Cannot use import statement outside a module").
- **`metro.config.js`** is the Expo-documented pnpm-monorepo setup (watch the workspace root, resolve
  from app + root `node_modules`, follow symlinks) so the app can bundle `@concerto/client`.
- **Benign peer warning:** `@types/react-dom@18` (a dev-only types peer) is pulled by `expo-router 6`'s
  deep `@radix-ui/*` transitive chain — upstream, does not affect typecheck/test/build.

## Tier-3 blockers (operator signs at the phase gate)
- **`expo prebuild` / native generation** — needs Xcode (iOS) + Android SDK/NDK; not available here.
- **EAS build + EAS Submit** (App Store / TestFlight / Play) — needs Expo account + signing creds.
- **Simulator / Detox UI-E2E**, on-device push to a locked phone, biometric gate, lock-screen chips,
  60fps RN diff on hardware — all physical-device, per PHASE5_PLANNING §7.2.
- The native module compile/load itself is Task 509 / 509.5 (cross-compile link-check is Tier-2 there;
  on-device load is Tier-3).
