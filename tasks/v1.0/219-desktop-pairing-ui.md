# Task 219 — Desktop Pairing UI: show/scan QR + Connect-to-Core picker + Settings→Connected Cores

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | web-ts |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 218 |
| Touches subsystem(s) | 15 (Desktop Client) |
| Smoke gate | unchanged |

## Goal
Build the **renderer-side pairing surfaces** `design/15 §3.10` specifies on top of Task 218's `CoreClient` + connected-Core registry: a **Connect-to-Core picker**, the **split-host pairing flow** (decode a pairing payload via *scan QR* or *paste token*, drive `Devices.StartPairing`/`CompletePairing` through Tauri commands, name + persist the new `PairedCore`), and the **Settings → Connected Cores** list (switch active / rename / remove, with reachability status dots). Today the Desktop has no pairing UI at all — `SettingsPanel.tsx` is a single-section placeholder hosting `AddRepositoryForm`. This task makes a fresh Desktop able to pair with a remote Core and manage its set of Cores entirely from the renderer, with all transport/registry/keychain work staying in the Rust shell from 218. It does **not** build the first-launch auto-spawn decision tree (Task 601) — only the picker + pairing + management UX that 601 will orchestrate.

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.10.2 (the launch decision tree — the picker appears at step 4 when there's no local Core and prior choices; you build the picker, **not** the auto-spawn steps 0–3 which are Task 601), §3.10.3 (the **pairing flow**: the renderer asks Scan-QR vs Paste-token; the Core emits a payload of `core_pubkey` / `pairing_token` / `iroh_endpoint_id` / `relay_hint`; the shell decodes it, runs Noise XX, writes the `PairedCore`; the user **names** the pairing, default = Core hostname), §3.10.4 (**Settings → Connected Cores**: list with reachable/unreachable/never-connected status dots; switch active → renderer reloads to clear cached state; remove → best-effort cert revoke; rename; add-another).
- `design/12_Security_Identity.md` §3.3 (the **pairing ceremony the UI drives**: QR payload = `base64({core_pubkey, pairing_token, lan_endpoint, relay_hint})`; the 32-byte `pairing_token` with **60s TTL**, one-shot; the Noise XX channel bootstrapped by the token; LAN-vs-relay path), §5.2 (the `Devices` gRPC surface — `StartPairing`/`CompletePairing`/`ListDevices`/`RevokeDevice`/`GetCoreInfo`). §3.3 also notes the Desktop pairing client may **scan QR via webcam or paste the base64 token** (the same UX V0.1 web/mobile use).
- `tasks/v1.0/218-desktop-dual-transport.md` → "Handoff Notes" — the `CoreClient` trait + the `cores.json`/keychain registry this UI reads/writes through, the **registry read-commands + the pairing-write seams 218 left for this task**, the `src/api/cores.ts` binding shape, the active-Core Zustand slice, and the **pnpm scripts (typecheck/lint/test) + vitest config 218 added** (this task reuses them). **Hard dependency — do not start until 218's handoff is readable.**
- `tasks/v1.0/207-pairing-noise-xx.md` + `tasks/v1.0/209-devices-service.md` → "Handoff Notes" — the `Devices.StartPairing`/`CompletePairing` RPCs the pairing flow calls (via the shell's Tauri commands), and the `ListDevices`/`RevokeDevice` surface the "Connected Cores" remove path uses. The renderer reaches these **only through 218's Tauri command surface**, never gRPC directly.
- `apps/desktop/src/components/SettingsPanel.tsx` — the overlay-`<aside>` settings pattern to extend with a "Connected Cores" section.
- `apps/desktop/src/components/ui/` — the existing primitives to reuse: `dialog.tsx`, `status-dot.tsx`, `button.tsx`, `input.tsx`, `menu.tsx`, `card.tsx`, `badge.tsx`. `apps/desktop/src/components/NewProjectModal.tsx` / `NewWorkspaceModal.tsx` — the modal + form conventions (controlled inputs, error surfacing via `errorMessage`, `useMutation` from React Query).
- `apps/desktop/src/api/client.ts` (`callRpc`/`errorMessage`/`invoke` conventions) + `apps/desktop/src/api/cores.ts` (218's registry binding) + `apps/desktop/src/state/useUiStore.ts` (Zustand UI-only state; add `pairingOpen`/picker open-state here, mirroring `settingsOpen`).

## Scope — in
- A **Connect-to-Core picker** component (`design/15 §3.10.2` step 4): lists previously paired Cores (from 218's registry binding, with status dots) → "Connect to <name>"; plus entry points "Start a local Core" (delegates to the shell command 601 will flesh out — wire the button, stub the action behind a registry/shell command) and "Pair with a remote Core" → opens the pairing flow.
- The **pairing flow** UI (`design/15 §3.10.3` / `design/12 §3.3`):
  - A mode choice: **Scan QR** (webcam) or **Paste token** (textarea accepting the base64 string a headless `concerto pair` prints).
  - Decode the payload, call the shell's pairing Tauri command(s) (which run Noise XX + `Devices.CompletePairing` and write the `PairedCore` — that logic is the shell's, from 207/209/218; the UI orchestrates + shows progress/errors/timeout).
  - Surface the **60s token TTL** (countdown / expiry error) and the one-shot semantics (`design/12 §3.3`).
  - A **name-the-pairing** step (default suggestion = Core hostname / `GetCoreInfo`), then set active.
- **Settings → Connected Cores** section in `SettingsPanel.tsx` (`design/15 §3.10.4`): list all `PairedCore` rows with reachable/unreachable/never-connected `status-dot`; per-row **Switch active** (triggers the renderer state-clear/reload), **Rename**, **Remove pairing** (calls the shell's remove → best-effort `RevokeDevice`), and **Add another** → re-enters the pairing flow.
- A **QR show** affordance only where co-located (`design/15 §3.11` says "Reveal pairing QR" is **disabled in split-host** — render the QR-show entry point conditional on `transport_kind === UDS`; for remote, show the "use the Core machine's tray or `concerto pair`" hint). Showing the QR renders the local Core's pairing payload (from `StartPairing` via the shell) as a QR image.
- Wire picker/pairing open-state into `useUiStore` (UI-only), mirroring `settingsOpen`.
- Component/unit tests (vitest + Testing Library) driving every surface against a **stub `CoreClient`** (mock the Tauri `invoke` for the registry/pairing/`Devices` commands).

## Scope — out
- The **transport/registry/keychain/Noise-XX** machinery — Task 218 (registry + `CoreClient`) + Tasks 207/209 (the pairing RPCs + Noise XX). This task is renderer-only; it calls the shell's Tauri commands and never re-implements the ceremony.
- The **first-launch auto-spawn decision tree** (resolve embedded mode → promote local UDS → auto-spawn launchd/`sc.exe`) — Task 601 (`design/15 §3.10.2` steps 0–3). This task builds the picker UI 601 orchestrates, not the spawn logic.
- **Remote-mode affordance suppression** beyond the QR-show conditional (Reveal-in-Finder, drag-drop→Files, etc.) — Task 602 (`design/15 §3.11`).
- **Mobile/Web pairing** (Tasks 511 / 522) — different clients; same protocol.
- Real webcam QR decoding on a real device + real cross-device pairing — **Tier-3** Phase-2 checklist. The Tier-2 double is component tests against a stub `CoreClient`/mocked `invoke`.

## Public interface this task locks
- **TS (FROZEN):** the **pairing-UI ↔ Tauri-command contract** — the exact set + argument/return shapes of the Tauri commands the renderer invokes for: start-pairing-show (local QR payload), complete-pairing-from-payload (decode token → Noise XX → write `PairedCore`, returns the new `core_id` + suggested name), list/switch/rename/remove paired Cores. (The command **implementations** live in the 218 shell; this task freezes the **renderer-facing signatures** it calls, co-designed with 218's seams.)
- The **payload-decode shape** the renderer accepts on "Paste token" (the base64 `{core_pubkey, pairing_token, lan_endpoint/iroh_endpoint_id, relay_hint}` envelope per `design/12 §3.3`) — must match what `concerto pair` (Task 713) emits.

## Implementation notes
- **QR libraries — choose + record in Handoff.** None are in `apps/desktop/package.json` today. Recommended: **`qrcode`** (or `qrcode.react`) for **showing** a QR (pure-JS render, MIT, no native deps) and, for **scanning**, a webcam decoder over `getUserMedia` — **`@zxing/browser`** (or `jsqr` + a manual `<video>`/canvas loop) (both MIT/Apache-2.0). Add them as `apps/desktop` deps and clear their license posture (the workspace gates Rust via `cargo deny`; for TS deps note MIT/Apache in the PR — Task 707 owns the full `pnpm licenses` gate). **Paste-token must work without a webcam** (the always-available path); webcam scan is the convenience path and is the part the Tier-3 checklist verifies on real hardware.
- **Camera permission lives in the Tauri shell.** `getUserMedia` in the WebView needs the macOS camera entitlement; if the current `capabilities/main.json` / `Info.plist` doesn't grant it, the scan path will silently fail. Either add the entitlement (and note it) or ship paste-token as the verified path and gate scan behind a graceful "camera unavailable → paste a token instead" fallback. Record what you did.
- **Server-canonical state.** The paired-Core list + reachability come from the shell registry via React Query (keyed off 218's read-commands), invalidated after pair/rename/remove/switch. Only open-state + the in-progress pairing draft live in Zustand (`design/15 §3.3`). **Switch active** must clear React Query caches / reload the renderer (`design/15 §3.10.4`) so stale data from the previous Core never shows.
- **Reuse the existing modal/dialog conventions** (`NewProjectModal`/`dialog.tsx`/`button.tsx`/`input.tsx`/`status-dot.tsx`) — do not introduce a second modal system. Errors surface via `errorMessage` (the `{kind,message}` envelope).
- **Test runner.** Reuse the **vitest** setup Task 218 added (Handoff names the config). If a component test needs DOM, use `@testing-library/react` + `jsdom` (add if 218 didn't). There is no screenshot harness in `apps/desktop` today — component/DOM tests are the Tier-2 double; do not invent a screenshot pipeline (Playwright lives at `apps/web`, Task 519+).

## Verification
**Tier 2.** The `web-ts` §5.3 set, against the **real `apps/desktop` pnpm scripts** (Task 218 added `typecheck`/`lint`/`test`; reuse them — verify the names in `package.json` before relying on them):
1. `pnpm -C apps/desktop typecheck` → clean (`tsc --noEmit`).
2. `pnpm -C apps/desktop lint` → clean.
3. `pnpm -C apps/desktop test` → vitest component/unit suite passes: the picker renders the paired-Core list + entry points; paste-token decodes a fixture payload and invokes the complete-pairing command; the 60s-TTL expiry surfaces an error; the name-the-pairing step defaults to the Core hostname; Settings → Connected Cores switch/rename/remove invoke the right commands and the QR-show entry point is hidden when `transport_kind !== UDS`. All driven against a **stub `CoreClient`** (mocked `@tauri-apps/api` `invoke`).
4. `pnpm -C apps/desktop build` → `tsc --noEmit && vite build` clean (QR libs bundle).

> No Playwright here — `apps/desktop` is Tauri (WebView), not a headless-Chromium web app; the data layer this touches is the Tauri command bridge, exercised via mocked `invoke` in vitest, not a browser harness. (Playwright is the `apps/web` story.)

**Tier-2 double + what it does NOT cover.** The double is **vitest component/DOM tests against a stub `CoreClient` (mocked `invoke`)** plus **fixture pairing payloads**. It proves: the picker, the paste-token decode + complete-pairing call path, TTL/error UX, naming, and the Connected-Cores switch/rename/remove command wiring. It does **NOT** cover: a **real webcam QR scan**, a **real cross-device pairing** (real Noise XX over a real Iroh connection to a second machine), or real OS camera-permission prompts. Those are the **Tier-3 Phase-2 checklist** lines ("pair a real second machine over LAN", "pair from a real remote network"). The end-to-end loopback pairing-over-Iroh is covered by Task 220's smoke (driver-level, not UI).

## Definition of Done
- [ ] Connect-to-Core picker (paired list + status dots + "Start local"/"Pair remote" entry points)
- [ ] Pairing flow: Scan-QR **and** Paste-token, payload decode, complete-pairing via shell command, 60s-TTL UX, name-the-pairing (default = Core hostname), set active
- [ ] Settings → Connected Cores: list + switch-active (clears cache/reloads) + rename + remove (best-effort revoke) + add-another
- [ ] QR-show affordance gated on `transport_kind === UDS` (disabled/hinted in split-host per §3.11)
- [ ] QR libs chosen + added + license posture noted; paste-token works with no webcam
- [ ] Pairing-UI ↔ Tauri-command contract frozen (co-designed with 218's seams); payload-decode shape matches `concerto pair`
- [ ] `web-ts` §5.3 set passes (typecheck/lint/test/build); co-located smoke unaffected (gate unchanged)
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate seams for 601 documented)
- [ ] Single commit with the message below

## Outputs
- `apps/desktop/src/components/ConnectCorePicker.tsx` (new — the picker)
- `apps/desktop/src/components/PairCoreModal.tsx` (new — scan/paste pairing flow + naming)
- `apps/desktop/src/components/ShowPairingQr.tsx` (new — local QR show, UDS-gated)
- `apps/desktop/src/components/SettingsPanel.tsx` (modified — Connected Cores section)
- `apps/desktop/src/components/ConnectedCoresList.tsx` (new — switch/rename/remove rows)
- `apps/desktop/src/api/cores.ts` (modified — add the pairing command bindings co-designed with 218)
- `apps/desktop/src/state/useUiStore.ts` (modified — picker/pairing open-state)
- `apps/desktop/src/components/*.test.tsx` (new — vitest component tests + fixture payloads)
- `apps/desktop/package.json` (modified — add `qrcode`/`@zxing/browser` (or chosen equivalents) + any `@testing-library/react`/`jsdom` devDeps)

## Commit message
```
phase-2: desktop pairing UI (picker + QR/token + Connected Cores)

Adds the renderer pairing surfaces on top of Task 218's CoreClient +
cores.json registry: a Connect-to-Core picker, the split-host pairing
flow (scan QR or paste token → Devices.CompletePairing via the shell,
60s-TTL UX, name + set active), and Settings → Connected Cores
(switch/rename/remove). Drives Devices RPCs (207/209) through Tauri
commands; never speaks gRPC directly.

Refs: tasks/v1.0/219-desktop-pairing-ui.md
```

## Handoff Notes (fill in when finishing)
- Drift from plan / QR-show + scan lib choices + license posture / camera-permission decision (entitlement vs paste-only) / the frozen pairing-UI↔Tauri-command signatures / vitest+RTL setup reused vs added / seams left for 601 / Open questions / Smoke-gate state
