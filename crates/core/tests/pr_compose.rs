//! Task 321: the PR-compose step (`compose_pr`) + the `OneShotLlm` seam reuse.
//!
//! In-process tests against a real `WorkareaManager` over a tempdir DB with a
//! real git repo/worktree. They exercise the FROZEN `compose_pr` entry point:
//! the deterministic LIVE Phase-3 path, the 2 s timeout/error/opt-out
//! fallbacks, the `action_prefs.pr_create` injection (recording stub), the
//! `.github/pull_request_template.md` fold, the always-appended footer, and the
//! caller-override-verbatim contract.
//!
//! **Tier note (D1):** Tier-1 covers the deterministic PR-compose path + the
//! 2 s timeout/fallback contract — the live P3 path. The live-LLM-quality path
//! is wired in P4 (412) and judged at that phase gate; it is not gated here.
//!
//! The pure-helper coverage (split / footer / template fold / caller-override)
//! lives in `crates/core/src/workspace_manager/pr_compose.rs`; this file covers
//! the manager wiring + the seam orchestration.

#![cfg(unix)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use concerto_core::llm::{OneShotLlm, OneShotRequest};
use concerto_core::repo_manager::RepoManager;
use concerto_core::workspace_manager::{PrComposeContext, WorkareaManager};
use concerto_error::{Error, Result};
use concerto_persist::{Persistence, PersistenceConfig, RepositoryId, WorkareaId};
use tempfile::TempDir;
use tokio::process::Command;

async fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_TERMINAL_PROMPT", "0")
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

struct RepoOnDisk {
    /// Kept alive so the bare repo's tempdir is not deleted under the test.
    _bare: TempDir,
    local_path: std::path::PathBuf,
}

async fn make_repo(data_dir: &Path, repo_id: &str) -> RepoOnDisk {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
    let url = format!("file://{}", bare.path().display());
    git(&["remote", "add", "origin", url.as_str()], work.path()).await;
    git(&["push", "-u", "origin", "main"], work.path()).await;

    let local_path = data_dir.join("repos").join(repo_id);
    tokio::fs::create_dir_all(local_path.parent().unwrap())
        .await
        .unwrap();
    git(
        &["clone", url.as_str(), &local_path.to_string_lossy()],
        Path::new("."),
    )
    .await;
    RepoOnDisk {
        _bare: bare,
        local_path,
    }
}

struct Fixture {
    _tmp: TempDir,
    persistence: Arc<Persistence>,
    data_dir: std::path::PathBuf,
    config_dir: std::path::PathBuf,
    repo: RepoOnDisk,
    repo_id: String,
    workspace_id: String,
}

async fn make_fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    let config_dir = tmp.path().join("config");
    let db_path = data_dir.join("concerto.db");
    let persistence = Arc::new(
        Persistence::open(PersistenceConfig {
            db_path,
            max_readers: 2,
        })
        .await
        .expect("open"),
    );

    {
        let mut w = persistence.writer().await;
        sqlx::query(
            "INSERT INTO workspaces (id, name, slug, created_at)
             VALUES ('ws', 'ws', 'ws', 0)",
        )
        .execute(&mut *w)
        .await
        .expect("workspace");
    }

    let repo_id = "r0".to_string();
    let repo = make_repo(&data_dir, &repo_id).await;
    {
        let mut w = persistence.writer().await;
        sqlx::query(
            "INSERT INTO repositories (id, name, url, local_path, clone_strategy, default_branch)
             VALUES (?, 'p', 'name-0', ?, 'full', 'main')",
        )
        .bind(&repo_id)
        .bind(repo.local_path.to_string_lossy().to_string())
        .execute(&mut *w)
        .await
        .expect("repo");
        sqlx::query(
            "INSERT INTO workspace_repos (workspace_id, repository_id, position) VALUES ('ws', ?, 0)",
        )
        .bind(&repo_id)
        .execute(&mut *w)
        .await
        .expect("workspace_repos");
    }

    Fixture {
        _tmp: tmp,
        persistence,
        data_dir,
        config_dir,
        repo,
        repo_id,
        workspace_id: "ws".to_string(),
    }
}

impl Fixture {
    fn manager(&self) -> WorkareaManager {
        let repo_manager =
            RepoManager::new(Arc::clone(&self.persistence), self.data_dir.join("repos"));
        WorkareaManager::new(
            Arc::clone(&self.persistence),
            repo_manager,
            Arc::new(self.data_dir.clone()),
            Arc::new(self.config_dir.clone()),
        )
    }

    /// Set the workspace's `pr_compose` opt-out flag.
    async fn set_pr_compose(&self, enabled: bool) {
        let mut w = self.persistence.writer().await;
        sqlx::query("UPDATE workspaces SET settings_json = ? WHERE id = 'ws'")
            .bind(format!("{{\"pr_compose\":{enabled}}}"))
            .execute(&mut *w)
            .await
            .expect("settings");
    }

