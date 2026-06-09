//! `concerto session ls [--workarea <id>]` — lists sessions as a table.
//!
//! The frozen `Sessions.ListSessions` RPC takes a `workarea_id`. With
//! `--workarea` it lists that workarea's sessions directly. Without it, we
//! walk the read-only resource tree
//! (`Workspaces.ListWorkspaces` → `Workareas.ListWorkareas` →
//! `Sessions.ListSessions`) and union the result, giving a useful global
//! `session ls` without a new RPC.

use std::path::Path;

use concerto_proto::v1::sessions_client::SessionsClient;
use concerto_proto::v1::workareas_client::WorkareasClient;
use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{
    ListSessionsRequest, ListWorkareasRequest, ListWorkspacesRequest, PermissionMode,
};
use serde::Serialize;
use tonic::transport::Channel;

use super::{call, CommandError, OutputFormat};
use crate::client;

/// One session row in the rendered table / JSON array.
#[derive(Debug, Serialize)]
struct SessionRow {
    id: String,
    workarea_id: String,
    agent_kind: String,
    status: String,
    /// `permission_mode` as the proto enum's string name.
    permission_mode: String,
    /// `model` if the agent reported one, else empty.
    model: String,
}

/// Run `concerto session ls`. `workarea` scopes to one workarea when `Some`;
/// otherwise every workarea's sessions are listed.
pub async fn run(
    socket: &Path,
    workarea: Option<String>,
    format: OutputFormat,
) -> Result<(), CommandError> {
    let channel = client::connect(socket).await?;

    let workarea_ids = match workarea {
        Some(id) => vec![id],
        None => discover_all_workareas(&channel).await?,
    };

    let mut sessions = SessionsClient::new(channel);
    let mut rows = Vec::new();
    for wa in workarea_ids {
        let resp = call(
            "Sessions.ListSessions",
            sessions.list_sessions(ListSessionsRequest {
                workarea_id: wa.clone(),
            }),
        )
        .await?;
        for s in resp.sessions {
            rows.push(SessionRow {
                id: s.id,
                workarea_id: s.workarea_id,
                agent_kind: s.agent_kind,
                status: s.status,
                permission_mode: PermissionMode::try_from(s.permission_mode)
                    .map(|m| m.as_str_name().to_string())
                    .unwrap_or_else(|_| format!("UNKNOWN({})", s.permission_mode)),
                model: s.model.unwrap_or_default(),
            });
        }
    }

    render(&rows, format)
}

/// Walk workspaces → workareas to enumerate every workarea id.
async fn discover_all_workareas(channel: &Channel) -> Result<Vec<String>, CommandError> {
    let mut workspaces = WorkspacesClient::new(channel.clone());
    let ws_resp = call(
        "Workspaces.ListWorkspaces",
        workspaces.list_workspaces(ListWorkspacesRequest {
            include_archived: true,
        }),
    )
    .await?;
    let workspace_ids: Vec<String> = ws_resp.workspaces.into_iter().map(|w| w.id).collect();

    let mut workareas = WorkareasClient::new(channel.clone());
    let mut workarea_ids = Vec::new();
    for ws in workspace_ids {
        let wa_resp = call(
            "Workareas.ListWorkareas",
            workareas.list_workareas(ListWorkareasRequest {
                workspace_id: ws,
                include_archived: true,
            }),
        )
        .await?;
        workarea_ids.extend(wa_resp.workareas.into_iter().map(|w| w.id));
    }

    Ok(workarea_ids)
}

fn render(rows: &[SessionRow], format: OutputFormat) -> Result<(), CommandError> {
    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No sessions.");
        return Ok(());
    }

    let id_w = col_width("ID", rows.iter().map(|r| r.id.as_str()));
    let wa_w = col_width("WORKAREA", rows.iter().map(|r| r.workarea_id.as_str()));
    let agent_w = col_width("AGENT", rows.iter().map(|r| r.agent_kind.as_str()));
    let status_w = col_width("STATUS", rows.iter().map(|r| r.status.as_str()));
    let mode_w = col_width("MODE", rows.iter().map(|r| r.permission_mode.as_str()));

    println!(
        "{:<id_w$}  {:<wa_w$}  {:<agent_w$}  {:<status_w$}  {:<mode_w$}  MODEL",
        "ID", "WORKAREA", "AGENT", "STATUS", "MODE"
    );
    for r in rows {
        println!(
            "{:<id_w$}  {:<wa_w$}  {:<agent_w$}  {:<status_w$}  {:<mode_w$}  {}",
            r.id, r.workarea_id, r.agent_kind, r.status, r.permission_mode, r.model,
        );
    }
    Ok(())
}

/// Column width = max(header, widest cell).
fn col_width<'a>(header: &str, cells: impl Iterator<Item = &'a str>) -> usize {
    cells
        .map(|c| c.len())
        .chain(std::iter::once(header.len()))
        .max()
        .unwrap_or(header.len())
}
