//! gRPC `Schedules` service handler (Task 38).
//!
//! Thin wrapper over [`crate::scheduler::SchedulerHandle`]: translate
//! `concerto.v1.Schedules` requests into handle calls and translate the
//! result back into proto messages.

use async_trait::async_trait;
use concerto_persist::{Schedule as PersistSchedule, ScheduleRun as PersistScheduleRun};
use concerto_persist::{ScheduleId as PersistScheduleId, WorkareaId as PersistWorkareaId};
use concerto_proto::v1::schedules_server::Schedules as SchedulesService;
use concerto_proto::v1::{
    CreateScheduleRequest, ListSchedulesRequest, ListSchedulesResponse, Schedule as ProtoSchedule,
    ScheduleHistoryResponse, ScheduleId as ProtoScheduleId, ScheduleRun as ProtoScheduleRun,
};
use prost_types::Timestamp;
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::scheduler::{CreateScheduleRequest as HandleRequest, SchedulerHandle};

#[derive(Clone)]
pub struct SchedulesHandler {
    scheduler: SchedulerHandle,
}

impl SchedulesHandler {
    pub fn new(scheduler: SchedulerHandle) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl SchedulesService for SchedulesHandler {
    #[tracing::instrument(skip_all, name = "Schedules::CreateSchedule")]
    async fn create_schedule(
        &self,
        request: Request<CreateScheduleRequest>,
    ) -> Result<Response<ProtoSchedule>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        if req.kind.is_empty() {
            return Err(Status::invalid_argument("kind is required"));
        }
        let inserted = self
            .scheduler
            .create_schedule(HandleRequest {
                workarea_id: PersistWorkareaId(req.workarea_id),
                kind: req.kind,
                interval_seconds: req.interval_seconds,
                prompt: req.prompt,
                agent_kind: req.agent_kind,
                expires_at_unix_ms: if req.expires_at_unix_ms > 0 {
                    Some(req.expires_at_unix_ms)
                } else {
                    None
                },
            })
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(schedule_to_proto(inserted)))
    }

    #[tracing::instrument(skip_all, name = "Schedules::ListSchedules")]
    async fn list_schedules(
        &self,
        request: Request<ListSchedulesRequest>,
    ) -> Result<Response<ListSchedulesResponse>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let rows = self
            .scheduler
            .list_schedules(&PersistWorkareaId(req.workarea_id))
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListSchedulesResponse {
            schedules: rows.into_iter().map(schedule_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Schedules::PauseSchedule")]
    async fn pause_schedule(
        &self,
        request: Request<ProtoScheduleId>,
    ) -> Result<Response<ProtoSchedule>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("schedule id is required"));
        }
        let updated = self
            .scheduler
            .pause_schedule(&PersistScheduleId(req.value))
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(schedule_to_proto(updated)))
    }

    #[tracing::instrument(skip_all, name = "Schedules::DeleteSchedule")]
    async fn delete_schedule(
        &self,
        request: Request<ProtoScheduleId>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("schedule id is required"));
        }
        self.scheduler
            .delete_schedule(&PersistScheduleId(req.value))
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Schedules::GetScheduleHistory")]
    async fn get_schedule_history(
        &self,
        request: Request<ProtoScheduleId>,
    ) -> Result<Response<ScheduleHistoryResponse>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("schedule id is required"));
        }
        let rows = self
            .scheduler
            .get_history(&PersistScheduleId(req.value))
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ScheduleHistoryResponse {
            runs: rows.into_iter().map(schedule_run_to_proto).collect(),
        }))
    }
}

fn schedule_to_proto(s: PersistSchedule) -> ProtoSchedule {
    ProtoSchedule {
        id: s.id.0,
        workarea_id: s.workarea_id.0,
        kind: s.kind,
        interval_seconds: s.interval_seconds,
        expires_at: Some(unix_ms_to_timestamp(s.expires_at)),
        last_run_at: s.last_run_at.map(unix_ms_to_timestamp),
        paused: s.paused,
        prompt: s.prompt,
        agent_kind: s.agent_kind,
        created_at: Some(unix_ms_to_timestamp(s.created_at)),
    }
}

fn schedule_run_to_proto(r: PersistScheduleRun) -> ProtoScheduleRun {
    ProtoScheduleRun {
        id: r.id.0,
        schedule_id: r.schedule_id.0,
        session_id: r.session_id.map(|s| s.0).unwrap_or_default(),
        started_at: Some(unix_ms_to_timestamp(r.started_at)),
        ended_at: r.ended_at.map(unix_ms_to_timestamp),
        terminal_state: r.terminal_state.unwrap_or_default(),
    }
}

fn unix_ms_to_timestamp(ms: i64) -> Timestamp {
    let seconds = ms.div_euclid(1000);
    let nanos = (ms.rem_euclid(1000) as i32) * 1_000_000;
    Timestamp { seconds, nanos }
}
