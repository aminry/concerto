# Task 107 — Retrofit Embedded-Core Mode into the Design

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | doc |
| Verification tier | 3 |
| Size | small (≤4h) |
| Depends on | — |
| Touches subsystem(s) | 01 (Core Daemon Runtime), 15 (Desktop Client), 18 (Distribution) |
| Smoke gate | unchanged |

## Goal
The embedded-Core mode (Core running in-process inside the Desktop, behind the `embedded-core` feature) was added manually after V0.1 and exists in code but **not in the design**. Per decision V2 (`tasks/v1.0/README.md §4`), embedded-Core is a first-class shipped mode for the single-user local case. This task makes the design reflect reality: a new `design/19_Embedded_Core_Mode.md` documenting it, and an amendment to `design/15_Desktop_Client.md`'s launch decision tree so embedded is an explicit branch, not an undocumented divergence. This is the one `doc` task whose `Outputs` legitimately include `design/` files.

## Inputs to read before starting
- `apps/desktop/src-tauri/src/embedded.rs` (the actual implementation: `Mode` enum, `resolve_mode`, `start`, the `AlreadyRunning` fallback, the shared-Tokio-runtime tradeoff documented in its header).
- `apps/desktop/src-tauri/Cargo.toml` (the `embedded-core` feature + optional `concerto-core`/`tokio-util` deps).
- `apps/desktop/src-tauri/tauri.conf.json` + `tauri.embedded.conf.json` (the two build variants).
- `scripts/dev-embedded.sh`, `scripts/smoke-embedded.sh`, Makefile targets (`dev-embedded`, `dev-embedded-scratch`, `smoke-embedded`, `build-embedded`).
- `design/15_Desktop_Client.md` §3.10.2 (the current launch decision tree — active Core → promote local UDS → auto-spawn daemon → Connect-to-Core picker; embedded is missing).
- `design/01_Core_Daemon_Runtime.md` §1, §6.1 (process model, single-instance guard — the PID lock that embedded uses as its coexistence guard).
- `design/18_Distribution_and_Operations.md` §3.1 (OSS binary boundary — confirm the embedded variant is still all-MIT).

## Scope — in
- Write `design/19_Embedded_Core_Mode.md` covering: purpose (zero-daemon single-user install), the three modes (`EmbeddedReal` / `EmbeddedScratch` / `External`) and how `resolve_mode` picks among them (env `CONCERTO_EMBEDDED`, `CONCERTO_HOME`, `--external`/`--embedded-scratch`), the PID-lock coexistence guard + `AlreadyRunning`→dial-the-daemon fallback, the shared-runtime tradeoff and when a dedicated runtime would be warranted, the relationship to split-host/remote (embedded is local-only; remote always implies a reachable Core — embedded Core can still be paired *to* by other devices over Iroh), the packaging story (feature-flagged; lean build links no Core), and the testing story (`smoke-embedded.sh`).
- Amend `design/15_Desktop_Client.md`'s launch decision tree (§3.10.2) to add the embedded branch and how it interacts with the existing daemon/auto-spawn/picker branches. Keep the amendment surgical and clearly marked (e.g. a dated "V1.0 amendment" note) so the doc's voice stays consistent.
- Add a one-line cross-reference from `design/01 §1` and `design/00 §5.3` (process types) pointing at the new doc.
- Update `design/00_Architecture_Overview.md §10` phase table row for subsystem 01/15 to mention embedded-Core as a V1.0 shipped mode (surgical amendment note, not a rewrite).

## Scope — out
- Any code change (embedded.rs is already implemented; Task 106 handles the agent-host resolution part).
- Multi-user / remote-host embedded (that's not a thing — embedded is single-user local; note the boundary explicitly).

## Public interface this task locks
- Documentation only. `design/19_Embedded_Core_Mode.md` becomes the canonical description of the mode; future embedded-Core changes update it.

## Implementation notes
- Match the existing design docs' structure (§1 Purpose, §2 Phase scope, §3 Key decisions, …) so doc 19 reads like a peer of 01–18, not a bolt-on.
- Be honest about the documented tradeoff already in `embedded.rs` (Core shares Tauri's global Tokio runtime). State it as a known V1.0 decision with the upgrade path, not a defect.
- This is a Tier-3 task: its real verification is the operator reading the doc and confirming it matches the shipped behavior on their Mac (Phase-1 checklist).

## Verification
Tier 3 (human-read gate).
1. `design/19_Embedded_Core_Mode.md` exists and covers all `Scope — in` bullets.
2. `design/15` launch tree includes the embedded branch; `design/00 §10` and `design/01 §1` cross-reference it.
3. Markdown link-check passes (no broken intra-doc links): the operator/CI link-checker if configured; otherwise spot-check the new cross-references resolve.
4. Operator (Phase-1 checklist): read doc 19 against `make dev-embedded` / `make dev-embedded-scratch` / `--external` behavior and confirm it matches.

## Definition of Done
- [ ] `design/19_Embedded_Core_Mode.md` written, peer-structured with 01–18
- [ ] `design/15` launch decision tree amended with the embedded branch
- [ ] `design/00 §10` + `design/01 §1` cross-references added
- [ ] No code changes; only `design/` files touched
- [ ] Single commit created with the message below

## Outputs
- `design/19_Embedded_Core_Mode.md` (new)
- `design/15_Desktop_Client.md` (amended)
- `design/00_Architecture_Overview.md` (amended)
- `design/01_Core_Daemon_Runtime.md` (amended)

## Commit message
```
phase-1: document embedded-core mode in the design

Adds design/19_Embedded_Core_Mode.md and amends the desktop launch
decision tree (design/15) + the phase table (design/00) to make
embedded-Core a first-class, documented V1.0 mode rather than an
undocumented post-V0.1 divergence.

Refs: tasks/v1.0/107-retrofit-embedded-core-design.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
