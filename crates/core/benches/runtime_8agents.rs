//! Criterion benchmark: Core RSS with 8 active agents < 600 MB (Task 50).
//!
//! Spawns a fresh Core, seeds a project + repo + workspace + workarea via
//! the same direct-DB pattern `crates/core/tests/sessions_grpc.rs` uses,
//! then fires 8 `agent_kind=echo` sessions in parallel against the same
//! workarea. Once every CreateSession has returned, samples Core RSS via
//! `ps -o rss=` and asserts the value is under the 600 MB budget from
//! `design/00 §7.7`.
//!
//! ## V0.1 simplification (Task 50 pre-decision 3)
//!
//! Echo sessions finish fast — they run `concerto-agent-host` in
//! `agent_kind=echo` mode and exit almost immediately. Holding eight
//! concurrent agent processes alive for a sustained interval would
//! require either a long-running echo loop or an immediate
//! respawn-on-finish loop. V0.1 simplifies to "spawn 8 sessions in
//! parallel; immediately read RSS at peak" — the eight
//! `concerto-agent-host` children plus the Core's per-session
//! bookkeeping are all live during the burst, which is the load the
//! budget is meant to cover. A follow-up task can promote this to a
//! sustained load test if the V0.1 budget proves too coarse.
//!
//! ## CI gating
//!
//! Heavy: spawns 9 subprocesses (1 Core + 8 agent-hosts) per iteration
//! and stresses the test environment. Gated behind:
//!   - the `mem-bench` Cargo feature (compile-time, via
//!     `required-features` on the `[[bench]]` entry), and
//!   - the routine `perf.yml` job which only runs `runtime_memory` —
//!     this bench is invoked manually / from nightly per Task 50's
//!     "`--ignored` semantics" pre-decision.

#[cfg(all(feature = "mem-bench", unix))]
use std::path::Path;
#[cfg(all(feature = "mem-bench", unix))]
use std::process::Command;
#[cfg(all(feature = "mem-bench", unix))]
use std::time::Duration;

#[cfg(all(feature = "mem-bench", unix))]
use criterion::{criterion_group, criterion_main, Criterion};
#[cfg(all(feature = "mem-bench", unix))]
use tempfile::TempDir;

#[cfg(all(feature = "mem-bench", unix))]
use concerto_proto::v1::{CreateSessionRequest, CreateWorkareaRequest};
#[cfg(all(feature = "mem-bench", unix))]
use concerto_test_harness::CoreUnderTest;

/// 8-agent RSS budget in kilobytes (600 MB per `design/00 §7.7`,
/// excluding the agent processes themselves).
#[cfg(all(feature = "mem-bench", unix))]
const EIGHT_AGENT_RSS_BUDGET_KB: u64 = 600 * 1024;

/// Number of concurrent echo sessions to spawn.
#[cfg(all(feature = "mem-bench", unix))]
const N_AGENTS: usize = 8;

/// Settle interval between "all sessions created" and the RSS sample.
/// Gives the supervisor's per-session bookkeeping a beat to land in
/// memory before we measure.
#[cfg(all(feature = "mem-bench", unix))]
const PEAK_SETTLE: Duration = Duration::from_secs(2);

/// Read RSS (KB) for `pid` via `ps -o rss=`. Mirrors `runtime_memory.rs`'s
/// implementation — duplicated rather than shared because bench files
/// don't share a module path.
#[cfg(all(feature = "mem-bench", unix))]
fn read_rss_kb(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().parse::<u64>().ok()
}

/// Shell out to git for fixture setup. Panic on failure — meaningless to
/// continue without a usable bare repo.
#[cfg(all(feature = "mem-bench", unix))]
async fn git(args: &[&str], cwd: &Path) {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "bench")
        .env("GIT_AUTHOR_EMAIL", "bench@example.com")
        .env("GIT_COMMITTER_NAME", "bench")
        .env("GIT_COMMITTER_EMAIL", "bench@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(all(feature = "mem-bench", unix))]
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
    let url = format!("file://{}", bare.path().display());
    git(&["remote", "add", "origin", url.as_str()], work.path()).await;
    git(&["push", "-u", "origin", "main"], work.path()).await;
    (url, bare, work)
}

#[cfg(all(feature = "mem-bench", unix))]
struct Seeded {
    workspace_id: String,
    _bare: TempDir,
    _work: TempDir,
}

