//! Integration test for Task 35's MCP config surfacing.
//!
//! Exercises [`concerto_core::agent_supervisor::mcp::list_mcp_servers`]
//! against fixture config files in a tempdir, with the `home_dir`
//! override pointed at the same tempdir so the test never touches the
//! developer's real `~/.claude/`.
//!
//! Three scenarios:
//!
//! 1. **Personal scope, happy path** — write a canonical
//!    `~/.claude/mcp.json` (under a tempdir) and a
//!    `~/.codex/config.toml` and assert both parsers surface their
//!    entries.
//! 2. **Personal scope, tolerant parsing** — write malformed files and
//!    assert the call returns `Ok(vec![])` without panicking. The
//!    warning is logged via `tracing::warn!`; the test only asserts on
//!    behaviour, not log capture.
//! 3. **Project scope** — seed a `projects` + `repositories` row whose
//!    `local_path` points at a tempdir, drop a `.mcp.json` inside it,
//!    and assert `Project(repo_id)` returns the entry.

#![cfg(unix)]

use std::sync::Arc;

use concerto_core::agent_supervisor::mcp::{list_mcp_servers, McpScope, McpScopeFilter};
use concerto_persist::{Persistence, PersistenceConfig, RepositoryId};
use tempfile::TempDir;

/// Build a fresh `Persistence` over a tempdir-backed SQLite DB. Mirrors
/// the pattern from `agent_spawn.rs`.
async fn make_persistence() -> (TempDir, Arc<Persistence>) {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("concerto.db");
    let cfg = PersistenceConfig {
        db_path,
        max_readers: 2,
    };
    let p = Arc::new(Persistence::open(cfg).await.expect("open persistence"));
    (tmp, p)
}

/// Insert a `projects` + `repositories` row with `local_path = path`
/// so `McpScopeFilter::Project(repo_id)` can resolve the worktree.
async fn seed_repo(persistence: &Persistence, repo_id: &str, local_path: &std::path::Path) {
    let mut writer = persistence.writer().await;
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, ?, ?)")
        .bind("proj-1")
        .bind("test-project")
        .bind(0_i64)
        .execute(&mut *writer)
        .await
        .expect("insert project");
    sqlx::query(
        "INSERT INTO repositories (id, project_id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, ?, 'full', 'main')",
    )
    .bind(repo_id)
    .bind("proj-1")
    .bind("repo-name")
    .bind("file:///tmp/fake")
    .bind(local_path.to_string_lossy().as_ref())
    .execute(&mut *writer)
    .await
    .expect("insert repository");
}

#[tokio::test(flavor = "multi_thread")]
async fn personal_scope_parses_claude_and_codex_fixtures() {
    let (_db_tmp, persistence) = make_persistence().await;
    let home = TempDir::new().expect("home tempdir");

    // ~/.claude/mcp.json
    let claude_dir = home.path().join(".claude");
    tokio::fs::create_dir_all(&claude_dir).await.unwrap();
    tokio::fs::write(
        claude_dir.join("mcp.json"),
        r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "/opt/mcp/fs",
                    "args": ["--root", "/"],
                    "env": {"DEBUG": "1"}
                },
                "search": {
                    "command": "/opt/mcp/search"
                }
            }
        }"#,
    )
    .await
    .unwrap();

    // ~/.codex/config.toml
    let codex_dir = home.path().join(".codex");
    tokio::fs::create_dir_all(&codex_dir).await.unwrap();
    tokio::fs::write(
        codex_dir.join("config.toml"),
        r#"
            [mcp_servers.docs]
            command = "/opt/mcp/docs"
            args = ["--index", "default"]

            [mcp_servers.docs.env]
            TOKEN = "xyz"
        "#,
    )
    .await
    .unwrap();

    let servers = list_mcp_servers(&persistence, McpScopeFilter::Personal, Some(home.path()))
        .await
        .expect("list mcp servers");

    let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"filesystem"), "got names {:?}", names);
    assert!(names.contains(&"search"), "got names {:?}", names);
    assert!(names.contains(&"docs"), "got names {:?}", names);
    assert_eq!(servers.len(), 3);

    let fs = servers.iter().find(|s| s.name == "filesystem").unwrap();
    assert_eq!(fs.scope, McpScope::Personal);
    assert_eq!(fs.command, "/opt/mcp/fs");
    assert_eq!(fs.args, vec!["--root", "/"]);
    assert_eq!(fs.env.get("DEBUG").map(String::as_str), Some("1"));

    let docs = servers.iter().find(|s| s.name == "docs").unwrap();
    assert_eq!(docs.command, "/opt/mcp/docs");
    assert_eq!(docs.env.get("TOKEN").map(String::as_str), Some("xyz"));
}

