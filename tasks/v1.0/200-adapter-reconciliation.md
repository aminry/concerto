# Task 200 — Reconcile the Tonic-over-Iroh Adapter Decision (spike 102 → design)

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | doc |
| Verification tier | 3 (human-read gate) |
| Size | small (≤4h) |
| Depends on | — |
| Touches subsystem(s) | 00 (Architecture), 10 (Client API), 11 (Transport), 15 (Desktop) |
| Smoke gate | unchanged |

## Goal
Reconcile the canonical design's **`tonic-iroh-transport`** references with the resolved Phase-1 decision. The design (`00 §6.6`, restated in `10`, `11`, `15`, and the TechStack eval) names `tonic-iroh-transport` as the **locked** Tonic-over-Iroh adapter. **Spike 102 ruled against it** (`design/spikes/tonic-iroh-findings.md` §2): that crate forces `tonic 0.14`, which collides head-on with the workspace's `tonic 0.12` pin, so the spike hand-rolled a ~70-line Iroh-duplex adapter on `tonic 0.12` and recommends that for Task 212. This task amends the design **in place** (dated-amendment style, exactly as the Phase-1 reconciliations in commit history) so that *every Phase-2 sub-agent reads correct guidance* — without this, a fresh agent on 201/204/212/218 could re-add the `tonic-iroh-transport` dependency and reintroduce the exact `tonic 0.14` conflict the spike retired. It also lifts the spike's four adapter gotchas into `design/11` as notes Task 212 inherits.

This runs **first** in Phase 2 (no deps) so the docs are correct before any other P2 task reads them. It mirrors the V7 discipline: *the design is canonical; code-vs-design drift is an explicit task, never a silent edit.*

## Inputs to read before starting
- `design/spikes/tonic-iroh-findings.md` — the whole doc, especially §2 ("Version decision: hand-rolled adapter on production tonic 0.12 (resolved)"), §2.1–§2.4 (the four gotchas), and §7 ("Handoff to Task 212").
- `design/spikes/iroh-nat-findings.md` §7 (the pinned-version trio: `iroh 0.98.2`, `iroh-relay 0.98.0`, and that `tonic-iroh-transport 0.9.2` was the *original suggestion* now superseded).
- The current `tonic-iroh-transport` mentions to amend — find them all: `grep -rn "tonic-iroh-transport\|tonic_iroh" design/`. As of writing, the **normative** spots are: `00_Architecture_Overview.md` (§6.6 + the TechStack-decisions table row), `10_Local_API_Protocol.md` §6.3 (`serve_iroh`/`IrohListener`), `11_Remote_Transport_Relay.md` (the §3 header inherit, §1, §3.1, §5.2, and the §6.1 mermaid node), and `15_Desktop_Client.md` (the `IrohCoreClient` struct comment + the split-host transport bullet).

## Scope — in
- For **each normative reference** (`00`, `10 §6.3`, `11`, `15`): replace the claim that the adapter *is* `tonic-iroh-transport` with the resolved decision — **a hand-rolled `tonic 0.12` ↔ Iroh-bidi-stream duplex adapter** — and append a dated **`V1.0 amendment (2026-06-02) — hand-rolled tonic-0.12 adapter, per spike 102`** note giving the one-line rationale (0.14 vs 0.12 conflict; the spike proved the hand-roll works on the production stack with no schema/codegen change). Do **not** delete the original sentence's intent; annotate it the way the Phase-1 reconciliations did (in-place edit + dated amendment block/sentence).
- In `design/11` (the transport doc that Task 212 implements against), add a short subsection or note block capturing the **four adapter gotchas from spike 102 §2.1–§2.4** as design guidance Task 212 inherits: (1) inherent-vs-trait `poll_read`/`poll_write` shadowing on `iroh::endpoint::{Send,Recv}Stream` → use fully-qualified `AsyncRead`/`AsyncWrite` trait syntax; (2) **one gRPC connection == one Iroh bidi stream**, many bidi streams per Iroh `Connection` (the "QUIC stream pool for gRPC" shape); (3) **acceptor priming** — the client connector sends a zero-byte `flush()` so the server's `accept_bi()` wakes promptly; (4) lift Tonic's 4 MiB decode/encode ceiling explicitly (the spike used 64 MiB). Cite `spikes/tonic-iroh-findings.md §2`.
- In `design/Concerto_TechStack_Evaluation.md` (historical evaluation, not a spec): add **one** brief forward-pointer sentence noting the spike resolved the adapter to the hand-roll; leave the original evaluation prose intact as the rationale-of-record.
- Update the pinned-stack statement everywhere it appears to the validated trio: `iroh 0.98.2` / `iroh-relay 0.98.0` / `tonic 0.12.3` + `prost 0.13.5` (NOT `tonic-iroh-transport`).

