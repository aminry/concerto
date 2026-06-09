//! `concerto workspace ls [--include-archived]` — lists workspaces as a table.
//!
//! Workspaces are a global, top-level entity after the Project→Workspace
//! collapse, so `Workspaces.ListWorkspaces` is unscoped; the only knob is
//! `--include-archived`.

use std::path::Path;

use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{ListWorkspacesRequest, PermissionMode};
use serde::Serialize;

use super::{call, CommandError, OutputFormat};
use crate::client;

/// One workspace row in the rendered table / JSON array.
#[derive(Debug, Serialize)]
struct WorkspaceRow {
    id: String,
    name: String,
    slug: String,
    /// `permission_mode` as the proto enum's string name, or `(default)`
    /// when the workspace inherits.
    permission_mode: String,
    /// `true` when `archived_at` is set.
    archived: bool,
}

/// Run `concerto workspace ls`. Lists every workspace; archived workspaces
/// are included only when `include_archived` is set.
pub async fn run(
    socket: &Path,
    include_archived: bool,
    format: OutputFormat,
) -> Result<(), CommandError> {
    let channel = client::connect(socket).await?;

    let mut workspaces = WorkspacesClient::new(channel);
    let resp = call(
        "Workspaces.ListWorkspaces",
        workspaces.list_workspaces(ListWorkspacesRequest { include_archived }),
    )
    .await?;
    let rows: Vec<WorkspaceRow> = resp
        .workspaces
        .into_iter()
        .map(|ws| WorkspaceRow {
            id: ws.id,
            name: ws.name,
            slug: ws.slug,
            permission_mode: render_permission_mode(ws.permission_mode),
            archived: ws.archived_at.is_some(),
        })
        .collect();

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
