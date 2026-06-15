//! gRPC `Notifications` service handler (Task 507) — a thin delegator over
//! [`NotificationHandle`] (design/14 §5.2). Registered at both front-door sites
//! (`add_core_services` + `connect_bridge`, D9) by the boot wiring (507b-3).
//!
//! `ActOnChip` records the first-wins marker + returns the resolved
//! [`ChipDispatch`] (resolve-approval / send-message / navigate). The Core-side
//! EXECUTION of an approval/message dispatch against the agent supervisor is the
//! boot-wired step (507b-3 handoff); a client may also drive it from the
//! returned `dispatch_kind`/`dispatch_arg` + the notification's
//! `ToolApprovalContext.approval_id`.

use async_trait::async_trait;
use tonic::{Request, Response, Status};

use concerto_proto::v1::notifications_server::Notifications as NotificationsService;
use concerto_proto::v1::{
    ActOnChipRequest, ActOnChipResponse, GetNotificationRequest, InboxFilter, InboxResponse,
    MarkReadRequest, Notification, UpdateWorkspaceNotifyRequest,
};

use crate::error_map::error_to_status;
use crate::notifications::chip_dispatch::ChipDispatch;
use crate::notifications::handle::NotificationHandle;

/// Implements the generated `Notifications` service trait over a
/// [`NotificationHandle`].
#[derive(Clone)]
pub struct NotificationsHandler {
    handle: NotificationHandle,
}

impl NotificationsHandler {
    pub fn new(handle: NotificationHandle) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl NotificationsService for NotificationsHandler {
    #[tracing::instrument(skip_all, name = "Notifications::GetInbox")]
    async fn get_inbox(
        &self,
        request: Request<InboxFilter>,
    ) -> Result<Response<InboxResponse>, Status> {
        let f = request.into_inner();
        let notifications = self
            .handle
            .get_inbox(
                f.workspace_id.as_deref(),
                f.workarea_id.as_deref(),
                f.unread_only,
                f.limit,
            )
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(InboxResponse { notifications }))
    }

    #[tracing::instrument(skip_all, name = "Notifications::GetNotification")]
    async fn get_notification(
        &self,
        request: Request<GetNotificationRequest>,
    ) -> Result<Response<Notification>, Status> {
        let r = request.into_inner();
        if r.id.is_empty() {
            return Err(Status::invalid_argument("id is required"));
        }
        let n = self
            .handle
            .get_notification(&r.id, &r.device_id)
            .await
            .map_err(error_to_status)?
            .ok_or_else(|| Status::not_found(format!("notification.unknown: {}", r.id)))?;
        Ok(Response::new(n))
    }

    #[tracing::instrument(skip_all, name = "Notifications::MarkRead")]
    async fn mark_read(&self, request: Request<MarkReadRequest>) -> Result<Response<()>, Status> {
        let r = request.into_inner();
        if r.id.is_empty() {
            return Err(Status::invalid_argument("id is required"));
        }
        self.handle
            .mark_read(&r.id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Notifications::ActOnChip")]
    async fn act_on_chip(
        &self,
        request: Request<ActOnChipRequest>,
    ) -> Result<Response<ActOnChipResponse>, Status> {
        let r = request.into_inner();
        if r.notification_id.is_empty() || r.chip_id.is_empty() {
            return Err(Status::invalid_argument(
                "notification_id and chip_id are required",
            ));
        }
        let outcome = self
            .handle
            .act_on_chip(&r.notification_id, &r.chip_id, &r.device_id)
            .await
            .map_err(error_to_status)?;
        let (dispatch_kind, dispatch_arg) = match outcome.dispatch {
            ChipDispatch::ResolveApproval { decision } => {
                ("resolve_approval".to_string(), decision)
            }
            ChipDispatch::SendMessage { prompt } => ("send_message".to_string(), prompt),
            ChipDispatch::Navigate { target } => ("navigate".to_string(), target),
        };
        Ok(Response::new(ActOnChipResponse {
            already_resolved: outcome.already_resolved,
            dispatch_kind,
            dispatch_arg,
        }))
    }

    #[tracing::instrument(skip_all, name = "Notifications::UpdateWorkspaceSettings")]
    async fn update_workspace_settings(
        &self,
        request: Request<UpdateWorkspaceNotifyRequest>,
    ) -> Result<Response<()>, Status> {
        let r = request.into_inner();
        if r.workspace_id.is_empty() {
            return Err(Status::invalid_argument("workspace_id is required"));
        }
        self.handle
            .set_workspace_opt_out(&r.workspace_id, r.opt_out)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }
}
