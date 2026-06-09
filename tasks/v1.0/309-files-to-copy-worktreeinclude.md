# Task 309 — Files-to-Copy: Multi-Repo Reference Worktree + Full `.worktreeinclude` (copy / symlink / exclude) + Windows Fallback

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 306, 307 |
| Touches subsystem(s) | 03 (Workspace/Session Manager) |
| Smoke gate | unchanged |

## Goal
Make files-to-copy a real **multi-repo** feature with a complete `.worktreeinclude` grammar and cross-platform symlink fallbacks. The V0.1 resolver (`crates/core/src/workspace_manager/files_to_copy.rs`) already parses `.worktreeinclude` and applies `copy` / `symlink` / `exclude` rules with last-match-wins + path-escape rejection — but it hard-codes the **single repo's `local_path` as the reference worktree** (a documented V0.1 simplification), and its `symlink` mode `Error::Internal`s on non-Unix. This task fills the three V1.0 gaps: (1) **reference-worktree selection** = the **first repo by `workspace_repos.position`** (the column Task 306 added; `design/03 §3.10` "default: first listed repo") — copies/symlinks resolve sources against that reference worktree and apply into **each** repo's new worktree (a per-repo `.env` lands in that repo's worktree); (2) the **Windows symlink fallback** — directory junction for dirs, hardlink for files, copy-with-one-time-warning where the filesystem has no symlink support (`design/03 §3.10` table); (3) the **broken-symlink warning chip** surface (`design/03 §3.10` "symlink to `<path>` is broken"). It owns the `.worktreeinclude` parser end-to-end (`copy`/`symlink`/`exclude`, gitignore-negation semantics). No migration. After this task a multi-repo workarea correctly materializes shared assets across all its repos with Windows-safe fallbacks.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.10 (the authoritative spec: the three modes + the `.worktreeinclude` grammar + the schema-equivalent JSON + **"matched against the workspace's reference worktree (the user can designate one repo's main worktree as the reference, default: first listed repo)"** + **"On Windows, falls back to a directory junction for directories and a hardlink for files; on filesystems without symlink support, falls back to `copy` with a one-time per-workarea warning"** + last-match-wins / gitignore-exclude semantics + **symlink safety: relative paths, broken-link warning chip, never traverse outside the reference-repo root → reject at create**), §3.13 (precedence: a **checked-in `.worktreeinclude` wins over local-DB `files_to_copy_rules`**; per-field), §8 (the "Files-to-copy source missing" failure row: skip for soft-existence `.env*` patterns, error for explicit paths).
- `crates/core/src/workspace_manager/files_to_copy.rs` — the **whole module** (it is mostly built — extend, do NOT rewrite): the module header documenting the V0.1 simplifications (lines 8–15) you are now resolving; `WORKTREEINCLUDE_RELPATH` (= `.concerto/.worktreeinclude`); `Mode` (`Copy`/`Symlink`/`Exclude`); `Rule` + `parse()` (the FROZEN grammar — bare=copy, trailing `!`=symlink, leading `!`=exclude); `apply()` / `apply_rules()` (the gitignore-walker + last-match-wins + the `file_to_copy.escapes_project_root` source/dest canonicalization checks at lines ~316–336); `create_symlink` (`#[cfg(unix)]` real / `#[cfg(not(unix))]` `Error::Internal` stub at lines 432–446 — **this is the Windows gap**); `dest_for_repo` (line 451).
- `crates/core/src/workspace_manager/workarea.rs` — `create_workarea` (after Task 306, the per-repo loop): the files-to-copy call site (V0.1: `files_to_copy::apply(repo_local, repo_worktree)` per repo in `spawn_blocking`, lines ~283–298). This task changes the call to pass the **reference worktree** (first repo by position) as the source root + each repo's worktree as the dest, and threads the resolved rule set (checked-in file > local-DB rules).
- `crates/persist/src/workspaces.rs` — `list_repos` (position-ordered after Task 306): the first element is the reference repo. `workspaces.settings_json` carries the local-DB `files_to_copy_rules` array (the fallback when no checked-in `.worktreeinclude` exists; the JSON shape is in `design/03 §3.10`).
- `tasks/v1.0/306-multi-repo-workspaces.md` → "Handoff Notes" — the `workspace_repos.position` ordering contract (FROZEN: `list_repos` returns `(position, repository_id)`-ordered) that defines "first listed repo."
- `tasks/v1.0/307-parallel-workareas-fsm.md` → "Handoff Notes" — the `partial` create path (a files-to-copy failure on one repo should not silently corrupt the others; coordinate with how 307 marks per-repo failures, though files-to-copy failures are soft per `design/03 §8`).
- `tasks/v1.0/PHASE3_PLANNING.md` §2 row 309 ("reference worktree = **first repo by `workspace_repos.position`**; **no per-workspace designation field in V1.0**") + §3 (309 has **no migration row** — it uses `workspace_repos.position` from 306, `workspaces.settings_json`, and repo-local `.worktreeinclude`). Note: full three-layer settings precedence is Task 310; 309 reads the checked-in file and the local-DB `files_to_copy_rules` directly (310's resolver may not have landed — see Implementation notes).

## Scope — in
- **Reference-worktree selection** = the first repo by `workspace_repos.position` (from 306's `list_repos`). The `.worktreeinclude` is read from `<reference_repo.local_path>/.concerto/.worktreeinclude`; the local-DB `files_to_copy_rules` come from `workspaces.settings_json`. Source paths resolve against the reference worktree; **no per-workspace designation field** is added in V1.0 (the design allows one but `PHASE3_PLANNING` defers it).
- **Per-repo application across all workarea repos.** For each repo in the workarea, apply the resolved rule set into that repo's new worktree: a pattern that matches in the reference worktree is copied/symlinked into **every** repo's worktree at the matching relative path; if the same pattern also matches files native to a non-reference repo, that repo's own matching files are handled per repo (each repo's `.env` → that repo's worktree, `design/03 §3.10`). Define the exact source-resolution rule (reference-worktree-relative) and FREEZE it in the module doc.
- **Resolve checked-in vs local-DB rules** (`design/03 §3.13`): if `<reference_repo>/.concerto/.worktreeinclude` exists, its parsed rules are the rule set (it **wins** over local DB); else parse the local-DB `files_to_copy_rules` JSON from `workspaces.settings_json` into the same `Vec<Rule>`. Add a `parse_json_rules(&str) -> Result<Vec<Rule>>` companion to `parse()` for the JSON-array form (the schema-equivalent JSON in §3.10). 310's full per-field resolver supersedes this read later (note the seam).
- **Windows symlink fallback** (`create_symlink` `#[cfg(not(unix))]` arm): for directories, create a **junction** (`std::os::windows::fs::symlink_dir` / junction crate-free `mklink /J` semantics — prefer the std symlink first, fall back to junction); for files, `std::fs::hard_link`; if both fail (no symlink privilege / unsupported FS), fall back to **`copy` with a one-time per-workarea warning** recorded on `workarea.events`. The Unix arm is unchanged. Keep the relative-target + escape-rejection invariants on every platform.
- **Broken-symlink + fallback warnings** surface as workarea-event chips: "symlink to `<path>` is broken" (broken link, non-blocking) and "symlinks unsupported here — copied `<path>` instead" (Windows fallback-to-copy). Emit through the existing `WorkareaEvent` broadcast; the chip rendering is the client's (322/323).
- **Preserve all FROZEN invariants** from V0.1: `parse()` grammar, `Mode` set, last-match-wins, relative symlinks, `file_to_copy.escapes_project_root` rejection (symlinks/`..` that escape the project root → hard error at create), `.git/` skip, idempotent re-apply.
- Tests (Tier 1, fixture filesystem): multi-repo apply (a reference-worktree `.env` lands in all repos' worktrees; a non-reference repo's native match goes to that repo); checked-in `.worktreeinclude` overrides local-DB JSON rules; `parse_json_rules` round-trips the §3.10 JSON; escape-rejection still fires; broken-symlink tolerated with a warning event; the Windows fallback path is unit-tested on the Windows CI lane (junction/hardlink/copy-fallback) — gate the real-symlink test `#[cfg(unix)]` and the fallback test `#[cfg(windows)]`.

## Scope — out
- **The full three-layer settings precedence resolver** (managed.json > checked-in > local DB > defaults, per-field, `notify`-rs live reload, `WorkspaceSettingsResolved` audit, opt-out) — Task 310. This task reads the checked-in `.worktreeinclude` + local-DB `files_to_copy_rules` **directly** with a simple "checked-in wins" rule; 310 generalizes it. If 310 has landed first, consume its resolved `files_to_copy_rules` instead of reading raw — note which path you took in Handoff.
- **A per-workspace "designated reference repo" field** — V1.0 uses first-by-position only (`PHASE3_PLANNING §2`); no schema/setting added.
- **The multi-repo worktree-creation loop itself** — Task 306 (this task is called inside it).
- **`workareas.status` / FSM / `partial`** — Task 307 (a files-to-copy failure is **soft** per `design/03 §8` — it does not mark the workarea `partial`; only a `git worktree add` failure does).
- **Desktop rendering of the warning chips** — Tasks 322/323.
- **Real Windows junction/long-path behavior on a physical Windows box** — the CI Windows lane (Task 113) proves the fallback logic compiles + runs; the real-Windows-junction confidence item is a Phase-3 Tier-3 checklist note (the manual checklist already covers cross-platform worktree paths).

## Public interface this task locks
- **Reference-worktree rule (FROZEN):** the files-to-copy source root = the workarea's **first repo by `workspace_repos.position`** (`design/03 §3.10` default). Sources resolve relative to that reference worktree; rules apply into **every** repo's worktree. No per-workspace designation field in V1.0.
- **Precedence (FROZEN, `design/03 §3.13`):** a checked-in `<reference_repo>/.concerto/.worktreeinclude` **wins** over the local-DB `workspaces.settings_json.files_to_copy_rules`. (310 later makes this per-field across all layers; the "checked-in wins over local DB" ordering it locks is the same.)
- **`files_to_copy` module surface (FROZEN additions):** `parse_json_rules(&str) -> Result<Vec<Rule>>` (the §3.10 JSON-array form → the same `Vec<Rule>` as `parse()`); a multi-repo apply entrypoint `apply_for_repo(reference_root: &Path, repo_worktree: &Path, rules: &[Rule]) -> Result<ApplyReport>` where `ApplyReport` carries the applied count + the list of warnings (broken-symlink / fallback-to-copy). The V0.1 `parse` / `Mode` / `Rule` / last-match-wins / `file_to_copy.escapes_project_root` invariants are **unchanged** (carried forward, not re-locked).
- **Cross-platform `create_symlink` contract (FROZEN):** Unix = relative symlink (unchanged); Windows/non-Unix = junction (dir) / hardlink (file) / copy-with-warning (unsupported), never an `Error::Internal` for the symlink mode. Escape-rejection + relative-target invariants hold on every platform.

## Implementation notes
- **Extend, don't rewrite.** The parser, the gitignore walker, last-match-wins, and the escape checks are correct and FROZEN from Task 30 — your deltas are (a) source root = reference worktree, (b) per-repo dest loop, (c) the JSON-rules companion, (d) the Windows `create_symlink` arm, (e) the warning surface. Resist refactoring the working core.
- **Reference-relative source resolution.** Today `apply()` reads rules from + resolves sources against the **same** root (the single repo). Split those: rules + sources come from the **reference** worktree; the dest is **each** repo's worktree. The escape check stays anchored on the reference root for sources and the dest root for destinations (keep both `file_to_copy.escapes_project_root` checks, retargeted).
- **Windows fallback ordering.** Try `std::os::windows::fs::symlink_dir`/`symlink_file` first (works if the process has the privilege or Developer Mode is on); on `ERROR_PRIVILEGE_NOT_HELD`, fall back to junction (dir) / `hard_link` (file); on a cross-volume hardlink failure, fall back to `copy` + warning. Do not pull a heavy junction crate if `std` + a small `mklink /J`-equivalent suffices — but a tiny well-licensed crate is acceptable if it clears `cargo deny` (flag it). Keep the symlink **target relative** so the workarea stays movable (junctions are absolute on Windows — document that divergence in the module doc as a known Windows limitation).
- **Warnings are non-blocking.** A broken source symlink or an unsupported-FS fallback must **not** fail the workarea create (`design/03 §3.10` "does not block the workarea"). Collect them into `ApplyReport.warnings` and emit `WorkareaEvent`s; only an **escape** (`..`/symlink out of project root) is a hard `Error::Validation` at create.
- **Soft source-missing.** Per `design/03 §8`, a missing source for a soft-existence pattern (`.env*`) is skipped silently; a missing source for an explicit path is an error — preserve the existing tolerance and don't tighten it.
- **310 sequencing.** 310 (the resolver) has no upstream deps and is meant to land before 309 consumes it — but if it hasn't, 309 reads `workspaces.settings_json.files_to_copy_rules` + the checked-in file directly with the "checked-in wins" rule. Either way the **result** (a resolved `Vec<Rule>`) is identical; thread it so 310 can swap the read without touching the apply logic. Record which path you took.
- **Cross-platform CI.** The `#[cfg(unix)]` symlink test + the `#[cfg(windows)]` fallback test both run on Task 113's lanes. Use `std::path` throughout; the only platform-specific code is inside `create_symlink`'s arms.
- **No migration, no proto change.** Warnings ride the existing `WorkareaEvent` broadcast. Interfaces regen is a no-op unless a `pub` Rust signature changes (`apply_for_repo`/`parse_json_rules`/`ApplyReport` are `pub` → regen + commit `rust-api.md`).

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core files_to_copy` → multi-repo apply (reference `.env` → all repos; non-reference native match → its own repo), checked-in-wins-over-DB-JSON, `parse_json_rules` round-trip, escape rejection still fires, broken-symlink tolerated + warned, idempotent re-apply. `#[cfg(unix)]` real-symlink test + `#[cfg(windows)]` junction/hardlink/copy-fallback test.
4. `cargo test --workspace --no-fail-fast` → all pass on the Unix lane; the Windows CI lane (Task 113) runs the `#[cfg(windows)]` fallback test.
5. `cargo deny check` → green (verify any junction helper crate, if introduced, clears the license/advisory floor — else stay std-only).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit if `rust-api.md` changed (new `pub` `apply_for_repo`/`parse_json_rules`); no proto/schema change.
7. `scripts/smoke.sh` → **unchanged gate** (the V0.1 single-repo files-to-copy path through `workspace-workarea` stays green; the reference repo of a 1-repo workspace is that repo — identical behavior).

**Tier-1 scope.** Fully CI-provable on fixture filesystems across both CI lanes. The one **Tier-3 confidence** remainder — real Windows junction + long-path behavior on a physical Windows machine — is folded into the Phase-3 manual checklist's existing cross-platform-worktree line (`design/03 §10`); the fallback **logic** is gated here.

## Definition of Done
- [x] Reference worktree = first repo by `workspace_repos.position`; sources resolve there, rules apply into every repo's worktree (per-repo native matches handled per repo)
- [x] Checked-in `.worktreeinclude` wins over local-DB `files_to_copy_rules`; `parse_json_rules` parses the §3.10 JSON form into `Vec<Rule>`
- [x] Windows `create_symlink` fallback: junction (dir) / hardlink (file) / copy-with-warning (unsupported) — no `Error::Internal` for symlink mode on any platform
- [x] Broken-symlink + fallback-to-copy warnings emitted as `WorkareaEvent` chips (non-blocking); escape (`..`/out-of-root) still a hard error
- [x] All V0.1 FROZEN invariants preserved (parse grammar, Mode set, last-match-wins, escape rejection, `.git/` skip, idempotency)
- [x] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams — e.g. the 310 resolver swap — in Handoff)
- [x] No files outside Outputs modified (one expected addition: `crates/core/src/handlers/streams.rs` — the exhaustive `WorkareaEvent` match needs the new `FilesToCopyWarning` arm; see Drift)
- [x] Interfaces regenerated + committed if a `pub` Rust surface changed (`rust-api.md`) — regen ran; no diff (these are internal-module `pub` symbols, not tracked in `rust-api.md`)
- [x] Smoke gate green (unchanged)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/workspace_manager/files_to_copy.rs` (modified — reference-relative source root, `apply_for_repo`, `parse_json_rules`, `ApplyReport`, Windows `create_symlink` arm, warning surface; module header updated to drop the V0.1-simplification notes)
- `crates/core/src/workspace_manager/workarea.rs` (modified — call `apply_for_repo` with the reference worktree + each repo's worktree; emit warning events)
- `crates/core/Cargo.toml` / root `Cargo.toml` (modified ONLY if a small junction helper crate is introduced — prefer std; flag in Handoff if added)
- `crates/core/tests/*` (new/modified — multi-repo + precedence + JSON-rules + escape + warning + `#[cfg(windows)]` fallback tests)
- `docs/interfaces/rust-api.md` (regenerated, if a `pub` type changed)

## Commit message
```
phase-3: files-to-copy multi-repo reference worktree + .worktreeinclude

Resolves files-to-copy sources against the workarea's first repo by
workspace_repos.position (the reference worktree) and applies copy/
symlink/exclude rules into every repo's worktree. Adds the Windows
fallback (junction/hardlink/copy-with-warning) so symlink mode no longer
errors off-Unix, the parse_json_rules companion for local-DB rules
(checked-in .worktreeinclude wins), and broken-symlink warning chips.

Refs: tasks/v1.0/309-files-to-copy-worktreeinclude.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan: one file beyond `Outputs` — `crates/core/src/handlers/streams.rs` — the new `WorkareaEvent::FilesToCopyWarning` variant requires an arm in the exhaustive `map_workarea_event` match (maps to the opaque `kind` `files_to_copy_warning`, no proto change). 310 had **already landed**, so 309 consumes its resolver per 310's handoff — `WorkareaManager::resolve_files_to_copy_rules` reads the checked-in `<reference>/.concerto/.worktreeinclude` first (wins; `files_to_copy::parse`), and ONLY on its absence falls back to `WorkspaceSettingsResolver::files_to_copy_rules()` (which itself layers checked-in `workspace_settings.json` > local-DB `settings_json` > default). Note: 310's resolver tracks `workspace_settings.json`'s `files_to_copy_rules` field — NOT the repo-local `.worktreeinclude` file — so 309 still owns the `.worktreeinclude` read + the "checked-in file wins over DB" ordering (`design §3.13`). No raw `settings_json` read in 309. The Windows junction uses the `cmd /C mklink /J` builtin (no junction crate) to keep `cargo deny` std-only; junctions are absolute (documented Windows divergence from the Unix relative-symlink invariant).
- Open questions for next task: none. The new `WorkareaEvent::FilesToCopyWarning { id, repository_id, message }` rides `workarea.events` with the opaque `kind` string `files_to_copy_warning` (no proto change); Desktop chip rendering is 322/323. The real-Windows junction/long-path confidence item stays on the Phase-3 manual cross-platform-worktree checklist (`design §10`) — the fallback *logic* is gated here via the `#[cfg(windows)]` unit test on Task 113's lane.
- Deliberate debt: none in new code (no TODO/FIXME/unimplemented). The 310 swap is already done (310 landed). Path escape is now a HARD abort of the whole create (not a soft `partial`) per `design §3.10`; a broken symlink / Windows copy-fallback is a non-blocking warning, matching the prior soft posture for source-missing.
- Smoke-gate state: GREEN, unchanged (`scripts/smoke.sh` PASSED, 105s). Full gate clean: `cargo check --workspace`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace --no-fail-fast` (incl. 5 `files_to_copy` integration tests + 21 module unit tests), `cargo deny check`, `cargo fmt --all --check`, `regen-interfaces` (no diff).
