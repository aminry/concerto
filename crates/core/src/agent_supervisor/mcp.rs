//! MCP configuration surfacing (Task 35).
//!
//! Read-only discovery for the four MCP scopes named in `design/04 §3.6`:
//!
//! - **Personal** — agent-specific user-level config:
//!   - Claude: `~/.claude/mcp.json` (the `mcpServers` map).
//!   - Codex: `~/.codex/config.toml` (the `[mcp_servers.*]` tables).
//! - **Project** — per-repository `<repo_local_path>/.mcp.json` checked
//!   into the workareas. The caller passes a `RepositoryId`; the
//!   supervisor resolves `repositories.local_path` and reads
//!   `<local_path>/.mcp.json`.
//! - **Plugin** — stubbed in V0.1. The eventual location (per
//!   `design/04 §3.6`) is `<plugin>/.mcp.json` shipped by a plugin pack;
//!   plugins themselves arrive in V1.0+.
//! - **Enterprise** — stubbed in V0.1. The eventual location is a
//!   `managed.json`-style policy file under `/etc/concerto/` or the
//!   platform's policy directory; the enterprise channel is V1.0+.
//!
//! ## V0.1 boundaries
//!
//! - **Read-only.** Writing `.mcp.json` (project scope) is V1.0; the
//!   gRPC `Sessions.UpsertProjectMcp` handler responds with
//!   `UNIMPLEMENTED` until then.
//! - **No wire protocol.** Concerto never implements the MCP transport
//!   itself — that's the agent's job. We only surface what the agent's
//!   config already declares so the Desktop can render a list.
//! - **Tolerant parsing.** A malformed file at any scope produces a
//!   `tracing::warn!` and an empty list *for that scope only*; the call
//!   never fails the whole listing because Claude's JSON has a typo.
//!
//! ## Public surface (FROZEN by this task)
//!
//! - [`McpServer`] — one parsed server entry; the wire shape mirrors
//!   the proto `concerto.v1.McpServer` message.
//! - [`McpScope`] — the four scopes, with `Project` carrying the
//!   repository id.
//! - [`McpScopeFilter`] — what callers ask for; `All` walks every
//!   reachable scope, the typed variants pick exactly one.
//! - [`list_mcp_servers`] — the only entry point. Takes the
//!   persistence handle (for project-scope `local_path` lookups) plus
//!   an optional `home_dir` override so tests can point at a tempdir
//!   without having to mock `home::home_dir()`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use concerto_error::Result;
use concerto_persist::{Persistence, RepositoryId};
use serde::Deserialize;

/// One MCP server entry. The wire shape (`concerto.v1.McpServer`) mirrors
/// these fields exactly so the handler is a thin re-encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// Server name as declared in the source config (the JSON object
    /// key or the TOML table suffix).
    pub name: String,
    /// Which scope this row was discovered under.
    pub scope: McpScope,
    /// Executable the agent runs to launch the server. Required.
    pub command: String,
    /// Argv tail passed to `command`. Defaults to empty if missing.
    pub args: Vec<String>,
    /// Environment overrides. Defaults to empty if missing.
    pub env: BTreeMap<String, String>,
    /// Absolute path to the file this entry was parsed from. Useful for
    /// the Desktop's "edit this config" affordance.
    pub source_path: PathBuf,
}

/// Discovery scope a server was found under. Frozen wire surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpScope {
    /// `~/.claude/mcp.json` or `~/.codex/config.toml`.
    Personal,
    /// `<repository.local_path>/.mcp.json` for the repository id
    /// carried in the variant.
    Project(RepositoryId),
    /// V0.1 stub — reserved for plugin-shipped MCP packs.
    Plugin,
    /// V0.1 stub — reserved for `/etc/concerto/managed.json`-style
    /// policy MCP declarations.
    Enterprise,
}

impl McpScope {
    /// Stable lowercase wire string used in the proto `McpServer.scope`
    /// field and on the `McpScopeRequest.scope` input.
    pub fn as_wire(&self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Project(_) => "project",
            Self::Plugin => "plugin",
            Self::Enterprise => "enterprise",
        }
    }
}

/// Selector passed to [`list_mcp_servers`]. `All` is the default the
/// gRPC handler picks when the caller omits both wire fields; the
/// typed variants narrow the walk to a single scope (and, for
/// `Project`, a single repository).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpScopeFilter {
    /// Read every reachable scope. Personal + every repository the
    /// caller has access to plus the V0.1 plugin/enterprise stubs
    /// (which return empty).
    All,
    /// Only `~/.claude/mcp.json` + `~/.codex/config.toml`.
    Personal,
    /// Only `<local_path>/.mcp.json` for the given repository.
    Project(RepositoryId),
    /// V0.1 stub — returns an empty list.
    Plugin,
    /// V0.1 stub — returns an empty list.
    Enterprise,
}

