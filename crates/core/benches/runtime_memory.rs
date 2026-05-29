//! Criterion benchmark: idle Core RSS < 100 MB (Task 50).
//!
//! Spawns a `concerto-core` subprocess via `concerto-test-harness`, lets
//! it idle for 10 s so any lazy initialization settles, then samples the
//! resident-set size via `ps -o rss= -p <pid>` and asserts the value is
//! under the 100 MB budget from `design/00 §7.7`.
//!
//! ## Why `ps` and not `procfs` / `mach2`
//!
//! Per Task 50 pre-decision 1: shelling out to `ps -o rss=` works on
//! both macOS and Linux without pulling in platform-specific deps
//! (`procfs` is Linux-only, `mach2` is Darwin-only). The numbers are
//! reported in KB on both platforms — same column semantics — so the
//! parsing branch is a single integer.
//!
//! ## Why a Criterion bench at all
//!
//! We use Criterion as the wrapper because Task 29 already pulls
//! `criterion` in for `concerto-gix-wrap`, but this bench performs a
//! single measurement (sample size = 10) and treats Criterion as a
//! printf harness. The actual budget gate is the `assert!(rss < BUDGET)`
//! inside the iter closure plus the CI workflow `grep`-ing the printed
//! RSS line (Task 50 pre-decision 9).
//!
//! ## CI gating
//!
//! Gated behind the `mem-bench` Cargo feature so plain `cargo test` does
//! not compile it. `cargo bench -p concerto-core --features mem-bench`
//! runs it. CI's `perf.yml` invokes the feature explicitly. Unix-only
//! because `concerto-test-harness` is `cfg(unix)`; the off-feature /
//! Windows fallback `main()` at the bottom keeps the bench binary
//! linkable on every host.

#[cfg(all(feature = "mem-bench", unix))]
use std::process::Command;
#[cfg(all(feature = "mem-bench", unix))]
use std::time::Duration;

#[cfg(all(feature = "mem-bench", unix))]
use criterion::{criterion_group, criterion_main, Criterion};

#[cfg(all(feature = "mem-bench", unix))]
use concerto_test_harness::CoreUnderTest;

/// Idle-RSS budget in kilobytes (100 MB per `design/00 §7.7`).
#[cfg(all(feature = "mem-bench", unix))]
const IDLE_RSS_BUDGET_KB: u64 = 100 * 1024;

/// Idle settle time before the RSS sample. 10 s matches the task spec.
#[cfg(all(feature = "mem-bench", unix))]
const IDLE_SETTLE: Duration = Duration::from_secs(10);

/// Read RSS (kilobytes) for `pid` by shelling out to `ps -o rss= -p
/// <pid>`. `ps` reports RSS in 1 KiB units on both macOS and Linux for
/// the `rss` column, so the parsed integer is comparable cross-platform.
///
/// Returns `None` if `ps` exits non-zero (process has already died) or
/// if the output cannot be parsed as a `u64`.
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

#[cfg(all(feature = "mem-bench", unix))]
fn bench_idle_memory(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio rt");

    let mut group = c.benchmark_group("core_idle_rss");
    // Spawning Core + idling 10 s + sampling RSS is ~12 s per iteration.
    // Criterion's default 100 samples would blow well past CI budgets;
    // 10 is enough for the locked budget gate.
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(180));

    group.bench_function("idle_10s", |b| {
        b.iter(|| {
            let rss_kb = rt.block_on(async {
                let core = CoreUnderTest::spawn().await.expect("spawn core");
                let pid = core.pid().expect("pid for live core");
                tokio::time::sleep(IDLE_SETTLE).await;
                let rss = read_rss_kb(pid).expect("ps -o rss=");
                core.shutdown().await.expect("shutdown");
                rss
            });
            // Print so CI can `grep` for the line (pre-decision 9).
            println!("CONCERTO_PERF idle_rss_kb={rss_kb} budget_kb={IDLE_RSS_BUDGET_KB}");
            assert!(
                rss_kb < IDLE_RSS_BUDGET_KB,
                "idle Core RSS {rss_kb} KB exceeded 100 MB budget ({IDLE_RSS_BUDGET_KB} KB)"
            );
        });
    });
    group.finish();
}

#[cfg(all(feature = "mem-bench", unix))]
criterion_group!(benches, bench_idle_memory);
#[cfg(all(feature = "mem-bench", unix))]
criterion_main!(benches);

#[cfg(not(all(feature = "mem-bench", unix)))]
fn main() {
    // The bench binary always needs a `main`. `required-features =
    // ["mem-bench"]` in Cargo.toml prevents Cargo from invoking this
    // fallback under `cargo bench`, but the file still has to link
    // when the feature is off (or on non-Unix hosts) so plain
    // `cargo check --workspace --all-targets` succeeds.
    eprintln!("runtime_memory bench requires --features mem-bench on a Unix host; skipping");
}
