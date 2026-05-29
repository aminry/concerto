//! `smoke-client create-loop --workarea <id> --interval <s> --prompt <s>`
//!
//! Calls `Schedules.CreateSchedule` with `kind = "loop"` (Task 38). The
//! smoke gate v3 block asserts the schedule row inserts (id returned to
//! stdout); we deliberately do NOT wait for a fire to land — see
//! `tasks/52-smoke-gate-v3.md` pre-decisions §8. Fire-loop behaviour
//! is exercised by `crates/core/tests/scheduler_loop.rs`.

use std::path::Path;

use concerto_proto::v1::schedules_client::SchedulesClient;
use concerto_proto::v1::CreateScheduleRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(
    socket: &Path,
    workarea_id: &str,
    interval_seconds: i64,
    prompt: &str,
) -> Result<(), String> {
    if workarea_id.is_empty() {
        return Err("create-loop: --workarea must be non-empty".to_string());
    }
    if !(30..=604800).contains(&interval_seconds) {
        return Err(format!(
            "create-loop: --interval must be in 30..=604800 seconds (got {interval_seconds})"
        ));
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = SchedulesClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.create_schedule(CreateScheduleRequest {
            workarea_id: workarea_id.to_string(),
            kind: "loop".to_string(),
            interval_seconds,
            prompt: prompt.to_string(),
            // Scheduler validates `agent_kind ∈ {claude|codex|gemini|maestro}`
            // (Task 38 — `echo` is rejected because the fire loop is
            // meant to spawn real agents). The smoke gate never waits
            // for a fire (see pre-decisions §8), so the row's
            // `agent_kind` value is only used by V1.0's fire-and-spawn
            // path; `"claude"` is the safe default per the proto.
            agent_kind: "claude".to_string(),
            // `0` lets the server default to `now + 3 days`.
            expires_at_unix_ms: 0,
        }),
    )
    .await
    .map_err(|_| format!("CreateSchedule timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("CreateSchedule rpc error: {status}"))?;

    let sched = resp.into_inner();
    println!("{}", sched.id);
    Ok(())
}
