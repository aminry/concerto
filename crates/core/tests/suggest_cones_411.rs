//! Task 411 integration coverage (Tier 2): the `Repositories.SuggestCones`
//! gRPC handler, the `MaestroConeSuggester` (the LIVE wiring of 305's seam
//! through 312's `OneShotLlm` / `DeterministicOneShot`), and the D10
//! `enterprise_data_privacy` resolver the `Vcs.FetchIssueByUrl` fix consults.
//!
//! In-process against a tempdir SQLite DB + a `file://` bare-repo fixture — no
//! network, no keychain. The Tier-2 doubles are a stub `ConeSuggester` (the
//! `SuggestCones` handler path) + the real git tree (the `MaestroConeSuggester`
//! deterministic ranking).
//!
//! What this does NOT cover (→ the Phase-4 Tier-3 line "create a workspace from
//! a real issue link"): the real GitHub/Linear issue round-trip + real-LLM cone
//! quality (412's provider). The privacy gate over the real `fetch_issue_url`
//! outbound call is covered at the `crates/vcs` layer
//! (`clients_linear_jira::enterprise_data_privacy_refuses_external_tracker_fetch`);
//! here we cover the NEW resolution logic the D10 fix added.

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use concerto_core::handlers::vcs::{CorePrivacyResolver, EnterprisePrivacyResolver};
use concerto_core::maestro::MaestroConeSuggester;
use concerto_core::repo_manager::{ConeSuggester, RepoManager};
use concerto_core::settings::workspace_file::OptOutConfig;
use concerto_error::Result as CResult;
use concerto_gix_wrap::{CloneStrategy, ConePath};
use concerto_persist::{NewWorkspace, Persistence, PersistenceConfig, RepositoryId, WorkspaceId};
use concerto_proto::v1::repositories_server::Repositories as _;
use concerto_proto::v1::SuggestConesRequest;
use tempfile::TempDir;
use tokio::process::Command;
use tonic::{Code, Request};

async fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: stderr={}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn make_persistence() -> (Arc<Persistence>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("concerto.db");
    let persistence = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open persistence");
    (Arc::new(persistence), tmp)
}

fn make_repo_manager(persistence: &Arc<Persistence>, root: &Path) -> RepoManager {
    RepoManager::new(Arc::clone(persistence), root.join("repos"))
}

/// A bare repo whose `main` carries `auth/`, `payments/`, `docs/` top-level
/// directories. Returns its `file://` URL.
async fn make_bare_with_dirs() -> (String, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;

    for dir in ["auth", "payments", "docs"] {
        tokio::fs::create_dir(work.path().join(dir)).await.unwrap();
        tokio::fs::write(work.path().join(dir).join("f.txt"), "x\n")
            .await
            .unwrap();
    }
    git(&["add", "-A"], work.path()).await;
    git(&["commit", "-m", "tree"], work.path()).await;
    git(
        &[
            "remote",
            "add",
            "origin",
            &format!("file://{}", bare.path().display()),
        ],
        work.path(),
    )
    .await;
    git(&["push", "-u", "origin", "main"], work.path()).await;
    (format!("file://{}", bare.path().display()), bare, work)
}

/// A stub `ConeSuggester` (the Tier-2 double) returning a fixed cone set.
struct StubSuggester {
    canned: Vec<ConePath>,
}
#[async_trait]
impl ConeSuggester for StubSuggester {
    async fn suggest_cones(
        &self,
        _repo: &RepositoryId,
        _issue_text: &str,
    ) -> CResult<Vec<ConePath>> {
        Ok(self.canned.clone())
    }
}

// ---------------------------------------------------------------------------
// (A) The SuggestCones gRPC handler — unwired → UNIMPLEMENTED, injected → set.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn suggest_cones_handler_unwired_is_unimplemented() {
    use concerto_core::handlers::repositories::RepositoriesHandler;
    let (persistence, tmp) = make_persistence().await;
    let manager = make_repo_manager(&persistence, tmp.path());
    // No ConeSuggester injected → the handler returns UNIMPLEMENTED (NOT an
    // empty success), via the FROZEN cone_suggest_error_to_status.
    let handler = RepositoriesHandler::new(manager);
    let status = handler
        .suggest_cones(Request::new(SuggestConesRequest {
            repository_id: "repo-1".to_string(),
            issue_text: "fix the auth bug".to_string(),
        }))
        .await
        .expect_err("unwired seam must surface as a Status, not empty success");
    assert_eq!(status.code(), Code::Unimplemented);
}

