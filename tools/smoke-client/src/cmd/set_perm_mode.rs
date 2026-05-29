//! `smoke-client set-perm-mode --workarea <id> --mode <s> [--ack <s>]`
//!
//! Calls `Workareas.UpdateWorkareaPermissionMode` to flip a workarea's
//! permission mode at runtime (Task 32 / Task 42). The Phase 3 smoke
//! block exercises the `auto` mode wiring end-to-end; `yolo` is
//! exercised manually in the `permission_runtime` integration test
//! (the smoke gate doesn't fake the "I understand" entry ceremony for
//! the destructive cases because doing so risks normalising the
//! ceremony's purpose).
//!
//! Mode strings accepted: `strict | normal | auto | yolo`. Anything
//! else errors locally before the RPC.

use std::path::Path;

use concerto_proto::v1::workareas_client::WorkareasClient;
use concerto_proto::v1::{PermissionMode, UpdateWorkareaPermissionModeRequest};

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(
    socket: &Path,
    workarea_id: &str,
    mode: &str,
    acknowledgement: Option<&str>,
) -> Result<(), String> {
    if workarea_id.is_empty() {
        return Err("set-perm-mode: --workarea must be non-empty".to_string());
    }
    let mode_enum = match mode {
        "strict" => PermissionMode::Strict,
        "normal" => PermissionMode::Normal,
        "auto" => PermissionMode::Auto,
        "yolo" => PermissionMode::Yolo,
        other => {
            return Err(format!(
                "set-perm-mode: unknown --mode {other:?} (expected strict|normal|auto|yolo)"
            ))
        }
    };

    let channel = connect_to_socket(socket).await?;
    let mut client = WorkareasClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.update_workarea_permission_mode(UpdateWorkareaPermissionModeRequest {
            workarea_id: workarea_id.to_string(),
            permission_mode: mode_enum as i32,
            acknowledgement: acknowledgement.unwrap_or("").to_string(),
        }),
    )
    .await
    .map_err(|_| format!("UpdateWorkareaPermissionMode timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("UpdateWorkareaPermissionMode rpc error: {status}"))?;

    let wa = resp.into_inner();
    // Emit the resulting effective-mode field (string) so the smoke
    // script can grep for the post-change value.
    let mode_str = PermissionMode::try_from(wa.permission_mode.unwrap_or(0))
        .map(|m| m.as_str_name().to_string())
        .unwrap_or_else(|_| "PERMISSION_MODE_UNSPECIFIED".to_string());
    println!("{mode_str}");
    Ok(())
}
