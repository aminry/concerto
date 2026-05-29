//! gRPC `Suggestions` service handler (Task 40).
//!
//! Thin wrapper over [`crate::suggestions::SuggestionEngineHandle`]:
//! translate `concerto.v1.Suggestions` requests into handle calls and
//! translate the result back into proto messages. V0.1's
//! `RecordSuggestionOutcome` is a logging stub — the learning loop
//! arrives in V1.0.

use async_trait::async_trait;
use concerto_persist::WorkareaId as PersistWorkareaId;
use concerto_proto::v1::suggestions_server::Suggestions as SuggestionsService;
use concerto_proto::v1::{
    Chip as ProtoChip, GetSuggestionsRequest, RecordSuggestionOutcomeRequest,
    RecordSuggestionOutcomeResponse, SuggestionsResponse,
};
use tonic::{Request, Response, Status};

use crate::suggestions::chip::Chip;
use crate::suggestions::SuggestionEngineHandle;

#[derive(Clone)]
pub struct SuggestionsHandler {
    engine: SuggestionEngineHandle,
}

impl SuggestionsHandler {
    pub fn new(engine: SuggestionEngineHandle) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl SuggestionsService for SuggestionsHandler {
    #[tracing::instrument(skip_all, name = "Suggestions::GetSuggestions")]
    async fn get_suggestions(
        &self,
        request: Request<GetSuggestionsRequest>,
    ) -> Result<Response<SuggestionsResponse>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let chips = self
            .engine
            .list_for_workarea(&PersistWorkareaId(req.workarea_id))
            .await;
        Ok(Response::new(SuggestionsResponse {
            chips: chips.into_iter().map(chip_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Suggestions::RecordSuggestionOutcome")]
    async fn record_suggestion_outcome(
        &self,
        request: Request<RecordSuggestionOutcomeRequest>,
    ) -> Result<Response<RecordSuggestionOutcomeResponse>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        if req.rule_id.is_empty() {
            return Err(Status::invalid_argument("rule_id is required"));
        }
        self.engine
            .record_outcome(
                &PersistWorkareaId(req.workarea_id),
                &req.rule_id,
                &req.outcome,
            )
            .await;
        Ok(Response::new(RecordSuggestionOutcomeResponse {}))
    }
}

pub(crate) fn chip_to_proto(chip: Chip) -> ProtoChip {
    ProtoChip {
        rule_id: chip.rule_id,
        workarea_id: chip.workarea_id.0,
        title: chip.title,
        priority: chip.priority,
        created_at_ms: chip.created_at,
        action: chip.action.as_wire_str().to_string(),
    }
}