#[tokio::test(flavor = "multi_thread")]
async fn personal_scope_tolerates_malformed_files() {
    let (_db_tmp, persistence) = make_persistence().await;
    let home = TempDir::new().expect("home tempdir");

    let claude_dir = home.path().join(".claude");
    tokio::fs::create_dir_all(&claude_dir).await.unwrap();
    tokio::fs::write(claude_dir.join("mcp.json"), "{ this is not json")
        .await
        .unwrap();

    let codex_dir = home.path().join(".codex");
    tokio::fs::create_dir_all(&codex_dir).await.unwrap();
    tokio::fs::write(
        codex_dir.join("config.toml"),
        "[mcp_servers.broken\n unterminated table",
    )
    .await
    .unwrap();

    let servers = list_mcp_servers(&persistence, McpScopeFilter::Personal, Some(home.path()))
        .await
        .expect("malformed files should not error the listing");
    assert!(
        servers.is_empty(),
        "malformed inputs should produce empty list; got {servers:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn personal_scope_handles_missing_files() {
    let (_db_tmp, persistence) = make_persistence().await;
    let home = TempDir::new().expect("home tempdir");
    // No files written — both readers should hit NotFound and short-circuit.
    let servers = list_mcp_servers(&persistence, McpScopeFilter::Personal, Some(home.path()))
        .await
        .expect("missing files should not error the listing");
    assert!(servers.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn project_scope_parses_per_repo_mcp_json() {
    let (_db_tmp, persistence) = make_persistence().await;
    let repo_tmp = TempDir::new().expect("repo tempdir");
    let repo_id = "repo-mcp-1";
    seed_repo(&persistence, repo_id, repo_tmp.path()).await;

    tokio::fs::write(
        repo_tmp.path().join(".mcp.json"),
        r#"{
            "mcpServers": {
                "project-tool": {
                    "command": "/opt/mcp/project",
                    "args": ["--workdir", "."]
                }
            }
        }"#,
    )
    .await
    .unwrap();

    let rid = RepositoryId(repo_id.to_string());
    let servers = list_mcp_servers(
        &persistence,
        McpScopeFilter::Project(rid.clone()),
        // home_dir is irrelevant for project scope; pass None.
        None,
    )
    .await
    .expect("list project mcp servers");

    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].name, "project-tool");
    assert_eq!(servers[0].scope, McpScope::Project(rid));
    assert_eq!(servers[0].command, "/opt/mcp/project");
    assert_eq!(
        servers[0].source_path,
        repo_tmp.path().join(".mcp.json"),
        "source_path should point at the parsed file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn project_scope_missing_repo_row_returns_empty() {
    let (_db_tmp, persistence) = make_persistence().await;
    let servers = list_mcp_servers(
        &persistence,
        McpScopeFilter::Project(RepositoryId("nonexistent".to_string())),
        None,
    )
    .await
    .expect("missing repo should not error the listing");
    assert!(servers.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_and_enterprise_scopes_return_empty_in_v01() {
    let (_db_tmp, persistence) = make_persistence().await;
    let home = TempDir::new().expect("home tempdir");
    for filter in [McpScopeFilter::Plugin, McpScopeFilter::Enterprise] {
        let servers = list_mcp_servers(&persistence, filter, Some(home.path()))
            .await
            .expect("V0.1 stub scopes do not error");
        assert!(servers.is_empty());
    }
}
