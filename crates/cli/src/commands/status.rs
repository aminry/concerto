//! `concerto status` — calls `Runtime.GetServerCapabilities` + `GetStatus`
//! and prints the Core's version, uptime, transport kind, and the services
//! it advertises.
//!
//! NOTE ON `actors`: Task 109's prose asked for an `actors` field, but the
//! frozen `runtime.proto` (`ServerCapabilities` / `RuntimeStatus`) has no
//! such field — the supervision-tree actor roster is not exposed over the
//! wire in V0.1. The closest advertised facet is
//! `ServerCapabilities.optional_services` (the optional gRPC services the
//! Core has enabled), which this command surfaces as `services`. See the
//! task's Handoff Notes for the drift record.

use std::path::Path;

use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::TransportKind;
use serde::Serialize;

use super::{call, CommandError, OutputFormat};
use crate::client;

/// Flattened, serde-serializable view of the two runtime RPCs. Built once,
/// then rendered as either a table or JSON — keeping `--json` a thin switch.
#[derive(Debug, Serialize)]
struct StatusView {
    /// `RuntimeStatus.version` (falls back to the capabilities' server
    /// version if status reports an empty string).
    version: String,
    /// `RuntimeStatus.uptime_seconds`.
    uptime_seconds: u64,
    /// `ServerCapabilities.transport_kind` as the proto enum's string name
    /// (e.g. `TRANSPORT_KIND_UDS`).
    transport_kind: String,
    /// `ServerCapabilities.schema_version`.
    schema_version: String,
    /// `ServerCapabilities.core_host_os`.
    core_host_os: String,
    /// `ServerCapabilities.core_hostname`.
    core_hostname: String,
    /// `ServerCapabilities.optional_services` — the optional gRPC services
    /// the Core advertises. Stands in for the requested-but-unexposed
    /// `actors` roster.
    services: Vec<String>,
}

/// Run `concerto status` against the Core at `socket`.
pub async fn run(socket: &Path, format: OutputFormat) -> Result<(), CommandError> {
    let channel = client::connect(socket).await?;
    let mut runtime = RuntimeClient::new(channel);

    let caps = call(
        "Runtime.GetServerCapabilities",
        runtime.get_server_capabilities(()),
    )
    .await?;
    let status = call("Runtime.GetStatus", runtime.get_status(())).await?;

    let transport_kind = TransportKind::try_from(caps.transport_kind)
        .map(|k| k.as_str_name().to_string())
        .unwrap_or_else(|_| format!("UNKNOWN({})", caps.transport_kind));

    let version = if status.version.is_empty() {
        caps.server_version.clone()
    } else {
        status.version.clone()
    };

    let view = StatusView {
        version,
        uptime_seconds: status.uptime_seconds,
        transport_kind,
        schema_version: caps.schema_version,
        core_host_os: caps.core_host_os,
        core_hostname: caps.core_hostname,
        services: caps.optional_services,
    };

    render(&view, format)
}

fn render(view: &StatusView, format: OutputFormat) -> Result<(), CommandError> {
    if format.is_json() {
        println!("{}", serde_json::to_string_pretty(view)?);
        return Ok(());
    }

    println!("version:        {}", view.version);
    println!("uptime:         {}", format_uptime(view.uptime_seconds));
    println!("transport:      {}", view.transport_kind);
    println!("schema version: {}", view.schema_version);
    println!("host os:        {}", view.core_host_os);
    println!("hostname:       {}", view.core_hostname);
    if view.services.is_empty() {
        println!("services:       (none)");
    } else {
        println!("services:       {}", view.services.join(", "));
    }
    Ok(())
}

/// Render an uptime in seconds as a compact `1d 2h 3m 4s` string. Zero-valued
/// leading units are omitted; `0s` is shown for a freshly-booted Core.
fn format_uptime(total_secs: u64) -> String {
    let days = total_secs / 86_400;
    let hours = (total_secs % 86_400) / 3_600;
    let mins = (total_secs % 3_600) / 60;
    let secs = total_secs % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 || !parts.is_empty() {
        parts.push(format!("{hours}h"));
    }
    if mins > 0 || !parts.is_empty() {
        parts.push(format!("{mins}m"));
    }
    parts.push(format!("{secs}s"));
    format!("{} ({total_secs}s)", parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::format_uptime;

    #[test]
    fn uptime_formats_compactly() {
        assert_eq!(format_uptime(0), "0s (0s)");
        assert_eq!(format_uptime(45), "45s (45s)");
        assert_eq!(format_uptime(90), "1m 30s (90s)");
        assert_eq!(format_uptime(3_661), "1h 1m 1s (3661s)");
        assert_eq!(format_uptime(90_061), "1d 1h 1m 1s (90061s)");
    }
}
