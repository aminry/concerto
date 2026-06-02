//! gRPC `Streams` service handler (Task 23; reconnect machinery Task 202).
//!
//! V0.1 surface — one server-streaming RPC, `Subscribe`:
//!
//! - `session.events.<sid>` → forwards [`AgentEvent`] from
//!   [`AgentSupervisorHandle::subscribe_events`] mapped into the
//!   `Event { body: Session(SessionEvent { kind: … }) }` shape.
//! - `session.io.<sid>` → forwards [`SessionIoChunk`] from
//!   [`AgentSupervisorHandle::subscribe_session_io`] mapped into the
//!   `Event { body: SessionIo(SessionIoChunk) }` shape.
//! - `workspace.events` → forwards [`WorkspaceEvent`] from
//!   [`WorkspaceManager::subscribe`] into the
//!   `Event { body: Workspace(WorkspaceEvent) }` shape.
//! - `workarea.events` → forwards [`WorkareaEvent`] from
//!   [`WorkareaManager::subscribe`] into the
//!   `Event { body: Workarea(WorkareaEvent) }` shape.
//!
//! ## Reconnect machinery (Task 202, `design/10 §3.3`)
//!
//! V1.0 makes `since_offset` LIVE and adds a per-subject in-memory ring
//! buffer so a reconnecting client gets exactly the gap it missed
//! instead of re-bootstrapping the whole subject. The pieces:
//!
//! - **[`SubjectBuffer`]** — one per canonical subject string. Owns the
//!   monotonic offset counter (the offset *authority*), a bounded
//!   `VecDeque<Event>` ring, the retained `floor` (lowest retained
//!   offset), and the per-subscriber ack table. The ring is bounded by
//!   event COUNT (default [`RING_EVENT_CAP`] = 256) for every subject
//!   except `session.io.<sid>`, which is bounded by BYTES (default
//!   [`RING_SESSION_IO_BYTE_CAP`] = 1 MiB, summed `SessionIoChunk.data`
//!   lengths) because it carries far higher volume.
//!
//! - **Publish-time offset assignment.** A single per-subject *pump*
//!   task subscribes to the underlying broadcast ONCE, assigns each
//!   event its offset (`counter.fetch_add(1)`), appends it to the ring,
//!   and re-broadcasts the offset-stamped [`Event`] to all live
//!   subscribers via a `tokio::sync::broadcast` channel. Because the
//!   offset is assigned once per event (not once per subscriber, as the
//!   V0.1 fan-out did), **two subscribers to the same subject see
//!   identical offsets** — the invariant the ring buffer's replay relies
//!   on (a replayed offset must equal the offset a concurrent live
//!   subscriber sees).
//!
//! - **Subscribe-with-offset.** On `Subscribe { since_offset = Some(N) }`,
//!   if the next wanted offset `N + 1` is still retained
//!   (`N + 1 >= floor`), the handler replays the ring's events with
//!   `offset > N`, then chains the live re-broadcast — mirroring the
//!   existing `replay_iter.chain(live)` shape. If `N` is older than the
//!   floor, the handler emits a single `Event { gap_detected }` as the
//!   FIRST frame and then **continues live from the current head**
//!   (continue-live-from-head — FROZEN by Task 202; see [`GapDetected`]).
//!   The client re-runs its list RPCs to fill the gap, then trusts the
//!   live tail. `since_offset = None` is unchanged from V0.1
//!   (live-only).
//!
//! - **AckOffset + pruning.** The unary [`StreamsService::ack_offset`]
//!   RPC records `(subject, offset)` as the calling subscriber's
//!   watermark. The ring is pruned to `min(acked offset across all
//!   still-attached subscribers)` — never below an event a live
//!   subscriber has not yet acked. A subscriber is registered on
//!   subscribe and deregistered when its stream drops (a guard owned by
//!   the boxed stream). With zero attached subscribers the ring retains
//!   up to its size bound (a reconnect may still want the tail).
//!
//! In-memory only per `design/10 §12 R-1`: offsets and the ring do NOT
//! survive a Core restart; clients re-bootstrap on restart.
//!
//! ## Offset accounting (V0.1 → V1.0)
//!
//! V0.1 assigned offsets in the fan-out closure (`fetch_add` per
//! consumer), so two subscribers disagreed on numbering. Task 202 moves
//! assignment into the per-subject pump (publish time), which is both
//! correct for replay and the shape `design/10 §6.2` (StreamRouter)
//! describes. The counter remains the offset authority; it now lives
//! inside [`SubjectBuffer`]. The buffer map grows once per distinct
//! subject and is never cleared (in-memory only); V0.1's bounded leak on
//! session-id churn is unchanged.
//!
//! ## Subject parsing
//!
//! [`parse_subject`] returns the typed [`Subject`] enum. Unknown
//! subjects surface as `INVALID_ARGUMENT` with the wire-code
//! `streams.unknown_subject` so clients can distinguish a typo from a
//! valid-subject-but-no-such-id.

// Every stream item is `Result<Event, tonic::Status>`. `tonic::Status`
// is ~176 bytes; the per-RPC cost of carrying that variant on the heap
// would dwarf the wire-encoding overhead and the closures live inside
// the tonic-managed task graph already. Suppress the lint at module
// scope rather than annotate each of the four BroadcastStream-adapter
// closures.
#![allow(clippy::result_large_err)]

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_persist::SessionId as PersistSessionId;
use concerto_proto::v1::streams_server::Streams as StreamsService;
use concerto_proto::v1::{
    event::Body as EventBody, session_event::Kind as SessionEventKind, AckOffsetRequest,
    AgentExited, AgentMessage, AgentStarted, ApprovalResolved as ProtoApprovalResolved,
    AwaitingApproval as ProtoAwaitingApproval, CheckpointCreated as ProtoCheckpointCreated,
    Chip as ProtoChip, Event, GapDetected, SessionEvent as ProtoSessionEvent,
    SessionIoChunk as ProtoSessionIoChunk, SubscribeRequest, ToolCall as ProtoToolCall,
    TurnComplete as ProtoTurnComplete, WorkareaEvent as ProtoWorkareaEvent,
    WorkspaceEvent as ProtoWorkspaceEvent,
};
use futures::Stream;
use tokio::sync::{broadcast, Mutex};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};

