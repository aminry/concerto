//! Tier-2 tests for Task 315: the inbound-webhook ingest pipeline
//! (`VcsHandle::ingest_webhook`, `design/13 §5.1`/§6.2/§8) and the Core's
//! `WebhookSink` mapping.
//!
//! The pipeline order is FROZEN (`design/13 §6.2`): idempotency-first
//! (delivery-id, restart-surviving via migration 0013) → constant-time
//! HMAC-SHA256 (per-repo `VcsSecretSlot::WebhookSecret`) → parse → targeted
//! cache invalidation (best-effort re-fetch + emit via 316's `ChecksAggregator`
//! seams). These tests drive that pipeline against 313's `testkit` `FakeGitHub`
//! (the re-fetch side) with the per-repo secret + provider seams injected as
//! fakes — no OS keychain, no real GitHub.
//!
//! What the loopback double does NOT cover (→ Phase-3 Tier-3 checklist): real
//! GitHub computing a real `X-Hub-Signature-256`, real webhook delivery over a
//! real relay on real infra, and GitHub's real redelivery policy. The relay
//! route ↔ Core `0x04` transport hop is covered by
//! `crates/relay/tests/webhook_forward.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use concerto_error::Result;
use concerto_persist::{
    NewProject, NewRepository, NewWorkarea, NewWorkareaRepo, NewWorkspace, Persistence,
    PersistenceConfig, ProjectId, RepositoryId, WorkareaId, WorkspaceId,
};
use concerto_vcs::provider::VcsProvider;
use concerto_vcs::testkit::{fixture, FakeGitHub};
use concerto_vcs::webhook::{
    IngestOutcome, WebhookPayload, WebhookProviderSource, WebhookSecretSource,
};
use concerto_vcs::VcsHandle;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tempfile::TempDir;

const REPO_FULL: &str = "owner/repo";
const HEAD_SHA: &str = "abc123";
const SECRET: &[u8] = b"itsasecret";

async fn make_persistence(tmp: &TempDir) -> Arc<Persistence> {
    let cfg = PersistenceConfig {
        db_path: tmp.path().join("concerto.db"),
        max_readers: 2,
    };
    Arc::new(Persistence::open(cfg).await.expect("open persistence"))
}

