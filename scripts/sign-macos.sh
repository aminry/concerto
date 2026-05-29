#!/usr/bin/env bash
# shellcheck shell=bash
#
# sign-macos.sh — codesign the Tauri-bundled Concerto.app with a
# Developer ID Application certificate.
#
# Operator-only: this script is part of the Concerto Inc release
# protocol per design/18 + dist/SIGNING.md. Self-hosters do NOT need to
# run it; unsigned builds are usable via `xattr -d com.apple.quarantine`
# (see docs/getting-started.md §5).
#
# Inputs (env):
#   IDENTITY        — the Developer ID Application identity string, as
#                     accepted by `codesign --sign`. Example:
#                     "Developer ID Application: Concerto Inc (TEAMID)".
#   APP_PATH        — optional override for the .app bundle. Defaults to
#                     target/release/bundle/macos/Concerto.app relative
#                     to the repo root.
#
# Exits non-zero on any failure (codesign verify gate at the end).
# Bash 3.2 compatible.

set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
    echo "sign-macos.sh: this script is macOS-only (uname=$(uname -s))" >&2
    exit 1
fi

if [ -z "${IDENTITY:-}" ]; then
    echo "sign-macos.sh: IDENTITY env var is required" >&2
    echo "  example: IDENTITY=\"Developer ID Application: Concerto Inc (TEAMID)\"" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_PATH="${APP_PATH:-$REPO_ROOT/target/release/bundle/macos/Concerto.app}"

if [ ! -d "$APP_PATH" ]; then
    echo "sign-macos.sh: app bundle not found at $APP_PATH" >&2
    echo "  Build it first with: cd apps/desktop && pnpm tauri build" >&2
    exit 1
fi

echo "==> Signing $APP_PATH"
echo "    Identity: $IDENTITY"

# --deep   : recursively sign nested helpers, frameworks, .dylibs.
# --force  : overwrite any pre-existing signature (Tauri's bundler
#            sometimes leaves an ad-hoc one).
# --timestamp : embed a secure timestamp (notarytool requires this).
# --options runtime : opt into the hardened runtime, required for
#                     notarization per Apple's TN3147.
codesign \
    --deep \
    --force \
    --timestamp \
    --options runtime \
    --sign "$IDENTITY" \
    "$APP_PATH"

echo "==> Verifying signature"
codesign --verify --deep --strict --verbose=2 "$APP_PATH"

echo "==> Signed."
echo "    Next step: scripts/notarize-macos.sh"
