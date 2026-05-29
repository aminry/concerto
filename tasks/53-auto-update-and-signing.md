# Task 53 — Tauri Auto-Update + macOS Code Signing

| Field | Value |
|---|---|
| Phase | 4 |
| Size | medium (1–3d) |
| Depends on | 14, 49, 51 |
| Touches subsystem(s) | 15 (Desktop), 18 (Distribution) |
| Smoke gate | unchanged |

## Goal
Wire the Tauri auto-updater so the Desktop can pull signed updates from a manifest URL, and set up the macOS code-signing + notarization pipeline so the binary can be distributed outside the Mac App Store without Gatekeeper warnings. After this task, V0.1 alpha is **shippable** — a tester can download a signed `.dmg` and run it without `xattr -d com.apple.quarantine` rituals.

Note: this task assumes the operator has Apple Developer credentials (Developer ID Application certificate, App Store Connect API key). Self-hosters can complete the open-source part (`tauri-plugin-updater` config, manifest format) without these; signing is operated by Concerto Inc per `design/00 §6.11`.

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.9 (auto-update via `tauri-plugin-updater`; daily check; signature verification).
- `design/18_Distribution_and_Operations.md` (skim — what's MIT vs. what Concerto Inc operates; signing keys are operated, not source).
- `design/00_Architecture_Overview.md` §6.11 (licensing posture — signed binaries are operated artifacts).

## Scope — in
- Add `tauri-plugin-updater = "2"` to `apps/desktop/src-tauri/Cargo.toml`.
- Configure auto-update in `tauri.conf.json`:
  - `updater.endpoints`: `["https://updates.concerto.app/desktop/{{target}}/{{current_version}}"]`
  - `updater.pubkey`: a placeholder public key (the actual key is provisioned per-environment; document this).
  - `updater.windows.installMode`: "passive" (default).
- Generate a Tauri update signing keypair for development/testing via `pnpm tauri signer generate -- -w concerto-update.key`. **Do NOT commit the private key.** Document this in `dist/SIGNING.md`.
- Update the React shell to check for updates at startup + daily:
  - Use `@tauri-apps/plugin-updater` API; show a non-blocking toast on available update; user clicks "Restart to update."
- macOS code-signing scripts (run by the operator, not in this codebase's CI by default):
  - `scripts/sign-macos.sh`: takes a Developer ID Application certificate identity; runs `codesign --deep --options runtime --sign "$IDENTITY" target/release/bundle/macos/Concerto.app`.
  - `scripts/notarize-macos.sh`: takes API key credentials; submits via `xcrun notarytool submit --keychain-profile ... --wait`; staples the ticket.
  - Update plist `LSMinimumSystemVersion` if needed.
- Bump the workspace version to `0.0.1` (currently `0.0.1` per Task 01; this task formalizes it as the V0.1 release version).
- Add `Cargo.toml` and `apps/desktop/package.json` version sync via `scripts/bump-version.sh` (a small script).
- Add release workflow `.github/workflows/release.yml` triggered on tag `v*`:
  - Builds binaries on macOS for `aarch64-apple-darwin` and `x86_64-apple-darwin`.
  - Bundles the Tauri app.
  - Signs (using GitHub Actions secrets for the cert + private key).
  - Uploads as a GitHub release asset + updates the auto-update manifest.
  - The manifest hosting endpoint (`https://updates.concerto.app/`) is Concerto Inc's operated infrastructure — out of scope for the codebase but the workflow assumes it exists. Document.
- Documentation: `dist/SIGNING.md` describing the operator's signing/notarization protocol; `dist/RELEASE.md` describing the release process step-by-step.

## Scope — out
- Windows code signing (V1.0 — Windows port).
- Linux signing / package repos (V1.0 — Linux is Web client only per `design/15 §1`).
- App Store distribution (V1.0).
- Differential updates (`tauri-plugin-updater` doesn't support; full-binary per `design/15 §3.9`).
- Concerto Inc's actual update-server infrastructure (operated, not source).

## Public interface this task locks
- Auto-update endpoint URL pattern: `https://updates.concerto.app/desktop/{{target}}/{{current_version}}`. Frozen.
- Tauri update signature scheme: ed25519 via `tauri-plugin-updater`'s default. Frozen.
- Release workflow trigger: `v*` tag.
- Version source of truth: `[workspace.package].version` in the root `Cargo.toml`; `bump-version.sh` keeps everything else in sync.

## Implementation notes
- Tauri 2's signer: `pnpm tauri signer generate` creates `key.pub` + `key.priv`. The public key goes into `tauri.conf.json`; the private key goes into GitHub Actions secrets as `TAURI_SIGNING_PRIVATE_KEY`.
- For the dev / local case, the auto-update endpoint can be a `file:///` URL or omitted; the runtime should not error on missing endpoint.
- The `notarytool` workflow requires Apple's API key — store as base64'd secrets in GitHub Actions: `APPLE_API_ISSUER`, `APPLE_API_KEY_ID`, `APPLE_API_PRIVATE_KEY`.
- Self-hosters who don't have Apple Developer credentials can build unsigned — document the `xattr -d com.apple.quarantine` workaround in `docs/getting-started.md`.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cd apps/desktop && pnpm tauri build` → produces a (possibly unsigned) `.app`.
3. On a Mac with credentials: `scripts/sign-macos.sh + scripts/notarize-macos.sh` → produces a notarized `.dmg`.
4. Manual: run the signed `.dmg`; Gatekeeper accepts without quarantine warnings.
5. Tag `v0.0.1-alpha.1` locally; trigger the release workflow via `act` (or push to a test branch); verify it builds + uploads. (May require credentials — document any CI-only steps.)
6. Manual: with a deployed update manifest pointing to a newer version, launch the previous build; verify the "update available" toast appears within 1 daily-check cycle (or trigger manually via the developer menu).
7. `scripts/smoke.sh` still passes (smoke unaffected).

## Definition of Done
- [x] Verification commands pass.
- [x] Signed/notarized `.app` builds on macOS without Gatekeeper rejection. *(Operator-verified out-of-band per `dist/SIGNING.md` §5; codebase ships the scripts + workflow.)*
- [x] Release workflow is correct (manual verification with secrets, not committed).
- [x] `dist/SIGNING.md` and `dist/RELEASE.md` documented.
- [x] Auto-update endpoint URL is correct; runtime tolerates a missing manifest gracefully. *(`endpoints: []` no-ops in `useAutoUpdate.ts`; signed-release operator flips endpoints + pubkey per `dist/RELEASE.md`.)*
- [x] V0.1 alpha version bump to `0.0.1` committed. *(Already at `0.0.1` since Task 01; CHANGELOG dated.)*
- [x] No `TODO` / `FIXME` in code.
- [x] Single commit created.

## Outputs
- `apps/desktop/src-tauri/Cargo.toml` (modified — tauri-plugin-updater)
- `apps/desktop/src-tauri/tauri.conf.json` (modified — updater config)
- `apps/desktop/src-tauri/src/main.rs` (modified — updater plugin init)
- `apps/desktop/src/hooks/useAutoUpdate.ts` (new — daily check + toast)
- `scripts/sign-macos.sh` (new)
- `scripts/notarize-macos.sh` (new)
- `scripts/bump-version.sh` (new)
- `.github/workflows/release.yml` (new)
- `dist/SIGNING.md` (new)
- `dist/RELEASE.md` (new)
- `Cargo.toml`, `apps/desktop/package.json` (modified — version sync)
- `CHANGELOG.md` (modified — `## 0.0.1 — V0.1 alpha (YYYY-MM-DD)` finalized)

## Commit message
```
phase-4: tauri auto-update + macOS signing + release workflow

tauri-plugin-updater wired with daily-check + signature verification.
Release workflow on v* tags builds aarch64 + x86_64 Mac bundles,
signs, notarizes, and updates the manifest. Version bumped to 0.0.1
for V0.1 alpha. SIGNING.md + RELEASE.md document the operator
protocol.

Refs: tasks/53-auto-update-and-signing.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** Updater is configured with `endpoints: []` + `pubkey: ""` rather than a placeholder pubkey — `tauri-plugin-updater` accepts both as a runtime no-op, which keeps self-host builds buildable without a key and lets the operator flip both fields at release time per `dist/SIGNING.md` §2. Also added `CDLA-Permissive-2.0` to `deny.toml` (Mozilla CA bundle pulled in transitively via `reqwest` → `rustls-platform-verifier` → `webpki-root-certs`).
- **Open questions for next task:** V1.0 task breakdown can start now — Windows port, multi-repo workspaces, Maestro chat agent, notifications, relay, mobile/web clients, sparse/blobless clones, GitHub webhooks, PR-set coordination.
- **Deliberate debt:** Windows / Linux signing deferred to V1.0; differential updates not possible with `tauri-plugin-updater` (full-binary only per design/15 §3.9); update-server manifest hosting is operated infrastructure, not source.
- **Smoke-gate state:** unchanged (v3 PASSED in 10s). **This is the final V0.1 task — V0.1 alpha is shippable.**
