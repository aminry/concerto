# shellcheck shell=bash
# Capability: cli.
#
# Read-only check (runs last, after every mutating check): builds and runs
# the shipped `concerto` CLI binary's `status` subcommand against the Core
# booted by 00-core-boot, asserting exit 0 and that it prints the version.
#
# Requires (from 00-core-boot):
#   SOCKET   path to the Core's UDS socket (concerto dials this via --socket).
check_cli() {
    echo "Smoke gate v3: building + running the concerto CLI (status)..."
    # Pre-build so `cargo run`'s compile step stays out of the assertion.
    cargo build --quiet -p concerto-cli --bin concerto

    # `concerto status` over the live Core's socket. Capture stdout so we can
    # assert it carries the version line; a non-zero exit trips `set -e` via
    # the command substitution failing the function.
    CLI_OUT=$(cargo run --quiet -p concerto-cli --bin concerto -- --socket "$SOCKET" status)
    if ! echo "$CLI_OUT" | grep -q '^version:'; then
        echo "FAIL cli"
        fail "concerto status missing version line; got: $CLI_OUT"
    fi

    echo "PASS cli"
}
