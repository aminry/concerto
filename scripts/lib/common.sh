# shellcheck shell=bash
# Concerto smoke-gate shared helpers.
#
# Sourced by scripts/smoke.sh (and any future smoke-extension scripts).
# Functions are kept bash 3.2-compatible because macOS ships old bash.
# Do NOT use zsh-specific syntax (no associative arrays, no =~ side effects
# that depend on bash 4+).

# fail <message...>
#   Print to stderr with a "smoke:" prefix and exit non-zero.
fail() {
    printf 'smoke: %s\n' "$*" >&2
    exit 1
}

# pid_alive <pid>
#   Return 0 if the process exists, non-zero otherwise. Uses kill -0 so it
#   works without sending a real signal.
pid_alive() {
    if [ -z "${1:-}" ]; then
        return 1
    fi
    kill -0 "$1" 2>/dev/null
}

# wait_for_port <port> <timeout-seconds>
#   Poll TCP 127.0.0.1:<port> until a connection succeeds or <timeout-seconds>
#   elapses. Returns 0 on success, non-zero on timeout.
wait_for_port() {
    if [ "$#" -lt 2 ]; then
        fail "wait_for_port: usage: wait_for_port <port> <timeout-seconds>"
    fi
    local port=$1
    local timeout=$2
    local deadline
    deadline=$(( $(date +%s) + timeout ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        if (exec 3<>/dev/tcp/127.0.0.1/"$port") 2>/dev/null; then
            exec 3<&- 3>&-
            return 0
        fi
        sleep 0.2
    done
    return 1
}

# wait_for_log <file> <regex> <timeout-seconds>
#   Wait for <regex> to appear (as extended regex via grep -E) in <file>, or
#   return non-zero after <timeout-seconds>. If <file> doesn't exist yet,
#   poll until it does or the deadline passes.
wait_for_log() {
    if [ "$#" -lt 3 ]; then
        fail "wait_for_log: usage: wait_for_log <file> <regex> <timeout-seconds>"
    fi
    local file=$1
    local regex=$2
    local timeout=$3
    local deadline
    deadline=$(( $(date +%s) + timeout ))
    while [ "$(date +%s)" -lt "$deadline" ]; do
        if [ -f "$file" ] && grep -qE "$regex" "$file"; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}
