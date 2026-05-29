//! `smoke-client list-loops --workarea <id>`
//!
//! Calls `Schedules.ListSchedules` and prints one schedule id per line.
//! The smoke gate v3 block uses the output to assert that a freshly-
//! created loop appears in the listing (round-trip on the Schedules
//! surface).

use std::path::Path;

use concerto_proto::v1::schedules_client::SchedulesClient;
use concerto_proto::v1::ListSchedulesRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, workarea_id: &str) -> Result<(), String> {
    if workarea_id.is_empty() {
        return Err("list-loops: --workarea must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = SchedulesClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.list_schedules(ListSchedulesRequest {
            workarea_id: workarea_id.to_string(),
        }),
    )
    .await
    .map_err(|_| format!("ListSchedules timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("ListSchedules rpc error: {status}"))?;

    for sched in resp.into_inner().schedules {
        println!("{}", sched.id);
    }
    Ok(())
}
