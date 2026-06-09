# shellcheck shell=bash
# Capability: workspace-workarea.
#
# Creates a workspace + workarea, then verifies the on-disk worktree layout
# (.context/ + a repo subdir whose .git is present).
#
# Requires (from core-boot + repo-clone):
#   SMOKE_CLIENT, SOCKET, CORE_DATA_DIR, REPO_ID.
# Exports (consumed by later checks):
#   WS_ID    the created workspace id.
#   WA_ID    the created workarea id.
#   WT_ROOT  the workarea root directory on disk.
check_workspace_workarea() {
    echo "Smoke gate v3: creating workspace / workarea..."
    WS_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" new-workspace --name "wsp" --repo-id "$REPO_ID")
    # WA_ID is consumed by later sourced checks (echo-session, perm, loop).
    # shellcheck disable=SC2034
    WA_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" new-workarea --workspace-id "$WS_ID")

    # Verify the workarea root on disk.
    # `WT_ROOT` is `<data_dir>/workspaces/<workspace.slug>/<composer-name>/`
    # per Task 20's locked layout. The composer name is server-allocated so
    # we glob for it via `find` (shellcheck SC2012 forbids `ls | head`).
    WT_ROOT=$(find "$CORE_DATA_DIR/workspaces/wsp" -maxdepth 1 -mindepth 1 -type d | head -n 1)
    if [ -z "$WT_ROOT" ]; then
        echo "FAIL workspace-workarea"
        fail "workarea root not found under $CORE_DATA_DIR/workspaces/wsp"
    fi
    if [ ! -d "$WT_ROOT/.context" ]; then
        echo "FAIL workspace-workarea"
        fail ".context/ missing in workarea root $WT_ROOT"
    fi
    # Workarea contains one repo subdir whose `.git` is present (single-repo
    # V0.1 layout per design/03 §4.2). `git worktree add` writes `.git` as a
    # regular file (containing `gitdir: <abspath>`), not a directory — `-e`
    # catches both forms.
    REPO_GIT_FOUND=0
    for repo_dir in "$WT_ROOT"/*/; do
        if [ -e "$repo_dir/.git" ]; then
            # Skip the `.context/` directory — it isn't a repo subdir.
            case "${repo_dir%/}" in
                *"/.context") continue ;;
            esac
            REPO_GIT_FOUND=1
        fi
    done
    if [ "$REPO_GIT_FOUND" -ne 1 ]; then
        echo "FAIL workspace-workarea"
        fail "repo .git missing in workarea root $WT_ROOT"
    fi

    echo "PASS workspace-workarea"
}
