# shellcheck shell=bash
# Capability: skills.
#
# Plants a fake SKILL.md under the scratch HOME and confirms skills discovery
# surfaces it.
#
# Requires (from earlier checks):
#   SMOKE_CLIENT, SOCKET, FAKE_HOME.
check_skills() {
    echo "Smoke gate v3: planting fake SKILL.md fixture..."
    mkdir -p "$FAKE_HOME/.claude/skills/test-skill"
    cat > "$FAKE_HOME/.claude/skills/test-skill/SKILL.md" <<'EOF'
---
name: test-skill
description: smoke gate v3 skill fixture
---
Body.
EOF
    SKILLS_OUT=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" list-skills --scope personal)
    if ! echo "$SKILLS_OUT" | grep -qx "test-skill"; then
        echo "FAIL skills"
        fail "list-skills missing test-skill; got: $SKILLS_OUT"
    fi

    echo "PASS skills"
}