    /// Write a checked-in `pr_create` action pref into the reference repo.
    async fn set_pr_create_pref(&self, pref: &str) {
        let concerto = self.repo.local_path.join(".concerto");
        tokio::fs::create_dir_all(&concerto).await.unwrap();
        tokio::fs::write(
            concerto.join("action_prefs.toml"),
            format!("pr_create = {pref:?}\n"),
        )
        .await
        .unwrap();
    }

    async fn ctx(&self, wa: &WorkareaId, last_user_message: &str) -> PrComposeContext {
        PrComposeContext {
            workarea_id: wa.clone(),
            repository_id: RepositoryId(self.repo_id.clone()),
            composer: "bach".to_string(),
            branch: "concerto/bach".to_string(),
            last_user_message: last_user_message.to_string(),
            change_summary: String::new(),
            agent_kind: "claude".to_string(),
        }
    }
}

/// A recording stub: captures the last prompt + counts calls, returns a fixed
/// `title\n\nbody`.
struct RecordingLlm {
    last_prompt: Arc<Mutex<String>>,
    calls: Arc<AtomicUsize>,
    response: String,
}

#[async_trait]
impl OneShotLlm for RecordingLlm {
    async fn suggest(&self, req: OneShotRequest) -> Result<String> {
        *self.last_prompt.lock().unwrap() = req.prompt.clone();
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }
}

/// A stub that sleeps past the 2 s budget (proves the timeout fallback).
struct SleepyLlm;

#[async_trait]
impl OneShotLlm for SleepyLlm {
    async fn suggest(&self, _req: OneShotRequest) -> Result<String> {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok("SLOW TITLE\n\nslow body".to_string())
    }
}

/// A stub that always errors (proves the error fallback).
struct ErrorLlm;

#[async_trait]
impl OneShotLlm for ErrorLlm {
    async fn suggest(&self, _req: OneShotRequest) -> Result<String> {
        Err(Error::Internal("provider boom".to_string()))
    }
}

async fn create_wa(mgr: &WorkareaManager, workspace_id: &str) -> concerto_persist::Workarea {
    mgr.create_workarea(workspace_id, None)
        .await
        .expect("create workarea")
}