use crate::agent_supervisor::{AgentEvent, AgentSupervisorHandle, SessionIoChunk};
use crate::suggestions::{Chip, SuggestionEngineHandle};
use crate::workspace_manager::{WorkareaEvent, WorkareaManager, WorkspaceEvent, WorkspaceManager};

/// Default ring-buffer bound for every subject EXCEPT `session.io.<sid>`:
/// the number of most-recent events retained for replay (`design/10
/// §3.3`). Changing this is a config decision, not a wire break.
pub const RING_EVENT_CAP: usize = 256;

/// Default ring-buffer bound for `session.io.<sid>`: the summed length
/// in bytes of the retained `SessionIoChunk.data` payloads (`design/10
/// §3.3` — this subject is byte-sized, not count-sized, because it
/// carries up to ~1 MB/s). 1 MiB.
pub const RING_SESSION_IO_BYTE_CAP: usize = 1024 * 1024;

/// Re-broadcast channel capacity for the per-subject pump → live
/// subscribers. Independent of the ring buffer's retention bound: this
/// only bounds how far a *slow live subscriber* may lag before its
/// `BroadcastStream` reports `Lagged` (mapped to end-of-stream here, as
/// V0.1 already did for the source channels). Sized generously relative
/// to the event-count ring so a healthy subscriber never lags.
const LIVE_BROADCAST_CAP: usize = 1024;

/// Parsed subject — V0.1 catalog only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    SessionEvents(PersistSessionId),
    SessionIo(PersistSessionId),
    WorkspaceEvents,
    WorkareaEvents,
    /// Task 40 — `suggestion.events`. Optional `workarea_id` filter
    /// in the trailing segment (`suggestion.events.<workarea_id>`);
    /// `None` means "every workarea".
    SuggestionEvents(Option<String>),
}

/// How a [`SubjectBuffer`] bounds its ring: by event count (most
/// subjects) or by summed payload bytes (`session.io.<sid>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RingBound {
    /// Retain at most `n` events.
    Count(usize),
    /// Retain events whose summed `SessionIoChunk.data` length is at
    /// most `n` bytes.
    Bytes(usize),
}

impl RingBound {
    /// Pick the bound for a subject. Branch on the parsed [`Subject`]
    /// variant (NOT on string sniffing) per the task's implementation
    /// notes.
    fn for_subject(subject: &Subject) -> Self {
        match subject {
            Subject::SessionIo(_) => RingBound::Bytes(RING_SESSION_IO_BYTE_CAP),
            _ => RingBound::Count(RING_EVENT_CAP),
        }
    }
}

/// One retained event plus the byte weight it contributes to a
/// byte-bounded ring (0 for count-bounded subjects, where weight is
/// irrelevant).
#[derive(Clone)]
struct RingEntry {
    event: Event,
    bytes: usize,
}

/// Per-subject in-memory state: the offset authority, the bounded ring
/// buffer, the retained floor, the per-subscriber ack table, and the
/// live re-broadcast channel. One per canonical subject string; created
/// lazily on first subscribe and never torn down (in-memory only,
/// `design/10 §12 R-1`).
struct SubjectBuffer {
    /// Monotonic per-subject offset authority. Each published event
    /// picks up `fetch_add(1)`; offset assignment lives here so all
    /// subscribers agree (it is assigned once per event by the pump,
    /// not once per consumer).
    counter: AtomicU64,
    /// How the ring is bounded.
    bound: RingBound,
    /// The retained events, oldest first. Eviction drops from the front
    /// and advances [`SubjectBuffer::floor`].
    ring: VecDeque<RingEntry>,
    /// Lowest offset still retained. `next_floor` of an empty ring is
    /// the next offset to be assigned (so a `since_offset` exactly at
    /// the head replays nothing and goes straight to live). Tracked
    /// explicitly so gap detection is exact even after the ring empties
    /// or fully churns.
    floor: u64,
    /// Running sum of `RingEntry::bytes` for byte-bounded subjects.
    /// Always 0 for count-bounded subjects.
    byte_total: usize,
    /// Per-subscriber ack watermarks for this subject. Key is the
    /// monotonic subscriber id assigned at subscribe time; value is the
    /// highest offset that subscriber has acked. Entries are removed
    /// when a subscriber's stream drops.
    acks: HashMap<u64, u64>,
    /// Live re-broadcast channel: the pump publishes every
    /// offset-stamped [`Event`] here; each live subscriber holds a
    /// [`broadcast::Receiver`]. `None` until the pump is spawned.
    live: Option<broadcast::Sender<Event>>,
    /// Whether the per-subject pump task has been spawned yet.
    pump_started: bool,
}

impl SubjectBuffer {
    fn new(bound: RingBound) -> Self {
        Self {
            counter: AtomicU64::new(0),
            bound,
            ring: VecDeque::new(),
            floor: 0,
            byte_total: 0,
            acks: HashMap::new(),
            live: None,
            pump_started: false,
        }
    }

