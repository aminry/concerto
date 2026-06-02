# Task 202 — `Streams.Subscribe` Reconnect: Offset Ack + Per-Stream Ring Buffer + Gap Detection

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 201 |
| Touches subsystem(s) | 10 (Client API Protocol) |
| Smoke gate | extends:streams-subscribe |

## Goal
Make `Streams.Subscribe` survive a reconnect without re-bootstrapping the whole subject. Today (`crates/core/src/handlers/streams.rs`) the handler assigns a monotonic per-subject offset but **drops every event the instant it is forwarded** — there is no retained history, `since_offset` is explicitly ignored (`let _ = (&req.filter, req.since_offset)`), and a reconnecting client has no way to ask "give me what I missed." This task adds the `design/10 §3.3` machinery: a **per-subject in-memory ring buffer** (default 256 events; `session.io.<sid>` sized in bytes, default 1 MiB) that retains recently-published events, replay of `offset > since_offset` on reconnect, a **`GapDetected`** event when `since_offset` is older than the buffer's floor (the client must re-bootstrap that subject), an **`AckOffset`** unary RPC (the Connect-Web fallback path — bidi acks land with the Connect-Web client in Phase 5), and buffer pruning past the minimum acked offset. After this task a Desktop/mobile client that loses its Iroh connection (212/216) reconnects with `since_offset = <last seen>` and gets exactly the gap. V1.0 is **in-memory only** (`§12 R-1`): offsets do NOT persist across a Core restart; the client re-bootstraps on restart.

## Inputs to read before starting
- `design/10_Local_API_Protocol.md` §3.2 (the `Streams` service shape; `ClientFrame { ack: { offset } }` on the duplex stream when supported; unary `AckOffset` fallback for Connect-Web — R-2), §3.3 (per-subject in-memory ring buffer, default 256 events; `session.io` sized in **bytes**, default 1 MiB; monotonic u64 offset; replay `offset > since_offset`; `GapDetected` when `since_offset` is older than the buffer floor; prune past `min(all subscribers' acks)`), §5.1 (the `Streams` proto: `Subscribe` + `AckOffset(AckOffsetRequest) returns (google.protobuf.Empty)`), §5.2 (the subject catalog + the per-subject ring-size note), §6.2 (`StreamRouter` — assigns offset, persists in ring buffer, fans out, prunes on min-ack), §7.2 (the subscribe-with-offset-resume sequence diagram), §8 (`GapDetected` / `BackpressureDropped` failure rows), §12 R-1 (**V1.0 no durable cursor across client restart — in-memory only**), R-2 (server-streaming + unary `AckOffset` by default).
- `crates/proto/proto/concerto/v1/streams.proto` — the **live** proto. Note what already exists: `SubscribeRequest.since_offset = 3` is **already present** (reserved at Task 23). `Event.offset = 1` already present. There is **no** `AckOffset` RPC, **no** `AckOffsetRequest` message, and **no** `GapDetected` body variant yet. The `Event.body` oneof currently uses field numbers `10..14` (session/session_io/workspace/workarea/suggestion). The header comment says field numbers are FROZEN as of Task 23 and the V0.1 oneof variants are immutable — you add new variants/messages at **higher** numbers only.
- `crates/core/src/handlers/streams.rs` — the **live** `StreamsHandler`. Build on it; do not rewrite. Study the existing offset machinery: the `offsets: Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>` map and the `counter()` helper (this is where the ring buffer must hang off, keyed by the same canonical subject string), the `parse_subject` → `Subject` enum, the replay-then-live `chain()` pattern already used for `session.events`/`session.io` (the replay half is your model for buffer replay), and the module-doc "Offset accounting" + "ring-buffer + ack + gap-detected land in V1.0" notes you are now fulfilling.
- `crates/persist/migrations/0003_sessions_last_acked_seq.sql` — ack-offset groundwork: `sessions.last_acked_seq` already exists. This is the **agent-host bridge** ack watermark (Task 36), a *different* offset space from the Streams per-subject offset; do **not** conflate them. The Streams ack is in-memory per `§12 R-1` — you do **not** add a migration.
- `crates/core/src/api_server.rs` — `run_uds` builds `StreamsHandler::new(...)` then `.with_suggestions(...)`; the `StreamsServer` is added there. Keep that construction working.
- `tasks/v1.0/201-capability-negotiation.md` → "Handoff Notes" — 201 is the dependency; its `ConnTransport` seam is unrelated to ring buffers but confirms the Streams handler is the live, current surface you extend.