// ---------------------------------------------------------------------------
// Permissive parse shapes
// ---------------------------------------------------------------------------

/// Top-level shape of `~/.claude/mcp.json` (and per-repo `.mcp.json`).
/// Claude's canonical schema is `{ "mcpServers": { name: McpEntry } }`;
/// we accept the snake_case `mcp_servers` alias too because earlier
/// Claude builds shipped that spelling.
#[derive(Debug, Deserialize, Default)]
struct ClaudeMcpFile {
    #[serde(default, rename = "mcpServers", alias = "mcp_servers")]
    mcp_servers: BTreeMap<String, McpEntry>,
}

/// Shape of one server entry shared across the JSON and TOML formats.
/// Both Claude and Codex agreed on the same field names; the only
/// per-format difference is the wrapper table syntax.
#[derive(Debug, Deserialize, Default, Clone)]
struct McpEntry {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    env: Option<BTreeMap<String, String>>,
}

/// Top-level shape of `~/.codex/config.toml`. Codex uses
/// `[mcp_servers.<name>]` tables, which `toml` deserializes as a
/// `HashMap<String, McpEntry>` under the `mcp_servers` key.
///
/// The legacy `[mcp]` table is accepted via `alias = "mcp"` so older
/// Codex installs surface too. Unknown top-level keys are ignored.
#[derive(Debug, Deserialize, Default)]
struct CodexConfigFile {
    #[serde(default, alias = "mcp")]
    mcp_servers: BTreeMap<String, McpEntry>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// List MCP servers discovered under `filter`.
///
/// `persistence` is used only when the filter resolves to a project
/// scope (so the function can read `repositories.local_path`). Pass
/// the same `Arc<Persistence>` the rest of the runtime uses.
///
/// `home_dir` overrides where personal-scope configs are read from.
/// Production callers pass `None` and the function uses
/// [`home::home_dir`]; tests pass `Some(&tempdir)` to avoid touching
/// the developer's real `~/.claude/`.
///
/// Errors short-circuit on persistence lookups (DB failure is fatal);
/// per-file parse failures are tolerated — a malformed `mcp.json`
/// produces a `tracing::warn!` and an empty list *for that file*, not
/// a propagated error.
pub async fn list_mcp_servers(
    persistence: &Persistence,
    filter: McpScopeFilter,
    home_dir: Option<&Path>,
) -> Result<Vec<McpServer>> {
    let mut out = Vec::new();
    match filter {
        McpScopeFilter::All => {
            out.extend(read_personal(home_dir).await);
            // Project scope under `All` would require enumerating every
            // repository in the DB, which is a sweep — V0.1 keeps `All`
            // cheap by only including personal + the (empty) plugin /
            // enterprise stubs. Callers that want per-project results
            // ask for `Project(id)` explicitly.
        }
        McpScopeFilter::Personal => {
            out.extend(read_personal(home_dir).await);
        }
        McpScopeFilter::Project(repo_id) => {
            if let Some(servers) = read_project(persistence, &repo_id).await? {
                out.extend(servers);
            }
        }
        // V0.1 stubs — both intentionally return empty. Documented in
        // the module doc-comment; the file paths land in V1.0+.
        McpScopeFilter::Plugin | McpScopeFilter::Enterprise => {}
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Per-scope readers
// ---------------------------------------------------------------------------

async fn read_personal(home_override: Option<&Path>) -> Vec<McpServer> {
    let home = match home_override.map(Path::to_path_buf).or_else(home::home_dir) {
        Some(h) => h,
        None => {
            tracing::warn!("mcp.personal: home_dir() returned None; skipping");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    out.extend(read_claude_personal(&home).await);
    out.extend(read_codex_personal(&home).await);
    out
}

async fn read_claude_personal(home: &Path) -> Vec<McpServer> {
    let path = home.join(".claude").join("mcp.json");
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "mcp.claude: failed to read personal config; skipping scope"
            );
            return Vec::new();
        }
    };
    parse_claude_json(&raw, &path, McpScope::Personal)
}

async fn read_codex_personal(home: &Path) -> Vec<McpServer> {
    let path = home.join(".codex").join("config.toml");
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "mcp.codex: failed to read personal config; skipping scope"
            );
            return Vec::new();
        }
    };
    parse_codex_toml(&raw, &path, McpScope::Personal)
}

async fn read_project(
    persistence: &Persistence,
    repo_id: &RepositoryId,
) -> Result<Option<Vec<McpServer>>> {
    let repo = match concerto_persist::repositories::get(persistence.readers(), repo_id).await? {
        Some(r) => r,
        None => {
            tracing::warn!(
                repo_id = %repo_id,
                "mcp.project: repository row not found; returning empty list"
            );
            return Ok(Some(Vec::new()));
        }
    };
    let path = PathBuf::from(&repo.local_path).join(".mcp.json");
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(Vec::new())),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "mcp.project: failed to read .mcp.json; returning empty list"
            );
            return Ok(Some(Vec::new()));
        }
    };
    Ok(Some(parse_claude_json(
        &raw,
        &path,
        McpScope::Project(repo_id.clone()),
    )))
}

