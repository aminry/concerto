//! gRPC `Streams` service handler (Task 23).
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
//! V0.1 ignores `since_offset` per `design/10 §3.3` — ring-buffer +
//! `AckOffset` + `GapDetected` semantics land in V1.0.
//!
//! ## Offset accounting
//!
//! Per-subject monotonic counters live in
//! `Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>` on the handler. Each
//! frame the handler forwards to a client picks up `fetch_add(1)` on
//! the counter for that subject string, so two subscribers to
//! `session.events.<sid>` agree on the offset numbering for the same
//! event. The map grows once per distinct subject and is cleared at
//! V1.0 ring-buffer time; V0.1 leaks subject strings on session-id
//! churn (bounded by the number of sessions ever created in a single
//! Core run).
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

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_persist::SessionId as PersistSessionId;
use concerto_proto::v1::streams_server::Streams as StreamsService;
use concerto_proto::v1::{
    event::Body as EventBody, session_event::Kind as SessionEventKind, AgentExited, AgentMessage,
    AgentStarted, ApprovalResolved as ProtoApprovalResolved,
    AwaitingApproval as ProtoAwaitingApproval, CheckpointCreated as ProtoCheckpointCreated,
    Chip as ProtoChip, Event, SessionEvent as ProtoSessionEvent,
    SessionIoChunk as ProtoSessionIoChunk, SubscribeRequest, ToolCall as ProtoToolCall,
    TurnComplete as ProtoTurnComplete, WorkareaEvent as ProtoWorkareaEvent,
    WorkspaceEvent as ProtoWorkspaceEvent,
};
use futures::Stream;
use tokio::sync::Mutex;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status};

