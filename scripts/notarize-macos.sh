#!/usr/bin/env bash
# shellcheck shell=bash
#
# notarize-macos.sh — submit Concerto.app to Apple's notarization
# service via `xcrun notarytool`, then staple the ticket so Gatekeeper
# clears the bundle offline.
#
# Operator-only: assumes the operator has set up an App Store Connect
# API key + stored credentials in a notarytool keychain profile via
#
#   xcrun notarytool store-credentials "$KEYCHAIN_PROFILE" \
#     --key /path/to/AuthKey_XXXXX.p8 \
#     --key-id KEYID --issuer ISSUERUUID
#
# See dist/SIGNING.md for the full operator protocol.
#
# Inputs (env):
#   KEYCHAIN_PROFILE — name of the notarytool credentials profile.
#                      Example: "concerto-notarytool".
#   APP_PATH         — optional override for the .app or .dmg path.
#                      Defaults to target/release/bundle/macos/Concerto.app.
#
# Bash 3.2 compatible.

set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
    echo "notarize-macos.sh: this script is macOS-only (uname=$(uname -s))" >&2
    exit 1
fi

if [ -z "${KEYCHAIN_PROFILE:-}" ]; then
    echo "notarize-macos.sh: KEYCHAIN_PROFILE env var is required" >&2
    echo "  Set it up first with: xcrun notarytool store-credentials" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_PATH="${APP_PATH:-$REPO_ROOT/target/release/bundle/macos/Concerto.app}"

if [ ! -e "$APP_PATH" ]; then
    echo "notarize-macos.sh: artifact not found at $APP_PATH" >&2
    exit 1
fi

# notarytool wants a flat archive (.zip or .dmg). If we were handed a
# .app bundle, ditto-zip it into a tempfile first.
SUBMIT_PATH="$APP_PATH"
CLEANUP=""
case "$APP_PATH" in
    *.app)
        SUBMIT_PATH="$(mktemp -t Concerto.XXXXXX).zip"
        CLEANUP="$SUBMIT_PATH"
        echo "==> Archiving $APP_PATH -> $SUBMIT_PATH"
        ditto -c -k --keepParent "$APP_PATH" "$SUBMIT_PATH"
        ;;
esac
# shellcheck disable=SC2064
trap "[ -n \"$CLEANUP\" ] && rm -f \"$CLEANUP\"" EXIT

echo "==> Submitting to notarytool (profile: $KEYCHAIN_PROFILE)"
xcrun notarytool submit \
    --keychain-profile "$KEYCHAIN_PROFILE" \
    --wait \
    "$SUBMIT_PATH"

# Staple the ticket onto the .app so the user's Mac doesn't need to
# hit Apple's servers on first launch.
case "$APP_PATH" in
    *.app|*.dmg|*.pkg)
        echo "==> Stapling $APP_PATH"
        xcrun stapler staple "$APP_PATH"
        echo "==> Verifying staple"
        xcrun stapler validate "$APP_PATH"
        ;;
    *)
        echo "==> Skipping staple (artifact is not .app/.dmg/.pkg)"
        ;;
esac

echo "==> Notarized."