#[tokio::test(flavor = "multi_thread")]
async fn deterministic_path_when_no_provider() {
    let fx = make_fixture().await;
    let mgr = fx.manager(); // default one_shot = DeterministicOneShot (the LIVE P3 path)
    let wa = create_wa(&mgr, &fx.workspace_id).await;

    let ctx = fx
        .ctx(&wa.id, "Add idempotency keys to the payments endpoint")
        .await;
    let (title, body) = mgr.compose_pr(ctx).await.expect("compose");

    // Deterministic title = composer + branch; body = last user message.
    assert_eq!(title, "bach · concerto/bach");
    assert!(
        body.contains("Add idempotency keys to the payments endpoint"),
        "body should carry the last user message; got {body}"
    );
    // Footer always appended.
    assert!(body.contains("Created from Concerto · workarea `bach` · agent `claude`"));
    assert!(body.contains(&format!("concerto://workarea/{}", wa.id.0)));
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_falls_back_within_budget() {
    let fx = make_fixture().await;
    let mgr = fx.manager().with_one_shot(Arc::new(SleepyLlm));
    let wa = create_wa(&mgr, &fx.workspace_id).await;

    let start = Instant::now();
    let ctx = fx.ctx(&wa.id, "Do the slow thing").await;
    let (title, body) = mgr.compose_pr(ctx).await.expect("compose");
    let elapsed = start.elapsed();

    // The deterministic fallback was used (NOT the sleepy stub's output).
    assert_eq!(title, "bach · concerto/bach");
    assert!(
        !body.contains("slow body"),
        "must not use the slow stub output"
    );
    assert!(body.contains("Do the slow thing"));
    // Returned in ~2 s, not the stub's 10 s sleep.
    assert!(
        elapsed < Duration::from_secs(5),
        "compose should return near the 2s budget, took {elapsed:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_error_falls_back() {
    let fx = make_fixture().await;
    let mgr = fx.manager().with_one_shot(Arc::new(ErrorLlm));
    let wa = create_wa(&mgr, &fx.workspace_id).await;

    let ctx = fx.ctx(&wa.id, "Handle the error case").await;
    let (title, body) = mgr.compose_pr(ctx).await.expect("compose");
    assert_eq!(title, "bach · concerto/bach");
    assert!(body.contains("Handle the error case"));
}

#[tokio::test(flavor = "multi_thread")]
async fn opt_out_skips_provider_and_uses_fallback() {
    let fx = make_fixture().await;
    fx.set_pr_compose(false).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let stub = RecordingLlm {
        last_prompt: Arc::new(Mutex::new(String::new())),
        calls: Arc::clone(&calls),
        response: "STUB TITLE\n\nstub body".to_string(),
    };
    let mgr = fx.manager().with_one_shot(Arc::new(stub));
    let wa = create_wa(&mgr, &fx.workspace_id).await;

    let ctx = fx.ctx(&wa.id, "Opt me out").await;
    let (title, body) = mgr.compose_pr(ctx).await.expect("compose");

    // The provider was NOT called (opt-out short-circuits the LLM path).
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "provider must not be called"
    );
    assert_eq!(title, "bach · concerto/bach");
    assert!(body.contains("Opt me out"));
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_output_used_when_on() {
    let fx = make_fixture().await;
    let stub = RecordingLlm {
        last_prompt: Arc::new(Mutex::new(String::new())),
        calls: Arc::new(AtomicUsize::new(0)),
        response: "Better Title\n\nA much better body.".to_string(),
    };
    let mgr = fx.manager().with_one_shot(Arc::new(stub));
    let wa = create_wa(&mgr, &fx.workspace_id).await;

    let ctx = fx.ctx(&wa.id, "raw message").await;
    let (title, body) = mgr.compose_pr(ctx).await.expect("compose");
    assert_eq!(title, "Better Title");
    assert!(body.starts_with("A much better body."));
    // Footer still appended to the composed body.
    assert!(body.contains("Created from Concerto · workarea `bach`"));
}

#[tokio::test(flavor = "multi_thread")]
async fn action_pref_pr_create_injected_into_prompt() {
    let fx = make_fixture().await;
    fx.set_pr_create_pref("Always start the body with a Summary section.")
        .await;
    let last_prompt = Arc::new(Mutex::new(String::new()));
    let stub = RecordingLlm {
        last_prompt: Arc::clone(&last_prompt),
        calls: Arc::new(AtomicUsize::new(0)),
        response: "T\n\nB".to_string(),
    };
    let mgr = fx.manager().with_one_shot(Arc::new(stub));
    let wa = create_wa(&mgr, &fx.workspace_id).await;

    let ctx = fx.ctx(&wa.id, "the change").await;
    let _ = mgr.compose_pr(ctx).await.expect("compose");

    let prompt = last_prompt.lock().unwrap().clone();
    assert!(
        prompt.contains("[pr_create preference]"),
        "the pr_create pref must be injected into the prompt; got {prompt}"
    );
    assert!(prompt.contains("Always start the body with a Summary section."));
}

#[tokio::test(flavor = "multi_thread")]
async fn pull_request_template_folded_into_body() {
    let fx = make_fixture().await;
    let mgr = fx.manager();
    let wa = create_wa(&mgr, &fx.workspace_id).await;

    // Write a PR template into the repo's worktree.
    let wt = concerto_persist::workareas::get_workarea_repo_worktree_path(
        fx.persistence.readers(),
        &wa.id,
        &RepositoryId(fx.repo_id.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    let github = Path::new(&wt).join(".github");
    tokio::fs::create_dir_all(&github).await.unwrap();
    tokio::fs::write(
        github.join("pull_request_template.md"),
        "## Checklist\n- [ ] Tests added\n",
    )
    .await
    .unwrap();

    let ctx = fx.ctx(&wa.id, "the change").await;
    let (_title, body) = mgr.compose_pr(ctx).await.expect("compose");
    assert!(
        body.contains("## Checklist"),
        "template should be folded into the body; got {body}"
    );
    assert!(body.contains("- [ ] Tests added"));
    // Footer still appended after the folded template + body.
    assert!(body.contains("Created from Concerto · workarea `bach`"));
}

#[tokio::test(flavor = "multi_thread")]
async fn create_pr_for_repo_requires_vcs_handle() {
    // `create_pr_for_repo` returns the typed `vcs.not_configured` error when no
    // VCS handle is wired (the standalone `compose_pr` entry point + the pure
    // `caller_override` helper cover the verbatim/compose split without a VCS
    // double — `caller_override` is unit-tested in `pr_compose.rs`).
    let fx = make_fixture().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let stub = RecordingLlm {
        last_prompt: Arc::new(Mutex::new(String::new())),
        calls: Arc::clone(&calls),
        response: "STUB\n\nstub".to_string(),
    };
    let mgr = fx.manager().with_one_shot(Arc::new(stub));
    let wa = create_wa(&mgr, &fx.workspace_id).await;

    let err = mgr
        .create_pr_for_repo(
            &wa.id,
            &RepositoryId(fx.repo_id.clone()),
            "main",
            "concerto/bach",
            "Explicit Title",
            "Explicit body.",
        )
        .await
        .expect_err("no vcs handle wired");
    assert!(
        err.to_string().contains("vcs.not_configured"),
        "expected vcs.not_configured; got {err}"
    );
    // The provider was never consulted on this no-VCS path.
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