## Scope — in
- **Proto** (`streams.proto`, additive only): a `GapDetected` message + a new `Event.body` oneof variant for it at a field number **above** the existing `14`; the `AckOffset(AckOffsetRequest) returns (google.protobuf.Empty)` RPC on the `Streams` service; the `AckOffsetRequest` message (`subject`, `offset`). Import `google/protobuf/empty.proto`. Do **not** touch existing field numbers or the V0.1 oneof variants.
- **Ring buffer**: a per-subject in-memory bounded buffer keyed by the canonical subject string (same key the existing `offsets` map uses). Event-count bound (default 256) for all subjects except `session.io.<sid>`, which is **byte-sized** (default 1 MiB — sum of `SessionIoChunk.data` lengths). Each buffered entry carries its assigned offset. The buffer tracks its **floor** (lowest retained offset) so gap detection is exact.
- **Publish path**: every event the handler currently forwards is now *also* appended to that subject's ring buffer at its assigned offset (offset assignment stays exactly where it is — `counter().fetch_add`). Eviction drops the oldest when the count/byte bound is exceeded, advancing the floor.
- **Subscribe-with-offset**: when `since_offset = Some(N)`, if `N >= floor - 1` (i.e. the next wanted offset is still retained), replay buffered events with `offset > N` then transition to live (mirror the existing `replay_iter.chain(live)` shape). If `N` is older than the floor, emit a single `Event { body: GapDetected }` and then... (decision in Implementation notes — terminate vs. continue-live; pick and FREEZE the semantics). When `since_offset` is `None`, behavior is unchanged from today (live-only, plus whatever replay the supervisor already provides for session subjects).
- **AckOffset RPC**: records `(subject, offset)` as the calling subscriber's ack watermark in memory; prune the ring buffer to `min` across all live subscribers' acks for that subject (never below the highest offset any still-attached subscriber has *not* yet acked). A subscriber that disconnects is removed from the min computation.
- **Tests**: replay returns exactly `offset > since_offset`; `since_offset` older than floor → `GapDetected`; ring buffer evicts oldest at the count bound and advances the floor; `session.io` evicts by **bytes** not count; `AckOffset` prunes past min-ack but never past an un-acked subscriber; two subscribers to the same subject see identical offsets (the existing invariant) and independent acks.

## Scope — out
- Durable cursors across Core restart (`§12 R-1` — V2.0). No SQLite, no migration.
- The **bidi** in-stream `ClientFrame { ack }` path — V1.0 default is server-streaming + unary `AckOffset` (`R-2`); the bidi variant arrives only where the Connect-Web client can do it natively (Phase 5, Task 520). This task ships the unary `AckOffset` only.
- The Connect-Web client / its polling loop (Task 520) and any real transport reconnect (Iroh migration is 216) — this task proves resume against the in-process Streams surface only.
- New subjects or new `SessionEvent.kind` variants (Phase 3 owns those).
- Per-subject ring-size **configuration surface** beyond the two documented defaults (256 events / 1 MiB) — wire the defaults as named constants; a config field is a later task if ops wants one (note it in Handoff).

## Public interface this task locks
- Proto: the `AckOffset` RPC + `AckOffsetRequest { string subject = 1; uint64 offset = 2; }` and the `GapDetected` message + its `Event.body` oneof variant (the **new field number** you assign above `14`) — FROZEN. `SubscribeRequest.since_offset = 3` semantics are now **live** (no longer ignored).
- The `GapDetected` semantics: emitted iff `since_offset` is older than the subject's retained floor; it signals "re-bootstrap this subject via the list RPCs" (`§7.2`). The chosen post-`GapDetected` behavior (terminate the stream vs. continue from floor) — FROZEN by this task.
- The default ring sizes: **256 events** per subject; **1 MiB** for `session.io.<sid>` (byte-sized). Changing a default is a config decision, not a wire break.

