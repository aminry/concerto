# Task 304 — Idle Blob Pre-fetch (AC + idle) + fsmonitor Supervision + Maintenance Schedule

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 301 |
| Touches subsystem(s) | 02 (Repository Manager) |
| Smoke gate | unchanged |

## Goal
Make a blobless+sparse repo (Tasks 301/302) actually usable offline-ish by materializing blobs ahead of agent need, with the three pre-fetch triggers `design/02 §3.3` specifies — **at worktree-create**, **eagerly on HEAD update** (default ON), and **idle-background** (default ON) — plus the rate-limit + cancellation policy from `§6.1`/`§6.3`, while extending (not rebuilding) the already-shipped fsmonitor restart-if-dead supervision. Today `crates/core/src/repo_manager/fsmonitor.rs` already supervises the daemon (the 30 s `spawn_supervisor` loop + `RestartHistory` 3-in-60 s cap + `bring_up_after_clone`), and `gix-wrap` already has `register_maintenance` (best-effort `git maintenance start`), but there is **no prewarm path at all**. This task adds: (a) a `prewarm_blobs(repo, cones, commit) → PrewarmHandle` helper (shell-out `git fetch` for the blobs reachable in-cone @ commit) that is cancellable; (b) an **idle scheduler loop** (mirroring `spawn_supervisor`) gated on AC + non-metered Wi-Fi + idle-longer-than-threshold (default 300 s), with the global 2-concurrent + per-repo bandwidth cap; (c) the **idle signal injected as a testable closure/trait** (like fsmonitor's `is_alive`) so the scheduler is CI-provable, with the real client-heartbeat wiring left as a documented small follow-on; (d) the eager triggers (worktree-create + HEAD-update) wired fully; (e) a `PrewarmBlobs(PrewarmRequest) → stream PrewarmProgress` RPC. After this task an idle, plugged-in machine pre-fetches cone blobs in the background (provably, with injected signals), and the fsmonitor supervisor is unchanged but its restart policy is reused for the prewarm scheduler's structure.

## Inputs to read before starting
- `design/02_Repository_Manager.md` §3.3 — the **three pre-fetch triggers** (at-worktree-create; eager-on-HEAD-update, default ON; idle-background, default ON) + the idle threshold (default **5 min**, Settings → Performance, idle signal from the Local API) + "rate-limited and pausable; Tray surfaces 'syncing'."
- `design/02_Repository_Manager.md` §6.1 — concurrency: **one write per repo** (existing per-repo `Mutex`); **pre-fetch global-rate-limited to N concurrent (default 2)** + a per-repo bandwidth cap; clone uses streaming progress.
- `design/02_Repository_Manager.md` §6.3 — the idle scheduler loop: check AC + Wi-Fi(non-metered) + idle > threshold; for each sparse+blobless repo walk cones for missing blobs @ HEAD of tracked branches; enqueue + drain with bandwidth limit; **cancellable on user activity**; the idle signal comes from the Local API (client heartbeats).
- `design/02_Repository_Manager.md` §3.4 — fsmonitor lifecycle (already built; this task does NOT rebuild it — read to confirm the supervisor contract you extend the *structure* of).
- `design/02_Repository_Manager.md` §5.1 — `prewarm_blobs(repo, cones, commit) → PrewarmHandle` (cancellable) — the Rust API signature you implement. §5.2 — `PrewarmBlobs(PrewarmRequest) → stream PrewarmProgress` gRPC. §5.3 — emit `repo.prefetch_started/finished` (broadcast).
- `design/02_Repository_Manager.md` §12 R-2 (Wi-Fi unlimited / metered off — OS reports metered status) + R-3 (idle threshold default 5 min, configurable).
- `crates/core/src/repo_manager/fsmonitor.rs` — `spawn_supervisor` (the 30 s loop to MIRROR for the idle scheduler's shape), `RestartHistory`/`record_restart` (the rate-cap pattern), `bring_up_after_clone` (post-clone bring-up; the worktree-create prewarm trigger hangs near here), `probe_all`/`probe_one` (the injected-`is_alive`-closure pattern to MIRROR for the injected idle signal). **fsmonitor restart-if-dead is DONE (Task 28) — extend the module with prewarm, do not duplicate the daemon supervision.**
- `crates/gix-wrap/src/api.rs` — `fetch` (the incremental fetch primitive), `register_maintenance` (best-effort `git maintenance start`; reuse for the weekly schedule), `cmd::run` (shell-out for the targeted blob fetch). `crates/core/src/repo_manager/actor.rs` — `RepoManager` (where `prewarm_blobs` + the scheduler handle live; the per-repo write-lock map is here).
- `crates/core/src/boot.rs` — where the idle-signal source (Local API client heartbeats) would wire; the seam is injected here, real wiring is the follow-on.
- `crates/proto/proto/concerto/v1/repositories.proto` — the header reserves `PrewarmBlobs`; append the RPC + `PrewarmRequest`/`PrewarmProgress`. Existing field numbers FROZEN. (Task 305 appends `EstimateConeSize`/`ConeStats` — coordinate so the two tasks don't collide on a message name; 304 owns `PrewarmBlobs`/`PrewarmProgress`, 305 owns `EstimateConeSize`/`ConeStats`.)
- `tasks/v1.0/301-blobless-treeless-clone.md` → "Handoff Notes" — blobless = lazy blobs; what `prewarm` must materialize + the `concerto-state.json` `prefetch_cursor` field (read-modify-write so 301's `size_bytes` is not clobbered).
- `tasks/v1.0/302-sparse-cone-lifecycle.md` → "Handoff Notes" — the cone resolver (which cones a repo's workareas have, so the scheduler knows what to walk).
- `tasks/v1.0/PHASE3_PLANNING.md` §2 (304 row: idle-prefetch "idle" signal **injected as a testable closure/trait** like fsmonitor's `is_alive` — scheduler CI-provable, real heartbeat wiring a small documented follow-on; eager triggers ship fully) + §4.6 (`PrewarmProgress` FROZEN by 304).

## Scope — in
- **`RepoManager::prewarm_blobs(repo, cones, commit) → PrewarmHandle`** — shell-out a targeted `git fetch` that materializes the blobs reachable in `cones` @ `commit` (e.g. a `git fetch origin <commit>` with a blob filter relaxation, or a `git cat-file --batch`-driven on-demand fetch loop over the in-cone tree). Cancellable: `PrewarmHandle` carries a cancellation token; dropping/aborting it stops the fetch. Respects the per-repo write-lock-free read path (prewarm is a fetch, serialized against clone/fetch via the existing per-repo mutex).
- **The idle scheduler loop** (`prefetch.rs`, mirroring `spawn_supervisor`): every tick, if `idle_signal()` reports idle > threshold AND on AC AND non-metered Wi-Fi, enqueue missing-blob prewarm jobs for each sparse+blobless repo's cones; drain with the **global 2-concurrent limit** + per-repo bandwidth cap; cancel in-flight jobs when `idle_signal()` flips to active. Emit `repo.prefetch_started/finished`.
- **Injected signals (the Tier-1 testability seam):** `idle_signal: Arc<dyn Fn() -> IdleState + Send + Sync>` (or a small trait) + `power_state: Arc<dyn Fn() -> PowerState>` injected into the scheduler, exactly like `probe_all`'s `is_alive: F` closure. Production passes a real (best-effort, macOS-first) implementation; tests pass a deterministic mock to drive every branch (idle→enqueue, active→cancel, on-battery→skip, metered→skip).
- **Eager triggers (ship fully):** (1) at worktree-create — after 302 sets the cone, kick a `prewarm_blobs` for that (workarea, repo) cone @ HEAD (settable; default ON). (2) on HEAD-update — when a repo's tracked branch advances, prewarm blobs touched by the new commits in each cone (default ON). Wire trigger (1) near `bring_up_after_clone`; trigger (2) hangs off the fetch/HEAD-advance path.
- **Maintenance schedule:** reuse `register_maintenance` (already best-effort `git maintenance start`) on clone bring-up; add the weekly cadence note (the schedule git itself manages — we just `start` it; the helper already swallows the CI-no-scheduler failure).
- **Settings:** the idle threshold (default 300 s) is read from resolved settings (Settings → Performance); for V1.0 a const default + a settings-key read is fine (the full resolver is Task 310 — read the key if present, else default). Metered/AC detection is OS-specific: macOS-first real impl, Windows/Linux best-effort stubs behind the injected closure.
- **proto + handler:** `PrewarmBlobs(PrewarmRequest) → stream PrewarmProgress` (streaming mirrors the existing `Clone` streaming in `handlers/repositories.rs`); handler delegates to `prewarm_blobs` + forwards progress.
- Tests (Tier 1): with injected mock signals — idle+AC+wifi → enqueue + drain (assert jobs ran); active → cancel in-flight; on-battery / metered → skip (no jobs); the global 2-concurrent limit holds; the per-repo bandwidth cap is honored (assert the rate-limiter is consulted); `record`-style restart history is untouched (fsmonitor supervision unchanged); `PrewarmHandle` cancellation stops the loop; `concerto-state.json` `prefetch_cursor` round-trips.

## Scope — out
- **The real idle-signal source (Local API client heartbeats)** — wired as a documented follow-on; 304 injects the closure + ships a best-effort default. Real AC/Wi-Fi/idle/bandwidth behavior on hardware is partly **Tier-3** (no power state in CI).
- **fsmonitor restart-if-dead** — already DONE (Task 28); 304 reuses the supervisor *structure* for the scheduler but does not touch the daemon supervision logic.
- **Cone-size telemetry / `EstimateConeSize` / `ConeStats`** — **Task 305** (304 owns `PrewarmBlobs`/`PrewarmProgress`; 305 owns `EstimateConeSize`/`ConeStats`).
- **The sparse-cone lifecycle** that defines which cones to walk — **Task 302** (304 reads the resolved cones).
- **Settings precedence resolver** — **Task 310**; 304 reads a single key with a default fallback, not the full three-layer walk.
- **Desktop "syncing" Tray surface + Settings → Performance UI** — Desktop tasks (322+); 304 emits the `repo.prefetch_started/finished` events the Tray renders.
- **Real metered/AC detection on Windows/Linux** — best-effort stubs behind the injected closure; the macOS path is the V1.0 real impl.

## Public interface this task locks
- **Rust (FROZEN), `crates/core/src/repo_manager`:** `pub async fn prewarm_blobs(&self, repo: &RepositoryId, cones: &[ConePath], commit: &str) -> Result<PrewarmHandle>`; `pub struct PrewarmHandle` (carries a `CancellationToken` + a `JoinHandle`; `cancel(self)` / `Drop` aborts). The injected-signal seam types: `IdleState { Idle(Duration) | Active }`, `PowerState { Ac | Battery }`, `NetState { WifiUnmetered | Metered | Other }` (or equivalent) + the scheduler's `Arc<dyn Fn() -> …>` injection points. FREEZE the closure signatures so tests + the real follow-on agree.
- **proto (FROZEN field numbers), `repositories.proto`:** `PrewarmRequest { string repository_id = 1; repeated string cone_paths = 2; string commit = 3; }`; **`PrewarmProgress { uint64 blobs_fetched = 1; uint64 blobs_total = 2; bool done = 3; }`** (exact per PHASE3_PLANNING §4.6); `rpc PrewarmBlobs(PrewarmRequest) returns (stream PrewarmProgress);` appended to `service Repositories` after Task 302's `SetCones`.
- **Policy constants (FROZEN):** global concurrency = 2 (`§6.1`); default idle threshold = 300 s (`§3.3`/R-3); the eager triggers default ON (`§3.3`).

## Implementation notes
- **Mirror, don't fork, `spawn_supervisor`.** The idle scheduler is the same shape: a `tokio::interval` loop, a `CancellationToken` for shutdown, injected closures for the un-CI-able inputs. Put it in a new `crates/core/src/repo_manager/prefetch.rs` and `spawn_prefetch_scheduler(...)` from `RepoManagerActor::run` right next to the existing `spawn_supervisor(...)` call. The two loops are independent.
- **The injected idle signal is the key to Tier-1.** Without it the scheduler is untestable (no idle/AC/metered in CI). Make EVERY external input a closure: idle, power, metered. The real impls are macOS-first (`IOPowerSources` / `SCNetworkReachability` flavoured), with Windows/Linux returning a conservative "Active/Battery/Metered → never prefetch" default so the feature is simply off until the follow-on wires real detection. Document this in Handoff as deliberate, bounded debt.
- **`git maintenance start` fails in CI** (no launchctl/cron) — `register_maintenance` already swallows that (`crates/gix-wrap/src/api.rs`). Do not re-add a failing path; reuse the swallowing helper.
- **Prewarm is a fetch — serialize against clone/fetch.** Acquire the existing per-repo write mutex (`write_lock_for` in `actor.rs`) for the duration of a prewarm fetch so it doesn't race a concurrent clone/fetch of the same repo. The global-2-concurrent limit is a *separate* semaphore across repos (a `tokio::sync::Semaphore` with 2 permits), held for the whole prewarm job.
- **Cancellation must be prompt.** `PrewarmHandle::cancel` / drop fires the token; the fetch subprocess is killed (`kill_on_drop(true)` is already set in `cmd.rs`, but a long `git fetch` needs the token checked between cone chunks or the child killed explicitly). The §6.3 contract is "cancellable if user activity resumes" — assert promptness in a test.
- **Streaming RPC:** mirror the `Clone` streaming exactly (`handlers/repositories.rs` `clone` — `mpsc::channel(32)` + a forwarder task + a `ReceiverStream`). `PrewarmProgress` maps `blobs_fetched`/`blobs_total`/`done`.
- **Cross-platform:** shell-out fetch on Win/Linux CI lanes (Task 113); the OS-specific power/net detection lives behind the closures so the non-mac build compiles with the conservative stub. No `std::os::unix` in the cross-platform paths.
- Regen: proto changed ⇒ `./scripts/regen-interfaces.sh`; the new `prewarm_blobs`/`PrewarmHandle` Rust surface updates `rust-api.md`. Commit both.

## Verification
Tier 1. The `rust` §5.3 set.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core prefetch` (+ `prewarm`) → idle-enqueue/active-cancel/on-battery-skip/metered-skip with injected mocks, global-2-concurrent holds, bandwidth cap consulted, `PrewarmHandle` cancellation, `concerto-state.json` cursor round-trip pass; the fsmonitor supervisor tests stay green (unchanged).
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new workspace deps; reuses `tokio`/`tokio-util`/`gix`/`serde_json`).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `PrewarmBlobs`/`PrewarmProgress`; `rust-api.md` gains `prewarm_blobs`/`PrewarmHandle`).
7. `scripts/smoke.sh` → **unchanged** (304 touches no smoke capability; 302 owns the `sparse-cone-clone` gate).

**Tier-1 scope + what it does NOT cover.** With injected signals, the **scheduler logic** (enqueue, rate-limit, cancel, skip-on-battery/metered, the eager triggers) is fully CI-provable. CI does **not** cover **real AC/Wi-Fi/idle/bandwidth behavior** (no power state or metered network in CI) — that is the Phase-3 Tier-3 confidence item (and the real idle-heartbeat wiring is a documented follow-on). The verification states the injected double drives every branch; the real-machine behavior is the operator's confirmation.

## Definition of Done
- [x] `prewarm_blobs → PrewarmHandle` (cancellable) materializes in-cone blobs via shell-out fetch
- [x] Idle scheduler loop (mirrors `spawn_supervisor`) gated on injected idle/power/net closures; global-2-concurrent + per-repo bandwidth cap; cancels on activity
- [x] Eager triggers (worktree-create + HEAD-update, default ON) wired fully; `repo.prefetch_started/finished` emitted
- [x] fsmonitor supervision unchanged (reused structure only); `register_maintenance` reused for the weekly schedule
- [x] `PrewarmBlobs` RPC + `PrewarmProgress` (fields exactly `blobs_fetched=1`/`blobs_total=2`/`done=3`) appended; streaming mirrors `Clone`
- [x] Injected-signal seam FROZEN; real heartbeat/power/metered detection documented as a bounded follow-on (Handoff)
- [x] All Verification commands pass on a clean checkout; smoke unchanged; interfaces regenerated
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (deliberate seams in Handoff)
- [x] No files outside Outputs modified
- [x] Single commit with the message below

## Outputs
- `crates/core/src/repo_manager/prefetch.rs` (new — idle scheduler + `prewarm_blobs` + `PrewarmHandle` + injected-signal seam) + `crates/core/src/repo_manager/mod.rs` (modified — `pub mod prefetch`)
- `crates/core/src/repo_manager/actor.rs` (modified — `prewarm_blobs` on the handle; spawn the scheduler from `run`; eager triggers; per-repo mutex acquisition)
- `crates/core/src/repo_manager/fsmonitor.rs` (modified only if the worktree-create eager trigger hooks `bring_up_after_clone`; do not change the daemon supervision)
- `crates/gix-wrap/src/api.rs` (modified only if a new targeted-fetch helper is cleaner than reusing `fetch`/`cmd::run`)
- `crates/proto/proto/concerto/v1/repositories.proto` (modified — `PrewarmBlobs`/`PrewarmRequest`/`PrewarmProgress`)
- `crates/core/src/handlers/repositories.rs` (modified — `prewarm_blobs` streaming handler)
- `crates/core/src/boot.rs` (modified — inject the (best-effort) idle/power/net closures; document the heartbeat follow-on seam)
- `crates/core/tests/prefetch_scheduler.rs` (new)
- `docs/interfaces/proto.md` + `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: idle blob prewarm + prewarm RPC; fsmonitor schedule reuse

Adds prewarm_blobs → PrewarmHandle (cancellable shell-out fetch) and an
idle prefetch scheduler (AC + non-metered Wi-Fi + idle>threshold, global
2-concurrent, per-repo bandwidth cap) with the idle/power/net signals
injected as testable closures (real heartbeat wiring a documented
follow-on). Eager worktree-create + HEAD-update triggers ship fully. New
PrewarmBlobs streaming RPC. fsmonitor supervision reused, not rebuilt.

Refs: tasks/v1.0/304-idle-prefetch-fsmonitor.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** (1) The targeted-fetch primitive landed as `gix-wrap::api::prewarm_blobs_in_cone` (cancellable `ls-tree` → chunked `cat-file --batch-check`, materializing lazy blobs of a blobless clone) — placing it in `api.rs` keeps the `regen-interfaces.sh` `rust-api.md` summary meaningful (the regen only scrapes `crates/*/src/api.rs`, never the `repo_manager` module, so `prewarm_blobs`/`PrewarmHandle` themselves do **not** appear in `rust-api.md`; the `PrewarmProgressEvent` struct does). (2) That helper needed a new `gix-wrap::cmd::run_with_stdin` (feeds the OID list on stdin to dodge argv limits) and a `lib.rs` re-export — two files beyond the listed `crates/gix-wrap/src/api.rs`; mechanical companions of the api.rs change, added to keep the seam clean. (3) `crates/core/src/repo_manager/fsmonitor.rs` was **not** modified: the worktree-create + HEAD-update eager triggers are methods on `RepoManager` (`prewarm_on_worktree_create` / `prewarm_on_head_update` in `actor.rs`) rather than hooks into `bring_up_after_clone`, so the daemon-supervision module stays byte-identical (the Outputs note allowed this — "modified only if … hooks bring_up_after_clone"). (4) `repo.prefetch_started/finished` are emitted as `tracing` audit-lines (same shape Task 301/28 used for `repo.size_warning`/`repo.fsmonitor_restarted`) because no repo-event broadcast subject is wired through the streams handler yet — the Tray-rendered broadcast is a Phase-3 follow-on shared with those prior tasks.
- **Open questions for next task:** Task 305 appends `EstimateConeSize`/`ConeStats` to the same `repositories.proto` + `Repositories` service — no collision (304 owns `PrewarmBlobs`/`PrewarmRequest`/`PrewarmProgress`; 305 owns the cone-size shapes) but 305 will rebase onto this proto. The background scheduler's per-repo prewarm scope is currently the whole tracked tree (empty cone) at HEAD; once 305's cone telemetry + 302's per-workarea cone *union* are available, `run_prewarm_pass` should walk the union of a repo's workarea cones rather than the whole tree — left as a consuming follow-on.
- **Deliberate debt:** (a) **Real idle/power/net signals are the conservative `never_prewarm` bundle** (`prefetch::signals::host_signals()` returns it; `boot.rs` injects it). The injected-closure seam (`IdleSignal`/`PowerSignal`/`NetSignal` + `PrewarmSignals`) is FROZEN and fully CI-proven via deterministic mocks (idle/active/battery/metered/below-threshold branches), but the **real Local-API client-heartbeat idle source** (`design/02 §6.3`) and the **macOS power (`pmset`) / net (`SCNetworkReachability`) probes** are a small documented follow-on — until they land, the *background* scheduler is inert by design (off-by-default, honest). The eager worktree-create + HEAD-update triggers do **not** depend on these signals and ship fully (fire unconditionally for blobless repos). No closing task number assigned (it is the heartbeat-wiring follow-on the README/PHASE3_PLANNING §2 anticipates, not a numbered Phase-3 task). (b) The **per-repo bandwidth cap** is a counting seam (`BandwidthLimiter::acquire` is always consulted on the prewarm path; tests assert the consult count) — the real token-bucket throttle needs the byte-counting fetch wiring and is the same follow-on. (c) `read_prefetch_cursor` is `#[cfg_attr(not(test), allow(dead_code))]` — exercised by the round-trip test today; the HEAD-update trigger's "skip if cursor already == new HEAD" optimization that will consume it in production is left for the follow-on (not needed for correctness).
- **Smoke-gate state:** unchanged. 304 touches no smoke capability (302 owns the `sparse-cone-clone` gate); `scripts/smoke.sh` not run (smoke field = unchanged).