/// Direct-DB project/repo/workspace seed. Mirrors
/// `sessions_grpc.rs::seed` — the bench needs the same on-disk state (a
/// cloned repo at the expected `local_path`) for
/// `Workareas.CreateWorkarea` to succeed.
#[cfg(all(feature = "mem-bench", unix))]
async fn seed(core: &CoreUnderTest, slug: &str) -> Seeded {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let (bare_url, bare, work) = make_bare_with_commit().await;

    let project_id = format!("proj-{slug}");
    let workspace_id = format!("ws-{slug}");
    let repo_id = format!("repo-{slug}");
    let repo_name = format!("name-{slug}");
    let local_path = core.data_dir.join("repos").join(&repo_id);

    let opts = SqliteConnectOptions::new()
        .filename(&core.db_path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open db write pool");
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, 'bench', 0)")
        .bind(&project_id)
        .execute(&pool)
        .await
        .expect("insert project");
    sqlx::query(
        "INSERT INTO repositories (id, project_id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, ?, 'full', 'main')",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&repo_name)
    .bind(&bare_url)
    .bind(local_path.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .expect("insert repository");
    sqlx::query(
        "INSERT INTO workspaces (id, project_id, name, slug, created_at) VALUES (?, ?, 'bench', ?, 0)",
    )
    .bind(&workspace_id)
    .bind(&project_id)
    .bind(slug)
    .execute(&pool)
    .await
    .expect("insert workspace");
    sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
        .bind(&workspace_id)
        .bind(&repo_id)
        .execute(&pool)
        .await
        .expect("insert workspace_repos");
    pool.close().await;

    // Clone the bare so `Workareas.CreateWorkarea` finds the on-disk
    // repo.
    tokio::fs::create_dir_all(local_path.parent().unwrap())
        .await
        .unwrap();
    let out = tokio::process::Command::new("git")
        .args(["clone", bare_url.as_str(), &local_path.to_string_lossy()])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("git clone");
    assert!(
        out.status.success(),
        "seed clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    Seeded {
        workspace_id,
        _bare: bare,
        _work: work,
    }
}

#[cfg(all(feature = "mem-bench", unix))]
fn bench_8agents_memory(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio rt");

    let mut group = c.benchmark_group("core_8agents_rss");
    // One iteration burns ~30 s of wall (seed + 8 spawns + settle +
    // shutdown). Criterion's stat defaults would multiply that by 100;
    // 10 samples is enough for the locked budget gate.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(300));

    group.bench_function("peak_after_8_spawns", |b| {
        b.iter(|| {
            let rss_kb = rt.block_on(async {
                let core = CoreUnderTest::spawn().await.expect("spawn core");
                let s = seed(&core, "bench").await;

                let mut wac = core.workareas_client().await.expect("workareas client");
                let wa = wac
                    .create_workarea(CreateWorkareaRequest {
                        workspace_id: s.workspace_id.clone(),
                        permission_mode: None,
                    })
                    .await
                    .expect("CreateWorkarea")
                    .into_inner();

                // Spawn N echo sessions in parallel. Each spawn dials a
                // fresh `SessionsClient` so the calls don't serialize on
                // a single channel's mutex.
                let mut handles = Vec::with_capacity(N_AGENTS);
                for _ in 0..N_AGENTS {
                    let socket = core.socket_path.clone();
                    let workarea_id = wa.id.clone();
                    handles.push(tokio::spawn(async move {
                        let mut client =
                            concerto_test_harness::clients::sessions_client(socket)
                                .await
                                .expect("sessions client");
                        client
                            .create_session(CreateSessionRequest {
                                workarea_id,
                                agent_kind: "echo".to_string(),
                                model: None,
                                permission_mode: None,
                            })
                            .await
                            .expect("CreateSession")
                            .into_inner()
                    }));
                }
                for h in handles {
                    let _ = h.await.expect("join CreateSession");
                }

                tokio::time::sleep(PEAK_SETTLE).await;
                let pid = core.pid().expect("pid for live core");
                let rss = read_rss_kb(pid).expect("ps -o rss=");
                core.shutdown().await.expect("shutdown");
                rss
            });
            println!(
                "CONCERTO_PERF eight_agent_rss_kb={rss_kb} budget_kb={EIGHT_AGENT_RSS_BUDGET_KB}"
            );
            assert!(
                rss_kb < EIGHT_AGENT_RSS_BUDGET_KB,
                "8-agent Core RSS {rss_kb} KB exceeded 600 MB budget ({EIGHT_AGENT_RSS_BUDGET_KB} KB)"
            );
        });
    });
    group.finish();
}

#[cfg(all(feature = "mem-bench", unix))]
criterion_group!(benches, bench_8agents_memory);
#[cfg(all(feature = "mem-bench", unix))]
criterion_main!(benches);

#[cfg(not(all(feature = "mem-bench", unix)))]
fn main() {
    // See `runtime_memory.rs` for the rationale on this fallback.
    eprintln!("runtime_8agents bench requires --features mem-bench on a Unix host; skipping");
}
