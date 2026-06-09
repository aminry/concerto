//! Integration tests for the Task 39 Skills Registry.
//!
//! Three focused happy-path / negative cases:
//!
//! 1. Discovery walks `<home_dir>/.claude/skills/` and a per-workspace
//!    `<repo.local_path>/.claude/skills/` (for every repo attached to a
//!    workspace), parses each well-formed SKILL.md, and upserts the rows.
//!    A malformed SKILL.md is warned + skipped (the walk still succeeds;
//!    the error message lands in `report.errors`).
//! 2. Toggle: disabling a discovered skill flips the `enabled` column;
//!    a subsequent `list` shows `enabled = false`.
//! 3. Idempotency: re-running `refresh` does NOT duplicate rows — the
//!    UNIQUE(scope, workspace_id, name) key collapses the second insert
//!    into an UPDATE.
//!
//! The test deliberately overrides `home_dir` to a tempdir so the
//! personal-scope walk does not touch the developer's actual
//! `~/.claude/skills/` directory.

use std::path::PathBuf;
use std::sync::Arc;

use concerto_core::skills::SkillsRegistryHandle;
use concerto_persist::{Persistence, PersistenceConfig, SkillFilter, SkillScope, WorkspaceId};
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    home_dir: PathBuf,
    workspace_repo_local: PathBuf,
    persistence: Arc<Persistence>,
    workspace_id: WorkspaceId,
}

async fn make_fixture() -> Fixture {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    let db_path = data_dir.join("concerto.db");
    let cfg = PersistenceConfig {
        db_path,
        max_readers: 2,
    };
    let persistence = Arc::new(Persistence::open(cfg).await.expect("open persistence"));

    let workspace_id = WorkspaceId("ws-test-skills".to_string());
    let repo_local = tmp.path().join("repo");
    tokio::fs::create_dir_all(&repo_local).await.unwrap();

    // Seed a repository + workspace + junction row so the workspace-scope
    // walk has something to chew on.
    {
        let mut writer = persistence.writer().await;
        sqlx::query(
            "INSERT INTO repositories (id, name, url, local_path,
                clone_strategy, default_branch)
             VALUES (?, ?, ?, ?, 'full', 'main')",
        )
        .bind("repo-test-skills")
        .bind("test-repo")
        .bind("file:///tmp/fake")
        .bind(repo_local.to_string_lossy().into_owned())
        .execute(&mut *writer)
        .await
        .expect("insert repository");
        sqlx::query("INSERT INTO workspaces (id, name, slug, created_at) VALUES (?, ?, ?, 0)")
            .bind(&workspace_id.0)
            .bind("test-skills")
            .bind("test-skills")
            .execute(&mut *writer)
            .await
            .expect("insert workspace");
        sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
            .bind(&workspace_id.0)
            .bind("repo-test-skills")
            .execute(&mut *writer)
            .await
            .expect("insert workspace_repos");
    }

    let home_dir = tmp.path().join("home");
    tokio::fs::create_dir_all(&home_dir).await.unwrap();

    Fixture {
        _tmp: tmp,
        home_dir,
        workspace_repo_local: repo_local,
        persistence,
        workspace_id,
    }
}

