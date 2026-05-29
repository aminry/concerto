//! Integration tests for the Task 40 Suggestion Engine.
//!
//! Exercises the rule pipeline + dedup + the
//! `turn_complete_with_uncommitted` async side check end-to-end. The
//! tests drive [`SuggestionEngineHandle::evaluate_event`] directly so
//! they do not depend on a live `AgentSupervisor` — that side of the
//! integration is exercised by `agent_spawn.rs`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use concerto_core::agent_supervisor::{AgentEvent, MessageRole};
use concerto_core::suggestions::{
    Chip, ChipAction, SuggestionEngineHandle, WorkareaState as _WorkareaStateMarker,
};
use concerto_persist::{Persistence, PersistenceConfig, SessionId, WorkareaId};
use tempfile::TempDir;

// Re-export the marker so the trait-bound import is observed (and the
// compiler doesn't flag the `use` as dead). The marker type is part of
// the V0.1 surface even though no test consumes it directly.
type _WorkareaState = _WorkareaStateMarker;

async fn make_persistence() -> (TempDir, Arc<Persistence>) {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    let cfg = PersistenceConfig {
        db_path: data_dir.join("concerto.db"),
        max_readers: 2,
    };
    let p = Arc::new(Persistence::open(cfg).await.expect("persistence opens"));
    (tmp, p)
}

/// Resolver that returns a fixed path. Tests use this so the engine's
/// `turn_complete_with_uncommitted` rule probes a tempdir-backed
/// worktree instead of the real `workareas` row.
struct StaticResolver {
    root: PathBuf,
}

#[async_trait]
impl concerto_core::suggestions::actor::WorktreeResolver for StaticResolver {
    async fn worktree_root(&self, _workarea_id: &WorkareaId) -> Option<PathBuf> {
        Some(self.root.clone())
    }
}

/// Resolver that always returns `None` — the worktree probe path
/// becomes a guaranteed-clean branch.
struct NoneResolver;

#[async_trait]
impl concerto_core::suggestions::actor::WorktreeResolver for NoneResolver {
    async fn worktree_root(&self, _workarea_id: &WorkareaId) -> Option<PathBuf> {
        None
    }
}

