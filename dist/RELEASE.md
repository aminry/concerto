# Release Process

End-to-end protocol for cutting a Concerto release. Targets the
**operator workflow** (Concerto Inc maintainer); self-hosters can stop
after step 1 and build a local unsigned bundle.

Companion docs:
- [`SIGNING.md`](SIGNING.md) — codesign + notarization + Tauri updater
  keys.
- [`SMOKE.md`](SMOKE.md) — what the smoke gate verifies before a tag is
  cut.

---

## 1. Pre-release checks

```sh
# All green required before tagging.
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo deny check
./scripts/smoke.sh
```

Plus:

- `CHANGELOG.md` has a dated heading for the new version
  (`## 0.0.2 — V0.1 alpha.2 (YYYY-MM-DD)`).
- README badges + screenshots still match the current shell.
- No `TODO` / `FIXME` in the diff since the previous tag
  (`git diff vPREV..HEAD | grep -E 'TODO|FIXME'`).

---

## 2. Bump the version literal

```sh
VERSION=0.0.2 ./scripts/bump-version.sh
git diff
git add Cargo.toml Cargo.lock apps/desktop/package.json apps/desktop/src-tauri/tauri.conf.json
git commit -m "release: v0.0.2"
```

The script also runs `cargo update --workspace` so `Cargo.lock` is in
sync. If you'd rather review lockfile changes separately, run with
`--no-lock-update` (TODO: not implemented yet — for V0.1 the merged
update is fine).

---

## 3. Tag and push

```sh
git tag -a v0.0.2 -m "Concerto v0.0.2"
git push origin main
git push origin v0.0.2
```

Pushing the tag triggers
[`.github/workflows/release.yml`](../.github/workflows/release.yml),
which:

1. Builds `aarch64-apple-darwin` and `x86_64-apple-darwin` Tauri
   bundles in parallel on `macos-latest`.
2. Codesigns each with the Developer ID Application cert from the
   `APPLE_CERTIFICATE` repository secret.
3. Notarizes via `xcrun notarytool` and staples the ticket.
4. Uploads the signed `.dmg`, the `.app.tar.gz` update bundle, and the
   detached `.sig` to a GitHub release named `v0.0.2`.

Job duration: ~12 minutes per arch, ~25 minutes wall clock (the two
matrix legs run in parallel).

---

## 4. Update the auto-update manifest

The auto-updater fetches its manifest from
`https://updates.concerto.app/desktop/{{target}}/{{current_version}}`
(see [`design/15_Desktop_Client.md`](../design/15_Desktop_Client.md)
§3.9). The endpoint is **Concerto Inc operated infrastructure** —
it is **not** part of this repository.

After step 3 completes, the operator publishes a new manifest entry
that points at the GitHub release assets:

```json
{
  "version": "0.0.2",
  "notes": "See https://github.com/aminry/concerto/releases/tag/v0.0.2",
  "pub_date": "2026-05-28T00:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "<contents of .app.tar.gz.sig>",
      "url": "https://github.com/aminry/concerto/releases/download/v0.0.2/Concerto_0.0.2_aarch64.app.tar.gz"
    },
    "darwin-x86_64": {
      "signature": "<contents of .app.tar.gz.sig>",
      "url": "https://github.com/aminry/concerto/releases/download/v0.0.2/Concerto_0.0.2_x64.app.tar.gz"
    }
  }
}
```

The update server's deployment runbook lives outside this repo (per
[`design/18_Distribution_and_Operations.md`](../design/18_Distribution_and_Operations.md)).

---

## 5. Post-release verification

```sh
# Download the .dmg from the GitHub release page.
curl -L -o /tmp/Concerto.dmg \
    https://github.com/aminry/concerto/releases/download/v0.0.2/Concerto_0.0.2_aarch64.dmg

# Mount + assess.
hdiutil attach /tmp/Concerto.dmg
spctl --assess --type execute --verbose=4 /Volumes/Concerto/Concerto.app
xcrun stapler validate /Volumes/Concerto/Concerto.app
hdiutil detach /Volumes/Concerto
```

Both checks should report `accepted` / `validated`. If either fails,
yank the release and investigate before pushing a fix tag.

Then exercise the auto-update path:

1. Install the **previous** version (`v0.0.1`).
2. Wait for the daily check, or force it via the developer menu.
3. Verify the green "Update available: 0.0.2" toast appears, the
   "Restart to update" button works, and the app relaunches at the
   new version.

---

## 6. Rollback

If a release ships broken:

1. Revert the manifest entry on the update server so existing clients
   don't see the bad version (auto-update is opt-in but most users
   click through).
2. Mark the GitHub release as a **pre-release** so it falls off the
   "latest" pointer.
3. Cut a fix release (`v0.0.3`) following §1–§5. Do **not** delete
   the bad tag — keeping it preserves git history and makes the next
   bisect easier.

There is no in-place patch path; auto-update only flows forward.
