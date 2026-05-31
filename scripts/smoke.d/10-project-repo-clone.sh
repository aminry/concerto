# shellcheck shell=bash
# Capability: project-repo-clone.
#
# Creates a seeded bare git repo, then a project, a repo pointing at it, and
# clones it through the Repo Manager.
#
# Requires (from core-boot):
#   CONCERTO_HOME, SMOKE_CLIENT, SOCKET.
# Exports (consumed by later checks):
#   BARE       path to the seeded bare repo.
#   PROJECT_ID the created project id.
#   REPO_ID    the created repo id.
check_project_repo_clone() {
    echo "Smoke gate v3: creating bare test repo..."
    BARE="$CONCERTO_HOME/bare-repo.git"
    mkdir -p "$BARE"
    git init --bare --quiet "$BARE"
    git -C "$BARE" symbolic-ref HEAD refs/heads/main

    # Push an initial commit via a temp clone so the bare repo has a real
    # default branch the `git clone` shell-out in the Repo Manager can find.
    TMP="$CONCERTO_HOME/seed"
    git clone --quiet "$BARE" "$TMP"
    echo "# smoke test" > "$TMP/README.md"
    git -C "$TMP" add -A
    git -C "$TMP" -c user.email=smoke@test -c user.name=Smoke commit -m "seed" --quiet
    git -C "$TMP" push --quiet origin main

    echo "Smoke gate v3: creating project / repo and cloning..."
    PROJECT_ID=$("${SMOKE_CLIENT[@]}" add-project --name "smoke")
    REPO_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" add-repo --project-id "$PROJECT_ID" --url "file://$BARE")
    if ! "${SMOKE_CLIENT[@]}" --socket "$SOCKET" clone --repo-id "$REPO_ID"; then
        echo "FAIL project-repo-clone"
        fail "clone"
    fi

    echo "PASS project-repo-clone"
}
