# shellcheck shell=bash
# Capability: core-boot (mandatory prerequisite for every other check).
#
# Builds the binaries, boots concerto-core under a scratch HOME, waits for
# the UDS socket, and round-trips Runtime.GetServerCapabilities to confirm
# the advertised transport is UDS. Clean shutdown is the driver's job.
#
# Requires (set by the driver before this runs):
#   CONCERTO_HOME   scratch home for the whole run.
#   CI_MODE         0/1 (currently unused here; reserved).
# Exports (consumed by later checks):
#   CORE_CONFIG_DIR  <CONCERTO_HOME>/.concerto (holds core.sock / core.pid).
#   CORE_DATA_DIR    <CONCERTO_HOME>/concerto  (SQLite + worktrees + audit).
#   FAKE_HOME        scratch HOME passed to Core (skills/MCP fixtures land here).
#   CORE_LOG         path to the Core stdout+stderr log.
#   CORE_PID         pid of the running Core (driver kills it on shutdown).
#   SOCKET           <CORE_CONFIG_DIR>/core.sock.
#   SMOKE_CLIENT     argv array prefix for invoking the smoke-client.
#   CONCERTO_DATA_DIR exported so smoke-client subcommands hit the same DB.
check_core_boot() {
    CORE_CONFIG_DIR="$CONCERTO_HOME/.concerto"
    CORE_DATA_DIR="$CONCERTO_HOME/concerto"
    mkdir -p "$CORE_CONFIG_DIR" "$CORE_DATA_DIR"

    # `FAKE_HOME` is what we pass as `HOME` to the Core process. The skills
    # registry walks `<HOME>/.claude/skills/` at boot; the MCP surfacer
    # walks `<HOME>/.claude/mcp.json` on each request. Pointing Core at a
    # scratch home means the smoke gate can plant fixtures there without
    # touching the developer's real ~/.claude/.
    FAKE_HOME="$CONCERTO_HOME/fake-home"
    mkdir -p "$FAKE_HOME/.claude/skills"

    # Pre-build all the binaries the smoke gate exercises so `cargo run`
    # doesn't slip a compile step into the wall clock. `--quiet` keeps the
    # build noise out of smoke output; real errors still surface because
    # cargo writes them to stderr. `concerto-agent-host` is pre-built because
    # the supervisor (Task 22) spawns it for `agent-kind=echo` sessions and
    # resolves it through `current_exe().parent()`.
    echo "Smoke gate v3: building concerto-core, concerto-agent-host, smoke-client..."
    cargo build --quiet -p concerto-core -p concerto-agent-host -p concerto-smoke-client

    echo "Smoke gate v3: starting concerto-core in background..."
    CORE_LOG="$CONCERTO_HOME/core.log"
    # `HOME=$FAKE_HOME` redirects skills + MCP filesystem lookups. The Core
    # process still reads `CONCERTO_CONFIG_DIR` + `CONCERTO_DATA_DIR` for
    # its own state directories; HOME only governs the agent-config scans.
    #
    # `RUSTUP_HOME` / `CARGO_HOME` must be forwarded explicitly so the
    # rustup wrapper (`cargo` resolves through it) still finds its config —
    # otherwise it tries to re-download the toolchain under the fake HOME
    # and the Core fails to launch within the wait_for_file budget.
    # Resolve defaults the same way rustup does: `$RUSTUP_HOME` else
    # `~/.rustup`, `$CARGO_HOME` else `~/.cargo`. The smoke gate captures
    # the developer's real HOME via the parent process so we can fall
    # back to it for those defaults.
    REAL_HOME="$HOME"
    SMOKE_RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
    SMOKE_CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
    HOME="$FAKE_HOME" \
        RUSTUP_HOME="$SMOKE_RUSTUP_HOME" \
        CARGO_HOME="$SMOKE_CARGO_HOME" \
        CONCERTO_CONFIG_DIR="$CORE_CONFIG_DIR" CONCERTO_DATA_DIR="$CORE_DATA_DIR" \
        cargo run --quiet --bin concerto-core > "$CORE_LOG" 2>&1 &
    # CORE_PID is read by the driver's cleanup trap + shutdown, not here.
    # shellcheck disable=SC2034
    CORE_PID=$!

    # Wait for the UDS socket to appear. Cap at 15s — longer than any
    # reasonable cold start, short enough to fail CI fast when the core is
    # wedged.
    SOCKET="$CORE_CONFIG_DIR/core.sock"
    if ! wait_for_file "$SOCKET" 15; then
        echo "smoke: core log:" >&2
        sed 's/^/    /' "$CORE_LOG" >&2 || true
        echo "FAIL core-boot"
        fail "core.sock not created within 15s"
    fi
    echo "Smoke gate v3: Core ready (socket: $SOCKET)"

    # Convenience: every subsequent smoke-client invocation passes
    # `--socket "$SOCKET"`. The data-dir is also exported so the
    # `add-project` subcommand resolves the same SQLite path the Core uses.
    export CONCERTO_DATA_DIR="$CORE_DATA_DIR"
    SMOKE_CLIENT=(cargo run --quiet -p concerto-smoke-client --bin smoke-client --)

    # Call GetServerCapabilities and confirm the response advertises UDS.
    echo "Smoke gate v3: calling Runtime.GetServerCapabilities..."
    RESPONSE=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" caps)
    echo "Smoke gate v3: response: $RESPONSE"
    if ! echo "$RESPONSE" | grep -q '"transport_kind": *"TRANSPORT_KIND_UDS"'; then
        echo "smoke: core log:" >&2
        sed 's/^/    /' "$CORE_LOG" >&2 || true
        echo "FAIL core-boot"
        fail "unexpected smoke-client output (missing TRANSPORT_KIND_UDS)"
    fi

    echo "PASS core-boot"
}