use crate::agent_supervisor::{AgentEvent, AgentSupervisorHandle, SessionIoChunk};
use crate::suggestions::{Chip, SuggestionEngineHandle};
use crate::workspace_manager::{WorkareaEvent, WorkareaManager, WorkspaceEvent, WorkspaceManager};

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
    /// Per-subject monotonic offset map. Subjects are keyed by their
    /// canonical string form.
    offsets: Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>,
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
            offsets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attach a [`SuggestionEngineHandle`] so the `suggestion.events`
    /// subject has a producer. Returns `self` for chaining at
    /// construction time (the api_server builder uses this pattern).
    pub fn with_suggestions(mut self, suggestions: SuggestionEngineHandle) -> Self {
        self.suggestions = Some(suggestions);
        self
    }

    /// Acquire (or lazily create) the offset counter for `subject`.
    async fn counter(&self, subject: &str) -> Arc<AtomicU64> {
        let mut map = self.offsets.lock().await;
        Arc::clone(
            map.entry(subject.to_string())
                .or_insert_with(|| Arc::new(AtomicU64::new(0))),
        )
    }
}

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
        // V0.1 ignores `since_offset` and `filter` per `design/10 §3.3`.
        let _ = (&req.filter, req.since_offset);

        let subject = parse_subject(&req.subject)?;
        let counter = self.counter(&req.subject).await;

        let stream: Self::SubscribeStream = match subject {
            Subject::SessionEvents(sid) => {
                let (replay, rx) = self
                    .supervisor
                    .subscribe_events_with_replay(&sid)
                    .await
                    .ok_or_else(|| Status::not_found(format!("session {sid} not running")))?;
                let counter_for_replay = Arc::clone(&counter);
                let replay_iter = futures::stream::iter(replay.into_iter().filter_map(move |ev| {
                    let offset = counter_for_replay.fetch_add(1, Ordering::Relaxed);
                    map_agent_event(ev, offset).map(Ok)
                }));
                let live = BroadcastStream::new(rx).filter_map(move |item| {
                    let counter = Arc::clone(&counter);
                    item.ok()
                        .and_then(|ev| {
                            let offset = counter.fetch_add(1, Ordering::Relaxed);
                            map_agent_event(ev, offset)
                        })
                        .map(Ok)
                });
                Box::pin(replay_iter.chain(live))
            }
            Subject::SessionIo(sid) => {
                let (replay, rx) = self
                    .supervisor
                    .subscribe_session_io_with_replay(&sid)
                    .await
                    .ok_or_else(|| Status::not_found(format!("session {sid} not running")))?;
                let counter_for_replay = Arc::clone(&counter);
                let replay_iter = futures::stream::iter(replay.into_iter().map(move |chunk| {
                    let offset = counter_for_replay.fetch_add(1, Ordering::Relaxed);
                    Ok(map_session_io(chunk, offset))
                }));
                let live = BroadcastStream::new(rx).filter_map(move |item| {
                    let counter = Arc::clone(&counter);
                    item.ok().map(|chunk| {
                        let offset = counter.fetch_add(1, Ordering::Relaxed);
                        Ok(map_session_io(chunk, offset))
                    })
                });
                Box::pin(replay_iter.chain(live))
            }
            Subject::WorkspaceEvents => {
                let rx = self.workspaces.subscribe();
                let s = BroadcastStream::new(rx).filter_map(move |item| {
                    let counter = Arc::clone(&counter);
                    item.ok().map(|ev| {
                        let offset = counter.fetch_add(1, Ordering::Relaxed);
                        Ok(map_workspace_event(ev, offset))
                    })
                });
                Box::pin(s)
            }
            Subject::WorkareaEvents => {
                let rx = self.workareas.subscribe();
                let s = BroadcastStream::new(rx).filter_map(move |item| {
                    let counter = Arc::clone(&counter);
                    item.ok().map(|ev| {
                        let offset = counter.fetch_add(1, Ordering::Relaxed);
                        Ok(map_workarea_event(ev, offset))
                    })
                });
                Box::pin(s)
            }
            Subject::SuggestionEvents(filter_workarea) => {
                let engine = self.suggestions.as_ref().ok_or_else(|| {
                    Status::invalid_argument(
                        "streams.suggestion_engine_unavailable: suggestion engine not attached",
                    )
                })?;
                let rx = engine.subscribe();
                let s = BroadcastStream::new(rx).filter_map(move |item| {
                    let counter = Arc::clone(&counter);
                    let filter = filter_workarea.clone();
                    item.ok().and_then(|chip| {
                        if let Some(ref expected) = filter {
                            if chip.workarea_id.as_str() != expected {
                                return None;
                            }
                        }
                        let offset = counter.fetch_add(1, Ordering::Relaxed);
                        Some(Ok(map_suggestion_event(chip, offset)))
                    })
                });
                Box::pin(s)
            }
        };
        Ok(Response::new(stream))
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
/// `session.events.<sid>` subject. Returns `None` for variants that the
/// V0.1 wire surface does not yet carry (`ContextUsage`, `Crashed`) so
/// the streaming layer can filter them out without conflating signals.
fn map_agent_event(ev: AgentEvent, offset: u64) -> Option<Event> {
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
        // mapper returns `None` so `filter_map` drops the frame on the
        // gRPC stream. Subscribers that care about these signals use
        // the `suggestion.events` subject instead.
        AgentEvent::ContextUsage { .. } | AgentEvent::Crashed { .. } => return None,
    };
    Some(Event {
        offset,
        at: Some(now_ts()),
        body: Some(EventBody::Session(ProtoSessionEvent {
            session_id: session_id.to_string(),
            kind: Some(kind),
        })),
    })
}

fn map_session_io(chunk: SessionIoChunk, offset: u64) -> Event {
    Event {
        offset,
        at: Some(now_ts()),
        body: Some(EventBody::SessionIo(ProtoSessionIoChunk {
            session_id: chunk.session_id.to_string(),
            stream: chunk.stream.to_string(),
            data: chunk.data,
        })),
    }
}

fn map_workspace_event(ev: WorkspaceEvent, offset: u64) -> Event {
    let (workspace_id, kind) = match ev {
        WorkspaceEvent::Created(ws) => (ws.id.to_string(), "created".to_string()),
        WorkspaceEvent::Archived(id) => (id.to_string(), "archived".to_string()),
        WorkspaceEvent::Restored(ws) => (ws.id.to_string(), "restored".to_string()),
    };
    Event {
        offset,
        at: Some(now_ts()),
        body: Some(EventBody::Workspace(ProtoWorkspaceEvent {
            workspace_id,
            kind,
        })),
    }
}

fn map_suggestion_event(chip: Chip, offset: u64) -> Event {
    Event {
        offset,
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

fn map_workarea_event(ev: WorkareaEvent, offset: u64) -> Event {
    let (workarea_id, kind) = match ev {
        WorkareaEvent::Created(wa) => (wa.id.to_string(), "created".to_string()),
        WorkareaEvent::Archived(id) => (id.to_string(), "archived".to_string()),
        WorkareaEvent::Restored(wa) => (wa.id.to_string(), "restored".to_string()),
    };
    Event {
        offset,
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
}
