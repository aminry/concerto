//! [`SuggestionEngineActor`] + cloneable [`SuggestionEngineHandle`]
//! (Task 40).
//!
//! Follows the same actor pattern as the other Core managers: the
//! actor's `run` parks on shutdown; all meaningful work flows through
//! the cheap-to-clone handle.
//!
//! ## V0.1 surface
//!
//! - [`SuggestionEngineHandle::evaluate_event`] — single entry point
//!   that runs every rule against `(workarea_id, event)`, performs
//!   the per-rule async side checks (e.g. `gix-wrap::status` for
//!   `turn_complete_with_uncommitted`), and emits matching chips on
//!   the broadcast channel after dedup.
//! - [`SuggestionEngineHandle::list_for_workarea`] — returns the
//!   chips currently buffered for `workarea_id` (within the dedup TTL
//!   window). The chip list is what the gRPC `GetSuggestions` handler
//!   surfaces.
//! - [`SuggestionEngineHandle::subscribe`] — broadcast receiver for
//!   the `suggestion.events` stream subject.
//! - [`SuggestionEngineHandle::record_outcome`] — V0.1 stub that logs
//!   via `tracing::info!`. V1.0's learning loop lands here.
//!
//! ## Subscription model
//!
//! The actor subscribes to [`crate::workspace_manager::WorkareaManager::subscribe`]
//! to learn about new workareas (so future enhancements can seed
//! per-workarea state). The per-session
//! [`crate::agent_supervisor::AgentSupervisorHandle::subscribe_events_with_replay`]
//! subscription is driven by the engine's host (`main.rs` calls
//! [`SuggestionEngineHandle::attach_session`] when a new session
//! starts); V0.1 keeps this attachment explicit so the engine doesn't
//! need a back-channel from the supervisor. Tests drive events
//! directly via `evaluate_event`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use concerto_error::Result;
use concerto_persist::{Persistence, SessionId, WorkareaId};
use tokio::sync::{broadcast, Mutex, RwLock};

use crate::agent_supervisor::{AgentEvent, AgentSupervisorHandle};
use crate::suggestions::chip::Chip;
use crate::suggestions::rules::{builtin_rules, SuggestionRule};
use crate::suggestions::state::WorkareaState;
use crate::supervisor::{Actor, ActorContext};

/// Broadcast channel capacity. Sized to match the other manager
/// channels (`workarea.events`, `workspace.events`).
const BROADCAST_CAPACITY: usize = 256;

/// Deduplication window. A rule that fires twice within this window
/// for the same `(workarea_id, rule_id)` emits only once. Frozen per
/// Task 40 §"Implementation notes".
pub const DEDUP_TTL: Duration = Duration::from_secs(60);

/// How long a chip remains in the per-workarea buffer surfaced by
/// `GetSuggestions`. Matched to the dedup TTL so the two stay in sync.
const CHIP_RETENTION: Duration = DEDUP_TTL;

/// Config for the actor's `run` loop. V0.1 has no knobs — the actor
/// parks on shutdown.
#[derive(Clone, Debug, Default)]
pub struct SuggestionEngineConfig;

/// Supervised actor that owns the suggestion engine handle. The
/// meaningful work flows through [`SuggestionEngineHandle`]; `run`
/// just parks on shutdown.
pub struct SuggestionEngineActor {
    handle: SuggestionEngineHandle,
}

/// Buffered chip + the [`Instant`] it landed at — the engine drops
/// entries older than [`CHIP_RETENTION`] on every `list_for_workarea`
/// call.
#[derive(Debug, Clone)]
struct BufferedChip {
    chip: Chip,
    inserted_at: Instant,
}