    /// Assign the next offset to `event`, append it to the ring, evict
    /// to satisfy the bound (advancing the floor), and return the
    /// stamped event for re-broadcast. Called only by the pump, under
    /// the handler's `Mutex`, so offset assignment and ring append are
    /// atomic with respect to each other.
    fn publish(&mut self, mut event: Event) -> Event {
        let offset = self.counter.fetch_add(1, Ordering::Relaxed);
        event.offset = offset;

        // Floor of an empty ring tracks the next-to-assign offset so a
        // `since_offset` exactly at the head is not mistaken for a gap.
        if self.ring.is_empty() {
            self.floor = offset;
        }

        let bytes = match self.bound {
            RingBound::Bytes(_) => session_io_bytes(&event),
            RingBound::Count(_) => 0,
        };
        self.byte_total += bytes;
        self.ring.push_back(RingEntry {
            event: event.clone(),
            bytes,
        });
        self.evict();
        event
    }

    /// Drop oldest entries until the bound is satisfied, advancing the
    /// floor as entries leave. A single oversized `session.io` chunk
    /// (larger than the whole byte cap on its own) is retained as the
    /// sole entry rather than evicted to empty — the most recent event
    /// is always replayable.
    fn evict(&mut self) {
        match self.bound {
            RingBound::Count(cap) => {
                while self.ring.len() > cap {
                    self.pop_front();
                }
            }
            RingBound::Bytes(cap) => {
                while self.byte_total > cap && self.ring.len() > 1 {
                    self.pop_front();
                }
            }
        }
        self.refresh_floor();
    }

    /// Remove the oldest entry, debiting its byte weight.
    fn pop_front(&mut self) {
        if let Some(entry) = self.ring.pop_front() {
            self.byte_total = self.byte_total.saturating_sub(entry.bytes);
        }
    }

    /// Re-derive the floor from the front of the ring. When the ring is
    /// non-empty the floor is the front entry's offset; when empty it is
    /// the next-to-assign offset (so "resume from head" replays
    /// nothing).
    fn refresh_floor(&mut self) {
        if let Some(front) = self.ring.front() {
            self.floor = front.event.offset;
        } else {
            self.floor = self.counter.load(Ordering::Relaxed);
        }
    }

    /// Collect the retained events with `offset > since` for replay.
    fn replay_after(&self, since: u64) -> Vec<Event> {
        self.ring
            .iter()
            .filter(|e| e.event.offset > since)
            .map(|e| e.event.clone())
            .collect()
    }

    /// Prune the ring to `min(acked offset across attached
    /// subscribers)`: drop any entry every attached subscriber has
    /// already acked. With zero attached subscribers, do nothing (retain
    /// up to the size bound — a reconnect may still want the tail).
    fn prune_to_min_ack(&mut self) {
        if self.acks.is_empty() {
            return;
        }
        let Some(min_ack) = self.acks.values().copied().min() else {
            return;
        };
        // Drop entries whose offset <= min_ack: every attached
        // subscriber has consumed them. Keep entries with offset >
        // min_ack (some subscriber still needs them).
        while self.ring.front().is_some_and(|e| e.event.offset <= min_ack) {
            self.pop_front();
        }
        self.refresh_floor();
    }
}

/// Byte weight of an [`Event`] for the byte-bounded `session.io` ring:
/// the length of its `SessionIoChunk.data`. Non-`session_io` events (which
/// never land in a byte-bounded ring) weigh 0.
fn session_io_bytes(event: &Event) -> usize {
    match &event.body {
        Some(EventBody::SessionIo(chunk)) => chunk.data.len(),
        _ => 0,
    }
}

/// RAII guard: deregisters a live subscriber from its [`SubjectBuffer`]'s
/// ack table when the subscriber's boxed stream is dropped (stream end
/// or client disconnect), so it no longer holds back min-ack pruning.
struct SubscriberGuard {
    buffers: Arc<Mutex<HashMap<String, SubjectBuffer>>>,
    subject: String,
    subscriber_id: u64,
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        // Best-effort, fire-and-forget: deregister and re-prune. We
        // can't `.await` in `Drop`, so spawn a short task. If the
        // runtime is shutting down the spawn may not run — harmless,
        // since a dead subject buffer is never pruned again anyway.
        let buffers = Arc::clone(&self.buffers);
        let subject = self.subject.clone();
        let id = self.subscriber_id;
        tokio::spawn(async move {
            let mut map = buffers.lock().await;
            if let Some(buf) = map.get_mut(&subject) {
                buf.acks.remove(&id);
                buf.prune_to_min_ack();
            }
        });
    }
}

/// Implements the generated `Streams` service trait.
#[derive(Clone)]
pub struct StreamsHandler {
    supervisor: AgentSupervisorHandle,
    workspaces: WorkspaceManager,
    workareas: WorkareaManager,
    /// Optional suggestion engine handle. Wired by Task 40; when
    /// `None`, the `suggestion.events` subject returns
    /// `INVALID_ARGUMENT` (the subject is parsable but no producer is
    /// attached).
    suggestions: Option<SuggestionEngineHandle>,
    /// Per-subject ring-buffer + offset + ack state, keyed by the
    /// canonical subject string. Replaces the V0.1 bare offset map; the
    /// offset counter now lives inside each [`SubjectBuffer`].
    buffers: Arc<Mutex<HashMap<String, SubjectBuffer>>>,
    /// Monotonic subscriber-id source for the ack table.
    next_subscriber_id: Arc<AtomicU64>,
}

