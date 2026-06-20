# Task 523 — Inbox / notification-center UI (shared `@concerto/ui`; desktop + web)

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | web-ts |
| Verification tier | 2 |
| Depends on | 507, 519, 520 |
| Touches subsystem(s) | 17 (Web Client) |
| Smoke gate | unchanged |

## Goal
Make the notifications inbox a **shared, transport-agnostic** React-DOM renderer so the desktop and
web clients show the **identical** inbox (decision D11). Extract the inbox component(s) from
`apps/web/src/App.tsx` into **`packages/ui` (`@concerto/ui`)**, refactor `apps/web` to consume it, and
mount it in `apps/desktop` too — folding desktop into the root pnpm workspace.

This is the "523-full" follow-up to 523 core (the live web inbox over the connect-web bridge): same
inbox, now ALSO rendered by the desktop shell.

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` §4.5 (`@concerto/ui` FROZEN by 519/523), D11 (renderer extraction is
  desktop+web; mobile shares only `@concerto/client`), D13 (Playwright + CI jobs).
- `apps/web/src/{App.tsx,index.css}` + `src/lib/data.ts` (519's inbox SPA over `@concerto/client`).
- `packages/client` (507.5: `DataClient` + generated `Notifications` proto).

## Scope — in
- **`packages/ui` (`@concerto/ui`):** the extracted React-DOM inbox renderer.
  - `src/Inbox.tsx` — `Inbox` (the title + unread-only toggle, idle/empty/error surfaces, the feed),
    `NotificationCard` (one severity-coded card + mark-read), and the pure rendering helpers
    `relativeTime` / `kindLabel`. Transport-agnostic: the host passes the notification list + handlers
    + load `status` as props (see `InboxProps`/`InboxStatus`), so the same component renders against the
    web connect-web transport and the desktop Tauri/iroh transport alike.
  - `src/inbox.css` — co-located, portable styling the consumer imports
    (`import "@concerto/ui/inbox.css"`): design tokens + the inbox-component rules + the shared `.btn`.
  - `src/index.ts` barrel; `src/Inbox.test.ts` (vitest unit tests for `kindLabel`/`relativeTime`).
- **`apps/web` refactor:** renders `<Inbox … />` from `@concerto/ui`; keeps the connect bar + the
  connect-web data fetch (`src/lib/data.ts`) in the app. `src/index.css` `@import`s `@concerto/ui/inbox.css`
  and keeps only the shell chrome (app frame, top bar, brand, connect bar).
- **`apps/desktop` fold + surface:** added to `pnpm-workspace.yaml`; `@concerto/ui` + `@concerto/client`
  workspace deps added; `src/components/InboxPanel.tsx` mounts the shared `Inbox` (+ `InboxPanel.test.tsx`),
  wired as a new "Inbox" right-rail tab (`RightRail.tsx` + the `RightRailTab` union/guard in
  `state/useUiStore.ts`). The standalone `apps/desktop/pnpm-lock.yaml` is removed (root workspace owns it).
- **CI:** `web.yml` adds a `@concerto/ui` typecheck+test step; `ci.yml`'s desktop job installs from the
  root workspace (`cache-dependency-path: pnpm-lock.yaml`, `pnpm install --frozen-lockfile` at root then
  `pnpm -C apps/desktop build`) since desktop is now a member.

## Scope — out
- Migrating the desktop → Core transport for the live `Notifications` service onto `@concerto/client`
  (desktop still uses its `concerto_rpc` Tauri bridge; the desktop inbox mounts the shared component in
  its idle state until that follow-up wires the live feed). Mobile's own RN tree (D11). Prefs/opt-out
  settings + the notification-derived confirmation-chip surface (a later notification-center step).

## Public interface this task locks
- `@concerto/ui` exports: `Inbox`, `NotificationCard`, `relativeTime`, `kindLabel`, and the
  `InboxProps`/`InboxStatus` types; the `@concerto/ui/inbox.css` style entry point.
- `apps/web` consumes `<Inbox/>` (connect bar + data layer stay in the app).
- `apps/desktop` is a pnpm-workspace member with an "Inbox" right-rail tab rendering `InboxPanel`.

## Verification
**Tier 2.**
- `pnpm install` (root; 5 workspace projects) · `pnpm install --frozen-lockfile` (root, clean).
- `pnpm -C packages/ui typecheck` (clean) · `pnpm -C packages/ui test` (5 tests green).
- `pnpm -C apps/web typecheck && pnpm -C apps/web build` (vite, 171 modules).
- `pnpm -C apps/web exec playwright install chromium; pnpm -C apps/web e2e` (3 E2E green; live-inbox
  skipped without `CONCERTO_LIVE`).
- `pnpm -C apps/desktop typecheck && pnpm -C apps/desktop test` (40 files / 227 tests green, incl. the
  new `InboxPanel.test.tsx`) · `pnpm -C apps/desktop build` (vite build green).

## Definition of Done
- [x] `@concerto/ui` extracted (inbox component(s) + co-located CSS); typechecks + unit-tested
- [x] `apps/web` renders the shared `<Inbox/>`; e2e (3) stay green
- [x] `apps/desktop` folded into the workspace; mounts the shared inbox (Inbox tab); all desktop tests green
- [x] CI updated: web.yml gates `@concerto/ui`; ci.yml desktop job installs at the root workspace

## Outputs
- `packages/ui/**` (`package.json`, `tsconfig.json`, `src/{Inbox.tsx,Inbox.test.ts,inbox.css,index.ts}`)
- `apps/web/{package.json,src/App.tsx,src/index.css}`
- `apps/desktop/{package.json,README.md,src/components/InboxPanel.tsx,src/components/InboxPanel.test.tsx,
  src/components/RightRail.tsx,src/state/useUiStore.ts}`; `apps/desktop/pnpm-lock.yaml` removed
- `pnpm-workspace.yaml`, `pnpm-lock.yaml`, `.github/workflows/{web.yml,ci.yml}`

## Handoff Notes
- **Workspace fold:** because the repo root carries `pnpm-workspace.yaml`, `pnpm -C apps/desktop install`
  already resolved to the root workspace (ignoring desktop's own lockfile) inside the tree — so folding
  desktop in is the honest fix, not a regression. Desktop's standalone `pnpm-lock.yaml` is deleted; the
  root lockfile owns its deps. CI's desktop job now installs at root then `pnpm -C apps/desktop build`.
- **Transport-agnostic by props:** the shared `Inbox` takes `items`/`status`/`unreadOnly`/handlers as
  props; the host owns the connection. Web keeps its connect-web fetch; desktop's live wiring (onto
  `@concerto/client` over the Tauri bridge) is the next desktop step — the surface mounts the component
  in its idle state today.
- **Styling portability:** `@concerto/ui/inbox.css` carries the design tokens + inbox rules + `.btn`;
  web's `index.css` `@import`s it and keeps only the shell chrome. Desktop's `InboxPanel` imports it too.