async fn write_skill_md(dir: &std::path::Path, name: &str, body: &str) {
    let skill_dir = dir.join(name);
    tokio::fs::create_dir_all(&skill_dir).await.unwrap();
    tokio::fs::write(skill_dir.join("SKILL.md"), body)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_walks_personal_and_workspace_scopes_and_skips_malformed() {
    let fx = make_fixture().await;

    let personal_root = fx.home_dir.join(".claude").join("skills");
    let workspace_root = fx.workspace_repo_local.join(".claude").join("skills");
    tokio::fs::create_dir_all(&personal_root).await.unwrap();
    tokio::fs::create_dir_all(&workspace_root).await.unwrap();

    // Valid personal-scope skill with full frontmatter.
    write_skill_md(
        &personal_root,
        "personal-skill",
        "---\nname: personal-skill\ndescription: hello\nslash-command: /personal\ntools:\n  - Read\n  - Edit\n---\n# body\n",
    )
    .await;
    // Valid workspace-scope skill (minimal frontmatter).
    write_skill_md(
        &workspace_root,
        "workspace-skill",
        "---\nname: workspace-skill\ndescription: per-repo\n---\nbody\n",
    )
    .await;
    // Malformed frontmatter — no trailing `---`.
    write_skill_md(
        &personal_root,
        "broken-skill",
        "---\nname: broken-skill\ndescription: oops\n",
    )
    .await;

    let registry = SkillsRegistryHandle::new(Arc::clone(&fx.persistence), fx.home_dir.clone());
    let report = registry.refresh(None).await.expect("refresh");
    assert_eq!(
        report.discovered_count, 2,
        "expected 2 well-formed skills; report = {report:?}"
    );
    assert_eq!(
        report.errors.len(),
        1,
        "expected 1 parse error; report = {report:?}"
    );
    assert!(report.errors[0].contains("broken-skill"));

    // List all rows; expect both well-formed scopes.
    let rows = registry.list(SkillFilter::default()).await.expect("list");
    assert_eq!(rows.len(), 2, "rows = {rows:?}");
    let mut by_name: Vec<_> = rows.iter().map(|r| (r.scope, r.name.as_str())).collect();
    by_name.sort();
    assert_eq!(
        by_name,
        vec![
            (SkillScope::Personal, "personal-skill"),
            (SkillScope::Workspace, "workspace-skill"),
        ]
    );

    // Validate that the personal-skill carries its slash_command + tools_json.
    let personal = rows
        .iter()
        .find(|r| r.name == "personal-skill")
        .expect("personal-skill row");
    assert_eq!(personal.slash_command.as_deref(), Some("/personal"));
    assert_eq!(personal.tools_json, "[\"Read\",\"Edit\"]");
    assert!(personal.enabled, "newly-discovered skill should be enabled");

    // Validate scope-filtered list.
    let only_workspace = registry
        .list(SkillFilter {
            scope: Some(SkillScope::Workspace),
            ..Default::default()
        })
        .await
        .expect("list workspace");
    assert_eq!(only_workspace.len(), 1);
    assert_eq!(only_workspace[0].name, "workspace-skill");
    assert_eq!(
        only_workspace[0].workspace_id.as_ref(),
        Some(&fx.workspace_id),
        "workspace-scope row should carry the workspace id"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn toggle_persists_across_list() {
    let fx = make_fixture().await;
    let personal_root = fx.home_dir.join(".claude").join("skills");
    tokio::fs::create_dir_all(&personal_root).await.unwrap();
    write_skill_md(
        &personal_root,
        "toggle-skill",
        "---\nname: toggle-skill\ndescription: x\n---\n",
    )
    .await;

    let registry = SkillsRegistryHandle::new(Arc::clone(&fx.persistence), fx.home_dir.clone());
    registry.refresh(None).await.expect("refresh");

    let row = registry
        .list(SkillFilter::default())
        .await
        .expect("list")
        .into_iter()
        .next()
        .expect("at least one row");
    assert!(row.enabled, "default enabled = true");

    // Disable.
    let updated = registry.toggle(&row.id, false).await.expect("toggle off");
    assert!(!updated.enabled, "toggle off should flip enabled");

    // Verify via list.
    let after = registry.list(SkillFilter::default()).await.expect("list");
    assert_eq!(after.len(), 1);
    assert!(!after[0].enabled);

    // `enabled_only` should now hide the row.
    let only_enabled = registry
        .list(SkillFilter {
            enabled_only: true,
            ..Default::default()
        })
        .await
        .expect("list enabled_only");
    assert!(only_enabled.is_empty());

    // Re-enable so the toggle path is symmetric.
    let updated = registry.toggle(&row.id, true).await.expect("toggle on");
    assert!(updated.enabled);
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_is_idempotent_and_preserves_toggle() {
    let fx = make_fixture().await;
    let personal_root = fx.home_dir.join(".claude").join("skills");
    tokio::fs::create_dir_all(&personal_root).await.unwrap();
    write_skill_md(
        &personal_root,
        "idem-skill",
        "---\nname: idem-skill\ndescription: first\n---\n",
    )
    .await;

    let registry = SkillsRegistryHandle::new(Arc::clone(&fx.persistence), fx.home_dir.clone());
    let report_1 = registry.refresh(None).await.expect("first refresh");
    assert_eq!(report_1.discovered_count, 1);

    let row_1 = registry
        .list(SkillFilter::default())
        .await
        .expect("list")
        .into_iter()
        .next()
        .unwrap();
    // Flip the user's toggle so we can prove the upsert preserves it.
    registry.toggle(&row_1.id, false).await.expect("toggle off");

    // Mutate the SKILL.md (new description) and re-discover.
    write_skill_md(
        &personal_root,
        "idem-skill",
        "---\nname: idem-skill\ndescription: second\n---\n",
    )
    .await;
    let report_2 = registry.refresh(None).await.expect("second refresh");
    assert_eq!(report_2.discovered_count, 1);

    let rows = registry.list(SkillFilter::default()).await.expect("list");
    assert_eq!(rows.len(), 1, "no duplicate row; got {rows:?}");
    assert_eq!(
        rows[0].id, row_1.id,
        "primary key should be stable across re-discovery"
    );
    assert_eq!(
        rows[0].description.as_deref(),
        Some("second"),
        "frontmatter fields should be overwritten"
    );
    assert!(
        !rows[0].enabled,
        "user's toggle should survive re-discovery"
    );
}