/// Seed the FK chain a `pull_requests` cache row needs + one PR row at `HEAD_SHA`
/// so the `check_run` targeted-invalidation has a row to locate.
async fn seed(persist: &Persistence) -> (WorkareaId, RepositoryId) {
    let project_id = ProjectId(format!("proj-{}", uuid::Uuid::now_v7()));
    let repo_id = RepositoryId(format!("repo-{}", uuid::Uuid::now_v7()));
    let workspace_id = WorkspaceId(format!("ws-{}", uuid::Uuid::now_v7()));
    let workarea_id = WorkareaId(format!("wa-{}", uuid::Uuid::now_v7()));
    let mut w = persist.writer().await;
    concerto_persist::projects::insert(
        &mut w,
        NewProject {
            id: project_id.clone(),
            name: "wh-test".into(),
            icon: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    concerto_persist::repositories::insert(
        &mut w,
        NewRepository {
            id: repo_id.clone(),
            project_id: project_id.0.clone(),
            name: "repo".into(),
            url: format!("https://github.com/{REPO_FULL}"),
            local_path: "/tmp/repo".into(),
            clone_strategy: "full".into(),
            default_branch: "main".into(),
        },
    )
    .await
    .unwrap();
    concerto_persist::workspaces::insert(
        &mut w,
        NewWorkspace {
            id: workspace_id.clone(),
            project_id: project_id.0.clone(),
            name: "wh-ws".into(),
            slug: "wh-ws".into(),
            description: None,
            permission_mode: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    concerto_persist::workspaces::update_repos(
        &mut w,
        &workspace_id,
        std::slice::from_ref(&repo_id),
    )
    .await
    .unwrap();
    concerto_persist::workareas::insert(
        &mut w,
        NewWorkarea {
            id: workarea_id.clone(),
            workspace_id: workspace_id.0.clone(),
            composer_name: "bach".into(),
            branch_name: "concerto/bach".into(),
            worktree_root: "/tmp/wa".into(),
            status: "active".into(),
            permission_mode: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    concerto_persist::workareas::insert_workarea_repo(
        &mut w,
        NewWorkareaRepo {
            workarea_id: workarea_id.clone(),
            repository_id: repo_id.clone(),
            worktree_path: "/tmp/wa/repo".into(),
            branch_override: None,
            sparse_cones_json: NewWorkareaRepo::empty_cones(),
        },
    )
    .await
    .unwrap();
    // A cached PR at HEAD_SHA so `workareas_for_sha` finds a row to invalidate.
    concerto_persist::pull_requests::upsert(
        &mut w,
        concerto_persist::NewPullRequest {
            id: concerto_persist::PullRequestId(uuid::Uuid::now_v7().to_string()),
            workarea_id: workarea_id.clone(),
            repository_id: repo_id.clone(),
            provider: "github".into(),
            pr_number: 7,
            base_ref: "main".into(),
            head_ref: "feature".into(),
            state: "open".into(),
            title: "t".into(),
            body: String::new(),
            url: String::new(),
            head_sha: HEAD_SHA.into(),
            created_at: 1,
            updated_at: 1,
        },
    )
    .await
    .unwrap();
    (workarea_id, repo_id)
}

/// A fixed-secret source returning `SECRET` for any repo (or `None` to model an
/// unconfigured hook).
struct FakeSecret {
    secret: Option<Vec<u8>>,
}

#[async_trait]
impl WebhookSecretSource for FakeSecret {
    async fn webhook_secret(&self, _repo_id: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.secret.clone())
    }
}

/// A provider source backed by the `testkit` `FakeGitHub` (so the
/// targeted-invalidation re-fetch hits the recorded fixtures, not real GitHub).
/// `None` models "no credential wired" — the cache rows are still dropped, so the
/// invalidation is a no-op (the webhook stays a strict accelerator).
struct FakeProvider {
    provider: Option<Arc<dyn VcsProvider>>,
}

#[async_trait]
impl WebhookProviderSource for FakeProvider {
    async fn provider_for(&self, _repo_full_name: &str) -> Result<Option<Arc<dyn VcsProvider>>> {
        Ok(self.provider.clone())
    }
}

/// Compute the GitHub `sha256=<hex>` header for `body` under `secret`.
fn sign(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(body);
    let hex: String = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256={hex}")
}

/// Build a handle wired with the fake secret + (optional) FakeGitHub provider.
async fn handle_with(
    persist: Arc<Persistence>,
    secret: Option<Vec<u8>>,
    gh: Option<&FakeGitHub>,
) -> VcsHandle {
    let secret_source = Arc::new(FakeSecret { secret });
    let provider: Option<Arc<dyn VcsProvider>> =
        gh.map(|gh| Arc::new(gh.provider()) as Arc<dyn VcsProvider>);
    let provider_source = Arc::new(FakeProvider { provider });
    VcsHandle::new(persist).with_webhook_sources(secret_source, provider_source)
}

fn check_run_body() -> Vec<u8> {
    format!(r#"{{"action":"completed","check_run":{{"head_sha":"{HEAD_SHA}"}},"repository":{{"full_name":"{REPO_FULL}"}}}}"#)
        .into_bytes()
}

// ---------------------------------------------------------------------------

/// A correct signature on a known `check_run` body verifies and is accepted; a
/// flipped signature byte is rejected (4xx) with no cache mutation.
#[tokio::test]
async fn hmac_good_accepts_bad_rejects() {
    let tmp = tempfile::tempdir().unwrap();
    let persist = make_persistence(&tmp).await;
    let (_wa, repo) = seed(&persist).await;
    let gh = FakeGitHub::start().await;
    gh.mount_get_json(
        &format!("/repos/{REPO_FULL}/commits/{HEAD_SHA}/check-runs"),
        fixture("check_runs.json"),
    )
    .await;
    let handle = handle_with(persist, Some(SECRET.to_vec()), Some(&gh)).await;

    let body = check_run_body();
    let good = WebhookPayload {
        delivery_id: "d-good".into(),
        signature_256: sign(SECRET, &body),
        event_type: "check_run".into(),
        body: body.clone(),
    };
    assert_eq!(
        handle.ingest_webhook(&repo, good).await.unwrap(),
        IngestOutcome::Accepted
    );

    // A flipped signature → reject; a fresh delivery-id so idempotency doesn't
    // short-circuit before HMAC.
    let mut bad_sig: Vec<char> = sign(SECRET, &body).chars().collect();
    let last = bad_sig.len() - 1;
    bad_sig[last] = if bad_sig[last] == '0' { '1' } else { '0' };
    let bad = WebhookPayload {
        delivery_id: "d-bad".into(),
        signature_256: bad_sig.into_iter().collect(),
        event_type: "check_run".into(),
        body,
    };
    assert_eq!(
        handle.ingest_webhook(&repo, bad).await.unwrap(),
        IngestOutcome::Reject
    );
}

/// A replay (same delivery-id) is dropped with `200` and never re-processed —
/// only ONE re-fetch emit reaches the broadcast across two identical deliveries.
#[tokio::test]
async fn replay_drops_no_double_update() {
    let tmp = tempfile::tempdir().unwrap();
    let persist = make_persistence(&tmp).await;
    let (wa, repo) = seed(&persist).await;
    let gh = FakeGitHub::start().await;
    gh.mount_get_json(
        &format!("/repos/{REPO_FULL}/commits/{HEAD_SHA}/check-runs"),
        fixture("check_runs.json"),
    )
    .await;
    let handle = handle_with(persist, Some(SECRET.to_vec()), Some(&gh)).await;
    let mut rx = handle.checks().subscribe();

    let body = check_run_body();
    let payload = WebhookPayload {
        delivery_id: "d-1".into(),
        signature_256: sign(SECRET, &body),
        event_type: "check_run".into(),
        body,
    };
    // First delivery: verified + processed + emits one check-run frame.
    assert_eq!(
        handle.ingest_webhook(&repo, payload.clone()).await.unwrap(),
        IngestOutcome::Accepted
    );
    let first = rx.try_recv().expect("first delivery emits");
    assert_eq!(first.workarea_id, wa.0);

    // Replay (same delivery-id): dropped with 200, NO second emit, NO second
    // HTTP fetch.
    let before = gh.server().received_requests().await.unwrap().len();
    assert_eq!(
        handle.ingest_webhook(&repo, payload).await.unwrap(),
        IngestOutcome::Accepted,
        "replay still acks 200 so GitHub stops retrying"
    );
    assert!(rx.try_recv().is_err(), "replay must not emit again");
    assert_eq!(
        gh.server().received_requests().await.unwrap().len(),
        before,
        "replay must not re-fetch (no double-update)"
    );
}

/// An unknown event type is a no-op 200 (forward-compat) — accepted, no emit.
#[tokio::test]
async fn unknown_event_noops_200() {
    let tmp = tempfile::tempdir().unwrap();
    let persist = make_persistence(&tmp).await;
    let (_wa, repo) = seed(&persist).await;
    let handle = handle_with(persist, Some(SECRET.to_vec()), None).await;
    let mut rx = handle.checks().subscribe();

    let body = br#"{"zen":"hi","repository":{"full_name":"owner/repo"}}"#.to_vec();
    let payload = WebhookPayload {
        delivery_id: "d-ping".into(),
        signature_256: sign(SECRET, &body),
        event_type: "ping".into(),
        body,
    };
    assert_eq!(
        handle.ingest_webhook(&repo, payload).await.unwrap(),
        IngestOutcome::Accepted
    );
    assert!(rx.try_recv().is_err(), "unknown event emits nothing");
}

/// A repo with no configured secret is dropped (4xx) with no sender-visible
/// reason (`design/13 §8`) — even with a syntactically valid signature.
#[tokio::test]
async fn missing_secret_drops() {
    let tmp = tempfile::tempdir().unwrap();
    let persist = make_persistence(&tmp).await;
    let (_wa, repo) = seed(&persist).await;
    let handle = handle_with(persist, None, None).await;

    let body = check_run_body();
    let payload = WebhookPayload {
        delivery_id: "d-nosecret".into(),
        // A signature that would verify if a secret existed — proves the drop is
        // on "no secret configured", not on the tag.
        signature_256: sign(SECRET, &body),
        event_type: "check_run".into(),
        body,
    };
    assert_eq!(
        handle.ingest_webhook(&repo, payload).await.unwrap(),
        IngestOutcome::Reject
    );
}

/// Restart-surviving idempotency: a redelivery after re-opening the SAME db file
/// (a Core bounce) is still deduped.
#[tokio::test]
async fn dedup_survives_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let body = check_run_body();
    let payload = WebhookPayload {
        delivery_id: "d-restart".into(),
        signature_256: sign(SECRET, &body),
        event_type: "check_run".into(),
        body,
    };
    let repo = {
        let persist = make_persistence(&tmp).await;
        let (_wa, repo) = seed(&persist).await;
        let handle = handle_with(Arc::clone(&persist), Some(SECRET.to_vec()), None).await;
        assert_eq!(
            handle.ingest_webhook(&repo, payload.clone()).await.unwrap(),
            IngestOutcome::Accepted
        );
        repo
    };
    // Reopen the same db (restart). The delivery is still recorded → replay.
    let persist = make_persistence(&tmp).await;
    let handle = handle_with(persist, Some(SECRET.to_vec()), None).await;
    assert_eq!(
        handle.ingest_webhook(&repo, payload).await.unwrap(),
        IngestOutcome::Accepted,
        "a redelivery after restart is a dedup (still 200)"
    );
}