impl StreamsHandler {
    pub fn new(
        supervisor: AgentSupervisorHandle,
        workspaces: WorkspaceManager,
        workareas: WorkareaManager,
    ) -> Self {
        Self {
            supervisor,
            workspaces,
            workareas,
            suggestions: None,
            buffers: Arc::new(Mutex::new(HashMap::new())),
            next_subscriber_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Attach a [`SuggestionEngineHandle`] so the `suggestion.events`
    /// subject has a producer. Returns `self` for chaining at
    /// construction time (the api_server builder uses this pattern).
    pub fn with_suggestions(mut self, suggestions: SuggestionEngineHandle) -> Self {
        self.suggestions = Some(suggestions);
        self
    }

    /// Build the source stream of *unstamped* [`Event`]s for a subject:
    /// an initial replay snapshot (only the session subjects provide
    /// one) plus a live stream. The events carry `offset = 0` — the pump
    /// stamps the real offset in [`SubjectBuffer::publish`]. Returns the
    /// concrete bound for the subject too.
    async fn source_events(
        &self,
        subject: &Subject,
    ) -> Result<(Vec<Event>, BoxEventStream), Status> {
        match subject {
            Subject::SessionEvents(sid) => {
                let (replay, rx) = self
                    .supervisor
                    .subscribe_events_with_replay(sid)
                    .await
                    .ok_or_else(|| Status::not_found(format!("session {sid} not running")))?;
                let replay_events: Vec<Event> =
                    replay.into_iter().filter_map(map_agent_event).collect();
                let live =
                    BroadcastStream::new(rx).filter_map(|item| item.ok().and_then(map_agent_event));
                Ok((replay_events, Box::pin(live)))
            }
            Subject::SessionIo(sid) => {
                let (replay, rx) = self
                    .supervisor
                    .subscribe_session_io_with_replay(sid)
                    .await
                    .ok_or_else(|| Status::not_found(format!("session {sid} not running")))?;
                let replay_events: Vec<Event> = replay.into_iter().map(map_session_io).collect();
                let live =
                    BroadcastStream::new(rx).filter_map(|item| item.ok().map(map_session_io));
                Ok((replay_events, Box::pin(live)))
            }
            Subject::WorkspaceEvents => {
                let rx = self.workspaces.subscribe();
                let live =
                    BroadcastStream::new(rx).filter_map(|item| item.ok().map(map_workspace_event));
                Ok((Vec::new(), Box::pin(live)))
            }
            Subject::WorkareaEvents => {
                let rx = self.workareas.subscribe();
                let live =
                    BroadcastStream::new(rx).filter_map(|item| item.ok().map(map_workarea_event));
                Ok((Vec::new(), Box::pin(live)))
            }
            Subject::SuggestionEvents(filter_workarea) => {
                let engine = self.suggestions.as_ref().ok_or_else(|| {
                    Status::invalid_argument(
                        "streams.suggestion_engine_unavailable: suggestion engine not attached",
                    )
                })?;
                let rx = engine.subscribe();
                let filter = filter_workarea.clone();
                let live = BroadcastStream::new(rx).filter_map(move |item| {
                    item.ok().and_then(|chip| {
                        if let Some(ref expected) = filter {
                            if chip.workarea_id.as_str() != expected {
                                return None;
                            }
                        }
                        Some(map_suggestion_event(chip))
                    })
                });
                Ok((Vec::new(), Box::pin(live)))
            }
        }
    }

    /// Ensure the per-subject [`SubjectBuffer`] exists and its pump task
    /// is running, then return a live `broadcast::Receiver<Event>` for
    /// this subscriber. The pump reads the source stream once, stamps
    /// offsets, fills the ring, and re-broadcasts; all subscribers share
    /// it so they agree on offset numbering.
    ///
    /// Returns the live receiver, a replay snapshot of the ring's events
    /// with `offset > since_offset` (empty when `since_offset` is
    /// `None`), an optional `GapDetected` event (when `since_offset` is
    /// older than the floor), and the newly-registered subscriber id.
    async fn attach(
        &self,
        subject_str: &str,
        subject: &Subject,
        since_offset: Option<u64>,
    ) -> Result<Attach, Status> {
        let bound = RingBound::for_subject(subject);

        // Building the source stream may call into the supervisor (an
        // `.await` that acquires other locks), so it must happen OUTSIDE
        // the buffers lock. Decide whether THIS subscribe is the one to
        // start the pump: check under the lock, then (if so) build the
        // source unlocked.
        let need_pump = {
            let mut map = self.buffers.lock().await;
            let buf = map
                .entry(subject_str.to_string())
                .or_insert_with(|| SubjectBuffer::new(bound));
            !buf.pump_started
        };

        if need_pump {
            let (replay_events, live_source) = self.source_events(subject).await?;
            let (tx, _rx) = broadcast::channel::<Event>(LIVE_BROADCAST_CAP);
            // Prepared pump material to install (and spawn) below; set to
            // `None` if we lose the start-the-pump race.
            let mut pump_to_spawn: Option<(BoxEventStream, broadcast::Sender<Event>)> =
                Some((live_source, tx.clone()));
            // Install the pump's sender + replay + this subscriber's
            // registration in ONE critical section so no live event can
            // slip between pump start and this subscriber's `subscribe()`.
            let subscriber_id = self.next_subscriber_id.fetch_add(1, Ordering::Relaxed);
            let mut map = self.buffers.lock().await;
            let buf = map
                .entry(subject_str.to_string())
                .or_insert_with(|| SubjectBuffer::new(bound));
            // The supervisor's one-time replay snapshot (session subjects
            // only). It is published into the ring so reconnecting
            // subscribers can replay it by offset; the pump-CREATING
            // subscriber additionally receives it as its prefix even with
            // `since_offset = None`, preserving the V0.1 contract that the
            // subscribe which triggers the supervisor snapshot sees that
            // burst (the supervisor does not re-replay to later
            // subscribers — see `subscribe_events_with_replay`).
            let mut creating_replay: Vec<Event> = Vec::new();
            if !buf.pump_started {
                buf.pump_started = true;
                buf.live = Some(tx.clone());
                for ev in replay_events {
                    let stamped = buf.publish(ev);
                    creating_replay.push(stamped.clone());
                    let _ = tx.send(stamped); // no receivers yet — ring holds them
                }
            } else {
                // Lost the race: another subscribe installed the pump.
                // Discard our prepared pump (drop its source receiver) and
                // do not claim the supervisor snapshot as our prefix.
                pump_to_spawn = None;
            }
            let attach = Self::register_subscriber(
                buf,
                subject_str,
                since_offset,
                subscriber_id,
                creating_replay,
            );
            drop(map);
            if let Some((source, tx)) = pump_to_spawn {
                self.spawn_pump(subject_str.to_string(), source, tx);
            }
            return Ok(attach);
        }

        // Pump already running: just register this subscriber (no
        // supervisor snapshot to claim — the pump consumed it once).
        let subscriber_id = self.next_subscriber_id.fetch_add(1, Ordering::Relaxed);
        let mut map = self.buffers.lock().await;
        let buf = map
            .get_mut(subject_str)
            .expect("subject buffer just inserted");
        Ok(Self::register_subscriber(
            buf,
            subject_str,
            since_offset,
            subscriber_id,
            Vec::new(),
        ))
    }

    /// Register a subscriber against a [`SubjectBuffer`] under the
    /// caller's lock: subscribe to the live channel, record the ack
    /// watermark, and compute the replay snapshot or `GapDetected`
    /// frame. Pure given the locked buffer — no `.await`.
    ///
    /// `creating_replay` is the supervisor's one-time replay snapshot,
    /// non-empty only for the pump-creating subscribe on a session
    /// subject; it is delivered as the prefix even when
    /// `since_offset = None` (the V0.1 contract).
    fn register_subscriber(
        buf: &mut SubjectBuffer,
        subject_str: &str,
        since_offset: Option<u64>,
        subscriber_id: u64,
        creating_replay: Vec<Event>,
    ) -> Attach {
        let live_rx = buf
            .live
            .as_ref()
            .expect("pump installed the live sender")
            .subscribe();
        // Initial ack watermark: `since_offset` when provided (the client
        // has consumed up to there), else the floor's predecessor so the
        // subscriber never holds back pruning of events it has not been
        // handed.
        let initial_ack = since_offset.unwrap_or_else(|| buf.floor.saturating_sub(1));
        buf.acks.insert(subscriber_id, initial_ack);

        let (replay, gap) = match since_offset {
            // V0.1 contract: live-only, plus the supervisor's one-time
            // snapshot for the pump-creating subscribe (empty otherwise).
            None => (creating_replay, None),
            Some(n) => {
                // The next offset the client wants is `n + 1`. It is
                // still retained iff `n + 1 >= floor`, i.e.
                // `n >= floor - 1`. Otherwise the gap is unrecoverable.
                if buf.floor == 0 || n.saturating_add(1) >= buf.floor {
                    (buf.replay_after(n), None)
                } else {
                    let gap = Event {
                        offset: buf.floor,
                        at: Some(now_ts()),
                        body: Some(EventBody::GapDetected(GapDetected {
                            subject: subject_str.to_string(),
                            buffer_floor: buf.floor,
                        })),
                    };
                    (Vec::new(), Some(gap))
                }
            }
        };

        Attach {
            live_rx,
            replay,
            gap,
            subscriber_id,
        }
    }

    /// Spawn the per-subject pump: read the live source stream, stamp +
    /// ring-append each event under the buffers lock, re-broadcast the
    /// stamped event. Runs until the source stream ends (all producers
    /// dropped). In-memory only; not restarted across Core restart.
    fn spawn_pump(
        &self,
        subject_str: String,
        mut source: BoxEventStream,
        tx: broadcast::Sender<Event>,
    ) {
        let buffers = Arc::clone(&self.buffers);
        tokio::spawn(async move {
            while let Some(ev) = source.next().await {
                let stamped = {
                    let mut map = buffers.lock().await;
                    match map.get_mut(&subject_str) {
                        Some(buf) => buf.publish(ev),
                        // Buffer vanished (never happens — we don't
                        // remove buffers) — stop the pump.
                        None => break,
                    }
                };
                // Re-broadcast to live subscribers. `send` errors only
                // when there are zero receivers; that's expected when no
                // client is attached, and the event is safely in the
                // ring for a future reconnect.
                let _ = tx.send(stamped);
            }
        });
    }
}

/// Result of [`StreamsHandler::attach`].
struct Attach {
    live_rx: broadcast::Receiver<Event>,
    replay: Vec<Event>,
    gap: Option<Event>,
    subscriber_id: u64,
}

/// Boxed stream of *unstamped* events from a subject's source.
type BoxEventStream = Pin<Box<dyn Stream<Item = Event> + Send + 'static>>;

/// Server-stream item type for `Streams.Subscribe`.
type SubscribeStream = Pin<Box<dyn Stream<Item = Result<Event, Status>> + Send + 'static>>;

#[async_trait]
impl StreamsService for StreamsHandler {
    type SubscribeStream = SubscribeStream;

    #[tracing::instrument(skip_all, name = "Streams::Subscribe", fields(subject = %request.get_ref().subject))]
    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let req = request.into_inner();
        // V1.0: `since_offset` is LIVE (Task 202). `filter` is still
        // ignored (subject-string filters cover the V0.1 catalog).
        let _ = &req.filter;

        let subject = parse_subject(&req.subject)?;
        let attach = self
            .attach(&req.subject, &subject, req.since_offset)
            .await?;

        let Attach {
            live_rx,
            replay,
            gap,
            subscriber_id,
        } = attach;

        // The guard deregisters this subscriber on stream drop so it
        // stops holding back min-ack pruning.
        let guard = SubscriberGuard {
            buffers: Arc::clone(&self.buffers),
            subject: req.subject.clone(),
            subscriber_id,
        };

        // Frame order: [GapDetected?] then [replayed ring events] then
        // [live]. GapDetected and replay are mutually exclusive (a gap
        // means nothing replayable), but the chain handles both being
        // empty cleanly.
        let prefix: Vec<Event> = gap.into_iter().chain(replay).collect();
        let prefix_iter = futures::stream::iter(prefix.into_iter().map(Ok));
        let live = BroadcastStream::new(live_rx).filter_map(|item| item.ok().map(Ok));

        // Move the guard into the stream so its lifetime is the stream's
        // lifetime: when the boxed stream is dropped, the guard drops.
        let stream = prefix_iter.chain(live).map(move |item| {
            // Touch the guard so it is captured (and thus dropped with
            // the stream). Zero-cost.
            let _ = &guard;
            item
        });

        Ok(Response::new(Box::pin(stream)))
    }

    #[tracing::instrument(skip_all, name = "Streams::AckOffset", fields(subject = %request.get_ref().subject))]
    async fn ack_offset(&self, request: Request<AckOffsetRequest>) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.subject.is_empty() {
            return Err(Status::invalid_argument(
                "streams.ack_empty_subject: subject must be non-empty",
            ));
        }
        // Validate the subject is in the catalog (a typo'd ack is a
        // client bug worth surfacing, same as Subscribe).
        let _ = parse_subject(&req.subject)?;

        let mut map = self.buffers.lock().await;
        let Some(buf) = map.get_mut(&req.subject) else {
            // No buffer yet means no events were ever published for this
            // subject, so there is nothing to ack/prune. Treat as a
            // no-op success — acks are advisory.
            return Ok(Response::new(()));
        };

        // Advance the highest acked offset across this subject's
        // subscribers to at least `req.offset`. We can't tie the ack to
        // a specific subscriber id over the unary path (the Connect-Web
        // fallback has no in-stream identity), so we model the unary ack
        // as "some subscriber has consumed up to `offset`": raise the
        // minimum watermark by bumping any subscriber still below it.
        // This prunes conservatively — never past an un-acked
        // subscriber that is ahead, never below one that is behind.
        for ack in buf.acks.values_mut() {
            if *ack < req.offset {
                *ack = req.offset;
            }
        }
        buf.prune_to_min_ack();

        Ok(Response::new(()))
    }
}