## Scope — out
- Any code change (this is a `doc` task; the adapter is *built* in Task 212).
- Editing the spike findings docs themselves (they are frozen Phase-1 artifacts and already state the hand-roll correctly).
- Touching `design/12` (the Noise layer is unaffected by the adapter choice).
- Vendoring Iroh as a sub-crate (a separate, later concern noted in `11 §8`).

## Public interface this task locks
- None (documentation only). The amendment establishes the **canonical adapter decision** that Task 212's `Public interface this task locks` will reference.

## Implementation notes
- Match the **exact dated-amendment voice** of the merged Phase-1 reconciliations (see `git log` for the design/09 audit-trait and design/02 git-status amendments): an in-place correction plus a clearly-labeled `**V1.0 amendment (YYYY-MM-DD) — …**` line citing the spike. Today's date for the amendment is **2026-06-02**.
- The TechStack eval is a different genre (decision rationale, not normative spec) — a single pointer sentence is enough; do not rewrite its analysis.
- After editing, re-run the grep and confirm **no normative doc still asserts `tonic-iroh-transport` as the adapter without an adjacent amendment pointer**.

## Verification
Tier 3 (human-read / doc gate).
1. `grep -rn "tonic-iroh-transport" design/` → every remaining hit is either (a) inside a spike findings doc, (b) inside an amendment note that explicitly supersedes it, or (c) the single TechStack forward-pointer. No bare normative claim survives.
2. `markdownlint` / link-check if configured (`scripts/` — run if present); otherwise the operator reads the diff at the phase gate.
3. The four spike-102 gotchas appear in `design/11` as Task-212-facing notes citing `spikes/tonic-iroh-findings.md §2`.
4. Operator spot-check: `git diff` reads as a faithful reconciliation, not a rewrite.

## Definition of Done
- [ ] All normative `tonic-iroh-transport` references in `00`/`10`/`11`/`15` amended in place with a dated `V1.0 amendment (2026-06-02)` note + rationale
- [ ] The four adapter gotchas captured in `design/11` as Task-212 inherited notes
- [ ] TechStack eval carries one forward-pointer; its rationale prose left intact
- [ ] Pinned trio (`iroh 0.98.2` / `iroh-relay 0.98.0` / `tonic 0.12.3` + `prost 0.13.5`) stated; `tonic-iroh-transport 0.9.2` marked superseded
- [ ] `grep` verification clean; operator-readable diff
- [ ] Single commit with the message below

## Outputs
- `design/00_Architecture_Overview.md` (modified)
- `design/10_Local_API_Protocol.md` (modified)
- `design/11_Remote_Transport_Relay.md` (modified — amendments + the gotchas notes)
- `design/15_Desktop_Client.md` (modified)
- `design/Concerto_TechStack_Evaluation.md` (modified — one forward-pointer)

## Commit message
```
phase-2: reconcile Tonic-over-Iroh adapter decision (spike 102 → design)

Amends 00/10/11/15 in place (dated V1.0 amendment) to replace the
locked `tonic-iroh-transport` adapter with the hand-rolled tonic-0.12
Iroh-duplex adapter spike 102 proved (tonic-iroh-transport forces
tonic 0.14, conflicting with the workspace pin). Lifts the spike's four
adapter gotchas into design/11 as Task 212 inherited notes.

Refs: tasks/v1.0/200-adapter-reconciliation.md
```

## Handoff Notes (fill in when finishing)
- Drift from plan / Any normative ref that resisted clean amendment / Smoke-gate state (unchanged)
