#!/usr/bin/env bash
# shellcheck shell=bash
#
# bump-version.sh — rewrite Concerto's version literals in the three
# places they live:
#
#   1. Root Cargo.toml `[workspace.package].version` — source of truth.
#   2. apps/desktop/package.json `.version` — Tauri's renderer manifest.
#   3. apps/desktop/src-tauri/tauri.conf.json `.version` — Tauri's
#      bundler config.
#
# `Cargo.lock` is refreshed via `cargo update --workspace` after the
# Cargo.toml edit so the lockfile stays in sync.
#
# Inputs (env or arg 1):
#   VERSION — the new version literal (semver). Example: "0.0.2".
#
# Bash 3.2 compatible (default macOS /bin/bash). Uses BSD-compatible
# `sed -i ''` so it runs identically on macOS and Linux. The JSON edits
# use `node` to keep them schema-safe (no JSON regex hacks).

set -euo pipefail

VERSION="${VERSION:-${1:-}}"
if [ -z "$VERSION" ]; then
    echo "bump-version.sh: VERSION env var or positional arg required" >&2
    echo "  usage: VERSION=0.0.2 ./scripts/bump-version.sh" >&2
    echo "     or: ./scripts/bump-version.sh 0.0.2" >&2
    exit 1
fi

# Loose semver guard — three numeric segments, optional pre-release.
if ! echo "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$'; then
    echo "bump-version.sh: VERSION '$VERSION' does not look like semver" >&2
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ROOT_CARGO="$REPO_ROOT/Cargo.toml"
DESKTOP_PKG="$REPO_ROOT/apps/desktop/package.json"
TAURI_CONF="$REPO_ROOT/apps/desktop/src-tauri/tauri.conf.json"

for f in "$ROOT_CARGO" "$DESKTOP_PKG" "$TAURI_CONF"; do
    if [ ! -f "$f" ]; then
        echo "bump-version.sh: missing required file $f" >&2
        exit 1
    fi
done

if ! command -v node >/dev/null 2>&1; then
    echo "bump-version.sh: node not found in PATH (needed for JSON edits)" >&2
    exit 1
fi

echo "==> Bumping to $VERSION"

# 1. Root Cargo.toml: rewrite the version literal inside the
# [workspace.package] block only. Two-pass awk keeps us from matching
# anything outside that section.
TMP_CARGO="$(mktemp -t Cargo.toml.XXXXXX)"
trap 'rm -f "$TMP_CARGO"' EXIT
awk -v ver="$VERSION" '
    BEGIN { in_wp = 0 }
    /^\[workspace\.package\]/ { in_wp = 1; print; next }
    /^\[/ && !/^\[workspace\.package\]/ { in_wp = 0 }
    in_wp && /^version[[:space:]]*=/ {
        print "version = \"" ver "\""
        next
    }
    { print }
' "$ROOT_CARGO" > "$TMP_CARGO"
mv -f "$TMP_CARGO" "$ROOT_CARGO"
trap - EXIT

echo "    [OK] Cargo.toml"

# 2. apps/desktop/package.json — preserves field ordering + indentation.
node -e "
    const fs = require('fs');
    const path = '$DESKTOP_PKG';
    const pkg = JSON.parse(fs.readFileSync(path, 'utf8'));
    pkg.version = '$VERSION';
    fs.writeFileSync(path, JSON.stringify(pkg, null, 2) + '\n');
"
echo "    [OK] apps/desktop/package.json"

# 3. apps/desktop/src-tauri/tauri.conf.json.
node -e "
    const fs = require('fs');
    const path = '$TAURI_CONF';
    const conf = JSON.parse(fs.readFileSync(path, 'utf8'));
    conf.version = '$VERSION';
    fs.writeFileSync(path, JSON.stringify(conf, null, 2) + '\n');
"
echo "    [OK] apps/desktop/src-tauri/tauri.conf.json"

# 4. Refresh Cargo.lock so the new version flows into the lockfile.
echo "==> Refreshing Cargo.lock"
(cd "$REPO_ROOT" && cargo update --workspace >/dev/null 2>&1) || {
    echo "bump-version.sh: cargo update failed; rerun manually" >&2
    exit 1
}
echo "    [OK] Cargo.lock"

echo "==> Bumped to $VERSION."
echo "    Review the diff, then commit + tag:"
echo "      git add -p && git commit -m 'release: v$VERSION'"
echo "      git tag v$VERSION"
