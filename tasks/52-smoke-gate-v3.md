# Task 52 — Smoke Gate v3 (Full V0.1 Coverage)

| Field | Value |
|---|---|
| Phase | 4 |
| Size | medium (1–3d) |
| Depends on | 27, 33, 38, 42, 43, 44, 45 |
| Touches subsystem(s) | all |
| Smoke gate | v3 |

## Goal
Extend the smoke gate to exercise every V0.1 feature in one CI-runnable script: Phase 1 (Core boot), Phase 2 (workspace/workarea/agent), Phase 3 (permission modes, tool approval, destructive intercept, audit log, scheduler /loop, skills discovery, VCS gh CLI). After this task, a green smoke gate means V0.1 actually works end-to-end.

## Inputs to read before starting
- `tasks/15-smoke-gate-v1.md`, `tasks/27-smoke-gate-v2.md` (current state).
- Every Phase 3 task's "Smoke-gate state" handoff note.

## Scope — in
- Extend `tools/smoke-client/` with subcommands for the V0.1 features not already covered:
  - `set-perm-mode --workarea <id> --mode auto`
  - `set-perm-mode --workarea <id> --mode yolo --ack "I understand"`
  - `resolve-approval --approval-id <id> --decision approve`
  - `create-loop --workarea <id> --interval 30 --prompt "..."`
  - `list-loops --workarea <id>`
  - `list-skills --scope personal`
  - `list-mcp --scope personal`
  - `list-audit --since <timestamp>` (reads the JSONL file directly)
- Extend `scripts/smoke.sh` Phase 3 block:
  ```sh
  echo "Smoke gate v3: testing permission modes..."
  cargo run --quiet --bin smoke-client -- set-perm-mode --workarea "$WA_ID" --mode auto
  # ... verify via list/get
  
  echo "Smoke gate v3: testing destructive intercept..."
  # Fake an agent tool-call with rm -rf args; verify the approval row exists
  # (or use a Rust test injected via an internal Tauri-style command)
  
  echo "Smoke gate v3: testing audit log..."
  grep -q '"kind":"WorkspaceCreated"' "$CORE_DATA_DIR/audit/audit-$(date -u +%F).jsonl" || fail "audit log missing WorkspaceCreated"
  
  echo "Smoke gate v3: testing /loop..."
  LID=$(cargo run --quiet --bin smoke-client -- create-loop --workarea "$WA_ID" --interval 30 --prompt "tick")
  # Wait 35s; verify schedule_runs row appears
  
  echo "Smoke gate v3: testing skills discovery..."
  mkdir -p "$CONCERTO_HOME/fake-claude-skills/test-skill"
  cat > "$CONCERTO_HOME/fake-claude-skills/test-skill/SKILL.md" <<EOF
  ---
  name: test-skill
  description: smoke
  ---
  Body.
  EOF
  HOME="$CONCERTO_HOME/fake-home" cargo run --quiet --bin smoke-client -- list-skills --scope personal | grep test-skill || fail "skill not discovered"
  
  echo "Smoke gate v3: PASSED"
  ```
- Add `--ci-mode` flag to `smoke.sh` that skips network-dependent checks (gh CLI doesn't need real GitHub auth in CI — we test the wiring only, not the actual API).
- Update `.github/workflows/smoke.yml` to use `--ci-mode` and run the script on macOS in addition to Linux (V0.1 is macOS-primary).
- Add a final summary at the end: "V0.1 alpha — N seconds, all checks PASSED."

## Scope — out
- Tauri/Desktop GUI in the smoke gate (still manual — V1.0 with headless Tauri).
- Multi-machine / split-host coverage (V1.0).
- Mobile / Web coverage (V1.0).
- Real GitHub API integration tests (V1.0 — currently mocked-gh).

## Public interface this task locks
- Smoke gate version `v3` means: every V0.1 feature surface is exercised by `scripts/smoke.sh` in CI.
- Smoke client subcommand list is the canonical "what V0.1 ships." Future task: any new feature gets a smoke client subcommand + a smoke.sh check.

## Implementation notes
- Some V0.1 features are inconvenient to test purely via gRPC (destructive intercept requires faking a tool call; the agent supervisor's parser would need to detect a regex match). For these:
  - Either inject a fake `ParseEvent::AwaitingApproval` via a test-only gRPC method (V0.1 expose a `Sessions.InjectTestEvent` gated by a `--test-mode` build feature), OR
  - Skip these checks in `--ci-mode` and document the manual verification protocol in `dist/SMOKE.md`.
  - The test-event injection is cleaner — adopt that. Document that it's compiled out of release builds.
- Total runtime target: < 3 minutes on a CI runner.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo clippy --workspace -- -D warnings` → clean.
3. `scripts/smoke.sh --ci-mode` locally on macOS → exits 0, prints "Smoke gate v3: PASSED" within 3 minutes.
4. CI smoke workflow on Linux AND macOS runs green.
5. Force-failure check: temporarily disable audit log emission for `WorkspaceCreated`; rerun; verify the smoke fails clearly; revert.
6. `shellcheck scripts/smoke.sh` → clean.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → no unintended drift.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Every Phase 3 feature has a corresponding check.
- [ ] Smoke gate v3 green on macOS + Linux CI.
- [ ] Total runtime < 3 minutes.
- [ ] Force-failure check confirmed for at least 3 feature areas.
- [ ] No `TODO` / `FIXME` in scripts or smoke client.
- [ ] Single commit created.

## Outputs
- `tools/smoke-client/src/cmd/set_perm_mode.rs`, `resolve_approval.rs`, `loops.rs`, `skills.rs`, `mcp.rs`, `audit.rs` (new)
- `tools/smoke-client/src/main.rs` (modified — subcommand dispatch)
- `scripts/smoke.sh` (modified — Phase 3 block)
- `.github/workflows/smoke.yml` (modified — macOS matrix)
- `dist/SMOKE.md` (new — explains what each gate version covers)
- Optional: `crates/proto/proto/concerto/v1/sessions.proto` (modified — `InjectTestEvent` gated by feature flag)

## Commit message
```
phase-4: smoke gate v3 — full V0.1 coverage

Every V0.1 feature surface (permission modes, destructive intercept,
audit log, /loop, skills discovery, MCP listing) exercised by
scripts/smoke.sh in < 3 minutes. CI runs on macOS + Linux.

Refs: tasks/52-smoke-gate-v3.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** GUI / multi-machine / mobile coverage deferred to V1.0.
- **Smoke-gate state:** **v3 active.** Covers every V0.1 feature. V1.0 features (Maestro, mobile, web, transport, push) will require v4 when they land.