/// Cheap-cloneable, shareable handle to the Suggestion Engine. Frozen
/// per Task 40 §"Public interface this task locks".
#[derive(Clone)]
pub struct SuggestionEngineHandle {
    #[allow(dead_code)]
    persistence: Arc<Persistence>,
    /// Rules — built once per process and shared by reference. The
    /// engine evaluates every rule on every event.
    rules: Arc<Vec<Box<dyn SuggestionRule>>>,
    /// Per-workarea event aggregator. Rules consult the state instead
    /// of walking the entire event history.
    state: Arc<RwLock<HashMap<WorkareaId, WorkareaState>>>,
    /// Per-`(WorkareaId, rule_id)` last-emit timestamps. Used for
    /// dedup — a rule fires twice within `DEDUP_TTL` is squashed to a
    /// single chip.
    last_emit: Arc<RwLock<HashMap<(WorkareaId, String), Instant>>>,
    /// Per-workarea ring buffer of recently emitted chips. The gRPC
    /// `GetSuggestions` handler reads from here; subscribers to the
    /// `suggestion.events` subject read from the broadcast channel.
    chips: Arc<RwLock<HashMap<WorkareaId, Vec<BufferedChip>>>>,
    /// Broadcast sender — subscribers receive [`Chip`]s.
    events: broadcast::Sender<Chip>,
    /// Optional worktree-root resolver. When set, the
    /// `turn_complete_with_uncommitted` rule probes
    /// `gix-wrap::status` against the resolved path before emitting.
    /// Tests inject a closure so the FS probe can be mocked.
    worktree_resolver: Arc<dyn WorktreeResolver>,
    /// Sessions the engine has already attached a pump to. Keyed by
    /// `SessionId` so the periodic supervisor poll can skip
    /// already-attached sessions in O(1).
    attached_sessions: Arc<Mutex<HashSet<SessionId>>>,
}

/// Trait the engine consults to map a `WorkareaId` to its worktree
/// root (for the `turn_complete_with_uncommitted` rule). The
/// production resolver reads from `Persistence`; tests inject a
/// closure that returns a fixture path.
#[async_trait]
pub trait WorktreeResolver: Send + Sync {
    /// Resolve the worktree root for the workarea. Returns `None`
    /// when the workarea is unknown (e.g. archived).
    async fn worktree_root(&self, workarea_id: &WorkareaId) -> Option<PathBuf>;
}

/// Production resolver — reads the `workareas.worktree_root` column.
pub struct PersistenceWorktreeResolver {
    persistence: Arc<Persistence>,
}

impl PersistenceWorktreeResolver {
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self { persistence }
    }
}

#[async_trait]
impl WorktreeResolver for PersistenceWorktreeResolver {
    async fn worktree_root(&self, workarea_id: &WorkareaId) -> Option<PathBuf> {
        concerto_persist::workareas::get(self.persistence.readers(), workarea_id)
            .await
            .ok()
            .flatten()
            .map(|row| PathBuf::from(row.worktree_root))
    }
}