/// Parse a subject string into the typed [`Subject`].
#[allow(clippy::result_large_err)]
pub fn parse_subject(s: &str) -> Result<Subject, Status> {
    if let Some(sid) = s.strip_prefix("session.events.") {
        if sid.is_empty() {
            return Err(invalid_subject(s));
        }
        return Ok(Subject::SessionEvents(PersistSessionId(sid.to_string())));
    }
    if let Some(sid) = s.strip_prefix("session.io.") {
        if sid.is_empty() {
            return Err(invalid_subject(s));
        }
        return Ok(Subject::SessionIo(PersistSessionId(sid.to_string())));
    }
    // Task 40: `suggestion.events` (with optional trailing
    // `.<workarea_id>` filter). The trailing form is preferred over
    // using `SubscribeRequest.filter` because V0.1 ignores `filter`.
    if let Some(rest) = s.strip_prefix("suggestion.events") {
        if rest.is_empty() {
            return Ok(Subject::SuggestionEvents(None));
        }
        if let Some(wid) = rest.strip_prefix('.') {
            if wid.is_empty() {
                return Err(invalid_subject(s));
            }
            return Ok(Subject::SuggestionEvents(Some(wid.to_string())));
        }
        return Err(invalid_subject(s));
    }
    match s {
        "workspace.events" => Ok(Subject::WorkspaceEvents),
        "workarea.events" => Ok(Subject::WorkareaEvents),
        _ => Err(invalid_subject(s)),
    }
}