// ---------------------------------------------------------------------------
// Format parsers — tolerant, log-and-empty on failure.
// ---------------------------------------------------------------------------

fn parse_claude_json(raw: &str, path: &Path, scope: McpScope) -> Vec<McpServer> {
    let parsed: ClaudeMcpFile = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "mcp.claude: malformed JSON; returning empty list for this scope"
            );
            return Vec::new();
        }
    };
    entries_to_servers(parsed.mcp_servers, path, scope)
}

fn parse_codex_toml(raw: &str, path: &Path, scope: McpScope) -> Vec<McpServer> {
    let parsed: CodexConfigFile = match toml::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "mcp.codex: malformed TOML; returning empty list for this scope"
            );
            return Vec::new();
        }
    };
    entries_to_servers(parsed.mcp_servers, path, scope)
}

fn entries_to_servers(
    entries: BTreeMap<String, McpEntry>,
    path: &Path,
    scope: McpScope,
) -> Vec<McpServer> {
    let mut out = Vec::with_capacity(entries.len());
    for (name, entry) in entries {
        let command = match entry.command {
            Some(c) if !c.is_empty() => c,
            _ => {
                tracing::warn!(
                    path = %path.display(),
                    server = %name,
                    "mcp: entry has no `command`; skipping"
                );
                continue;
            }
        };
        out.push(McpServer {
            name,
            scope: scope.clone(),
            command,
            args: entry.args.unwrap_or_default(),
            env: entry.env.unwrap_or_default(),
            source_path: path.to_path_buf(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    //! Unit tests live next to the public surface; the larger
    //! integration tests live in `crates/core/tests/mcp_listing.rs`
    //! (per Task 35 §"Outputs").

    use super::*;

    #[test]
    fn scope_wire_strings_round_trip() {
        assert_eq!(McpScope::Personal.as_wire(), "personal");
        assert_eq!(
            McpScope::Project(RepositoryId("r-1".to_string())).as_wire(),
            "project"
        );
        assert_eq!(McpScope::Plugin.as_wire(), "plugin");
        assert_eq!(McpScope::Enterprise.as_wire(), "enterprise");
    }

    #[test]
    fn entries_to_servers_skips_missing_command() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "ok".to_string(),
            McpEntry {
                command: Some("/usr/bin/true".to_string()),
                args: Some(vec!["--flag".to_string()]),
                env: None,
            },
        );
        entries.insert(
            "no-cmd".to_string(),
            McpEntry {
                command: None,
                args: None,
                env: None,
            },
        );
        let servers = entries_to_servers(
            entries,
            std::path::Path::new("/tmp/fake.json"),
            McpScope::Personal,
        );
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "ok");
        assert_eq!(servers[0].command, "/usr/bin/true");
        assert_eq!(servers[0].args, vec!["--flag".to_string()]);
    }

    #[test]
    fn claude_json_parses_canonical_shape() {
        let raw = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "/opt/mcp/fs",
                    "args": ["--root", "/"],
                    "env": {"DEBUG": "1"}
                }
            }
        }"#;
        let servers = parse_claude_json(
            raw,
            std::path::Path::new("/tmp/mcp.json"),
            McpScope::Personal,
        );
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "filesystem");
        assert_eq!(servers[0].command, "/opt/mcp/fs");
        assert_eq!(servers[0].env.get("DEBUG").map(String::as_str), Some("1"));
    }

    #[test]
    fn codex_toml_parses_mcp_servers_table() {
        let raw = r#"
            [mcp_servers.search]
            command = "/opt/mcp/search"
            args = ["--index", "default"]

            [mcp_servers.search.env]
            TOKEN = "xyz"
        "#;
        let servers = parse_codex_toml(
            raw,
            std::path::Path::new("/tmp/config.toml"),
            McpScope::Personal,
        );
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].name, "search");
        assert_eq!(servers[0].args, vec!["--index", "default"]);
        assert_eq!(servers[0].env.get("TOKEN").map(String::as_str), Some("xyz"));
    }

    #[test]
    fn malformed_json_returns_empty_not_panic() {
        let servers = parse_claude_json(
            "{ this is not json",
            std::path::Path::new("/tmp/mcp.json"),
            McpScope::Personal,
        );
        assert!(servers.is_empty());
    }

    #[test]
    fn malformed_toml_returns_empty_not_panic() {
        let servers = parse_codex_toml(
            "[mcp_servers.broken\n unterminated",
            std::path::Path::new("/tmp/config.toml"),
            McpScope::Personal,
        );
        assert!(servers.is_empty());
    }
}