impl SuggestionEngineHandle {
    /// Build a fresh handle with the production worktree resolver.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        let resolver: Arc<dyn WorktreeResolver> =
            Arc::new(PersistenceWorktreeResolver::new(Arc::clone(&persistence)));
        Self::with_resolver(persistence, resolver)
    }

    /// Build a handle with a custom worktree resolver (used in tests
    /// to inject a closure that bypasses the DB read).
    pub fn with_resolver(
        persistence: Arc<Persistence>,
        worktree_resolver: Arc<dyn WorktreeResolver>,
    ) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            persistence,
            rules: Arc::new(builtin_rules()),
            state: Arc::new(RwLock::new(HashMap::new())),
            last_emit: Arc::new(RwLock::new(HashMap::new())),
            chips: Arc::new(RwLock::new(HashMap::new())),
            events,
            worktree_resolver,
            attached_sessions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Attach the engine to a live session: subscribe to the agent
    /// supervisor's per-session [`AgentEvent`] broadcast and spawn a
    /// pump that forwards every event into [`Self::evaluate_event`].
    ///
    /// Idempotent — calling twice for the same `session_id` is a noop
    /// (the second call hits the `attached_sessions` set and returns
    /// early).
    ///
    /// Returns `true` on the first call (pump spawned), `false` on
    /// every subsequent call.
    pub async fn attach_session(
        &self,
        supervisor: &AgentSupervisorHandle,
        workarea_id: WorkareaId,
        session_id: SessionId,
    ) -> bool {
        {
            let mut attached = self.attached_sessions.lock().await;
            if !attached.insert(session_id.clone()) {
                return false;
            }
        }
        let Some((replay, mut rx)) = supervisor.subscribe_events_with_replay(&session_id).await
        else {
            // Session vanished between the poll and the subscribe —
            // forget the attachment so a future call can retry once
            // the supervisor has the entry.
            let mut attached = self.attached_sessions.lock().await;
            attached.remove(&session_id);
            return false;
        };
        // Replay first so chips reflect the burst of events the
        // session emitted before the engine attached.
        for ev in replay {
            let _ = self.evaluate_event(&workarea_id, &ev).await;
        }
        let handle = self.clone();
        tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                let _ = handle.evaluate_event(&workarea_id, &ev).await;
            }
        });
        true
    }

    /// Spawn the per-supervisor poll loop that picks up new sessions
    /// and calls [`Self::attach_session`] on each. Run from `main.rs`
    /// once the supervisor handle exists. Cancelled when `shutdown`
    /// fires.
    pub fn spawn_session_pump(
        &self,
        supervisor: AgentSupervisorHandle,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        let handle = self.clone();
        tokio::spawn(async move {
            // 1s tick keeps the attach latency low for newly-started
            // sessions without flooding the DB. The poll reads the
            // read-only pool so it does not contend with writers.
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tick.tick() => {
                        // Walk every active workarea's live sessions
                        // and ensure the engine is subscribed.
                        let live = match list_all_live_sessions(supervisor.persistence()).await {
                            Ok(rows) => rows,
                            Err(e) => {
                                tracing::debug!(error = %e, "suggestions.session_pump: list failed");
                                continue;
                            }
                        };
                        for (wid, sid) in live {
                            let _ = handle.attach_session(&supervisor, wid, sid).await;
                        }
                    }
                }
            }
            tracing::debug!("suggestions.session_pump exited");
        });
    }

    /// Subscribe to chip emissions across every workarea. The gRPC
    /// `Streams` handler subscribes here and filters by `workarea_id`
    /// on the subject.
    pub fn subscribe(&self) -> broadcast::Receiver<Chip> {
        self.events.subscribe()
    }

    /// Return chips currently buffered for `workarea_id`, freshest
    /// first. Stale entries (older than [`CHIP_RETENTION`]) are
    /// dropped before the snapshot.
    pub async fn list_for_workarea(&self, workarea_id: &WorkareaId) -> Vec<Chip> {
        let mut chips = self.chips.write().await;
        let now = Instant::now();
        if let Some(buf) = chips.get_mut(workarea_id) {
            buf.retain(|c| now.duration_since(c.inserted_at) < CHIP_RETENTION);
            let mut out: Vec<Chip> = buf.iter().map(|c| c.chip.clone()).collect();
            // Highest priority first, then most recently emitted.
            out.sort_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then(b.created_at.cmp(&a.created_at))
            });
            out
        } else {
            Vec::new()
        }
    }

    /// V0.1 outcome stub — logs and returns. The persistence write +
    /// rule weighting arrive with V1.0's learning loop.
    pub async fn record_outcome(&self, workarea_id: &WorkareaId, rule_id: &str, outcome: &str) {
        tracing::info!(
            workarea = %workarea_id,
            rule = rule_id,
            outcome = outcome,
            "suggestions.record_outcome"
        );
    }

    /// Drive the rule pipeline against a single
    /// `(workarea_id, event)` pair. Updates the per-workarea state,
    /// evaluates every rule, performs any required async side checks,
    /// applies dedup, and broadcasts surviving chips. Returns the
    /// chips that were actually emitted (post-dedup, post-side-check)
    /// so tests can assert directly.
    pub async fn evaluate_event(&self, workarea_id: &WorkareaId, event: &AgentEvent) -> Vec<Chip> {
        self.update_state(workarea_id, event).await;
        let snapshot = {
            let states = self.state.read().await;
            states.get(workarea_id).cloned().unwrap_or_default()
        };

        let mut candidates: Vec<Chip> = self
            .rules
            .iter()
            .filter_map(|r| r.applies(workarea_id, &snapshot, event))
            .collect();

        // Side check: the `turn_complete_with_uncommitted` rule's chip
        // is only legitimate if the worktree actually has uncommitted
        // changes. Drop it otherwise. Other rules have no async side
        // effects in V0.1.
        let mut keep_uncommitted = true;
        if candidates
            .iter()
            .any(|c| c.rule_id == crate::suggestions::rules::commit_uncommitted::RULE_ID)
        {
            keep_uncommitted = self.has_uncommitted(workarea_id).await;
        }
        if !keep_uncommitted {
            candidates
                .retain(|c| c.rule_id != crate::suggestions::rules::commit_uncommitted::RULE_ID);
        }

        // Dedup pass — drop any chip whose `(workarea, rule_id)` last
        // fired within `DEDUP_TTL`.
        let mut emitted: Vec<Chip> = Vec::new();
        {
            let now = Instant::now();
            let mut last_emit = self.last_emit.write().await;
            // Garbage-collect stale entries on every pass so the map
            // does not grow unbounded across the process lifetime.
            last_emit.retain(|_, t| now.duration_since(*t) < DEDUP_TTL);

            for chip in candidates.into_iter() {
                let key = (chip.workarea_id.clone(), chip.rule_id.clone());
                if let Some(prev) = last_emit.get(&key) {
                    if now.duration_since(*prev) < DEDUP_TTL {
                        continue;
                    }
                }
                last_emit.insert(key, now);
                emitted.push(chip);
            }
        }

        // Buffer + broadcast.
        if !emitted.is_empty() {
            let mut chips = self.chips.write().await;
            let buf = chips.entry(workarea_id.clone()).or_default();
            let now = Instant::now();
            for chip in &emitted {
                buf.push(BufferedChip {
                    chip: chip.clone(),
                    inserted_at: now,
                });
                // Best-effort broadcast — a closed channel (no
                // subscribers) is not an error.
                let _ = self.events.send(chip.clone());
            }
        }
        emitted
    }

    /// Update the per-workarea aggregator with one fresh event.
    async fn update_state(&self, workarea_id: &WorkareaId, event: &AgentEvent) {
        let mut states = self.state.write().await;
        let s = states.entry(workarea_id.clone()).or_default();
        match event {
            AgentEvent::ContextUsage { pct, .. } => s.last_context_pct = Some(*pct),
            AgentEvent::TurnComplete { .. } => s.last_turn_complete_ms = Some(now_unix_ms()),
            AgentEvent::AwaitingApproval { .. } => {
                s.awaiting_approval_count = s.awaiting_approval_count.saturating_add(1);
            }
            AgentEvent::ApprovalResolved { .. } => {
                s.awaiting_approval_count = s.awaiting_approval_count.saturating_sub(1);
            }
            AgentEvent::Crashed { .. } => s.crashed = true,
            AgentEvent::Started { .. } => s.crashed = false,
            AgentEvent::Message { content, .. } => {
                s.last_message_content.push_str(content);
                s.trim_message_buffer();
            }
            // Variants the rules don't consult: noop. Future events
            // hook in here as the engine gains rules.
            AgentEvent::Exited { .. }
            | AgentEvent::ToolCall { .. }
            | AgentEvent::CheckpointCreated { .. } => {}
        }
    }

    /// Async side check for the `turn_complete_with_uncommitted`
    /// rule. Returns `true` when the workarea's worktree has at least
    /// one entry in `git status --porcelain=v1`.
    async fn has_uncommitted(&self, workarea_id: &WorkareaId) -> bool {
        let Some(root) = self.worktree_resolver.worktree_root(workarea_id).await else {
            return false;
        };
        match concerto_gix_wrap::status(&root).await {
            Ok(report) => !report.files.is_empty(),
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    workarea = %workarea_id,
                    "suggestions: gix-wrap::status failed; skipping commit chip"
                );
                false
            }
        }
    }
}

impl SuggestionEngineActor {
    /// Build a new actor with a fresh handle.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            handle: SuggestionEngineHandle::new(persistence),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> SuggestionEngineHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for SuggestionEngineActor {
    const NAME: &'static str = "suggestion-engine";
    type Config = SuggestionEngineConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        tracing::info!("Suggestion engine ready");
        ctx.shutdown.cancelled().await;
        tracing::debug!("Suggestion engine actor shutting down");
        Ok(())
    }
}

/// Read every live `sessions` row (one whose `ended_at` is NULL)
/// across the DB and return `(workarea_id, session_id)` pairs. The
/// session pump uses this to pick up new sessions on every tick.
async fn list_all_live_sessions(
    persistence: Arc<Persistence>,
) -> concerto_error::Result<Vec<(WorkareaId, SessionId)>> {
    use sqlx::Row as _;
    let rows = sqlx::query("SELECT id, workarea_id FROM sessions WHERE ended_at IS NULL")
        .fetch_all(persistence.readers())
        .await
        .map_err(|e| concerto_error::Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                WorkareaId(r.get::<String, _>("workarea_id")),
                SessionId(r.get::<String, _>("id")),
            )
        })
        .collect())
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
