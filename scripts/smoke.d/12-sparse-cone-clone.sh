# shellcheck shell=bash
# Capability: sparse-cone-clone (Task 302).
#
# Proves the cone-mode-mandatory, sparse-index-always-on lifecycle wires
# end-to-end over the live UDS Core:
#   1. seed a bare repo with a known multi-dir tree (`a/`, `b/`, `c/`),
#   2. add it with clone_strategy=blobless + with_sparse=true (Task 301's
#      flags — the worktree lands empty for the cone-set to populate),
#   3. clone it through the Repo Manager,
#   4. create a workspace + workarea (so a `workarea_repos` row + worktree
#      exist — cones are per-(workarea, repo)),
#   5. SetCones the workarea's cone to `a/`,
#   6. assert `git sparse-checkout list` reports `a/` and the worktree
#      materializes ONLY in-cone paths (a/ present; b/ and c/ collapsed),
#      and that the sparse index is active.
#
# Self-contained: it seeds its OWN project/repo/workspace/workarea rather
# than reusing the shared chain, so it does not perturb later checks. It is
# registered after `project-repo-clone` only because it needs the Repo
# Manager + Workspace/Workarea services already exercised.
#
# The real 2M-file monorepo cone latency is Task 303's bench + the Phase-3
# Tier-3 line; this check proves the CAPABILITY wires, not the perf bar.
#
# Requires (from core-boot + project-repo-clone):
#   SMOKE_CLIENT, SOCKET, CONCERTO_HOME, CORE_DATA_DIR, CORE_LOG.
check_sparse_cone_clone() {
    echo "Smoke gate: sparse-cone-clone — seeding multi-dir bare repo..."

    # `git sparse-checkout` requires a reasonably modern git. SKIP cleanly
    # if the lane's git is too old to support `--sparse-index` (git < 2.27).
    if ! git sparse-checkout --help >/dev/null 2>&1; then
        echo "SKIP sparse-cone-clone (git lacks sparse-checkout subcommand)"
        return 0
    fi

    SC_BARE="$CONCERTO_HOME/sparse-cone-bare.git"
    mkdir -p "$SC_BARE"
    git init --bare --quiet "$SC_BARE"
    git -C "$SC_BARE" symbolic-ref HEAD refs/heads/main

    SC_SEED="$CONCERTO_HOME/sparse-cone-seed"
    git clone --quiet "$SC_BARE" "$SC_SEED"
    # A known multi-dir tree: a/ b/ c/ each with a file (so they exist as
    # directories in HEAD — cone paths must name directories present in the
    # tree). A top-level file too (always materialized in cone mode).
    echo "root" > "$SC_SEED/ROOT.md"
    mkdir -p "$SC_SEED/a" "$SC_SEED/b" "$SC_SEED/c"
    echo "in a" > "$SC_SEED/a/file_a.txt"
    echo "in b" > "$SC_SEED/b/file_b.txt"
    echo "in c" > "$SC_SEED/c/file_c.txt"
    git -C "$SC_SEED" add -A
    git -C "$SC_SEED" -c user.email=smoke@test -c user.name=Smoke commit -m "seed a/b/c" --quiet
    git -C "$SC_SEED" push --quiet origin main

    echo "Smoke gate: sparse-cone-clone — add (blobless+sparse) + clone..."
    SC_REPO_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" add-repo \
        --url "file://$SC_BARE" --name "sparse-cone-repo" \
        --clone-strategy blobless --with-sparse)
    if ! "${SMOKE_CLIENT[@]}" --socket "$SOCKET" clone --repo-id "$SC_REPO_ID"; then
        echo "FAIL sparse-cone-clone"
        fail "clone (blobless+sparse)"
    fi

    echo "Smoke gate: sparse-cone-clone — create workspace + workarea..."
    SC_WS_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" new-workspace \
        --name "sparse-cone-ws" --repo-id "$SC_REPO_ID")
    SC_WA_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" new-workarea --workspace-id "$SC_WS_ID")

    # Resolve the workarea's repo worktree: <data>/workspaces/<slug>/<composer>/sparse-cone-repo
    SC_WT_ROOT=$(find "$CORE_DATA_DIR/workspaces" -maxdepth 2 -mindepth 2 -type d -path "*sparse-cone-ws*" | head -n 1)
    if [ -z "$SC_WT_ROOT" ]; then
        echo "FAIL sparse-cone-clone"
        fail "workarea root not found under $CORE_DATA_DIR/workspaces (ws=$SC_WS_ID)"
    fi
    SC_REPO_WT="$SC_WT_ROOT/sparse-cone-repo"
    if [ ! -e "$SC_REPO_WT/.git" ]; then
        echo "FAIL sparse-cone-clone"
        fail "repo worktree .git missing at $SC_REPO_WT"
    fi

    echo "Smoke gate: sparse-cone-clone — SetCones a/ ..."
    SC_APPLIED=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" set-cones \
        --workarea "$SC_WA_ID" --repo "$SC_REPO_ID" --cone "a/")
    if [ "$SC_APPLIED" != "a/" ] && [ "$SC_APPLIED" != "a" ]; then
        echo "smoke: set-cones returned: '$SC_APPLIED'" >&2
        echo "FAIL sparse-cone-clone"
        fail "set-cones did not echo the applied cone (got '$SC_APPLIED')"
    fi

    # Assert the cone is exactly `a` (cone-mode normalizes a/ -> a).
    SC_LIST=$(git -C "$SC_REPO_WT" sparse-checkout list 2>/dev/null | tr -d '\r')
    if ! printf '%s\n' "$SC_LIST" | grep -qx "a"; then
        echo "smoke: sparse-checkout list:" >&2
        printf '%s\n' "$SC_LIST" | sed 's/^/    /' >&2
        echo "FAIL sparse-cone-clone"
        fail "sparse-checkout list does not report 'a' as the cone"
    fi
    # Only `a/` materialized: a/ present on disk, b/ and c/ collapsed (their
    # files must NOT be checked out).
    if [ ! -f "$SC_REPO_WT/a/file_a.txt" ]; then
        echo "FAIL sparse-cone-clone"
        fail "in-cone path a/file_a.txt was NOT materialized"
    fi
    if [ -f "$SC_REPO_WT/b/file_b.txt" ] || [ -f "$SC_REPO_WT/c/file_c.txt" ]; then
        echo "FAIL sparse-cone-clone"
        fail "out-of-cone paths b/ or c/ were materialized (cone not enforced)"
    fi

    # Assert the sparse index is active (`git ls-files --sparse` reports the
    # out-of-cone trees as collapsed directory entries `b/` / `c/`). This is
    # the --sparse-index lever Task 303 leans on.
    SC_SPARSE_FILES=$(git -C "$SC_REPO_WT" ls-files --sparse 2>/dev/null | tr -d '\r')
    if ! printf '%s\n' "$SC_SPARSE_FILES" | grep -qE '^(b/|c/)$'; then
        echo "smoke: ls-files --sparse output:" >&2
        printf '%s\n' "$SC_SPARSE_FILES" | sed 's/^/    /' >&2
        echo "FAIL sparse-cone-clone"
        fail "sparse index not active (no collapsed b/ or c/ directory entry)"
    fi

    echo "PASS sparse-cone-clone"
}
