//! gRPC `Workareas` service handler (Task 20).
//!
//! Translates `concerto.v1.Workareas` requests into calls against
//! [`crate::workspace_manager::WorkareaManager`]. V0.1 surface:
//!
//! - `CreateWorkarea` — allocates composer name, sets up worktree +
//!   `.context/`, persists rows, transitions `created → active`.
//! - `GetWorkarea` — returns the persisted row.
//! - `ListWorkareas` — scoped by workspace; `include_archived` knob.
//! - `ArchiveWorkarea` — idempotent UPDATE (status → archived, sets
//!   `archived_at`).

use std::pin::Pin;

use async_trait::async_trait;
use concerto_persist::{
    RepositoryId as PersistRepositoryId, WorkareaId as PersistWorkareaId,
    WorkspaceId as PersistWorkspaceId,
};
use concerto_proto::v1::workareas_server::Workareas as WorkareasService;
use concerto_proto::v1::{
    merge_progress, ArchiveWorkareaRequest, CreateWorkareaRequest, DiffHunk as ProtoDiffHunk,
    DiffKind as ProtoDiffKind, DiffPayload as ProtoDiffPayload, FailureKind as ProtoFailureKind,
    FileDiff as ProtoFileDiff, GetDiffRequest, GetWorkareaPrSetResponse, ListWorkareasRequest,
    ListWorkareasResponse, MergePlan as ProtoMergePlan, MergeProgress as ProtoMergeProgress,
    MergeSetMerged, MergeSetPaused, MergeStep as ProtoMergeStep, MergeStepCompleted,
    MergeStepFailed, MergeStepStarted, MergeWorkareaPrSetRequest, PermissionMode,
    PullRequest as ProtoPullRequest, RevertOutcome as ProtoRevertOutcome,
    RevertReport as ProtoRevertReport, RevertStep as ProtoRevertStep, RevertWorkareaPrSetRequest,
    SetMergeOrderRequest, SetWorkareaBypassDestructiveGuardRequest,
    UpdateWorkareaPermissionModeRequest, Workarea as ProtoWorkarea, WorkareaId as ProtoWorkareaId,
};
use concerto_vcs::provider::MergeMethod;
use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::workspace_manager::{
    ArchiveOpts, FailureKind, MergeOpts, MergePlan, MergeProgress, MergeStep, RevertOpts,
    RevertOutcome, RevertReport, RevertStep, WorkareaManager, DEFAULT_MERGE_CHECK_TIMEOUT,
};

/// Implements the generated `Workareas` service trait.
#[derive(Clone)]
pub struct WorkareasHandler {
    workarea_manager: WorkareaManager,
}

impl WorkareasHandler {
    pub fn new(workarea_manager: WorkareaManager) -> Self {
        Self { workarea_manager }
    }
}

