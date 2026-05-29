# Signing & Notarization Protocol

This document describes how the **Concerto Inc operator** signs and
notarizes the macOS desktop bundle. The signing keys themselves are
**operated artifacts** per [`design/00_Architecture_Overview.md`](../design/00_Architecture_Overview.md)
§6.11 — they are not in this repository, and self-hosters do not need
them to build or run Concerto locally.

> Self-hosters: see [`docs/getting-started.md`](../docs/getting-started.md)
> §5 for the `xattr -d com.apple.quarantine` workaround that lets an
> unsigned local build run on macOS without Gatekeeper friction.

---

## 1. Prerequisites (operator)

| Item | Where it lives | Provisioned by |
|---|---|---|
| Developer ID Application certificate | Apple Developer portal → exported `.p12` | Concerto Inc Apple Developer account |
| App Store Connect API key | App Store Connect → Users and Access → Keys → `.p8` file + Key ID + Issuer ID | Concerto Inc App Store Connect admin |
| Tauri updater signing key | `pnpm tauri signer generate -w concerto-update.key` | Generated once, stored in 1Password |

**None of these are committed.** All three live in 1Password ("Concerto
Inc / Release Signing") and are projected into GitHub Actions as
encrypted repository secrets (see §4).

---

## 2. Local one-time setup

```sh
# 1. Generate the Tauri updater signing keypair (run ONCE; key is
#    archived in 1Password and never regenerated unless rotated).
cd apps/desktop
pnpm tauri signer generate -w ~/.concerto-secrets/concerto-update.key
#   -> ~/.concerto-secrets/concerto-update.key      (private; archive)
#   -> ~/.concerto-secrets/concerto-update.key.pub  (public; ship)

# 2. Paste the public key into `apps/desktop/src-tauri/tauri.conf.json`
#    under `plugins.updater.pubkey`. The V0.1 codebase ships with `""`
#    (empty) so the runtime no-ops; flip it at release time.

# 3. Store notarytool credentials in your local keychain.
xcrun notarytool store-credentials concerto-notarytool \
    --key ~/.concerto-secrets/AuthKey_XXXXXXXXXX.p8 \
    --key-id XXXXXXXXXX \
    --issuer 12345678-1234-1234-1234-123456789012
```

After step 2 the value of `plugins.updater.pubkey` is the
**`untrusted comment: minisign public key …` + base64 blob**, all on
one line, as printed by `tauri signer generate`. Do not check this
into the repo until release time.

---

## 3. Local manual release

For a manual ad-hoc release without CI:

```sh
# 1. Bump the version literal.
VERSION=0.0.2 ./scripts/bump-version.sh

# 2. Build the Tauri bundle (this takes a few minutes the first run).
cd apps/desktop
pnpm install
pnpm tauri build --target aarch64-apple-darwin

# 3. Sign.
cd ../..
IDENTITY="Developer ID Application: Concerto Inc (XXXXXXXXXX)" \
    APP_PATH="apps/desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Concerto.app" \
    ./scripts/sign-macos.sh

# 4. Notarize + staple.
KEYCHAIN_PROFILE=concerto-notarytool \
    APP_PATH="apps/desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Concerto.app" \
    ./scripts/notarize-macos.sh
```

After step 4 the `.app` (and the `.dmg` next to it, if Tauri bundled
one) is Gatekeeper-acceptable.

---

## 4. CI release (preferred)

The GitHub Actions workflow at
[`.github/workflows/release.yml`](../.github/workflows/release.yml)
runs on every `v*` tag push. It performs steps 2–4 of §3 inside a
disposable runner keychain, then uploads the artifacts to a GitHub
release named after the tag.

**Required repository secrets** (set under *Settings → Secrets and
variables → Actions*):

| Secret | Encoding | Source |
|---|---|---|
| `APPLE_CERTIFICATE` | base64 of the `.p12` | `base64 -i cert.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | raw | password used when exporting `.p12` |
| `APPLE_SIGNING_IDENTITY` | raw | `"Developer ID Application: Concerto Inc (TEAMID)"` |
| `APPLE_TEAM_ID` | raw | 10-char Apple Developer Team ID |
| `APPLE_API_ISSUER` | raw | Issuer UUID from App Store Connect |
| `APPLE_API_KEY_ID` | raw | Key ID from App Store Connect |
| `APPLE_API_PRIVATE_KEY` | base64 of the `.p8` | `base64 -i AuthKey_XXX.p8` |
| `TAURI_SIGNING_PRIVATE_KEY` | contents of `concerto-update.key` | from §2 step 1 |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | raw | password used in §2 step 1 |

---

## 5. Verifying a release locally

```sh
# 1. Download the signed .app or .dmg.
# 2. Run Apple's offline Gatekeeper assessment:
spctl --assess --type execute --verbose=4 /path/to/Concerto.app

# Expected output:
#   /path/to/Concerto.app: accepted
#   source=Notarized Developer ID

# 3. Verify the stapled ticket:
xcrun stapler validate /path/to/Concerto.app
```

If either step fails, the bundle is not shippable. Re-run the relevant
script and submit again.

---

## 6. Key rotation

Tauri updater keypair (ed25519) and Apple Developer ID certs rotate on
different cadences:

- **Apple Developer ID Application cert**: 5-year validity; rotate ~30
  days before expiry. Old releases keep working because notarization
  is stapled.
- **Tauri updater keypair**: rotate only if compromised. Rotating
  invalidates all older clients' ability to verify updates — they will
  need a manual reinstall from a fresh download.

When rotating the Tauri keypair, ship a **transition release** signed
by *both* the old and new keys (Tauri 2 supports a comma-separated
`pubkey` field for this), then a follow-up release signed by only the
new key. Coordinate the cutover with the
[`design/18_Distribution_and_Operations.md`](../design/18_Distribution_and_Operations.md)
runbook.
