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
- [ ] `streams.proto` gains `AckOffset` + `AckOffsetRequest` + `GapDetected` (additive field numbers above 14); no existing number touched
- [ ] Per-subject in-memory ring buffer (256 events default; `session.io` byte-sized 1 MiB) appends at the assigned offset, evicts oldest, tracks floor
- [ ] `Subscribe { since_offset }` replays `offset > N` or emits `GapDetected` when older than floor (frozen post-gap behavior)
- [ ] `AckOffset` records per-subscriber watermark; buffer prunes to min-ack across attached subscribers
- [ ] Tests for replay, gap, count-eviction, byte-eviction (`session.io`), min-ack pruning, two-subscriber offset agreement
- [ ] No durable cursor / no migration (V1.0 in-memory only per §12 R-1)
- [ ] Verification commands pass; smoke green; interfaces regenerated
- [ ] Single commit with the message below

## Outputs
- `crates/proto/proto/concerto/v1/streams.proto` (modified — `AckOffset`/`AckOffsetRequest`/`GapDetected`)
- `crates/core/src/handlers/streams.rs` (modified — ring buffer + replay + gap + `ack_offset` impl)
- `crates/core/tests/streams_reconnect.rs` (new — replay/gap/ack/eviction integration tests) *(or extend the handler's `#[cfg(test)]` module)*
- `scripts/smoke.d/<NN>-streams-subscribe.sh` (modified — assert reconnect-with-offset)
- `docs/interfaces/proto.md` (regenerated)

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

## Handoff Notes (fill in when finishing)
- Drift from plan / Open questions for next task / Deliberate debt / Smoke-gate state
