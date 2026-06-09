//! `smoke-client streams-replay-probe --repo-id <r>` —
//! end-to-end probe of the Task 202 `Streams.Subscribe` reconnect path
//! over the live UDS Core. Self-contained and deterministic:
//!
//! 1. Subscribe to `workspace.events` (this starts the subject's
//!    publish-time pump + ring buffer).
//! 2. Create two workspaces on the same channel → offsets 0 and 1.
//! 3. Drain the two live events; assert their offsets are 0 and 1 (the
//!    publish-time monotonic-offset invariant).
//! 4. Reconnect with `since_offset = 0`; assert the ring replays exactly
//!    offset 1 (the gap the client missed), proving `since_offset`
//!    resume works over the real wire.
//! 5. Ack offset 1, reconnect with `since_offset = 0` again; assert the
//!    pruned ring now yields a single `GapDetected` frame (offset 0 is
//!    below the advanced floor), proving the gap-detection path.
//!
//! Exits 0 on success; on any mismatch prints the discrepancy to stderr
//! and exits 1 (surfaced by the smoke gate).

use std::path::Path;
use std::time::Duration;

use concerto_proto::v1::event::Body as EventBody;
use concerto_proto::v1::streams_client::StreamsClient;
use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{
    AckOffsetRequest, CreateWorkspaceRequest, Event, SubscribeRequest, WorkspaceRepoSpec,
};
use futures::StreamExt;
use tonic::transport::Channel;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

const SUBJECT: &str = "workspace.events";

pub async fn run(socket: &Path, repo_id: &str) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut streams = StreamsClient::new(channel.clone());
    let mut workspaces = WorkspacesClient::new(channel);

    // 1. Subscribe live (starts the pump) BEFORE generating events.
    let mut live = subscribe(&mut streams, None).await?;

    // 2. Create two workspaces → offsets 0, 1.
    for i in 0..2 {
        create_workspace(&mut workspaces, repo_id, &format!("replay-probe-{i}")).await?;
    }

    // 3. Drain the two live workspace events; assert offsets 0, 1.
    let mut offsets = Vec::new();
    while offsets.len() < 2 {
        let ev = next_workspace_event(&mut live, Duration::from_secs(10))
            .await
            .ok_or_else(|| "did not receive expected live workspace events".to_string())?;
        offsets.push(ev.offset);
    }
    if offsets != [0, 1] {
        return Err(format!(
            "expected live offsets [0, 1], got {offsets:?} (publish-time offset invariant broken)"
        ));
    }

    // 4. Reconnect with since_offset = 0 → replay exactly offset 1.
    let mut resub = subscribe(&mut streams, Some(0)).await?;
    let replayed = next_workspace_event(&mut resub, Duration::from_secs(10))
        .await
        .ok_or_else(|| "since_offset=0 reconnect replayed nothing".to_string())?;
    if replayed.offset != 1 {
        return Err(format!(
            "expected replay to start at offset 1, got offset {}",
            replayed.offset
        ));
    }

    // 5. Ack offset 1, then reconnect with since_offset = 0 again → gap.
    //    Ack with both attached subscribers (live, resub) raises the min
    //    watermark to 1, pruning offsets <= 1 and advancing the floor.
    streams
        .ack_offset(AckOffsetRequest {
            subject: SUBJECT.to_string(),
            offset: 1,
        })
        .await
        .map_err(|s| format!("AckOffset rpc error: {s}"))?;

    let mut gap_sub = subscribe(&mut streams, Some(0)).await?;
    let gap_frame = next_event(&mut gap_sub, Duration::from_secs(10))
        .await
        .ok_or_else(|| "since_offset=0 after prune yielded no frame".to_string())?;
    match gap_frame.body {
        Some(EventBody::GapDetected(g)) => {
            if g.subject != SUBJECT {
                return Err(format!(
                    "GapDetected.subject mismatch: expected {SUBJECT}, got {}",
                    g.subject
                ));
            }
        }
        other => {
            return Err(format!("expected GapDetected after prune, got {other:?}"));
        }
    }

    println!("streams-replay-probe: OK (replay + gap verified)");
    Ok(())
}

async fn subscribe(
    client: &mut StreamsClient<Channel>,
    since_offset: Option<u64>,
) -> Result<tonic::Streaming<Event>, String> {
    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.subscribe(SubscribeRequest {
            subject: SUBJECT.to_string(),
            filter: None,
            since_offset,
        }),
    )
    .await
    .map_err(|_| format!("Subscribe timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|s| format!("Subscribe rpc error: {s}"))?;
    Ok(resp.into_inner())
}

async fn create_workspace(
    client: &mut WorkspacesClient<Channel>,
    repo_id: &str,
    name: &str,
) -> Result<(), String> {
    tokio::time::timeout(
        RPC_TIMEOUT,
        client.create_workspace(CreateWorkspaceRequest {
            name: name.to_string(),
            repos: vec![WorkspaceRepoSpec {
                repository_id: repo_id.to_string(),
                sparse_cones: vec![],
            }],
            permission_mode: None,
            description: None,
            icon: None,
        }),
    )
    .await
    .map_err(|_| format!("CreateWorkspace timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|s| format!("CreateWorkspace rpc error: {s}"))?;
    Ok(())
}

/// Next frame of any body within `budget`.
async fn next_event(stream: &mut tonic::Streaming<Event>, budget: Duration) -> Option<Event> {
    match tokio::time::timeout(budget, stream.next()).await {
        Ok(Some(Ok(ev))) => Some(ev),
        _ => None,
    }
}

/// Next `workspace.events`-bodied frame within `budget`.
async fn next_workspace_event(
    stream: &mut tonic::Streaming<Event>,
    budget: Duration,
) -> Option<Event> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let ev = next_event(stream, remaining).await?;
        if matches!(ev.body, Some(EventBody::Workspace(_))) {
            return Some(ev);
        }
    }
}
