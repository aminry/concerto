# shellcheck shell=bash
# Capability: mcp.
#
# Plants a fake mcp.json under the scratch HOME and confirms MCP listing
# surfaces the configured server.
#
# Requires (from earlier checks):
#   SMOKE_CLIENT, SOCKET, FAKE_HOME.
check_mcp() {
    echo "Smoke gate v3: planting fake mcp.json fixture..."
    mkdir -p "$FAKE_HOME/.claude"
    cat > "$FAKE_HOME/.claude/mcp.json" <<'EOF'
{
  "mcpServers": {
    "test-mcp": {
      "command": "/bin/true",
      "args": [],
      "env": {}
    }
  }
}
EOF
    MCP_OUT=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" list-mcp --scope personal)
    if ! echo "$MCP_OUT" | grep -qx "test-mcp"; then
        echo "FAIL mcp"
        fail "list-mcp missing test-mcp; got: $MCP_OUT"
    fi

    echo "PASS mcp"
}