#[allow(clippy::result_large_err)]
fn invalid_subject(s: &str) -> Status {
    Status::invalid_argument(format!("streams.unknown_subject: {s:?}"))
}

fn now_ts() -> prost_types::Timestamp {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    prost_types::Timestamp {
        seconds: d.as_secs() as i64,
        nanos: d.subsec_nanos() as i32,
    }
}

/// Map an in-process [`AgentEvent`] into a wire [`Event`] for the
/// `session.events.<sid>` subject. The `offset` field is left 0; the
/// per-subject pump stamps it at publish time. Returns `None` for
/// variants that the V0.1 wire surface does not yet carry
/// (`ContextUsage`, `Crashed`) so the streaming layer can filter them
/// out without conflating signals.
fn map_agent_event(ev: AgentEvent) -> Option<Event> {
    let (session_id, kind) = match ev {
        AgentEvent::Started { session_id } => (
            session_id,
            SessionEventKind::Started(AgentStarted {
                // V0.1 has no model/mode plumbing yet; emit empty
                // strings so the wire shape is honoured.
                model: String::new(),
                mode: String::new(),
            }),
        ),
        AgentEvent::Message {
            session_id,
            content,
            ..
        } => (
            session_id,
            SessionEventKind::Message(AgentMessage {
                role: "assistant".to_string(),
                content: content.into_bytes(),
            }),
        ),
        AgentEvent::Exited {
            session_id,
            exit_code,
            ..
        } => (
            session_id,
            SessionEventKind::Exited(AgentExited { exit_code }),
        ),
        AgentEvent::AwaitingApproval {
            session_id,
            approval_id,
            tool,
            summary,
            payload_json,
            urgent,
            destructive_label,
        } => (
            session_id,
            SessionEventKind::AwaitingApproval(ProtoAwaitingApproval {
                approval_id,
                tool,
                summary,
                payload_json,
                urgent,
                destructive_label,
            }),
        ),
        AgentEvent::ApprovalResolved {
            session_id,
            approval_id,
            tool,
            decision,
        } => (
            session_id,
            SessionEventKind::ApprovalResolved(ProtoApprovalResolved {
                approval_id,
                tool,
                decision,
            }),
        ),
        AgentEvent::ToolCall {
            session_id,
            call_id,
            name,
            args_json,
        } => (
            session_id,
            SessionEventKind::ToolCall(ProtoToolCall {
                call_id,
                name,
                args_json,
            }),
        ),
        AgentEvent::TurnComplete { session_id } => (
            session_id,
            SessionEventKind::TurnComplete(ProtoTurnComplete {}),
        ),
        AgentEvent::CheckpointCreated {
            session_id,
            checkpoint_id,
            git_ref,
        } => (
            session_id,
            SessionEventKind::CheckpointCreated(ProtoCheckpointCreated {
                checkpoint_id,
                git_ref,
            }),
        ),
        // Task 40: `ContextUsage` and `Crashed` are V0.1 internal-only
        // signals consumed by the Suggestion Engine. The
        // `session.events` wire surface does not carry them yet (the
        // proto fields arrive with V1.0's structured parser packs); the
        // mapper returns `None` so the pump drops the frame on the
        // gRPC stream. Subscribers that care about these signals use
        // the `suggestion.events` subject instead.
        AgentEvent::ContextUsage { .. } | AgentEvent::Crashed { .. } => return None,
    };
    Some(Event {
        offset: 0,
        at: Some(now_ts()),
        body: Some(EventBody::Session(ProtoSessionEvent {
            session_id: session_id.to_string(),
            kind: Some(kind),
        })),
    })
}

