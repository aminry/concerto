#!/usr/bin/env bash
# Bake the pre-packed sparse-cone fixture the Task 303 bench gate restores.
#
# Spike 104 (design/spikes/gix-sparse-cone-findings.md §7 rec 3) mandates a
# PRE-PACKED fixture image so the CI status gate measures `git status`, not
# fixture construction: generating loose objects per run is the minutes-long
# cost the gate must never pay. This script bakes that image ONCE and writes
# an uncompressed tar of a real git repo whose objects already live in a
# pack and whose index is already a sparse index. The bench gate + the
# integration test simply untar it and run `status` — no per-run laydown.
#
# ## What it builds (FROZEN restore contract)
#
# A cone-mode sparse-checkout repo laid out at `top/<t>/sub/<s>/f<i>.txt`:
#   - TOTAL top-level dirs (`top/00000`..), each holding sub-dirs of files.
#   - The FIRST `CONE_DIRS` top-level dirs form the cone (materialized on
#     disk); every other top dir is tracked in the commit but collapsed to a
#     single sparse-index directory entry (skip-worktree) — the lever the
#     `< 100 ms` bar leans on (spike §4a).
#   - `feature.manyFiles=true` (→ index v4 + skipHash + untrackedCache),
#     `core.untrackedCache=true`, a written commit-graph — the large-repo
#     config bundle git recommends (design/00 §7.7).
#   - `git sparse-checkout init --cone` + `set <cone dirs>` +
#     `reapply --sparse-index` so the ON-DISK index is in the sparse format.
#   - A handful of in-cone tracked files modified + a few untracked files
#     added so `status` reports REAL work (not a clean-tree no-op).
#   - `git repack -ad` + `git gc` so ALL objects live in a single pack (no
#     loose objects) — this is the "pre-packed" part.
#
# The committed fixture is intentionally SMALL (hundreds of files, not 2M):
# it must fit the repo + restore identically on every OS CI lane (Task 113),
# and at this scale cold `status` is a few ms — the gate passes the 100 ms
# bar with enormous margin. The real 2M-file-monorepo number is the
# Phase-3 Tier-3 checklist line, corroborated by spike 104's extrapolation
# (~75 ms p50 linear). The gate proves the regression FLOOR + the wiring;
# the 2M confirmation is the operator's at the phase gate.
#
# ## Determinism
#
# Fixed author/committer identity + a fixed commit date + a pinned config
# (GIT_CONFIG_GLOBAL/SYSTEM=/dev/null) keep the *logical* tree reproducible.
# The tar bytes are NOT asserted to be byte-identical across git versions
# (pack layout can differ); the restore contract is "a valid sparse-index
# repo with a known cone + known dirty set", which every modern git restores
# identically. The bench asserts the restored index IS sparse before timing.
#
# ## Usage
#
#   scripts/build-sparse-fixture.sh            # bake the default fixture
#   FIXTURE_TOTAL_DIRS=8 FIXTURE_CONE_DIRS=1 scripts/build-sparse-fixture.sh
#
# Writes: crates/gix-wrap/tests/fixtures/sparse-cone.tar
#
# This is a bake-time tool. It needs a working `git` with the
# `sparse-checkout` subcommand (git >= 2.27; --sparse-index >= 2.32). It is
# NOT run in CI — CI restores the committed tar. macOS/Linux bash.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="$ROOT/crates/gix-wrap/tests/fixtures"
OUT_TAR="$OUT_DIR/sparse-cone.tar"

# Fixture shape (override via env). Defaults: 6 top dirs × 4 sub × 25 files
# = 600 tracked files; cone = first 1 top dir (100 files materialized). Small
# enough to commit, large enough that the sparse index visibly collapses the
# out-of-cone dirs.
TOTAL_DIRS="${FIXTURE_TOTAL_DIRS:-6}"
CONE_DIRS="${FIXTURE_CONE_DIRS:-1}"
SUBS_PER_TOP="${FIXTURE_SUBS:-4}"
FILES_PER_SUB="${FIXTURE_FILES_PER_SUB:-25}"

# A fixed commit date keeps the logical history reproducible.
export GIT_AUTHOR_NAME="concerto-fixture"
export GIT_AUTHOR_EMAIL="fixture@concerto.invalid"
export GIT_COMMITTER_NAME="concerto-fixture"
export GIT_COMMITTER_EMAIL="fixture@concerto.invalid"
export GIT_AUTHOR_DATE="2026-01-01T00:00:00 +0000"
export GIT_COMMITTER_DATE="2026-01-01T00:00:00 +0000"
# Never inherit the developer's global/system git config.
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_TERMINAL_PROMPT=0

# `git sparse-checkout -h` prints usage and exits non-zero on every git, so
# capture the help text (pipefail-safe) and look for the subcommand name.
sc_help="$(git sparse-checkout -h 2>&1 || true)"
case "$sc_help" in
*sparse-checkout*) ;;
*)
    echo "error: this git lacks the 'sparse-checkout' subcommand (need >= 2.27)" >&2
    exit 1
    ;;
esac

WORK="$(mktemp -d "${TMPDIR:-/tmp}/concerto-sparse-fixture.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT
REPO="$WORK/repo"
mkdir -p "$REPO"
cd "$REPO"

echo "[build-sparse-fixture] init repo at $REPO" >&2
git init -q -b main .
# Large-repo config bundle (design/00 §7.7).
git config feature.manyFiles true
git config core.untrackedCache true
git config index.version 4
git config core.commitGraph true

