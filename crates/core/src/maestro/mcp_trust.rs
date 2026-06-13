//! Pre-seed Claude's per-project MCP-server approval so the Maestro session
//! never blocks on the interactive "trust this MCP server?" gate. Mirrors
//! `agent_supervisor::ensure_claude_trusts_dir` (the folder-trust preseed) but
//! targets `projects.<scratch>.enabledMcpjsonServers`.

use std::path::Path;

use concerto_error::{Error, Result};

use crate::maestro::mcp::SERVER_NAME;

/// Preseed the MCP-server trust for the Maestro scratch project. Idempotent.
pub fn ensure_maestro_mcp_trusted(scratch_cwd: &Path) -> Result<()> {
    let home = home::home_dir()
        .ok_or_else(|| Error::Internal("cannot resolve home dir for ~/.claude.json".into()))?;
    ensure_mcp_trusted_at(&home.join(".claude.json"), scratch_cwd)
}

/// Testable core: take the config path explicitly.
pub fn ensure_mcp_trusted_at(config_path: &Path, scratch_cwd: &Path) -> Result<()> {
    let key = std::fs::canonicalize(scratch_cwd)
        .unwrap_or_else(|_| scratch_cwd.to_path_buf())
        .to_string_lossy()
        .into_owned();

    let mut root: serde_json::Value = match std::fs::read(config_path) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|e| Error::Internal(format!("parse ~/.claude.json: {e}")))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            serde_json::Value::Object(Default::default())
        }
        Err(e) => return Err(Error::Io(e)),
    };
    if !root.is_object() {
        return Err(Error::Internal("~/.claude.json is not an object".into()));
    }
    let proj = root
        .as_object_mut()
        .unwrap()
        .entry("projects")
        .or_insert_with(|| serde_json::Value::Object(Default::default()))
        .as_object_mut()
        .ok_or_else(|| Error::Internal("`projects` not an object".into()))?
        .entry(key)
        .or_insert_with(|| serde_json::Value::Object(Default::default()));
    let obj = proj
        .as_object_mut()
        .ok_or_else(|| Error::Internal("project entry not an object".into()))?;
    let list = obj
        .entry("enabledMcpjsonServers")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| Error::Internal("enabledMcpjsonServers not an array".into()))?;
    if !list.iter().any(|s| s == SERVER_NAME) {
        list.push(serde_json::Value::String(SERVER_NAME.to_string()));
    }
    std::fs::write(config_path, serde_json::to_vec_pretty(&root).expect("serialize"))
        .map_err(Error::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preseed_adds_server_to_enabled_list_idempotently() {
        let home = tempfile::tempdir().unwrap();
        let scratch = home.path().join("concerto/maestro");
        std::fs::create_dir_all(&scratch).unwrap();
        let cfg = home.path().join(".claude.json");
        ensure_mcp_trusted_at(&cfg, &scratch).unwrap();
        ensure_mcp_trusted_at(&cfg, &scratch).unwrap(); // idempotent
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
        let key = std::fs::canonicalize(&scratch).unwrap().to_string_lossy().into_owned();
        let arr = v["projects"][&key]["enabledMcpjsonServers"].as_array().unwrap();
        assert_eq!(arr.iter().filter(|s| *s == "concerto-maestro-mcp").count(), 1);
    }
}