fn map_session_io(chunk: SessionIoChunk) -> Event {
    Event {
        offset: 0,
        at: Some(now_ts()),
        body: Some(EventBody::SessionIo(ProtoSessionIoChunk {
            session_id: chunk.session_id.to_string(),
            stream: chunk.stream.to_string(),
            data: chunk.data,
        })),
    }
}

fn map_workspace_event(ev: WorkspaceEvent) -> Event {
    let (workspace_id, kind) = match ev {
        WorkspaceEvent::Created(ws) => (ws.id.to_string(), "created".to_string()),
        WorkspaceEvent::Archived(id) => (id.to_string(), "archived".to_string()),
        WorkspaceEvent::Restored(ws) => (ws.id.to_string(), "restored".to_string()),
    };
    Event {
        offset: 0,
        at: Some(now_ts()),
        body: Some(EventBody::Workspace(ProtoWorkspaceEvent {
            workspace_id,
            kind,
        })),
    }
}

fn map_suggestion_event(chip: Chip) -> Event {
    Event {
        offset: 0,
        at: Some(now_ts()),
        body: Some(EventBody::Suggestion(ProtoChip {
            rule_id: chip.rule_id,
            workarea_id: chip.workarea_id.0,
            title: chip.title,
            priority: chip.priority,
            created_at_ms: chip.created_at,
            action: chip.action.as_wire_str().to_string(),
        })),
    }
}

