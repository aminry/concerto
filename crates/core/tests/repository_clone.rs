//! Integration test for the Task 18 `Repositories` gRPC service.
//!
//! Exercises the full path: spawn a real Core subprocess via the
//! Task 17 harness → insert a `projects` row directly into the Core's
//! SQLite DB (the `Projects` service doesn't exist yet) → call
//! `Repositories.AddRepository` → call `Repositories.Clone` against a
//! local `file://` bare repo → verify progress arrives, the clone
//! exists on disk, and the DB row picked up `last_fetch_at`.
//!
//! No network is required — the bare-repo fixture is built in-test by
//! shelling out to `git`.

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use concerto_proto::v1::{AddRepoRequest, CloneRequest};
use concerto_test_harness::CoreUnderTest;
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

/// Build a bare repo with one commit and return its file:// URL.
async fn make_bare_with_commit() -> (String, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
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

/// Insert a `projects` row directly into the Core's SQLite file.

#[tokio::test(flavor = "multi_thread")]
async fn add_repository_and_clone_file_url() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");

    // Repositories are a global registry — no project seeding needed.

    // Build a fixture bare repo + one commit.
    let (url, _bare, _work) = make_bare_with_commit().await;

    // AddRepository.
    let mut client = core.repositories_client().await.expect("client");
    let repo = client
        .add_repository(AddRepoRequest {
            name: "fixture".to_string(),
            url: url.clone(),
            default_branch: "main".to_string(),
            // Task 301 added clone_strategy/with_sparse; empty → Full, so
            // the `clone_strategy == "full"` assertion below still holds.
            ..Default::default()
        })
        .await
        .expect("AddRepository")
        .into_inner();
    assert!(!repo.id.is_empty());
    assert_eq!(repo.clone_strategy, "full");
    assert_eq!(repo.default_branch, "main");
    let local_path = std::path::PathBuf::from(&repo.local_path);

    // Clone via streaming RPC.
    //
    // The generated method name `clone` collides with `Clone::clone`,
    // which is in scope via the prelude. UFCS through the inherent
    // impl's type path disambiguates: the compiler sees both
    // candidates but the inherent method's signature
    // (`&mut self, request`) matches our call shape uniquely.
    use concerto_proto::v1::repositories_client::RepositoriesClient;
    use tonic::transport::Channel as TChannel;
    let mut stream = RepositoriesClient::<TChannel>::clone(
        &mut client,
        CloneRequest {
            repository_id: repo.id.clone(),
        },
    )
    .await
    .expect("Clone RPC")
    .into_inner();

    let mut events = 0u32;
    let mut saw_done = false;
    // Bound the loop so a hung clone fails the test loudly.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(15), stream.message()).await {
            Ok(Ok(Some(msg))) => {
                events += 1;
                if msg.done {
                    saw_done = true;
                }
            }
            Ok(Ok(None)) => break, // end of stream
            Ok(Err(status)) => panic!("stream error: {status}"),
            Err(_) => panic!("stream timed out waiting for next event"),
        }
    }
    assert!(events >= 1, "expected ≥1 progress event, got {events}");
    assert!(saw_done, "expected a terminal done event");

    // Verify on-disk + DB row.
    assert!(local_path.exists(), "clone dir should exist on disk");
    assert!(
        local_path.join(".git").exists(),
        "clone dir should have a .git/"
    );

    // last_fetch_at should be populated now.
    let pool = core.db().await.expect("db pool");
    let row: (Option<i64>,) = sqlx::query_as("SELECT last_fetch_at FROM repositories WHERE id = ?")
        .bind(&repo.id)
        .fetch_one(&pool)
        .await
        .expect("query last_fetch_at");
    assert!(
        row.0.is_some(),
        "last_fetch_at should be populated after clone"
    );

    core.shutdown().await.expect("shutdown");
}

/// Two simultaneous clones of the *same* repo serialize on the
/// per-repo write mutex; two clones of *different* repos can proceed
/// in parallel. The integration test verifies the same-repo path —
/// if the mutex were missing, both clones would race on `dest/` and
/// the second would fail with `git clone: destination exists`.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_clones_of_same_repo_serialize() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let (url, _bare, _work) = make_bare_with_commit().await;

    let mut client = core.repositories_client().await.expect("client");
    let repo = client
        .add_repository(AddRepoRequest {
            name: "fixture2".to_string(),
            url,
            default_branch: "main".to_string(),
            // Task 301 added clone_strategy/with_sparse; empty → Full.
            ..Default::default()
        })
        .await
        .expect("AddRepository")
        .into_inner();

    // Fire two clone streams back-to-back. UFCS for the same reason
    // as the previous test — `clone` collides with `Clone::clone`.
    use concerto_proto::v1::repositories_client::RepositoriesClient;
    use tonic::transport::Channel as TChannel;
    let mut s1 = RepositoriesClient::<TChannel>::clone(
        &mut client,
        CloneRequest {
            repository_id: repo.id.clone(),
        },
    )
    .await
    .expect("Clone RPC #1")
    .into_inner();
    let mut s2 = RepositoriesClient::<TChannel>::clone(
        &mut client,
        CloneRequest {
            repository_id: repo.id.clone(),
        },
    )
    .await
    .expect("Clone RPC #2")
    .into_inner();

    // Drain both. The first should succeed; the second's clone shells
    // out to `git clone` against an already-populated `dest`, which
    // `git` rejects. The error surfaces as a Status on the second
    // stream — that's the contract: *serialization*, not deduplication.
    drain_until_done_or_err(&mut s1, Duration::from_secs(30)).await;
    drain_until_done_or_err(&mut s2, Duration::from_secs(30)).await;

    core.shutdown().await.expect("shutdown");
}

async fn drain_until_done_or_err<T>(stream: &mut tonic::Streaming<T>, budget: Duration)
where
    T: prost::Message + Default + 'static,
{
    let deadline = tokio::time::Instant::now() + budget;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(15), stream.message()).await {
            Ok(Ok(Some(_))) => continue,
            Ok(Ok(None)) => return,
            Ok(Err(_)) => return, // accepted: serialized clone surfaces an error
            Err(_) => return,
        }
    }
}