#[async_trait]
impl WorkareasService for WorkareasHandler {
    #[tracing::instrument(skip_all, name = "Workareas::CreateWorkarea")]
    async fn create_workarea(
        &self,
        request: Request<CreateWorkareaRequest>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        let permission_mode = req
            .permission_mode
            .map(permission_mode_from_i32)
            .transpose()?;
        let row = self
            .workarea_manager
            .create_workarea(&req.workspace_id, permission_mode)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workarea_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::GetWorkarea")]
    async fn get_workarea(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        match self
            .workarea_manager
            .get(&id)
            .await
            .map_err(error_to_status)?
        {
            Some(wa) => Ok(Response::new(workarea_to_proto(wa))),
            None => Err(Status::not_found(format!("workarea {id} not found"))),
        }
    }

    #[tracing::instrument(skip_all, name = "Workareas::ListWorkareas")]
    async fn list_workareas(
        &self,
        request: Request<ListWorkareasRequest>,
    ) -> Result<Response<ListWorkareasResponse>, Status> {
        let req = request.into_inner();
        if req.workspace_id.is_empty() {
            return Err(Status::invalid_argument("workspace_id is required"));
        }
        let ws_id = PersistWorkspaceId(req.workspace_id);
        let rows = self
            .workarea_manager
            .list_by_workspace(&ws_id, req.include_archived)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListWorkareasResponse {
            workareas: rows.into_iter().map(workarea_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Workareas::GetWorkareaRepoDiff")]
    async fn get_workarea_repo_diff(
        &self,
        request: Request<GetDiffRequest>,
    ) -> Result<Response<ProtoDiffPayload>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        let wa_id = PersistWorkareaId(req.workarea_id);
        let repo_id = PersistRepositoryId(req.repository_id);
        let payload = self
            .workarea_manager
            .get_repo_diff(&wa_id, &repo_id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(diff_payload_to_proto(payload)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::ArchiveWorkarea")]
    async fn archive_workarea(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        self.workarea_manager
            .archive(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Workareas::ArchiveWorkareaWithOpts")]
    async fn archive_workarea_with_opts(
        &self,
        request: Request<ArchiveWorkareaRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let id = PersistWorkareaId(req.workarea_id);
        let opts = ArchiveOpts {
            remove_worktree: req.remove_worktree,
        };
        self.workarea_manager
            .archive_workarea(&id, opts)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Workareas::RestoreWorkarea")]
    async fn restore_workarea(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        let row = self
            .workarea_manager
            .restore_workarea(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workarea_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::UpdateWorkareaPermissionMode")]
    async fn update_workarea_permission_mode(
        &self,
        request: Request<UpdateWorkareaPermissionModeRequest>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        // V0.1: `PERMISSION_MODE_UNSPECIFIED` clears the override
        // (inherit-from-workspace) — same convention as the workarea
        // settings_json field shape. The proto wire is a non-optional
        // enum so we map UNSPECIFIED → None.
        let mode: Option<String> = match PermissionMode::try_from(req.permission_mode) {
            Ok(PermissionMode::Unspecified) => None,
            Ok(PermissionMode::Strict) => Some("strict".to_string()),
            Ok(PermissionMode::Normal) => Some("normal".to_string()),
            Ok(PermissionMode::Auto) => Some("auto".to_string()),
            Ok(PermissionMode::Yolo) => Some("yolo".to_string()),
            Err(_) => {
                return Err(Status::invalid_argument(format!(
                    "permission_mode {} is not a known enum value",
                    req.permission_mode
                )));
            }
        };
        let id = PersistWorkareaId(req.workarea_id);
        let row = self
            .workarea_manager
            .update_workarea_permission_mode(&id, mode.as_deref(), &req.acknowledgement)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workarea_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::GetWorkareaPrSet")]
    async fn get_workarea_pr_set(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<GetWorkareaPrSetResponse>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        let rows = self
            .workarea_manager
            .list_pr_set(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(GetWorkareaPrSetResponse {
            pull_requests: rows.into_iter().map(pull_request_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Workareas::SetMergeOrder")]
    async fn set_merge_order(
        &self,
        request: Request<SetMergeOrderRequest>,
    ) -> Result<Response<GetWorkareaPrSetResponse>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        let workarea_id = PersistWorkareaId(req.workarea_id);
        let repository_id = PersistRepositoryId(req.repository_id);
        let rows = self
            .workarea_manager
            .set_merge_order(&workarea_id, &repository_id, req.merge_order)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(GetWorkareaPrSetResponse {
            pull_requests: rows.into_iter().map(pull_request_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Workareas::SetWorkareaBypassDestructiveGuard")]
    async fn set_workarea_bypass_destructive_guard(
        &self,
        request: Request<SetWorkareaBypassDestructiveGuardRequest>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let id = PersistWorkareaId(req.workarea_id);
        let row = self
            .workarea_manager
            .set_workarea_bypass_destructive_guard(&id, req.enable, &req.acknowledgement)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workarea_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::GetWorkareaMergePlan")]
    async fn get_workarea_merge_plan(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<ProtoMergePlan>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        let plan = self
            .workarea_manager
            .get_workarea_merge_plan(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(merge_plan_to_proto(plan)))
    }

    /// Streaming response type for `Workareas.MergeWorkareaPrSet`.
    type MergeWorkareaPrSetStream =
        Pin<Box<dyn Stream<Item = Result<ProtoMergeProgress, Status>> + Send + 'static>>;

    #[tracing::instrument(skip_all, name = "Workareas::MergeWorkareaPrSet")]
    async fn merge_workarea_pr_set(
        &self,
        request: Request<MergeWorkareaPrSetRequest>,
    ) -> Result<Response<Self::MergeWorkareaPrSetStream>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let method = MergeMethod::parse(&req.method).map_err(error_to_status)?;
        let timeout = if req.timeout_secs == 0 {
            DEFAULT_MERGE_CHECK_TIMEOUT
        } else {
            std::time::Duration::from_secs(req.timeout_secs)
        };
        let opts = MergeOpts {
            method,
            timeout,
            allow_failing_checks: req.allow_failing_checks,
        };
        let id = PersistWorkareaId(req.workarea_id);

        // Mirror the `Repositories.Clone` streaming handler: the merge loop runs
        // on a spawned task that owns the `mpsc::Sender<MergeProgress>`; the
        // handler returns the `ReceiverStream` immediately so the client sees
        // frames live. A terminal `Err` (e.g. policy.locked, NotFound) is
        // forwarded as the last stream item.
        let (out_tx, out_rx) = mpsc::channel::<Result<ProtoMergeProgress, Status>>(32);
        let (ev_tx, mut ev_rx) = mpsc::channel::<MergeProgress>(32);

        // Forwarder: reshape each domain `MergeProgress` into the proto frame.
        let out_tx_for_forward = out_tx.clone();
        let forward_handle = tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                if out_tx_for_forward
                    .send(Ok(merge_progress_to_proto(ev)))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        // Worker: drive the coordinated merge loop. Closing `ev_tx` (by drop)
        // ends the forwarder; an early `Err` is sent on the outbound channel.
        let manager = self.workarea_manager.clone();
        let out_tx_for_worker = out_tx;
        tokio::spawn(async move {
            let result = manager.merge_workarea_pr_set(&id, opts, ev_tx).await;
            let _ = forward_handle.await;
            if let Err(err) = result {
                let _ = out_tx_for_worker.send(Err(error_to_status(err))).await;
            }
            // Dropping `out_tx_for_worker` terminates the client stream.
        });

        let stream: Self::MergeWorkareaPrSetStream = Box::pin(ReceiverStream::new(out_rx));
        Ok(Response::new(stream))
    }

    #[tracing::instrument(skip_all, name = "Workareas::RevertWorkareaPrSet")]
    async fn revert_workarea_pr_set(
        &self,
        request: Request<RevertWorkareaPrSetRequest>,
    ) -> Result<Response<ProtoRevertReport>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let id = PersistWorkareaId(req.workarea_id);
        let opts = RevertOpts {
            hard_reset: req.hard_reset,
        };
        let report = self
            .workarea_manager
            .revert_workarea_pr_set(&id, opts)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(revert_report_to_proto(report)))
    }
}

fn merge_plan_to_proto(plan: MergePlan) -> ProtoMergePlan {
    ProtoMergePlan {
        workarea_id: plan.workarea_id,
        steps: plan.steps.into_iter().map(merge_step_to_proto).collect(),
    }
}

fn merge_step_to_proto(s: MergeStep) -> ProtoMergeStep {
    ProtoMergeStep {
        step: s.step,
        total: s.total,
        repository_id: s.repository_id,
        repository_full_name: s.repository_full_name,
        pr_number: s.pr_number,
        head_sha: s.head_sha,
        merge_order: s.merge_order,
        state: s.state,
    }
}

fn failure_kind_to_proto(k: FailureKind) -> ProtoFailureKind {
    match k {
        FailureKind::ChecksFailed => ProtoFailureKind::ChecksFailed,
        FailureKind::ChecksTimeout => ProtoFailureKind::ChecksTimeout,
        FailureKind::MergeConflict => ProtoFailureKind::MergeConflict,
        FailureKind::MergeRejected => ProtoFailureKind::MergeRejected,
    }
}

fn merge_progress_to_proto(p: MergeProgress) -> ProtoMergeProgress {
    let event = match p {
        MergeProgress::StepStarted {
            step,
            total,
            repository_full_name,
            pr_number,
        } => merge_progress::Event::StepStarted(MergeStepStarted {
            step,
            total,
            repository_full_name,
            pr_number,
        }),
        MergeProgress::StepCompleted {
            step,
            total,
            merge_sha,
        } => merge_progress::Event::StepCompleted(MergeStepCompleted {
            step,
            total,
            merge_sha,
        }),
        MergeProgress::StepFailed {
            step,
            total,
            reason,
            kind,
        } => merge_progress::Event::StepFailed(MergeStepFailed {
            step,
            total,
            reason,
            kind: failure_kind_to_proto(kind) as i32,
        }),
        MergeProgress::SetMerged { total } => {
            merge_progress::Event::SetMerged(MergeSetMerged { total })
        }
        MergeProgress::SetPaused {
            paused_at_step,
            total,
            reason,
        } => merge_progress::Event::SetPaused(MergeSetPaused {
            paused_at_step,
            total,
            reason,
        }),
    };
    ProtoMergeProgress { event: Some(event) }
}

fn revert_outcome_to_proto(o: RevertOutcome) -> ProtoRevertOutcome {
    match o {
        RevertOutcome::Reverted => ProtoRevertOutcome::Reverted,
        RevertOutcome::Skipped => ProtoRevertOutcome::Skipped,
        RevertOutcome::Failed => ProtoRevertOutcome::Failed,
    }
}

fn revert_step_to_proto(s: RevertStep) -> ProtoRevertStep {
    ProtoRevertStep {
        repository_full_name: s.repository_full_name,
        pr_number: s.pr_number,
        outcome: revert_outcome_to_proto(s.outcome) as i32,
        detail: s.detail,
    }
}

fn revert_report_to_proto(r: RevertReport) -> ProtoRevertReport {
    ProtoRevertReport {
        workarea_id: r.workarea_id,
        steps: r.steps.into_iter().map(revert_step_to_proto).collect(),
    }
}

fn workarea_to_proto(row: concerto_persist::Workarea) -> ProtoWorkarea {
    ProtoWorkarea {
        id: row.id.to_string(),
        workspace_id: row.workspace_id.to_string(),
        composer_name: row.composer_name,
        branch_name: row.branch_name,
        worktree_root: row.worktree_root,
        status: row.status,
        permission_mode: row.permission_mode.as_deref().map(permission_mode_to_i32),
        created_at: Some(epoch_ms_to_ts(row.created_at)),
        last_activity_at: row.last_activity_at.map(epoch_ms_to_ts),
        archived_at: row.archived_at.map(epoch_ms_to_ts),
    }
}

fn epoch_ms_to_ts(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
    }
}

#[allow(clippy::result_large_err)]
fn permission_mode_from_i32(v: i32) -> Result<String, Status> {
    let pm = PermissionMode::try_from(v).map_err(|_| {
        Status::invalid_argument(format!("permission_mode {v} is not a known enum value"))
    })?;
    match pm {
        PermissionMode::Unspecified => Err(Status::invalid_argument(
            "permission_mode must be one of STRICT|NORMAL|AUTO|YOLO",
        )),
        PermissionMode::Strict => Ok("strict".to_string()),
        PermissionMode::Normal => Ok("normal".to_string()),
        PermissionMode::Auto => Ok("auto".to_string()),
        PermissionMode::Yolo => Ok("yolo".to_string()),
    }
}

/// Convert the Rust [`concerto_gix_wrap::DiffPayload`] into its proto
/// equivalent. The two types intentionally mirror each other one-to-one;
/// the conversion is a flat field-by-field copy plus enum mapping.
fn diff_payload_to_proto(p: concerto_gix_wrap::DiffPayload) -> ProtoDiffPayload {
    ProtoDiffPayload {
        files: p.files.into_iter().map(file_diff_to_proto).collect(),
    }
}

fn file_diff_to_proto(f: concerto_gix_wrap::FileDiff) -> ProtoFileDiff {
    let path = path_to_string(&f.path);
    let old_path = f.old_path.as_deref().map(path_to_string);
    ProtoFileDiff {
        path,
        kind: diff_kind_to_i32(&f.kind),
        old_path,
        hunks: f.hunks.into_iter().map(diff_hunk_to_proto).collect(),
    }
}

fn diff_hunk_to_proto(h: concerto_gix_wrap::DiffHunk) -> ProtoDiffHunk {
    ProtoDiffHunk {
        old_start: h.old_start,
        old_lines: h.old_lines,
        new_start: h.new_start,
        new_lines: h.new_lines,
        body: h.body,
    }
}

fn diff_kind_to_i32(k: &concerto_gix_wrap::DiffKind) -> i32 {
    match k {
        concerto_gix_wrap::DiffKind::Added => ProtoDiffKind::Added as i32,
        concerto_gix_wrap::DiffKind::Deleted => ProtoDiffKind::Deleted as i32,
        concerto_gix_wrap::DiffKind::Modified => ProtoDiffKind::Modified as i32,
        concerto_gix_wrap::DiffKind::Renamed => ProtoDiffKind::Renamed as i32,
    }
}

fn path_to_string(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

fn pull_request_to_proto(row: concerto_persist::PullRequest) -> ProtoPullRequest {
    ProtoPullRequest {
        id: row.id.to_string(),
        workarea_id: row.workarea_id.to_string(),
        repository_id: row.repository_id.to_string(),
        provider: row.provider,
        pr_number: row.pr_number,
        base_ref: row.base_ref,
        head_ref: row.head_ref,
        state: row.state,
        title: row.title,
        body: row.body,
        url: row.url,
        head_sha: row.head_sha,
        merge_order: row.merge_order,
        external_id: row.external_id,
        repository_full_name: row.repository_full_name,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn permission_mode_to_i32(s: &str) -> i32 {
    match s {
        "strict" => PermissionMode::Strict as i32,
        "normal" => PermissionMode::Normal as i32,
        "auto" => PermissionMode::Auto as i32,
        "yolo" => PermissionMode::Yolo as i32,
        _ => PermissionMode::Unspecified as i32,
    }
}