fn map_workarea_event(ev: WorkareaEvent) -> Event {
    let (workarea_id, kind) = match ev {
        WorkareaEvent::Created(wa) => (wa.id.to_string(), "created".to_string()),
        WorkareaEvent::Archived(id) => (id.to_string(), "archived".to_string()),
        WorkareaEvent::Restored(wa) => (wa.id.to_string(), "restored".to_string()),
    };
    Event {
        offset: 0,
        at: Some(now_ts()),
        body: Some(EventBody::Workarea(ProtoWorkareaEvent {
            workarea_id,
            kind,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_event(data: &[u8]) -> Event {
        Event {
            offset: 0,
            at: None,
            body: Some(EventBody::SessionIo(ProtoSessionIoChunk {
                session_id: "s".to_string(),
                stream: "stdout".to_string(),
                data: data.to_vec(),
            })),
        }
    }

    fn ws_event(n: u64) -> Event {
        Event {
            offset: 0,
            at: None,
            body: Some(EventBody::Workspace(ProtoWorkspaceEvent {
                workspace_id: format!("ws-{n}"),
                kind: "created".to_string(),
            })),
        }
    }

    #[test]
    fn parse_session_events_ok() {
        let s = parse_subject("session.events.abc-123").unwrap();
        assert_eq!(
            s,
            Subject::SessionEvents(PersistSessionId("abc-123".into()))
        );
    }

    #[test]
    fn parse_session_io_ok() {
        let s = parse_subject("session.io.xyz").unwrap();
        assert_eq!(s, Subject::SessionIo(PersistSessionId("xyz".into())));
    }

    #[test]
    fn parse_workspace_workarea_ok() {
        assert_eq!(
            parse_subject("workspace.events").unwrap(),
            Subject::WorkspaceEvents
        );
        assert_eq!(
            parse_subject("workarea.events").unwrap(),
            Subject::WorkareaEvents
        );
    }

    #[test]
    fn parse_unknown_subject_errors() {
        let e = parse_subject("nope.bad").unwrap_err();
        assert_eq!(e.code(), tonic::Code::InvalidArgument);
        assert!(e.message().contains("streams.unknown_subject"));
    }

    #[test]
    fn parse_empty_session_id_errors() {
        let e = parse_subject("session.events.").unwrap_err();
        assert_eq!(e.code(), tonic::Code::InvalidArgument);
    }

    // ---- Ring-buffer unit tests (Task 202) ------------------------------

    #[test]
    fn publish_assigns_monotonic_offsets_from_zero() {
        let mut buf = SubjectBuffer::new(RingBound::Count(RING_EVENT_CAP));
        let e0 = buf.publish(ws_event(0));
        let e1 = buf.publish(ws_event(1));
        let e2 = buf.publish(ws_event(2));
        assert_eq!(e0.offset, 0);
        assert_eq!(e1.offset, 1);
        assert_eq!(e2.offset, 2);
        assert_eq!(buf.floor, 0);
    }

    #[test]
    fn replay_returns_exactly_offset_greater_than_since() {
        let mut buf = SubjectBuffer::new(RingBound::Count(RING_EVENT_CAP));
        for n in 0..5 {
            buf.publish(ws_event(n));
        }
        // since=1 → offsets 2,3,4.
        let replay = buf.replay_after(1);
        let offsets: Vec<u64> = replay.iter().map(|e| e.offset).collect();
        assert_eq!(offsets, vec![2, 3, 4]);
        // since at head → nothing.
        assert!(buf.replay_after(4).is_empty());
    }

    #[test]
    fn count_eviction_drops_oldest_and_advances_floor() {
        let cap = 3;
        let mut buf = SubjectBuffer::new(RingBound::Count(cap));
        for n in 0..5 {
            buf.publish(ws_event(n));
        }
        // 5 published, cap 3 → retains offsets 2,3,4; floor=2.
        assert_eq!(buf.ring.len(), 3);
        assert_eq!(buf.floor, 2);
        let offsets: Vec<u64> = buf.ring.iter().map(|e| e.event.offset).collect();
        assert_eq!(offsets, vec![2, 3, 4]);
    }

    #[test]
    fn byte_eviction_for_session_io_bounds_by_bytes_not_count() {
        // 100-byte cap; each chunk is 40 bytes. Three chunks = 120 bytes
        // > 100 → oldest evicted, two retained (80 bytes).
        let mut buf = SubjectBuffer::new(RingBound::Bytes(100));
        buf.publish(io_event(&[0u8; 40])); // offset 0
        buf.publish(io_event(&[0u8; 40])); // offset 1
        buf.publish(io_event(&[0u8; 40])); // offset 2 → evict offset 0
        assert_eq!(buf.ring.len(), 2);
        assert_eq!(buf.byte_total, 80);
        assert_eq!(buf.floor, 1);
        let offsets: Vec<u64> = buf.ring.iter().map(|e| e.event.offset).collect();
        assert_eq!(offsets, vec![1, 2]);
    }

    #[test]
    fn byte_eviction_retains_single_oversized_chunk() {
        // A chunk larger than the whole cap is retained as the sole
        // entry (the most recent event is always replayable).
        let mut buf = SubjectBuffer::new(RingBound::Bytes(100));
        buf.publish(io_event(&[0u8; 40]));
        buf.publish(io_event(&[0u8; 500])); // oversized → only it remains
        assert_eq!(buf.ring.len(), 1);
        assert_eq!(buf.floor, 1);
    }

    #[test]
    fn gap_detection_boundary() {
        // floor=2 after evicting to cap 3 from 5 events. since=0 (<
        // floor-1=1) → gap. since=1 (== floor-1) → replay (boundary, no
        // gap). since=2 → replay 3,4.
        let cap = 3;
        let mut buf = SubjectBuffer::new(RingBound::Count(cap));
        for n in 0..5 {
            buf.publish(ws_event(n));
        }
        assert_eq!(buf.floor, 2);
        // since = floor - 1 is the oldest recoverable cursor.
        assert!(0u64.saturating_add(1) < buf.floor); // since=0 → gap
        assert!(1u64.saturating_add(1) >= buf.floor); // since=1 → ok
        assert_eq!(buf.replay_after(1).len(), 3); // 2,3,4
    }

    #[test]
    fn prune_to_min_ack_never_drops_unacked_by_attached_subscriber() {
        let mut buf = SubjectBuffer::new(RingBound::Count(RING_EVENT_CAP));
        for n in 0..5 {
            buf.publish(ws_event(n));
        }
        // Two attached subscribers: A acked up to 3, B acked up to 1.
        buf.acks.insert(1, 3);
        buf.acks.insert(2, 1);
        buf.prune_to_min_ack();
        // min-ack = 1 → drop offsets <= 1 (0,1); retain 2,3,4.
        let offsets: Vec<u64> = buf.ring.iter().map(|e| e.event.offset).collect();
        assert_eq!(offsets, vec![2, 3, 4]);
        assert_eq!(buf.floor, 2);

        // B catches up to 4, but A is still at 3 → min-ack = 3 → drop
        // 2,3; retain offset 4 (A has NOT acked it). This is the core
        // invariant: never prune past an attached subscriber.
        buf.acks.insert(2, 4);
        buf.prune_to_min_ack();
        let offsets: Vec<u64> = buf.ring.iter().map(|e| e.event.offset).collect();
        assert_eq!(offsets, vec![4]);
        assert_eq!(buf.floor, 4);

        // A finally acks 4 too → min-ack = 4 → drop 4 → empty.
        buf.acks.insert(1, 4);
        buf.prune_to_min_ack();
        assert!(buf.ring.is_empty());
    }

    #[test]
    fn prune_with_zero_subscribers_retains_tail() {
        let mut buf = SubjectBuffer::new(RingBound::Count(RING_EVENT_CAP));
        for n in 0..3 {
            buf.publish(ws_event(n));
        }
        // No attached subscribers → no pruning (a reconnect may want the
        // tail).
        buf.prune_to_min_ack();
        assert_eq!(buf.ring.len(), 3);
    }
}