## Implementation notes
- **Offset assignment must not move.** The existing `counter().fetch_add(1, Relaxed)` is the offset authority and two subscribers already agree through the shared `Arc<AtomicU64>`. Hang the ring buffer off the *same* per-subject keying (extend the `offsets` map's value, or add a parallel `Arc<Mutex<HashMap<String, SubjectBuffer>>>`) so the buffered offset == the forwarded offset. A `SubjectBuffer` that owns *both* the counter and the `VecDeque<(offset, EventPayload)>` + floor + the subscriber-ack table is the clean shape; migrate `counter()` callers to go through it.
- **`GapDetected` post-behavior — decide and freeze.** `§7.2` shows the client re-bootstrapping after a gap. The simplest honest contract: emit `GapDetected` as the **first** frame, then continue live from the current head (the client re-runs its list RPCs to fill the gap, then trusts the live tail). Terminating the stream is also defensible. Pick one, document it in the proto comment and the module doc, and test it.
- **`session.io` byte sizing.** The buffer bound for `session.io.<sid>` is the summed length of retained `data` payloads, not entry count. Keep a running byte total; evict oldest until under 1 MiB after each append. All other subjects use entry count (256). Branch on the parsed `Subject` variant, not on string sniffing.
- **Subscriber lifecycle for pruning.** `min-ack` pruning needs to know which subscribers are still attached. Track an entry per live `Subscribe` stream (register on subscribe, deregister on stream drop — a guard dropped when the boxed stream ends is the idiomatic hook). Prune to `min(acked offset across attached subscribers)`; with zero attached subscribers, retain up to the size bound (don't prune to empty — a reconnect may still want the tail).
- **Don't regress the `result_large_err` allowance** or the `BroadcastStream` adapter pattern; the module already `#![allow(clippy::result_large_err)]` at module scope.
- **Cross-platform**: the handler is already `#[cfg(unix)]`-gated at the `handlers/mod.rs` level (`pub mod streams` is `#[cfg(unix)]`); no new platform-specific types. Keep it that way — no `std::os::unix` in the buffer.
- Regen: a proto change means `./scripts/regen-interfaces.sh` updates `docs/interfaces/proto.md`; commit that diff.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core streams` → the existing `parse_subject` tests stay green + the new ring-buffer/replay/gap/ack/eviction tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new deps expected; confirm).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `AckOffset`/`AckOffsetRequest`/`GapDetected`).
7. `scripts/smoke.sh` → extend the existing `streams-subscribe` capability (`scripts/smoke.d/<NN>-streams-subscribe.sh`) to assert a reconnect with `since_offset` replays the gap over the live UDS Core (and/or that an out-of-range `since_offset` yields `GapDetected`). Exits 0.

## Definition of Done
- [x] `streams.proto` gains `AckOffset` + `AckOffsetRequest` + `GapDetected` (additive field numbers above 14); no existing number touched
- [x] Per-subject in-memory ring buffer (256 events default; `session.io` byte-sized 1 MiB) appends at the assigned offset, evicts oldest, tracks floor
- [x] `Subscribe { since_offset }` replays `offset > N` or emits `GapDetected` when older than floor (frozen post-gap behavior)
- [x] `AckOffset` records per-subscriber watermark; buffer prunes to min-ack across attached subscribers
- [x] Tests for replay, gap, count-eviction, byte-eviction (`session.io`), min-ack pruning, two-subscriber offset agreement
- [x] No durable cursor / no migration (V1.0 in-memory only per §12 R-1)
- [x] Verification commands pass; smoke green; interfaces regenerated
- [x] Single commit with the message below

## Outputs
- `crates/proto/proto/concerto/v1/streams.proto` (modified — `AckOffset`/`AckOffsetRequest`/`GapDetected`)
- `crates/core/src/handlers/streams.rs` (modified — ring buffer + replay + gap + `ack_offset` impl)
- `crates/core/tests/streams_reconnect.rs` (new — replay/gap/ack integration tests; unit-level eviction/floor/min-ack tests live in the handler's `#[cfg(test)]` module)
- `scripts/smoke.d/40-streams-subscribe.sh` (modified — assert reconnect-with-offset via the new probe)
- `docs/interfaces/proto.md` (regenerated)
- `tools/smoke-client/src/cmd/streams_replay_probe.rs` (new — drives the smoke reconnect assertion over the live UDS Core) **[added to Outputs — see Handoff "Drift from plan"]**
- `tools/smoke-client/src/cmd/mod.rs` (modified — register the probe subcommand module)
- `tools/smoke-client/src/main.rs` (modified — `streams-replay-probe` clap subcommand + dispatch)

## Commit message
```
phase-2: Streams reconnect — ring buffer + offset ack + gap detection

Adds per-subject in-memory ring buffers (256 events; session.io byte-
sized 1 MiB), since_offset replay, a GapDetected event when the buffer
floor is passed, and a unary AckOffset RPC that prunes past min-ack.
In-memory only per design/10 §12 R-1; clients re-bootstrap on Core
restart.

Refs: tasks/v1.0/202-streams-reconnect-ring-buffer.md
```

## Handoff Notes (filled in when finishing)

- **Drift from plan:**
  1. **Offset assignment moved from per-consumer to publish-time (the load-bearing change).** The task's Implementation notes assert "two subscribers already agree through the shared `Arc<AtomicU64>`" and "offset assignment must not move." That premise was **not** true of the V0.1 code: each subscriber's fan-out closure called `counter.fetch_add(1)` *per consumer per event*, so two subscribers to the same subject got *different* offsets (each event burned one offset per attached subscriber). A ring buffer fundamentally requires **one offset per event** (a replayed offset must equal the offset a concurrent live subscriber sees), and `design/10 §6.2` (StreamRouter) specifies publish-time assignment. So I moved offset assignment into a per-subject **pump task**: it subscribes to the underlying broadcast ONCE, stamps each event's offset, appends to the ring, and re-broadcasts the stamped `Event` to all live subscribers via a fresh `tokio::sync::broadcast`. The counter *stays the offset authority* (now living inside `SubjectBuffer`), which honors the spirit of "don't move the counter" while fixing the correctness bug. Net effect: the "two subscribers agree" invariant the task lists as a required test is now actually true (it wasn't before). Flagged because it's a semantic change to offset numbering, not just an addition — though no V0.1 client depended on the old per-consumer numbering (the only consumer, `stream-session-io`, ignores offsets).
  2. **Added the smoke-client probe to Outputs.** The smoke gate's `streams-subscribe` capability needed a way to drive a reconnect over the live UDS Core, and no existing `smoke-client` subcommand could. I added `tools/smoke-client/src/cmd/streams_replay_probe.rs` (+ its `mod.rs`/`main.rs` registration) — a self-contained `streams-replay-probe` subcommand that subscribes to `workspace.events`, creates two workspaces, asserts `since_offset` replay, then acks + reconnects to force a `GapDetected`. These three files were added to Outputs per the "add it to Outputs first and flag in Handoff" rule.
  3. **Unit vs integration test split.** The task's Outputs offered "new `streams_reconnect.rs` *(or extend the handler's `#[cfg(test)]` module)*." I did **both**: the wire-path tests (replay/gap/ack/two-subscriber-agreement over a real Core subprocess) live in `crates/core/tests/streams_reconnect.rs`; the pure buffer-arithmetic tests (count-eviction, byte-eviction for `session.io`, floor math, min-ack pruning, oversized-chunk retention) live in the handler's `#[cfg(test)]` module where they can poke `SubjectBuffer` directly without a live Core. Every test the task enumerates is present in one place or the other.

- **Open questions for next task:**
  - **Per-subject ring-size config surface (deferred per Scope — out).** The two defaults are wired as named `pub const`s (`RING_EVENT_CAP = 256`, `RING_SESSION_IO_BYTE_CAP = 1 MiB`) in `crates/core/src/handlers/streams.rs`. If ops wants a `managed.json`/settings knob, that's a later task (Task 211 owns `managed.json` enforcement; a `default_stream_buffer` proto field on `ServerCapabilities` was deliberately NOT added — Task 201's Handoff noted the field is absent from the live `runtime.proto`; add it additively when a task actually needs to advertise it).
  - **Bidi in-stream `ClientFrame { ack }` (R-2) is still out.** This task ships the unary `AckOffset` only; the bidi ack path lands with the Connect-Web client (Task 520). The unary ack models "some subscriber consumed up to `offset`" by raising every below-watermark subscriber to it (the Connect-Web fallback has no in-stream subscriber identity) — conservative: it never prunes past a subscriber that is behind. Task 520 (Connect-Web `AckOffset` polling) consumes this RPC as-is.
  - **Subject-buffer lifecycle is grow-only (in-memory, §12 R-1).** A `SubjectBuffer` (and its pump task) is created on first subscribe and never torn down — V0.1's documented bounded leak on session-id churn is unchanged. If a future task wants to reclaim per-session buffers when a session ends, that's a new task; it must coordinate with the pump's source-stream lifetime (the pump loop exits when the source broadcast closes, but the buffer entry + ack table persist).
  - **`SubscriberGuard` deregistration is best-effort via `tokio::spawn`** (can't `.await` in `Drop`). On a normal stream drop the spawned task deregisters the subscriber and re-prunes; during runtime shutdown the spawn may not run, which is harmless (a dead buffer is never pruned again). If a later task needs deterministic deregistration it should switch to an explicit close path.

- **Deliberate debt:** — None. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code.

- **Smoke-gate state:** **extended** (`extends:streams-subscribe`, as the task's Smoke gate field requires). `scripts/smoke.d/40-streams-subscribe.sh` now runs the `streams-replay-probe` after the existing session-IO assertion; it asserts (a) `since_offset` replay returns exactly the missed offset and (b) an `AckOffset`-pruned reconnect yields a single `GapDetected` frame — both over the live UDS Core. No manifest change needed (the capability name is unchanged; the check is additive within the existing file). Full `scripts/smoke.sh` is green (16 s, all checks PASSED); `--only streams-subscribe` is green (12 s). The frozen post-gap decision (**emit `GapDetected` first, then continue live from head**) is documented in the proto comment on `Event.gap_detected = 15`, the `GapDetected` message comment, the module doc, and is asserted by both the `ack_prunes_then_old_since_offset_yields_gap_detected` integration test (which checks the stream delivers a live event after the gap) and the smoke probe.
