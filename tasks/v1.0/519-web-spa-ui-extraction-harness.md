# Task 519 — `apps/web` SPA + Playwright UI-E2E/screenshot harness

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | web-ts |
| Verification tier | 2 |
| Size | medium |
| Depends on | 507.5, 218 |
| Touches subsystem(s) | 17 (Web Client) |
| Smoke gate | unchanged |

## Goal
Stand up the Concerto Web Client (React SPA over the Core's connect-web bridge) consuming
`@concerto/client`, plus the **Playwright UI-E2E + screenshot harness** + the web CI job — the first
visible web UI and the durable screenshot/E2E gate for Track B (519–523).

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` D11 (web reuses `@concerto/ui`; mobile only `@concerto/client`),
  D13 (Playwright + CI jobs); `design/17`.
- `packages/client` (507.5: `DataClient` + `createConnectWebDataClient` + generated proto).

## Scope — in
- `apps/web` (`@concerto/web`): Vite + React + TS workspace member. A polished **notifications inbox**
  (connect bar, unread toggle, severity-coded cards, idle/empty/error states) over the live
  `Notifications` service via `createConnectWebDataClient` (gRPC-Web) — `src/{App,main,index.css}.tsx`,
  `src/lib/data.ts`.
- Playwright harness: `playwright.config.ts` (vite dev webServer, IPv4-pinned), `e2e/inbox.spec.ts`
  (idle render + connection-error banner + unread toggle, with screenshots).
- `.github/workflows/web.yml`: codegen-drift check + client/web typecheck+test + web build + Playwright
  UI-E2E (the README §5.3 web-ts gate, which never ran in CI before).

## Scope — out
- `@concerto/ui` renderer extraction (deferred — desktop is still standalone; web has its own UI for
  now). The Connect-Web data-client features (HTTP/2 + SSE + AckOffset) (520); LAN TLS / relay (521);
  ephemeral pairing (522); the full inbox/notification-center UI + live-Core E2E against a spawned
  Core (523). Live-data round-trips are 520/523's harness; 519's E2E covers idle/error (no live Core).

## Public interface this task locks
- `@concerto/web` app + the `data.ts` helpers (`makeDataClient`/`fetchInbox`/`markRead`) + the
  Playwright harness conventions (`data-testid` hooks, `e2e/` layout).

## Verification
**Tier 2.** `pnpm -C apps/web typecheck` (clean) · `pnpm -C apps/web build` (vite, 169 modules) ·
`pnpm -C apps/web e2e` (3 Playwright tests green; screenshots in `e2e/__screenshots__/`). The double =
no live Core (idle + error states); live notifications through a browser is 523's harness. Screenshots
visually reviewed (idle "Connect to a Core" + red error banner — clean, modern).

## Definition of Done
- [x] `apps/web` SPA (inbox UI) typechecks + builds; consumes `@concerto/client`
- [x] Playwright harness (3 E2E tests + screenshots) green; IPv4 bind fix
- [x] web CI workflow (codegen-drift + typecheck/test/build + Playwright)
- [x] Single commit per part (part 1 app `74b91b1`; part 2 harness)

## Outputs
- part 1: `apps/web/{package.json,tsconfig.json,vite.config.ts,index.html}` + `src/**` ·
  `packages/client/package.json` (./gen/* export fix) · `pnpm-workspace.yaml`
- part 2: `apps/web/playwright.config.ts` + `e2e/inbox.spec.ts` + `package.json` (e2e scripts) ·
  `.github/workflows/web.yml` · `.gitignore` (playwright artifacts)

## Commit message (part 2)
```
phase-5: web Playwright UI-E2E + screenshot harness + web CI (519 part 2)

Adds the Playwright harness (idle render, connection-error banner, unread
toggle + screenshots; IPv4-pinned vite webServer) and .github/workflows/web.yml
(codegen-drift + client/web typecheck+test + web build + Playwright UI-E2E) —
the README §5.3 web-ts gate that never ran in CI. 3 E2E tests green.

Refs: tasks/v1.0/519-web-spa-ui-extraction-harness.md
```

## Handoff Notes (filled in when finishing)
- **Vite IPv4 bind:** `localhost` resolved to `::1` so Playwright/curl at `127.0.0.1` failed — pinned
  `server.host`/`preview.host` to `127.0.0.1` + strictPort.
- **Screenshots** (`e2e/__screenshots__/`) are gitignored (regenerable); the e2e specs are the durable
  gate. Visually confirmed clean/modern (light + dark via prefers-color-scheme).
- **`@concerto/ui` extraction deferred:** web has its own inbox UI; sharing the desktop renderer is a
  later step (needs the desktop-migration-onto-@concerto/client follow-up too).
- 520 next: the Connect-Web data client (HTTP/2 + SSE fallback + AckOffset) + turning the bridge on;
  523: the inbox/notification-center UI proven live against a spawned Core (the Track-A→UI bridge).
