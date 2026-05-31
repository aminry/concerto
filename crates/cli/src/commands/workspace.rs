//! `concerto workspace ls [--project <id>]` — lists workspaces as a table.
//!
//! The frozen `Workspaces.ListWorkspaces` RPC takes a `project_id`, so when
//! the caller doesn't pass `--project` we enumerate every project via
//! `Projects.ListProjects` and union their workspaces — giving a useful
//! cross-project `workspace ls` without inventing a new RPC.

use std::path::Path;

use concerto_proto::v1::projects_client::ProjectsClient;
use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{ListProjectsRequest, ListWorkspacesRequest, PermissionMode};
use serde::Serialize;

use super::{call, CommandError, OutputFormat};
use crate::client;

/// One workspace row in the rendered table / JSON array.
#[derive(Debug, Serialize)]
struct WorkspaceRow {
    id: String,
    project_id: String,
    name: String,
    slug: String,
    /// `permission_mode` as the proto enum's string name, or `(default)`
    /// when the workspace inherits.
    permission_mode: String,
    /// `true` when `archived_at` is set.
    archived: bool,
}

/// Run `concerto workspace ls`. `project` filters to a single project when
/// `Some`; otherwise every project's workspaces are listed.
pub async fn run(
    socket: &Path,
    project: Option<String>,
    format: OutputFormat,
) -> Result<(), CommandError> {
    let channel = client::connect(socket).await?;

    let project_ids = match project {
        Some(id) => vec![id],
        None => {
            let mut projects = ProjectsClient::new(channel.clone());
            let resp = call(
                "Projects.ListProjects",
                projects.list_projects(ListProjectsRequest {}),
            )
            .await?;
            resp.projects.into_iter().map(|p| p.id).collect()
        }
    };

    let mut workspaces = WorkspacesClient::new(channel);
    let mut rows = Vec::new();
    for pid in project_ids {
        let resp = call(
            "Workspaces.ListWorkspaces",
            workspaces.list_workspaces(ListWorkspacesRequest {
                project_id: pid.clone(),
            }),
        )
        .await?;
        for ws in resp.workspaces {
            rows.push(WorkspaceRow {
                id: ws.id,
                project_id: ws.project_id,
                name: ws.name,
                slug: ws.slug,
                permission_mode: render_permission_mode(ws.permission_mode),
                archived: ws.archived_at.is_some(),
            });
        }
    }

    render(&rows, format)
}

/// Render an optional `permission_mode` enum int as a friendly string.
fn render_permission_mode(mode: Option<i32>) -> String {
    match mode {
        None => "(default)".to_string(),
        Some(value) => PermissionMode::try_from(value)
            .map(|m| m.as_str_name().to_string())
            .unwrap_or_else(|_| format!("UNKNOWN({value})")),
    }
}

fn render(rows: &[WorkspaceRow], format: OutputFormat) -> Result<(), CommandError> {
    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No workspaces.");
        return Ok(());
    }

    // Width the columns to their content so the table stays aligned.
    let id_w = col_width("ID", rows.iter().map(|r| r.id.as_str()));
    let name_w = col_width("NAME", rows.iter().map(|r| r.name.as_str()));
    let slug_w = col_width("SLUG", rows.iter().map(|r| r.slug.as_str()));
    let mode_w = col_width("MODE", rows.iter().map(|r| r.permission_mode.as_str()));

    println!(
        "{:<id_w$}  {:<name_w$}  {:<slug_w$}  {:<mode_w$}  ARCHIVED",
        "ID", "NAME", "SLUG", "MODE"
    );
    for r in rows {
        println!(
            "{:<id_w$}  {:<name_w$}  {:<slug_w$}  {:<mode_w$}  {}",
            r.id,
            r.name,
            r.slug,
            r.permission_mode,
            if r.archived { "yes" } else { "no" },
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