echo "[build-sparse-fixture] laying down ${TOTAL_DIRS} top dirs ($((TOTAL_DIRS * SUBS_PER_TOP * FILES_PER_SUB)) files)" >&2
t=0
while [ "$t" -lt "$TOTAL_DIRS" ]; do
    top="$(printf 'top/%05d' "$t")"
    s=0
    while [ "$s" -lt "$SUBS_PER_TOP" ]; do
        sub="$(printf '%s/sub/%04d' "$top" "$s")"
        mkdir -p "$sub"
        i=0
        while [ "$i" -lt "$FILES_PER_SUB" ]; do
            printf 'top%d sub%d f%d\n' "$t" "$s" "$i" >"$(printf '%s/f%04d.txt' "$sub" "$i")"
            i=$((i + 1))
        done
        s=$((s + 1))
    done
    t=$((t + 1))
done

git add -A
git commit -q -m "seed sparse-cone fixture"
git commit-graph write --reachable >/dev/null 2>&1 || true

echo "[build-sparse-fixture] cone = first ${CONE_DIRS} top dir(s); enabling sparse-index" >&2
git sparse-checkout init --cone
cone_args=()
t=0
while [ "$t" -lt "$CONE_DIRS" ]; do
    cone_args+=("$(printf 'top/%05d' "$t")")
    t=$((t + 1))
done
git sparse-checkout set "${cone_args[@]}"
# The lever the < 100 ms bar leans on: rewrite the on-disk index in the
# sparse format so out-of-cone dirs collapse to single directory entries.
git sparse-checkout reapply --sparse-index

# Confirm the index is genuinely sparse before packing — a fixture that lost
# its sparse index would silently make the gate measure full-repo status.
if ! git ls-files --sparse | grep -q '/$'; then
    echo "error: index is NOT sparse (no collapsed directory entries found)" >&2
    exit 1
fi

echo "[build-sparse-fixture] dirtying a handful of in-cone files + untracked" >&2
# Modify a few in-cone tracked files so status reports real modifications.
d=0
for f in "$(printf 'top/%05d' 0)"/sub/0000/f0000.txt \
    "$(printf 'top/%05d' 0)"/sub/0000/f0001.txt \
    "$(printf 'top/%05d' 0)"/sub/0001/f0000.txt; do
    if [ -f "$f" ]; then
        printf 'modified by build-sparse-fixture\n' >"$f"
        d=$((d + 1))
    fi
done
echo "[build-sparse-fixture] modified $d in-cone files" >&2
# A couple of untracked files (also in-cone so the dirwalk reports them).
printf 'untracked\n' >"$(printf 'top/%05d' 0)/untracked-a.txt"
printf 'untracked\n' >"$(printf 'top/%05d' 0)/untracked-b.txt"

# Pre-pack: move every object into a single pack, drop loose objects. This is
# the "pre-packed" guarantee — the gate never hashes loose objects.
echo "[build-sparse-fixture] repacking objects into a single pack" >&2
git repack -adq
git gc -q --prune=now >/dev/null 2>&1 || true
# `gc` can transiently expand the sparse index to a full index on some git
# versions; reapply --sparse-index so the COMMITTED on-disk index is sparse.
git sparse-checkout reapply --sparse-index

# Re-confirm the index is sparse after the repack/gc/reapply round-trip — the
# committed fixture's whole point is a sparse index, so fail the bake loudly
# if it is not.
if ! git ls-files --sparse | grep -q '/$'; then
    echo "error: index is NOT sparse after repack/gc; aborting bake" >&2
    exit 1
fi

# Sanity: the status we just baked must be non-empty (real work to report).
nstatus="$(git status --porcelain=v1 | wc -l | tr -d ' ')"
echo "[build-sparse-fixture] baked status reports $nstatus changed entries" >&2
if [ "$nstatus" -eq 0 ]; then
    echo "error: baked fixture has a clean tree; status would be a no-op" >&2
    exit 1
fi

# The fsmonitor config key would record this machine's daemon socket path;
# strip it so the restored fixture is portable + runs COLD by default
# (deterministic floor, per the task). The bench re-derives cold semantics
# anyway, but leaving a stale fsmonitor pointer in the committed config is
# noise — unset it.
git config --unset core.fsmonitor 2>/dev/null || true

mkdir -p "$OUT_DIR"
echo "[build-sparse-fixture] writing tar -> $OUT_TAR" >&2
# Uncompressed tar (no compression-crate dep on the restore side). `-C` so
# the archive root is the repo dir itself (restore = untar into a fresh dir).
# `COPYFILE_DISABLE=1` suppresses macOS AppleDouble `._*` resource-fork
# entries that bsdtar would otherwise embed — they would restore as spurious
# untracked files on every OS and inflate the baked status. We also pass
# `--no-mac-metadata` when the local tar supports it (belt-and-braces).
mac_meta_flag=""
if tar --help 2>&1 | grep -q -- '--no-mac-metadata'; then
    mac_meta_flag="--no-mac-metadata"
fi
COPYFILE_DISABLE=1 tar -cf "$OUT_TAR" $mac_meta_flag -C "$REPO" .

sz="$(wc -c <"$OUT_TAR" | tr -d ' ')"
echo "[build-sparse-fixture] done: $OUT_TAR (${sz} bytes)" >&2