/// Spawn `git init` + a dirty worktree at `dir`. Returns once the
/// directory is a valid git worktree with at least one untracked file.
async fn make_dirty_worktree(dir: &Path) {
    use tokio::process::Command;
    let run = |args: &[&str]| {
        let dir = dir.to_path_buf();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        async move {
            let out = Command::new("git")
                .args(&args)
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .output()
                .await
                .expect("git spawn");
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    };
    tokio::fs::create_dir_all(dir).await.unwrap();
    run(&["init", "-b", "main", "."]).await;
    tokio::fs::write(dir.join("dirty.txt"), "hello\n")
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn context_usage_55_emits_context_window_50_chip() {
    let (_tmp, persistence) = make_persistence().await;
    let engine = SuggestionEngineHandle::with_resolver(persistence, Arc::new(NoneResolver));
    let wid = WorkareaId("wa-1".into());

    let emitted = engine
        .evaluate_event(
            &wid,
            &AgentEvent::ContextUsage {
                session_id: SessionId("sess-1".into()),
                pct: 55,
            },
        )
        .await;

    assert_eq!(emitted.len(), 1, "expected one chip, got {emitted:?}");
    let chip = &emitted[0];
    assert_eq!(chip.rule_id, "context_window_50");
    assert_eq!(chip.action, ChipAction::Compress);
    assert_eq!(chip.workarea_id, wid);
}

#[tokio::test(flavor = "multi_thread")]
async fn context_usage_85_emits_only_80_rule() {
    let (_tmp, persistence) = make_persistence().await;
    let engine = SuggestionEngineHandle::with_resolver(persistence, Arc::new(NoneResolver));
    let wid = WorkareaId("wa-1".into());

    let emitted = engine
        .evaluate_event(
            &wid,
            &AgentEvent::ContextUsage {
                session_id: SessionId("sess-1".into()),
                pct: 85,
            },
        )
        .await;

    // Both rules' `applies` thresholds are non-overlapping (50..80,
    // 80..=100) so a single event fires exactly one chip.
    assert_eq!(emitted.len(), 1, "expected one chip, got {emitted:?}");
    assert_eq!(emitted[0].rule_id, "context_window_80");
    assert_eq!(emitted[0].action, ChipAction::NewSession);
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_complete_with_uncommitted_fires_when_worktree_dirty() {
    let (tmp, persistence) = make_persistence().await;
    let worktree = tmp.path().join("wt");
    make_dirty_worktree(&worktree).await;
    let engine = SuggestionEngineHandle::with_resolver(
        persistence,
        Arc::new(StaticResolver { root: worktree }),
    );
    let wid = WorkareaId("wa-1".into());

    let emitted = engine
        .evaluate_event(
            &wid,
            &AgentEvent::TurnComplete {
                session_id: SessionId("sess-1".into()),
            },
        )
        .await;

    let commit = emitted
        .iter()
        .find(|c| c.rule_id == "turn_complete_with_uncommitted")
        .expect("commit chip should fire on dirty worktree");
    assert_eq!(commit.action, ChipAction::CommitAndPush);
}

#[tokio::test(flavor = "multi_thread")]
async fn turn_complete_without_worktree_resolver_skips_commit_chip() {
    let (_tmp, persistence) = make_persistence().await;
    let engine = SuggestionEngineHandle::with_resolver(persistence, Arc::new(NoneResolver));
    let wid = WorkareaId("wa-1".into());

    let emitted = engine
        .evaluate_event(
            &wid,
            &AgentEvent::TurnComplete {
                session_id: SessionId("sess-1".into()),
            },
        )
        .await;

    assert!(
        emitted
            .iter()
            .all(|c| c.rule_id != "turn_complete_with_uncommitted"),
        "commit chip must be filtered when status probe returns false; got {emitted:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dedup_squashes_repeated_emissions_within_window() {
    let (_tmp, persistence) = make_persistence().await;
    let engine = SuggestionEngineHandle::with_resolver(persistence, Arc::new(NoneResolver));
    let wid = WorkareaId("wa-1".into());

    let first = engine
        .evaluate_event(
            &wid,
            &AgentEvent::ContextUsage {
                session_id: SessionId("sess-1".into()),
                pct: 55,
            },
        )
        .await;
    let second = engine
        .evaluate_event(
            &wid,
            &AgentEvent::ContextUsage {
                session_id: SessionId("sess-1".into()),
                pct: 55,
            },
        )
        .await;

    assert_eq!(first.len(), 1, "first call emits");
    assert!(second.is_empty(), "second call within TTL is dedup'd");

    // `list_for_workarea` returns the still-buffered chip.
    let buffered: Vec<Chip> = engine.list_for_workarea(&wid).await;
    assert_eq!(buffered.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn awaiting_approval_emits_review_tool_chip() {
    let (_tmp, persistence) = make_persistence().await;
    let engine = SuggestionEngineHandle::with_resolver(persistence, Arc::new(NoneResolver));
    let wid = WorkareaId("wa-1".into());

    let emitted = engine
        .evaluate_event(
            &wid,
            &AgentEvent::AwaitingApproval {
                session_id: SessionId("sess-1".into()),
                approval_id: "ap-1".into(),
                tool: "Write".into(),
                summary: "write to foo.rs".into(),
                payload_json: "{}".into(),
                urgent: false,
                destructive_label: None,
            },
        )
        .await;

    let chip = emitted
        .iter()
        .find(|c| c.rule_id == "awaiting_approval")
        .expect("awaiting_approval chip should fire");
    assert_eq!(chip.action, ChipAction::ReviewTool);
}

#[tokio::test(flavor = "multi_thread")]
async fn crashed_event_emits_resume_chip() {
    let (_tmp, persistence) = make_persistence().await;
    let engine = SuggestionEngineHandle::with_resolver(persistence, Arc::new(NoneResolver));
    let wid = WorkareaId("wa-1".into());

    let emitted = engine
        .evaluate_event(
            &wid,
            &AgentEvent::Crashed {
                session_id: SessionId("sess-1".into()),
            },
        )
        .await;

    let chip = emitted
        .iter()
        .find(|c| c.rule_id == "agent_crashed")
        .expect("agent_crashed chip should fire");
    assert_eq!(chip.action, ChipAction::Resume);
}

#[tokio::test(flavor = "multi_thread")]
async fn message_with_tests_failed_pattern_emits_chip() {
    let (_tmp, persistence) = make_persistence().await;
    let engine = SuggestionEngineHandle::with_resolver(persistence, Arc::new(NoneResolver));
    let wid = WorkareaId("wa-1".into());

    let emitted = engine
        .evaluate_event(
            &wid,
            &AgentEvent::Message {
                session_id: SessionId("sess-1".into()),
                role: MessageRole::Assistant,
                content: "ran the suite — 3 tests failed".into(),
            },
        )
        .await;

    let chip = emitted
        .iter()
        .find(|c| c.rule_id == "tests_failed")
        .expect("tests_failed chip should fire on regex match");
    assert_eq!(chip.action, ChipAction::OpenTestFailure);
}
