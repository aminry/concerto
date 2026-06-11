# Task 407 — Maestro side-channel tools: `notify_user` (typed stub against 14) + `propose_chip` (Maestro-owned slate) (consumes 401's FROZEN `concerto-maestro-mcp` schemas)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | 401 |
| Touches subsystem(s) | 08 (Maestro), 14 (Notifications — stub), 07 (Suggestion Engine — shape-mirror, not extended) |
| Smoke gate | unchanged |

## Goal
Fill the **two side-channel tool impls** behind 401's FROZEN `concerto-maestro-mcp` schema registry: `notify_user(text, severity)` and `propose_chip(chip)` (`design/08 §5.1`). Today the `concerto-maestro-mcp` server (Task 401, `crates/core/src/maestro/mcp.rs`) registers all 16 tools with their FROZEN input/output JSON schemas but every tool returns a typed `unimplemented` MCP error until its impl task lands (PHASE4_PLANNING §2, the 305 seam discipline); `notify_user`/`propose_chip` are the two side-channel slots of that registry, currently unimplemented. There is **no Notifications service** in the codebase — `design/14 §5` (`Notifications` gRPC + `NotificationHandle`) is owned by Phase-5 Task 501/507, so nothing for `notify_user` to dispatch into yet; and the V0.1 `SuggestionEngineHandle` (`crates/core/src/suggestions/actor.rs`) has **no `propose_chip`/`ChipRanker`/`next_step_chips`**, and its chips **evaporate after `DEDUP_TTL` (60 s)** (`suggestions/actor.rs:59`, `CHIP_RETENTION = DEDUP_TTL`) — the wrong home for a Maestro-proposed chip the user must still see minutes later. This task implements both as the file `crates/core/src/maestro/tools/side.rs` (PHASE4_PLANNING §2 sub-decision, 405/406/407 tool-file split): (1) `notify_user` is a **TYPED stub** — it records the notification intent (a `pub struct NotifyIntent { text, severity: NotifySeverity }`, `NotifySeverity = {Low, Medium, High}` mirroring `design/14 §4`'s `low|medium|high`) into a small in-process recorder and returns `Ok` (NOT `unimplemented!()`, NOT empty-silent-failure), to be wired to the live `NotificationHandle` by **Task 507** (the README **"`notify_user` (P4) stubs against 14 and is wired live in P5"** precedent, §6); (2) `propose_chip` adds a chip to a **Maestro-OWNED current slate** (`pub struct ChipSlate`, an `Arc<Mutex<Vec<MaestroChip>>>` held by the Maestro), **NOT** the volatile suggestion-engine buffer (PHASE4_PLANNING **D11**) — `MaestroChip` mirrors the `Chip` shape (`crates/core/src/suggestions/chip.rs`) but **does not extend the suggestion engine** and **survives the 60 s `DEDUP_TTL` window**. Both impls **consume** 401's FROZEN tool schemas (PHASE4_PLANNING §4.1) — this task never re-shapes a tool schema. After this task the two side-channel slots in the `concerto-maestro-mcp` registry return real results; Task **507** consumes `NotifyIntent`/`NotifySeverity` to dispatch `notify_user` live through the P5 `NotificationHandle`, and Task **409** (digest) attaches its persisted chips to the same Maestro-owned slate (D11). The live push delivery of `notify_user`, the gRPC surfacing of the slate, and digest-chip persistence stay out (Tier-3 / Tasks 507 / 414 / 409).

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §4.1 — **AUTHORITATIVE.** The 16 Maestro MCP tool schemas (incl. the 2 side-channels `notify_user`, `propose_chip`) are **FROZEN by Task 401**; 407 fills impls **behind** these frozen schemas and **never** re-shapes them. Read also §2 (the tool-file split: side-channels → `maestro/tools/side.rs`, lead-owned `tools/mod.rs` one-line registration) and **D11** (the `propose_chip` Maestro-owned-slate decision — the single load-bearing rule of this task).
- `tasks/v1.0/PHASE4_PLANNING.md` §1 D11 + §8.1 (407 write-set row) — **AUTHORITATIVE** decision this obeys: `propose_chip` adds to a Maestro-owned current slate (not the volatile suggestion buffer); 409's digest chips persist on the Maestro side; `propose_chip` **mirrors** the `Chip` shape but does not extend the suggestion engine. Write-set is `crates/core/src/maestro/tools/side.rs` (new) + `crates/core/src/maestro/tools/mod.rs` (one-line registration; lead-owned soft seam shared with 405/406).
- `design/08_Maestro_Agent.md` §5.1 — the side-channel tool inventory: `notify_user(text, severity) → goes through 14` and `propose_chip(chip) → adds to current slate`. Note "**16 tools total** … `propose_chip` surfaces as a confirmation chip for any user-visible side effect" (the strict-mode classification of `propose_chip` is **402's** `ToolClass` job, PHASE4_PLANNING D4 — 407 just produces the chip, it does not classify it).
- `design/08_Maestro_Agent.md` §9 + §6 — the Maestro→14 (`notify_user` flows here) and Maestro→07 (next-step chips for digest) dependency arrows; confirms 14 is the eventual `notify_user` sink and that the digest reuses the chip shape. (`propose_chip` example: `propose_chip("Compare TokenStore.ts")`, `design/08` §… mermaid line 500/522.)
- `crates/core/src/suggestions/chip.rs` — the `Chip { rule_id, workarea_id, title, priority:i32, created_at, action: ChipAction }` + `ChipAction { Compress, NewSession, OpenTestFailure, CommitAndPush, ReviewTool, Resume }` shape to **mirror** (not import-and-extend) for `MaestroChip`. `ChipAction::as_wire_str` is the wire-token convention.
- `crates/core/src/suggestions/actor.rs` — the **anti-pattern** this task deliberately avoids: `DEDUP_TTL = 60s` (line 59), `CHIP_RETENTION = DEDUP_TTL` (line 63), chips evaporate from the buffer; `record_outcome` is a V0.1 stub. The Maestro slate must **outlive** this window. Do NOT call into `SuggestionEngineHandle`.
- `design/14_Notifications_Push.md` §4 (the six notification kinds + severity column `low | medium | high`) + §5 (the eventual `NotificationKind`/`Notification` model + `Notifications` gRPC) — the shape `NotifySeverity` mirrors; confirms the live `NotificationHandle` does **not** exist until Phase-5 Task 501/507, so `notify_user` must be a typed stub here. **Do not** add a notifications table, proto, or service (those are 501/507).
- `crates/core/src/maestro/mcp.rs` + `crates/core/src/maestro/tools/mod.rs` (both authored by **Task 401**) — the FROZEN `concerto-maestro-mcp` `rmcp` server + the tool registry these impls plug into, and the lead-owned `tools/mod.rs` where 407 adds **one** `pub mod side;` + registration line. Read 401's Handoff Notes for the exact registry hook signature + the typed-`unimplemented`-MCP-error convention (the 305 seam discipline) the unfilled slots use.
- `tasks/v1.0/305-cone-stats-suggest-seam.md` → "Handoff Notes" — the seam discipline this task inherits: a not-yet-wired path returns a **typed error value** mapped to a runtime status (NEVER the `unimplemented!()`/`todo!()` macro, NEVER empty-success). `notify_user`'s stub is the inverse: it returns **`Ok`** with the intent recorded (a real result, debt documented → 507), not an error.
- `tasks/v1.0/401-*.md` → "Handoff Notes" — drift from the FROZEN side-channel schemas (the exact `notify_user`/`propose_chip` input/output JSON the registry froze); the maestro `mod.rs` soft-seam region; whether `rmcp` landed cleanly.
- *(No migration / no author-check needed.)* 407 adds **no** migration, no proto, no schema — the slate is an in-memory `Arc<Mutex<…>>`, the notify recorder is in-memory. Highest migration on `main` is `0014`; 407 does not touch `crates/persist/migrations/`. (If 403/410 already landed 0015/0016, it is irrelevant here — 407 is migration-free.)

## Scope — in
- **`crates/core/src/maestro/tools/side.rs` (new):**
  - **`notify_user` impl (typed stub against 14):**
    - `pub enum NotifySeverity { Low, Medium, High }` with `pub fn as_wire_str(&self) -> &'static str` returning `"low"|"medium"|"high"` (mirrors `design/14 §4`'s severity column; mirror, do not import a 14 type — none exists).
    - `pub struct NotifyIntent { pub text: String, pub severity: NotifySeverity, pub created_at_ms: i64 }` — the recorded intent Task 507 will dispatch.
    - A small **in-process recorder** `pub struct NotifyRecorder(Arc<Mutex<Vec<NotifyIntent>>>)` (or a sink trait `pub trait NotifySink: Send + Sync { fn record(&self, intent: NotifyIntent); }` with a `RecordingSink` default) so 507 can swap the recorder for a live `NotificationHandle`-backed sink **without changing this tool's body or the FROZEN MCP schema**. Provide `pub fn drain(&self) -> Vec<NotifyIntent>` (or `snapshot`) for the Tier-1 test + 507.
    - The tool body: parse the FROZEN `notify_user` MCP input (`{ text: string, severity: string }`, exactly as 401 froze it), build a `NotifyIntent`, `record` it, return the FROZEN `notify_user` success output (`Ok` — a real success, e.g. `{ recorded: true }` per 401's output schema). **Document the deliberate debt → Task 507** in a doc-comment: "wired live to `NotificationHandle` (14) in Phase-5 Task 507; until then records intent + returns ok (the README `notify_user`-stubbed-until-P5 precedent). NEVER `unimplemented!()`."
  - **`propose_chip` impl (Maestro-owned slate):**
    - `pub struct MaestroChip { pub title: String, pub priority: i32, pub action: String, pub workarea_id: Option<WorkareaId>, pub created_at_ms: i64 }` — **mirrors** `suggestions::chip::Chip` (title/priority/created_at/action) but: `action` is a free-form wire-token string (matching the V0.1 `Chip.action` wire convention, `suggestions/chip.rs` doc), `workarea_id` is `Option` (Maestro chips can be workspace-scoped / unscoped, e.g. the digest's "Compare TokenStore.ts"), and there is **no `rule_id`** (these are Maestro-proposed, not rule-emitted). Document: "mirror of `suggestions::chip::Chip`; deliberately NOT that type and NOT routed through the suggestion engine (D11) — its chips evaporate after `DEDUP_TTL`; the Maestro slate persists in-process across the window."
    - `pub struct ChipSlate { inner: Arc<Mutex<Vec<MaestroChip>>> }` (clone-cheap) with `pub fn propose(&self, chip: MaestroChip)`, `pub fn current(&self) -> Vec<MaestroChip>` (snapshot), and a `pub fn clear(&self)` (slate refresh). The slate is the **Maestro's** current chip set — held by the Maestro, surfaced by 414's gRPC later; 409 appends its digest chips here (D11).
    - The tool body: parse the FROZEN `propose_chip` MCP input (the chip fields, exactly as 401 froze them), build a `MaestroChip`, `slate.propose(..)`, return the FROZEN `propose_chip` success output (`Ok`). **No `DEDUP_TTL`, no eviction** — the slate is replaced on the next digest/turn, not time-expired.
  - **Wiring shape:** both tools take their backing handle (`NotifyRecorder`/`ChipSlate` or the sink trait) by injection from the `concerto-maestro-mcp` server state 401 set up, so the Maestro owns the slate + recorder. **No global statics.**
- **`crates/core/src/maestro/tools/mod.rs` (modified — ONE region):** add `pub mod side;` + the **one-line** registration that binds the two side-channel slots in 401's frozen registry to these impls (the lead-owned seam, PHASE4_PLANNING §2). Touch only the additive region; do not reorder 405/406's lines.
- Tests (Tier 1): (1) **`notify_user` records + returns ok** — invoke the tool with `{text:"build broke", severity:"high"}`, assert the `NotifyRecorder` `drain()` holds one `NotifyIntent { text:"build broke", severity: High, .. }` and the tool returned the FROZEN success (NOT an error, NOT a panic). (2) **`notify_user` severity round-trip** — `"low"|"medium"|"high"` parse to `Low|Medium|High` and `as_wire_str` back; an unknown severity maps to a documented default (`Medium`) **without** erroring. (3) **`propose_chip` adds to the slate** — invoke with a chip, assert `slate.current()` has it. (4) **slate survives the 60 s window** — propose a chip, assert it is still in `slate.current()` after a simulated `DEDUP_TTL`-equivalent (no time-based eviction exists; a unit assertion that `ChipSlate` has no TTL field + a propose-then-snapshot-after-`clear`-only test, contrasting the suggestion-engine `CHIP_RETENTION` behavior). (5) **multiple proposals accumulate** in slate order until `clear()`.

## Scope — out
- **Live notification delivery / `NotificationHandle` / Expo push / inbox / `notifications` table** — **Task 507** (Phase 5) wires `notify_user` into the real `Notifications` service (501/507); 407 leaves the `NotifyRecorder`/`NotifySink` seam so 507 swaps the sink with **zero** change to the FROZEN `notify_user` MCP schema or this tool body. (Tasks 501/502/503/504 build the model/inbox/push first.)
- **The `notify_user` notification model (kinds, dedup, multi-device fan-out, ID-only wakeup)** — **Tasks 501–506** (Phase 5). 407 records only `{text, severity}`; the mapping to a `NotificationKind` + chips + device fan-out is 507's job.
- **Surfacing the chip slate on the wire (gRPC) / `maestro.events`** — **Task 414** (the `Maestro` gRPC impl + `maestro.events` publishing). 407 holds the slate in-process; 414 reads `ChipSlate::current()` to surface chips to the Desktop. The proto/handle are FROZEN by **401.5** — 407 does not touch them.
- **Digest chip generation + persistence** — **Task 409** (digest). 409 appends its grouped next-step chips to **this** `ChipSlate` (D11 — "409's digest chips are persisted by the Maestro, attached to the digest's `chat_messages` row"), reusing `MaestroChip`/`ChipSlate::propose` — a pure consumer, no re-shape.
- **The strict-mode confirmation chip for `propose_chip`** — **Task 402** (the `ToolClass` / strict-`MustAsk` matrix, D4). `design/08 §5.1` lists `propose_chip` among the tools that surface a confirmation chip for a user-visible side effect; that classification + the `AwaitingApproval`/`ResolveApproval` flow is 402/406's job — 407 only produces the `MaestroChip`.
- **Extending the V0.1 Suggestion Engine** — explicitly forbidden (D11). Do NOT add `propose_chip` to `SuggestionEngineHandle`, do NOT route through its broadcast buffer, do NOT touch `crates/core/src/suggestions/*`.
- **Real-world Tier-3:** confirming a `notify_user` actually reaches a locked phone (and the digest chips render on the lock screen) is the **Phase-5** manual checklist's job ("receive a real push and approve a tool call from the lock screen"); 407 + 507 + the Phase-4 Maestro Tier-3 checklist ("judge digest quality … confirm budget-exhaust goes inert while routing still works") cover the chip-slate behavior, not real delivery.

## Public interface this task locks
> **407 owns the side-channel tool impls + their backing in-process types only. It CONSUMES (does not re-lock) the 2 side-channel MCP tool JSON schemas FROZEN by Task 401 (PHASE4_PLANNING §4.1) and the `concerto-maestro-mcp` registry hook 401 froze.** The `MaestroChip`/`NotifySeverity`/`NotifyIntent`/`ChipSlate`/`NotifyRecorder` Rust types below are this task's locked surface; 409 (slate append) + 414 (slate read on the wire) + 507 (notify sink swap) build on them.

**Rust (FROZEN, `design/08 §5.1` / PHASE4_PLANNING §4.1 + D11), `crates/core/src/maestro/tools/side.rs`:**
```rust
/// notify_user severity — mirrors design/14 §4's `low | medium | high`
/// severity column. NOT a `concerto-notifications` type (none exists until
/// Phase-5 Task 501/507); a Maestro-local mirror so the FROZEN MCP schema
/// is stable when 507 swaps in the live NotificationHandle.
pub enum NotifySeverity { Low, Medium, High }
impl NotifySeverity {
    pub fn as_wire_str(&self) -> &'static str;          // "low" | "medium" | "high"
    pub fn from_wire(s: &str) -> Self;                  // unknown ⇒ Medium (documented default; never errors)
}

/// One recorded notify_user intent. Task 507 dispatches these via the
/// live NotificationHandle (14); until then they are recorded + returned ok.
pub struct NotifyIntent {
    pub text: String,
    pub severity: NotifySeverity,
    pub created_at_ms: i64,
}

/// Pluggable sink for notify_user. The P4 default records in-process;
/// Task 507 supplies a NotificationHandle-backed sink (no MCP-schema change).
pub trait NotifySink: Send + Sync {
    fn record(&self, intent: NotifyIntent);
}
pub struct NotifyRecorder { /* Arc<Mutex<Vec<NotifyIntent>>> */ }
impl NotifyRecorder {
    pub fn new() -> Self;
    pub fn snapshot(&self) -> Vec<NotifyIntent>;        // for the Tier-1 test + 507 handoff
}
impl NotifySink for NotifyRecorder { fn record(&self, intent: NotifyIntent); }

/// A Maestro-proposed chip. MIRRORS `suggestions::chip::Chip` (title /
/// priority / created_at / action) but is NOT that type and does NOT route
/// through the suggestion engine (D11): the engine's chips evaporate after
/// DEDUP_TTL (60 s); the Maestro slate persists in-process across the window.
pub struct MaestroChip {
    pub title: String,
    pub priority: i32,                                  // higher wins; mirrors Chip.priority (1..=100)
    pub action: String,                                 // free-form wire token (mirrors V0.1 Chip.action)
    pub workarea_id: Option<WorkareaId>,                // Maestro chips may be workspace-scoped/unscoped
    pub created_at_ms: i64,
}

/// The Maestro-OWNED current chip slate (D11). Clone-cheap; held by the
/// Maestro, surfaced on the wire by Task 414, appended-to by Task 409's
/// digest. NO time-based eviction — replaced on the next digest/turn.
pub struct ChipSlate { /* Arc<Mutex<Vec<MaestroChip>>> */ }
impl ChipSlate {
    pub fn new() -> Self;
    pub fn propose(&self, chip: MaestroChip);
    pub fn current(&self) -> Vec<MaestroChip>;          // snapshot, slate order
    pub fn clear(&self);                                // slate refresh (NOT a TTL)
}
```
**The two MCP tool bodies** register against 401's FROZEN `notify_user` / `propose_chip` JSON schemas and return their FROZEN success outputs (`Ok`); they are registered via the **one-line** `tools/mod.rs` hook 401 froze. **Consumes the side-channel tool JSON schemas + the registry hook as frozen by Task 401 (PHASE4_PLANNING §4.1).** **Consumes the strict-mode `ToolClass` classification of `propose_chip` as frozen by Task 402 (PHASE4_PLANNING §4.8 / D4).**

## Implementation notes
- **The single load-bearing rule (D11): `propose_chip` writes the Maestro-owned `ChipSlate`, NEVER the suggestion engine.** The V0.1 `SuggestionEngineHandle` buffer is time-bounded (`DEDUP_TTL = 60 s`, `CHIP_RETENTION = DEDUP_TTL`, `suggestions/actor.rs:59/63`); a Maestro chip proposed during a digest must still be there when the user reads the digest minutes later. So the slate is a plain `Arc<Mutex<Vec<MaestroChip>>>` with **no TTL** — it is replaced wholesale (`clear()` + re-`propose`) on the next digest/turn, not aged out. Mirror the `Chip` field shape so the Desktop renderer (415) and 414's wire-mapping are trivial, but **do not** import or call `suggestions::*`.
- **`notify_user` is a typed stub that returns `Ok`, NOT a typed error and NOT the macro.** This inverts the usual seam discipline: most unwired Maestro tools return a typed `unimplemented` MCP error (the 305/401 convention); `notify_user` instead *succeeds* by recording the intent, because the README contract is "**stubs against 14 and is wired live in P5**" (§6) — the Maestro must believe its notification was accepted (so it doesn't retry/loop), while the actual delivery is deferred to 507. The `NotifySink` trait is the seam: 507 supplies a `NotificationHandle`-backed sink, the tool body and the FROZEN MCP schema are untouched. Document this deliberate debt → 507 in the doc-comment and the Handoff (it is **not** a `todo!()`).
- **Reuse-don't-reinvent:** `MaestroChip` mirrors `suggestions::chip::Chip`'s field names + the `as_wire_str` token convention so 414's proto-mapping (to the FROZEN `concerto.v1.Chip`-shaped message, `suggestions.proto` Chip = `rule_id/workarea_id/title/priority/created_at_ms/action`) reuses the existing chip wire vocabulary. Reuse `NotifySeverity`'s `low|medium|high` from `design/14 §4` verbatim (do not invent a 4th level).
- **Injection, not statics:** the `NotifyRecorder` and `ChipSlate` are owned by the Maestro and handed to the `concerto-maestro-mcp` server state (the place 401 stores per-server tool dependencies). Both are `Arc`-cheap-clone so 409 (slate append) and 414 (slate read) can hold their own handle to the **same** slate. No `lazy_static`/`OnceCell` global.
- **Cross-platform:** pure in-memory Rust (`Arc<Mutex<…>>`, integer ms timestamps) — no `#[cfg(unix)]` gate needed here (407 does not touch the agent supervisor / PTY / streams handlers; the `#[cfg(unix)]` gating lives on 402's spawn path + 414's handler, not on these tool bodies). Works on the Windows/Linux CI lanes (Task 113) unchanged.
- **No gRPC, no proto, no migration, no two-site registration:** 407 adds no service, so there is **no** `add_core_services`/`connect_bridge.rs` second-site work and **no** `regen-interfaces.sh` proto/SQL delta. The only generated surface that *could* move is `rust-api.md` IF the new `pub` structs land in a `crates/*/src/api.rs` — they do not (they live in `crates/core/src/maestro/tools/side.rs`, which `regen-interfaces.sh` does not scan, matching the 305 Handoff observation that `repo_manager/*`/free-fn surfaces are not captured). Run the regen check anyway to prove a no-op diff.
- **Regen:** no proto/SQL/`api.rs` change ⇒ `./scripts/regen-interfaces.sh` produces **no** diff; commit nothing under `docs/interfaces/`. (If it *does* diff, something leaked outside the write-set — stop.)
- **Parallel build hint:** **solo (small)** — single new file `side.rs` + one `tools/mod.rs` line; no disjoint fan-out sub-parts. The two impls (`notify_user`, `propose_chip`) are trivially co-located in one file and one commit; do not spawn helper sub-agents.

## Verification
**Tier 1.** The `rust` §5.3 fast-local set (CI re-runs the full matrix).
1. `cargo check --workspace` → clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` → clean (no `unimplemented!()`/`todo!()` anywhere in `side.rs`; the `notify_user` stub returns `Ok`).
3. `cargo fmt --all -- --check` → clean.
4. `cargo test -p concerto-core maestro::tools::side` (and/or the `side`/`notify`/`propose_chip`/`chip_slate` filter) → proves: (a) `notify_user` records one `NotifyIntent{text,severity:High}` and returns the FROZEN success (not error/panic); (b) severity `low|medium|high` round-trips and unknown ⇒ `Medium` without erroring; (c) `propose_chip` adds to `ChipSlate::current()`; (d) the slate **survives the 60 s window** — no TTL eviction (contrast `suggestions` `CHIP_RETENTION`); (e) multiple proposals accumulate in order until `clear()`.
5. `cargo test --workspace --no-fail-fast` → all pass (407 does not regress the 401 registry tests; the two side-channel slots now return real results instead of the typed `unimplemented` MCP error).
6. `cargo deny check` → green (407 adds **no** new crates — pure in-memory `std`/`tokio` `Mutex`).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → **no diff** (407 changes no proto/SQL/`api.rs`); commit nothing under `docs/interfaces/`.
8. `scripts/smoke.sh` → **unchanged** (407 turns on no smoke capability; the two tools are CI-provable in-process via direct tool invocation + slate/recorder snapshots — no `scripts/smoke.d/*` or `scripts/smoke.manifest` change).

**Tier-1 scope + what it does NOT cover.** Both side-channel impls are deterministic, in-process, and fully CI-provable: `notify_user` records intent + returns ok (the typed P5 stub), and `propose_chip` adds to the Maestro-owned slate that **outlives** the suggestion engine's 60 s `DEDUP_TTL`. CI does **not** cover: (1) real notification **delivery** to a device — `notify_user`'s live dispatch is **Task 507** (Phase 5), proven on the **Phase-5** Tier-3 checklist ("receive a real push and approve a tool call from the lock screen"); (2) the chips actually **rendering** on the Desktop — that is **Task 415** against 414's live wire. **No Tier-3 line is added by 407 itself** (it defers to the existing Phase-5 push-delivery line + the Phase-4 Maestro digest/chip checklist line); the impl is complete and CI-green for what it owns.

## Definition of Done
- [x] `crates/core/src/maestro/tools/side.rs` implements `notify_user` (typed stub: records a `NotifyIntent` via `NotifySink`/`NotifyRecorder`, returns the FROZEN `Ok` success — NOT `unimplemented!()`, NOT empty-silent-failure) and `propose_chip` (adds a `MaestroChip` to the Maestro-owned `ChipSlate`)
- [x] `NotifySeverity{Low,Medium,High}` mirrors `design/14 §4`'s `low|medium|high`; `from_wire` defaults unknown ⇒ `Medium` without erroring
- [x] `propose_chip` writes the Maestro-owned `ChipSlate` (no TTL), **NOT** the V0.1 suggestion-engine buffer (D11); `MaestroChip` mirrors `Chip` but does not import/extend `suggestions::*`
- [x] `ChipSlate` slate **survives** the 60 s `DEDUP_TTL` window (no time-based eviction; replaced via `clear()`); proven by a test contrasting `suggestions` `CHIP_RETENTION`
- [x] Both impls **consume** 401's FROZEN side-channel MCP schemas + registry hook (PHASE4_PLANNING §4.1) — schemas NOT re-shaped; `tools/mod.rs` gains exactly one `pub mod side;` + registration region
- [x] `NotifySink` seam left so Task 507 swaps in a `NotificationHandle`-backed sink with no MCP-schema / tool-body change
- [x] Tests (Tier 1): notify-records-and-ok, severity round-trip + unknown-default, propose-adds-to-slate, slate-survives-window, proposals-accumulate
- [x] All Verification commands pass on a clean checkout; smoke unchanged; `cargo deny` green (no new crates)
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (the `notify_user` stub returns a real `Ok` with the intent recorded — deliberate P5 debt documented in Handoff → Task 507; signature-frozen behavior, not the macro)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any schema/contract changed (407 changes none — regen is a verified no-op, nothing committed under `docs/interfaces/`)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/tools/side.rs` (new — `notify_user` typed-stub impl + `NotifySeverity`/`NotifyIntent`/`NotifySink`/`NotifyRecorder`; `propose_chip` impl + `MaestroChip`/`ChipSlate`; the two MCP tool bodies behind 401's FROZEN schemas; the Tier-1 tests)
- `crates/core/src/maestro/tools/mod.rs` (modified — one additive region: `pub mod side;` + the one-line registration binding the two side-channel slots in 401's frozen registry to these impls; lead-owned soft seam shared with 405/406)

## Commit message
```
phase-4: Maestro side-channel tools — notify_user stub + propose_chip slate

Fills 401's two FROZEN side-channel MCP slots. notify_user records a
typed NotifyIntent{text,severity} via a swappable NotifySink and returns
ok — the README notify_user-stubbed-until-P5 contract; Task 507 wires the
live NotificationHandle with no schema change. propose_chip adds a
MaestroChip to a Maestro-owned ChipSlate (D11) that outlives the V0.1
suggestion engine's 60s DEDUP_TTL — not the volatile suggestion buffer;
Task 409's digest appends to the same slate, 414 surfaces it on the wire.
No proto/migration/new-crate; deferred real push delivery is the P5 Tier-3
checklist's job.

Refs: tasks/v1.0/407-maestro-side-channel-tools.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** — None of substance. 401's FROZEN schemas matched the assumed shapes exactly: `notify_user` input is `{ text: string, severity: string }` (both required), `propose_chip` input is `{ chip: object }` (required); **both side-channel output schemas are the empty object `{}`** (no `required` keys), so each tool returns `json!({})` as its FROZEN success. `MaestroChip.action` landed as the **free-form `String`** (not an enum) per spec; `workarea_id` is `Option<WorkareaId>`; no `rule_id`. The default `NotifySink` impl is `NotifyRecorder` (`snapshot()` provided, not `drain()` — non-clearing, which 507 can swap freely). Entry point is `pub fn dispatch_side(name, args, sink: &dyn NotifySink, slate: &ChipSlate, now_ms) -> Result<Value, McpError>` — mirrors 405's `dispatch_read` signature so 402/414's MCP-server wiring threads the Maestro's `NotifyRecorder` + `ChipSlate` handles in by **injection** (Arc-clone fields on the server state, no global statics). The handle-less `tools/mod.rs::dispatch` arm for `notify_user`/`propose_chip` is **intentionally left** returning 401's typed-unimplemented seam error (it has no handles to reach); `dispatch_side` is the live route, exactly as `read::dispatch_read` is for the read tools (the `mod.rs` `pub mod side;` line + its region comment document this). 401's registry tests (the 18-tool frozen set, the `dispatch`-returns-typed-unimplemented test) are unchanged and still green.
- **Open questions for next task:** — **Task 507** (Phase 5) consumes the FROZEN `NotifyIntent`/`NotifySeverity` + the `NotifySink` seam to dispatch `notify_user` live through the P5 `NotificationHandle` (supply a `NotificationHandle`-backed `NotifySink` in place of `NotifyRecorder`; `notify_user`'s body + the FROZEN MCP schema are untouched; `NotifyRecorder::snapshot()` is the in-process drain for tests/migration). **Task 409** (digest) appends its grouped next-step chips to **this** `ChipSlate` via `ChipSlate::propose(MaestroChip { .. })` (D11) and reuses `MaestroChip`; replace-on-new-digest is `clear()` + re-`propose`, never a TTL. **Task 414** reads `ChipSlate::current()` (snapshot, slate order) to surface chips on the `Maestro` gRPC / `maestro.events` wire (proto FROZEN by 401.5); `MaestroChip`'s fields mirror `concerto.v1.Chip` so the proto-mapping is `title/priority/action/workarea_id/created_at_ms` (no `rule_id`). **Task 402** classifies `propose_chip` under the strict-mode `ToolClass` matrix (D4) — 407 only produces the chip. (Note for 507/414: the slate has **no TTL** by design — D11.)
- **Deliberate debt:** — `notify_user` is a **typed stub against 14**: it records the intent + returns `Ok({})` (so the Maestro believes the notification was accepted and does not retry/loop), but performs **no real delivery** until Task 507 wires the live `NotificationHandle`. This is the README "`notify_user` (P4) stubs against 14 and is wired live in P5" precedent — it is **not** a `todo!()`/`unimplemented!()` macro and not an empty-silent-failure; it is a complete, tested, signature-frozen seam (`NotifySink`). No other debt. (The only `unimplemented!()`/`todo!()` text in `side.rs` is in doc-comments stating what the code is **NOT** — no macro is invoked; verified by grep.)
- **Smoke-gate state:** — **Unchanged** (expected). 407 turns on no smoke capability; the two side-channel tools are CI-provable in-process (direct tool invocation + `NotifyRecorder::snapshot()` / `ChipSlate::current()` assertions — 11 unit tests, all green). No `scripts/smoke.d/*` or `scripts/smoke.manifest` change. No new crates ⇒ `cargo deny` green; no proto/SQL/`api.rs` ⇒ `regen-interfaces.sh` is a verified no-op (nothing committed under `docs/interfaces/`).
