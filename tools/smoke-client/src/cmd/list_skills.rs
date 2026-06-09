//! `smoke-client list-skills [--scope <s>] [--workspace-id <s>]`
//!
//! Calls `Skills.RefreshMarketplaces` (to pick up filesystem changes
//! that may have happened after Core boot — the smoke gate writes a
//! fake `SKILL.md` under `$HOME/.claude/skills/` and then calls this
//! subcommand) and then `Skills.ListSkills`. Prints one `name` per
//! line so the smoke script can grep for the fixture entry.

use std::path::Path;

use concerto_proto::v1::skills_client::SkillsClient;
use concerto_proto::v1::{ListSkillsRequest, RefreshMarketplacesRequest};

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(
    socket: &Path,
    scope: Option<&str>,
    workspace_id: Option<&str>,
) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut client = SkillsClient::new(channel);

    // Re-scan first — boot-time discovery may have run before the
    // smoke gate planted its fixture SKILL.md.
    tokio::time::timeout(
        RPC_TIMEOUT,
        client.refresh_marketplaces(RefreshMarketplacesRequest {
            workspace_id: workspace_id.map(|s| s.to_string()),
        }),
    )
    .await
    .map_err(|_| format!("RefreshMarketplaces timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("RefreshMarketplaces rpc error: {status}"))?;

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.list_skills(ListSkillsRequest {
            scope: scope.map(|s| s.to_string()),
            workspace_id: workspace_id.map(|s| s.to_string()),
            enabled_only: None,
        }),
    )
    .await
    .map_err(|_| format!("ListSkills timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("ListSkills rpc error: {status}"))?;

    for skill in resp.into_inner().skills {
        println!("{}", skill.name);
    }
    Ok(())
}
