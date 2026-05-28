//! `smoke-client caps` — calls `Runtime.GetServerCapabilities` and
//! prints a one-line JSON object whose `transport_kind` field is the
//! proto enum's string name. The smoke script's Phase 1 block greps
//! for `"TRANSPORT_KIND_UDS"` in that output.

use std::path::Path;

use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::TransportKind;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut client = RuntimeClient::new(channel);

    let resp = tokio::time::timeout(RPC_TIMEOUT, client.get_server_capabilities(()))
        .await
        .map_err(|_| format!("GetServerCapabilities timed out after {RPC_TIMEOUT:?}"))?
        .map_err(|status| format!("GetServerCapabilities rpc error: {status}"))?;

    let caps = resp.into_inner();

    // Render `transport_kind` as the proto enum's string name so the
    // smoke script can grep for `"TRANSPORT_KIND_UDS"`.
    let transport_kind_str = TransportKind::try_from(caps.transport_kind)
        .map(|k| k.as_str_name().to_string())
        .unwrap_or_else(|_| format!("UNKNOWN({})", caps.transport_kind));

    let out = serde_json::json!({
        "server_version": caps.server_version,
        "schema_version": caps.schema_version,
        "transport_kind": transport_kind_str,
        "core_host_os": caps.core_host_os,
        "core_hostname": caps.core_hostname,
        "limits": caps.limits.map(|l| serde_json::json!({
            "max_concurrent_streams": l.max_concurrent_streams,
            "max_payload_bytes": l.max_payload_bytes,
        })),
    });

    println!("{out}");
    Ok(())
}