#[tokio::test(flavor = "multi_thread")]
async fn suggest_cones_handler_injected_returns_cone_set() {
    use concerto_core::handlers::repositories::RepositoriesHandler;
    let (persistence, tmp) = make_persistence().await;
    let manager =
        make_repo_manager(&persistence, tmp.path()).with_cone_suggester(Arc::new(StubSuggester {
            canned: vec!["auth".to_string(), "payments".to_string()],
        }));
    let handler = RepositoriesHandler::new(manager);
    let resp = handler
        .suggest_cones(Request::new(SuggestConesRequest {
            repository_id: "repo-1".to_string(),
            issue_text: "auth + payments".to_string(),
        }))
        .await
        .expect("injected suggester returns a cone set")
        .into_inner();
    assert_eq!(
        resp.cone_paths,
        vec!["auth".to_string(), "payments".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn suggest_cones_handler_rejects_empty_repository_id() {
    use concerto_core::handlers::repositories::RepositoriesHandler;
    let (persistence, tmp) = make_persistence().await;
    let manager = make_repo_manager(&persistence, tmp.path());
    let handler = RepositoriesHandler::new(manager);
    let status = handler
        .suggest_cones(Request::new(SuggestConesRequest {
            repository_id: String::new(),
            issue_text: "x".to_string(),
        }))
        .await
        .expect_err("empty repository_id is rejected");
    assert_eq!(status.code(), Code::InvalidArgument);
}

// ---------------------------------------------------------------------------
// (B) The MaestroConeSuggester — deterministic ranking over the REAL git tree.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn maestro_cone_suggester_picks_keyword_matched_dirs() {
    let (persistence, tmp) = make_persistence().await;
    let manager = make_repo_manager(&persistence, tmp.path());
    let (url, _bare, _work) = make_bare_with_dirs().await;
    let repo = manager
        .add_repository("fixture", &url, "main", CloneStrategy::Full, false)
        .await
        .expect("add_repository");
    manager
        .clone_repo(&repo.id, None)
        .await
        .expect("clone_repo");

    // The LIVE P4 suggester (DeterministicOneShot fallback). The issue mentions
    // "payments" + "auth" but not "docs" → those two real dirs are picked.
    let suggester = MaestroConeSuggester::new(
        manager.clone(),
        concerto_core::maestro::digest::default_oneshot(),
    );
    let cones = suggester
        .suggest_cones(&repo.id, "Wire payments into the auth flow")
        .await
        .expect("suggestion succeeds");
    let mut got = cones.clone();
    got.sort();
    assert_eq!(got, vec!["auth".to_string(), "payments".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn maestro_cone_suggester_falls_back_to_top_dirs_when_no_keyword_matches() {
    let (persistence, tmp) = make_persistence().await;
    let manager = make_repo_manager(&persistence, tmp.path());
    let (url, _bare, _work) = make_bare_with_dirs().await;
    let repo = manager
        .add_repository("fixture", &url, "main", CloneStrategy::Full, false)
        .await
        .expect("add_repository");
    manager
        .clone_repo(&repo.id, None)
        .await
        .expect("clone_repo");

    let suggester = MaestroConeSuggester::new(
        manager.clone(),
        concerto_core::maestro::digest::default_oneshot(),
    );
    // No directory name appears in the text → fall back to the real top dirs
    // (never a silent empty success).
    let cones = suggester
        .suggest_cones(&repo.id, "make it faster please")
        .await
        .expect("suggestion succeeds");
    assert!(!cones.is_empty(), "fallback must carry the real top dirs");
    for c in &cones {
        assert!(
            ["auth", "payments", "docs"].contains(&c.as_str()),
            "suggested cone {c} must be a REAL repo directory"
        );
    }
}

// ---------------------------------------------------------------------------
// (C) The D10 fix — CorePrivacyResolver resolves the effective privacy value.
// ---------------------------------------------------------------------------

async fn seed_workspace(persistence: &Persistence, id: &str, settings_json: &str) {
    let mut w = persistence.writer().await;
    concerto_persist::workspaces::insert(
        &mut w,
        NewWorkspace {
            id: WorkspaceId(id.to_string()),
            name: format!("ws-{id}"),
            slug: id.to_string(),
            icon: None,
            description: None,
            permission_mode: None,
            created_at: 0,
        },
    )
    .await
    .expect("insert workspace");
    if settings_json != "{}" {
        concerto_persist::workspaces::set_settings_json(
            &mut w,
            &WorkspaceId(id.to_string()),
            settings_json,
        )
        .await
        .expect("set settings");
    }
}

fn managed_with_privacy(privacy: bool) -> concerto_core::security::managed::ManagedPolicy {
    concerto_core::security::managed::ManagedPolicy {
        enterprise_data_privacy: Some(privacy),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn d10_resolver_workspace_local_db_privacy_true_blocks() {
    let (persistence, _tmp) = make_persistence().await;
    seed_workspace(
        &persistence,
        "ws-private",
        r#"{"enterprise_data_privacy":true}"#,
    )
    .await;

    // No managed layer for privacy (default `None`); the workspace's local-DB
    // setting flips it to true → external trackers must be blocked for this
    // workspace. (A managed `Some(false)` would OVERRIDE the workspace, so the
    // managed privacy field must be unset here.)
    let resolver = CorePrivacyResolver::new(
        Arc::clone(&persistence),
        concerto_core::security::managed::ManagedPolicy::default(),
        OptOutConfig::default(),
    );
    assert!(
        resolver.enterprise_data_privacy("ws-private").await,
        "a workspace with enterprise_data_privacy=true must resolve true"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn d10_resolver_empty_scope_falls_back_to_managed_floor() {
    let (persistence, _tmp) = make_persistence().await;

    // Empty workspace scope + a managed-privacy Core → blocked (the floor).
    let blocked = CorePrivacyResolver::new(
        Arc::clone(&persistence),
        managed_with_privacy(true),
        OptOutConfig::default(),
    );
    assert!(
        blocked.enterprise_data_privacy("").await,
        "empty scope under managed privacy resolves to the managed floor (true)"
    );

    // Empty scope + no managed privacy → allowed (default false).
    let allowed = CorePrivacyResolver::new(
        Arc::clone(&persistence),
        managed_with_privacy(false),
        OptOutConfig::default(),
    );
    assert!(
        !allowed.enterprise_data_privacy("").await,
        "empty scope with no managed privacy defaults to false"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn d10_resolver_unknown_workspace_falls_back_to_managed_floor() {
    let (persistence, _tmp) = make_persistence().await;
    // An unknown workspace id under a managed-privacy Core resolves the floor
    // (never silently allows).
    let resolver = CorePrivacyResolver::new(
        Arc::clone(&persistence),
        managed_with_privacy(true),
        OptOutConfig::default(),
    );
    assert!(
        resolver.enterprise_data_privacy("ws-missing").await,
        "an unknown workspace under managed privacy still blocks (the floor)"
    );
}
