# Concerto V1.0 — AI-Agent Task Breakdown

*Meta-document for the V1.0 task files in this directory. Read this first.*

| Field | Value |
|---|---|
| Status | Approved (2026-05-30) |
| Scope | **V1.0 only** (public beta). Builds on shipped V0.1. Stop line is the design docs' V1.5/V2.0 boundary. |
| Owner | Amin Roudaki |
| Supersedes | Nothing. V0.1 breakdown lives in `tasks/` (root); this is the follow-on the V0.1 README §1/§9 promised. |
| Related docs | `../../design/00_Architecture_Overview.md` §10 (phase table), `../../design/01..18_*.md`, `../../design/Concerto_PRD.md` §20–21 |

---

## 1. Purpose

V0.1 shipped: a working Rust Core (11 gRPC services, ~22K LoC, tested), SQLite persistence, the `concerto-agent-host` PTY supervisor, `gix-wrap`, keychain, and a macOS Tauri Desktop — plus, added manually after the 53-task build, an **embedded-Core mode** (Core in-process behind the `embedded-core` feature), a UI redesign, and real RPC-error surfacing.

V1.0 turns that single-machine macOS alpha into the public-beta product the PRD describes: **remote/multi-device access**, the **Maestro chat agent**, **multi-repo workspaces + monorepo support + PR sets**, **mobile and web clients**, **push notifications**, **VCS depth (GitHub API + webhooks + Linear/Jira)**, and **Windows/Linux Core ports**. It is roughly 4× the scope of V0.1.

This document captures *how* the V1.0 build is decomposed (decisions, phases, verification, task inventory). The individual task files (`NNN-<slug>.md`) capture *what* each task does. As with V0.1, **each task file IS the prompt**: a fresh agent with no prior context can complete it.

The single biggest difference from V0.1: **not every V1.0 task can be machine-verified inside a sub-agent loop.** Real NAT traversal, real push delivery, real devices, code-signing, and app-store submission are physical/external. §5 defines a three-tier verification model that keeps every task honest about what it proves.

---

## 2. Scope of V1.0

**In scope** — the V1.0 column of `design/00 §10`, namely:

- **Transport/security spine:** `Files` RPC + streaming reconnect/ring-buffer (10), Iroh QUIC + self-hosted relay + mDNS + WSS bridge (11), QR pairing + device certs + Noise IK + revocation (12).
- **Maestro (08)** — wholly new: in-process MCP server, 16-tool set, summary cache, digests, deterministic routing, daily condensation, pluggable LLM backends, budget + privacy enforcement.
- **Monorepo + multi-X:** blobless/sparse/sparse-index (02); multi-repo workspaces, parallel workareas, multi-session, PR sets (03).
- **VCS depth (13):** octocrab GitHub client (gh demoted to fallback), webhook receiver, review-thread sync, PR-set coordinated merge, Linear + Jira.
- **Notifications/push (14):** Expo, ID-only wakeups, inbox, multi-device first-to-approve-wins, lock-screen chips.
- **Mobile (16):** iOS + Android via React Native + Expo, Iroh native module, touch diff, voice dictation, pairing.
- **Web (17):** React SPA over Connect-Web/WSS, ephemeral pairing.
- **Desktop split-host (15):** dual transport, connected-Core registry, command palette, Skill/Workflow Explorer windows, Windows build.
- **Thickening:** Scheduler persistent tasks + budget (05); Skills marketplace (06); Suggestion learning (07); Persistence hardening + backup + audit subscribers (09); the `concerto` CLI (10).
- **Platform/ops (18, 01):** Windows Service + systemd Core, agent-host ConPTY, watchdog, OTLP (opt-in), signing pipeline, perf-budget gates, self-host parity.

**Decisions made for this breakdown (see §4):** embedded-Core is a first-class shipped mode for the single-user local case; everything company-operated (relay fleet, Expo project, store accounts, signing keys, Concerto Pro) is built **self-hostable / BYO-credentials** and the operated side is an ops note at the relevant phase gate, not an engineering task.

**Out of scope (the V1.0 stop line — do not build):** multi-tenant relay; GitLab/Bitbucket; Claude Agent SDK backend; org-managed CA / read-write-admin scopes / MDM hooks; Apple Watch; full-duplex voice; Tantivy cross-workarea search (V1.0 uses live grep); native SwiftUI/Compose mobile; Iroh-in-browser; service-worker offline; managed Concerto Cloud; SIEM forwarding; at-rest audit encryption; reproducible builds (V1.0 is SLSA L1, signed-not-reproducible). These are V1.5/V2.0 per the design docs and must be left as trait seams only.

**What "done" means for V1.0:** a 100-engineer org can adopt Concerto as their primary tool — create multi-repo workspaces from a monorepo, run parallel agent sessions, pair a phone and a borrowed laptop, get push approvals, drive everything through the Maestro chat, and merge coordinated PR sets — on macOS or Windows desktops with a Linux/Windows/macOS Core, all self-hostable with no phone-home and no license check. The measurable bar is `design/Concerto_PRD.md §21` (the success-metric and performance-budget tables).

---

## 3. What we inherit from V0.1 (unchanged)

The V0.1 README §3 decisions D1–D6 still hold, with these clarifications:

- **D2 granularity:** foundations small (≤4h), feature work medium (1–3d). Spikes are their own size class (§5).
- **D3 verification:** the V0.1 per-task bar (compile + clippy + unit + integration + smoke gate + interface snapshots) remains the **Tier-1** bar. V1.0 adds Tier-2 and Tier-3 (§5).
- **D4 interface contracts:** `*.proto`, `migrations/*.sql`, `pub` Rust traits are canonical; `docs/interfaces/<file>.md` are the generated summaries read first. V1.0 adds TS client contracts under the same regen discipline where a generator exists.
- **D5 sequencing:** foundations topological → spine → per-subsystem thickening → ship-readiness. V1.0 prepends a **spike phase**.
- **D6 task-file format:** the strict template (now extended, §6).

The V0.1 root `tasks/` files are **frozen history**. Never edit them. Where a V1.0 task revises a V0.1-locked interface, it says so explicitly and re-locks at a new version (§9).

---

## 4. Decisions locked for V1.0

Made during the 2026-05-30 planning conversation. Each task file inherits these as fixed; revising any is a new planning conversation.

| # | Decision | Choice |
|---|---|---|
| V1 | Generation strategy | This README + full task **inventory** for all phases now; full **task files** generated phase-by-phase, each phase just before it starts. Rationale: later phases' shape depends on earlier learnings (esp. the spike outcomes); pre-writing ~130 files guarantees staleness. |
| V2 | Embedded-Core status | **First-class shipped mode** for the single-user local case. Folded into the design (Task 107 retrofits `design/15` + adds `design/19_Embedded_Core_Mode.md`), packaged, on the Desktop launch decision tree, and smoke-tested. Daemon mode remains the production default for split-host/remote. |
| V3 | Company-operated vs self-hostable | Build **everything self-hostable / BYO-credentials**: the `concerto-relay` binary, `PushBackend`+`ExpoPushBackend` wired to the operator's own Expo credentials, signing *scripts* that take keys as input. Concerto-Inc fleet/store-accounts/paid-tier are out-of-scope ops items noted at the phase gate, not tasks. |
| V4 | Phase 1 = spikes | The four `design/00 §11` validation spikes (Iroh NAT, Tonic-over-Iroh, RN diff perf, gix sparse-cone latency) run first. **Phases 5 (mobile/web) and the relay tasks do not start until the Iroh spike clears its >70%-direct bar** (contingency: tsnet sidecar — operator decision). |
| V5 | Phase ordering | P1 spikes+cleanup → P2 transport/security spine → P3 multi-X+monorepo+VCS → P4 Maestro → P5 notifications+mobile+web → P6 desktop split-host+Windows+thickening → P7 ports+signing+ship. (§6.) |
| V6 | Verification tiers | Three tiers (§5). Every task declares its tier. Tier-3 (un-automatable) items become the per-phase **manual verification checklist** the operator signs off at the phase gate. |
| V7 | Breakdown + spec home | Breakdown lives in `tasks/v1.0/`. Design retrofits (embedded mode, verification model, spike findings) are amendments to `design/` (the canonical spec), authored as their own tasks, not a separate spec dir. |
| V8 | Client monorepo placement | All clients in this repo: `apps/mobile` (Expo), `apps/web` (reuses the Desktop SPA via a shared `packages/` extraction). Each has its own Tier-2 verification (vitest/jest + simulator/headless screenshot tests). |
| V9 | Task types | Every task is one of: `rust` / `web-ts` / `rn-mobile` / `infra-ops` / `spike` / `doc`. Each type has its own verification command set (§5.3). The orchestrator (`AUTO_EXECUTE_PROMPT.md`) branches on the type. |

> **Amendment (2026-06-08) — Project→Workspace collapse.** After this breakdown was approved, the 4-level hierarchy `Project → Workspace → Workarea → Session` was collapsed to a 3-level hierarchy `Workspace → Workarea → Session` over a **global Repository registry**. The `Project` entity is gone (no `projects` table, no `Projects` gRPC service, no `ProjectId`); everything it owned (shared settings/scripts, permission/deliberation defaults, icon, repo ownership) moved onto the **Workspace**, and repositories became a global registry that workspaces select from via `workspace_repos`. Design + rationale: `docs/superpowers/specs/2026-06-08-collapse-project-into-workspace-design.md`; execution plan: `docs/superpowers/plans/2026-06-08-collapse-project-into-workspace.md`. The canonical design docs (`design/00`, `02`, `03`, `09`, `10`, `13`, `15`) were updated to match. **The `tasks/v1.0/NNN-*.md` task files in this directory remain FROZEN history and are NOT rewritten** — where a task file describes the old Project-scoped model, read it through this amendment.

---

## 5. Verification model

V0.1's bar was airtight because it was all Rust. V1.0 spans Rust, TypeScript, React Native/Expo, real networks, real devices, and signing infrastructure. We keep the rigor by tiering verification and being explicit about what each task actually proves.

### 5.1 The three tiers

**Tier 1 — CI-self-verifiable (the V0.1 bar).** Provable end-to-end by a sub-agent with no human or hardware. The Rust core, persistence, protocol logic, most of the daemon. Bar: `cargo check/clippy/test/deny` + interface snapshots + smoke gate, all green.

**Tier 2 — CI-self-verifiable via test doubles.** The *logic* is proven in CI against a stand-in for the physical reality:
- Transport: a **loopback Iroh transport** (two endpoints on one host) proves the gRPC-over-Iroh, pairing, Noise-IK, reconnect, and `Files` paths without a real NAT.
- Push: a **mock `PushBackend`** proves fan-out, first-to-approve-wins, dedup, and the ID-only payload invariant without Expo/APNs/FCM.
- Mobile: **iOS Simulator / Android emulator** plus jest/RN-testing-library unit + screenshot tests prove the UI and the TS transport binding.
- Web: **headless Chromium** (Playwright) against a Core's loopback gRPC-Web server proves the SPA, the `DataClient`, and ephemeral pairing.
- A Tier-2 task's `Verification` section lists the double it uses and states plainly what the double does *not* cover (that uncovered part is a Tier-3 line in the phase checklist).

**Tier 3 — Manual phase-gate only.** Genuinely physical/external; a sub-agent builds to the Tier-2 bar, the operator confirms reality at the phase boundary:
- Real-NAT direct-connection % across real networks; real LTE↔Wi-Fi migration.
- Real push delivery to a locked iPhone/Android; lock-screen action chips.
- Mobile builds installed via EAS on real devices; App Store / Play Store submission.
- Code-signing + notarization of installers; Windows EV signing.
- Cross-machine split-host (Desktop on laptop, Core on workstation/VM).
- Relay deployed on real infrastructure (e.g. Fly.io anycast).

Each phase's manual checklist (§6) is the catalogue of its Tier-3 lines. **A phase is not "done" until its Tier-1/Tier-2 tasks are all merged green AND the operator has ticked the Tier-3 checklist.**

### 5.2 The `spike` type

Spike tasks (Phase 1) do not ship product code. A spike produces **(a)** a throwaway harness under `spikes/<name>/` and **(b)** a findings doc at `design/spikes/<name>-findings.md` ending in an explicit **GO / NO-GO** against a numeric bar from the design. Its `Definition of Done` is "harness runs + findings doc committed with a GO/NO-GO and the measured numbers," not a green smoke gate. A NO-GO is a **Stop-and-ask** for the operator (it may trigger a design contingency, e.g. the tsnet sidecar).

### 5.3 Per-type verification command sets

The full per-type command set is below (the task file's own `Verification` section may add more). The orchestrator runs these **tiered** for speed (see `AUTO_EXECUTE_PROMPT.md` → *Concurrency model* / Step 4): a **fast local gate** before pushing each task, with the **expensive full test suite + smoke delegated to CI** as the authoritative pre-merge gate (CI re-runs the whole matrix anyway). The full local set is still the contract every task must pass *somewhere* (local or CI) before merge — nothing is skipped, only relocated.

| Type | Full command set (fast-local gate **bold**; rest delegated to CI unless high-risk) |
|---|---|
| `rust` | **`cargo check --workspace`** · **`cargo clippy --workspace --all-targets -- -D warnings`** · **`cargo fmt --all -- --check`** · **`cargo deny check`** · **`./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/`** · `cargo test --workspace --no-fail-fast` · `scripts/smoke.sh` (if the task's smoke field ≠ unchanged — then run locally too, it's high-risk). **Note on `cargo fmt`:** CI's `format.yml` runs `cargo fmt --all -- --check` (stable rustfmt; `--all` = every workspace member). `rustfmt.toml` sets `imports_granularity = "Crate"` (skipped on stable with a harmless warning) + `max_width = 100`; a plain `cargo fmt --check` (no `--all`) only checks the root package and will miss real drift — always use `--all`. |
| `web-ts` | `pnpm -C apps/web typecheck` · `pnpm -C apps/web lint` · `pnpm -C apps/web test` · `pnpm -C apps/web build` · (+ Playwright headless suite if the task touches the data layer) |
| `rn-mobile` | `pnpm -C apps/mobile typecheck` · `pnpm -C apps/mobile lint` · `pnpm -C apps/mobile test` · `pnpm -C apps/mobile exec expo prebuild --no-install` (compile gate) · simulator screenshot suite if present |
| `infra-ops` | task-specific (e.g. `shellcheck scripts/*.sh`, a dry-run of the script, a CI-workflow lint); always states its exact gate |
| `spike` | harness command(s) + "findings doc committed with GO/NO-GO" |
| `doc` | `markdownlint` / link-check if configured; otherwise a human-read gate (operator spot-check) |

The smoke gate (`scripts/smoke.sh`) grows the same way it did in V0.1 — Task 108 refactors it into composable per-capability checks plus a V1.0 manifest so each capability (pairing, files-transfer, multi-repo, maestro-digest, push-fanout) is a named check the relevant task turns on.

### 5.4 Execution model (pipelined + bounded-parallel)

`deps` define a **partial** order, not a strict serial one. The orchestrator (`AUTO_EXECUTE_PROMPT.md` → *Concurrency model*) overlaps wall-clock three ways without weakening any gate: **(1)** it pipelines — backgrounds a task's CI watch and starts the next eligible task instead of idling on `gh pr checks --watch`; **(2)** it builds up to **K = 3** *dependency-ready, file-disjoint* tasks concurrently, each in its own `git worktree`; **(3)** it validates tiered (§5.3). The invariant that never bends: **a task merges only after its CI is green AND every task it depends on is merged; `main` stays green; merges are serialized in dependency order and the other in-flight branches rebase onto each new `main`.** Two tasks that both write a hard-to-merge seam (a `*.proto`, a shared `mod.rs`/`lib.rs`/`boot.rs`, a migration) are never built concurrently; on any substantive rebase conflict the later task is re-dispatched fresh on the updated `main`. Each phase's concurrency/wave map (which tasks are safe to overlap) lives in that phase's planning addendum — for Phase 3, `PHASE3_PLANNING.md §8`.

---

## 6. Phase structure & task inventory

Seven phases, **~130 tasks**. Tasks are numbered `Pss` — first digit = phase, last two = sequence (`101`…`113`, `201`…, `701`…). This sorts correctly, never renumbers across phases, and keeps `Refs:` stable. Inserts use `.5` (e.g. `203.5`) as in V0.1.

Each inventory row: **task — one-line goal — `deps` — `tier` — `type`**. Full task files (template §7) are generated per V1 just before the phase starts; this inventory is the contract for what each will contain.

> Cross-phase dependency notes are called out inline. The most important: **`wait_for_check_runs` (Scheduler 05) lands in P3** because PR-set coordinated merge needs it; the rest of the Scheduler thickening is P6. **Maestro's `notify_user` (P4) stubs against 14 and is wired live in P5.**

### Phase 1 — Spikes & Foundation Cleanup (~13 tasks)

De-risk the locked bets and clean the foundation the rest of V1.0 builds on. Phases 2–7 assume these are done.

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| 101 | Iroh NAT-diversity spike: harness + findings vs the >70%-direct bar (contingency: tsnet) | — | spike | spike |
| 102 | Tonic-over-Iroh latency/throughput spike vs "within 30% of UDS, >1 MB/s session.io" | 101 | spike | spike |
| 103 | React Native diff-viewer perf spike vs "1000-line diff <1.5s, 60fps on iPhone13+/Pixel6+" | — | spike | spike |
| 104 | `gix` sparse-cone `status` latency spike vs "<100 ms on 2M-file repo / 100k cone" | — | spike | spike |
| 105 | Delete dead crates (`pty-sup`, `desktop-shell`); update workspace + interface regen | — | 1 | rust |
| 106 | Harden agent-host binary resolution: `CONCERTO_AGENT_HOST_BIN` override + robust search; remove dev-loop fragility | — | 1 | rust |
| 107 | Retrofit embedded-Core into the design (`design/19` + `design/15` launch-tree edit); make it a first-class branch | — | 3 | doc |
| 108 | Smoke-gate refactor: composable per-capability checks + V1.0 manifest | — | 1 | infra-ops |
| 109 | `concerto` CLI skeleton (`crates/cli`): `status`, `pair`, `workspace ls`, `session ls` over the gRPC API | 108 | 1 | rust |
| 110 | Persistence hardening (09): startup `PRAGMA quick_check`, forward-only guard, binary-downgrade refusal | 108 | 1 | rust |
| 111 | `concerto backup` (09): `VACUUM INTO` + optional worktree tar + audit-range export | 108, 109, 110 | 1 | rust |
| 112 | Audit-log rotation + `AuditLogSubscriber` trait + Jsonl/Stdout/Syslog/HttpsForwarder impls (09) | 108 | 1 | rust |
| 113 | CI matrix: add Windows + Linux Core build/test runners (agent-host PTY gated off Windows for now) | 105 | 2 | infra-ops |

**Phase 1 manual checklist (Tier-3):** read each of the 4 findings docs and record GO/NO-GO; confirm embedded-Core launch behavior on your Mac (real + scratch + external fallback); confirm `concerto status` / `workspace ls` run against a live Core (`concerto pair` arrives in Phase 7); confirm Windows + Linux Core CI lanes go green.

### Phase 2 — Transport & Security Spine (~20 tasks)

The hardest phase and the dependency root for all remote features. Heavy Tier-2 (loopback) with Tier-3 reality at the gate.

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| 200 | Reconcile the Tonic-over-Iroh adapter decision (spike 102 → design): amend `00`/`10`/`11`/`15` from `tonic-iroh-transport` to the hand-rolled tonic-0.12 adapter; runs first (added 2026-06-02 per the Phase-2 planning B1 decision) | — | 3 | doc |
| 201 | Proto: `transport_kind`/`core_host_os`/`core_hostname` on `ServerCapabilities` + connect-time capability negotiation | — | 1 | rust |
| 202 | `Streams.Subscribe` reconnect: offset ack + server-side per-stream ring buffer + gap detection | 201 | 1 | rust |
| 203 | `Files` service: `Upload`/`Download` streaming, chunked, blake2b checksum, allow-list enforced | 201 | 1 | rust |
| 204 | Connect-Web bridge: Core loopback `hyper` server (gRPC-Web + SSE fallback) | 201 | 2 | rust |
| 205 | Crypto primitives: Ed25519 device identity, BLAKE2b device_id, `DeviceCert` (deterministic CBOR) sign/verify | — | 1 | rust |
| 206 | `DeviceCertIssuer` trait + `LocalCoreIssuer`; issuance/expiry/validation (<200µs hot path) | 205 | 1 | rust |
| 207 | Pairing: Noise XX over 32-byte token (60s TTL, one-shot, ≤3 active); `Devices.Start/CompletePairing` | 206 | 2 | rust |
| 208 | Noise IK session layer (AES-256-GCM, rekey 1 GB/1 h) + test vectors + `validate_cert` fuzz | 205 | 2 | rust |
| 209 | `Devices` service: `ListDevices`/`RevokeDevice`/`GetCoreInfo`; revoke-mid-stream <1 s; `devices` table wiring | 207 | 1 | rust |
| 210 | Auth middleware: device-cert path (Iroh) + peer-UID fast path (UDS) into identical handlers | 206, 209 | 1 | rust |
| 211 | `managed.json` enforcement: `disable_remote`, allowed devices, max paired devices | 210 | 1 | rust |
| 212 | `crates/transport`: Iroh endpoint in Core (QUIC, hole-punch + relay fallback), 3 logical channels | 102, 208 | 2 | rust |
| 213 | mDNS LAN discovery (`_concerto._tcp.local` TXT: endpoint_id/pubkey/version/caps) | 212 | 2 | rust |
| 214 | `crates/relay`: self-hosted relay binary (iroh-relay config + env + Prometheus) | 212 | 2 | rust |
| 215 | WSS bridge at relay (WSS↔Iroh, ciphertext-only) | 214 | 2 | rust |
| 216 | QUIC connection migration (Wi-Fi↔LTE) + NAT-success telemetry by client kind | 212 | 2 | rust |
| 217 | `TransportHandle` API: start/stop, listen_pairing, current/switch_relay, nat_stats, send_wakeup_hint, close_sessions_for_device | 212 | 1 | rust |
| 217.5 | Wire the Iroh transport into Core boot: spawn `serve_iroh` (config-gated) + Core-side Noise-XX pairing responder over the `0x03` channel + live `TransportHandle`-backed `SessionCloser` — the boot-wiring 212/217 deferred; makes the spine live end-to-end for 220 + the Tier-3 checklist (added 2026-06-04 per the Phase-2 220-blocker decision) | 212, 217, 207, 208, 209, 210, 211 | 2 | rust |
| 218 | Desktop dual transport: `CoreClient` trait + `UdsCoreClient`/`IrohCoreClient` + connected-Core registry (`cores.json`+keychain) | 217 | 2 | web-ts |
| 219 | Desktop pairing UI: show/scan QR + Connect-to-Core picker + Settings→Connected Cores | 218 | 2 | web-ts |
| 220 | Split-host loopback smoke: two Iroh endpoints, end-to-end RPC + stream + Files transfer | 217 | 2 | infra-ops |

**Phase 2 manual checklist (Tier-3):** pair a real second machine over LAN (mDNS direct); pair from a real remote network and confirm `nat_stats` direct-% on real NATs; transfer a file both directions split-host; revoke a device and confirm <60 s stream teardown; confirm `disable_remote` truly disables remote.

### Phase 3 — Multi-X, Monorepo & VCS (~26 tasks)

"The features that justify switching." Independent of the transport spine except where the Desktop UI consumes it; can proceed in parallel conceptually but executes sequentially.

> **Phase-3 planning addendum:** the 2026-06-05 planning conversation locked nine decisions, a migration-number reservation table, and the cross-task frozen contracts in **`tasks/v1.0/PHASE3_PLANNING.md`** — read it before any Phase-3 task file. It added two inserts (315.0, 320.5, below).

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| 301 | Blobless/treeless clone strategies + repo-size→strategy recommendation | — | 1 | rust |
| 302 | Sparse-checkout + cone + sparse-index lifecycle; per-workarea cones | 301 | 1 | rust |
| 303 | `gix status` hot path on a sparse cone + bench gate (builds on spike 104) | 104, 302 | 1 | rust |
| 304 | Idle blob pre-fetch (AC+idle); fsmonitor supervision (restart if dead); maintenance schedule | 301 | 1 | rust |
| 305 | `suggest_cones` interface + `ConeStats`/`EstimateConeSize` RPC (Maestro delegate wired in P4) | 302 | 1 | rust |
| 306 | Multi-repo workspaces (1..N repos): wire create/manage over `workspace_repos` | — | 1 | rust |
| 307 | Multiple workareas per workspace (parallel attempts) + full workarea FSM | 306 | 1 | rust |
| 308 | Multiple sessions per workarea (multi-agent) + per-workarea edit mutex | 307 | 1 | rust |
| 309 | Files-to-copy symlink mode + `.worktreeinclude` (copy/symlink/exclude) | 307 | 1 | rust |
| 310 | Three-layer settings precedence resolver (managed > checked-in > local DB > defaults) | — | 1 | rust |
| 311 | `exclude_from_maestro` per-workarea toggle (schema + API) | 307 | 1 | rust |
| 312 | Branch-rename hook (one-shot LLM via 04, cross-repo `git branch -m`) + `suggest_workarea_branch_name` | 307, 310 | 1 | rust |
| 313 | `VcsProvider` trait + octocrab `GitHubProvider` (default) + `GitHubProviderViaCli` (fallback) | — | 2 | rust |
| 314 | GitHub App option + dual rate-limit pools + degraded cadence | 313 | 2 | rust |
| 315.0 | Design amendment: relay→Core inbound-webhook framing on a new `0x04` Webhook channel (added 2026-06-05; precedes 315 like 200 preceded 201) | 215 | 3 | doc |
| 315 | Webhook receiver (HMAC verify, delivery-id idempotency) on Core via relay path | 313, 215, 315.0 | 2 | rust |
| 316 | Review-thread sync (GraphQL) + check-run/deploy aggregation | 313 | 2 | rust |
| 317 | Native Linear + Jira clients (Atlassian OAuth in settings) | — | 2 | rust |
| 318 | Scheduler `wait_for_check_runs` primitive (poll + backoff + webhook), consumed by PR-set merge | 315 | 1 | rust |
| 319 | PR-set semantics: implicit per-workarea, `merge_order`, `GetPrSet` | 308, 313 | 1 | rust |
| 320 | Coordinated merge loop (merge → wait_for_check_runs → continue/pause-on-fail) + coordinated revert | 318, 319 | 2 | rust |
| 320.5 | Linear/Jira issue write-back on coordinated-merge completion (per-project opt-in; reuses 317's seam; added 2026-06-05 per decision D5) | 317, 320 | 2 | rust |
| 321 | LLM-composed PR title/body (on by default, 2s deterministic fallback) | 313, 312, 310 | 1 | rust |
| 322 | Desktop: multi-repo session UI + sparse-cone picker | 302, 306, 218 | 2 | web-ts |
| 323 | Desktop: parallel workareas + multi-agent session tabs UI | 308, 218 | 2 | web-ts |
| 324 | Desktop: PR-set panel (replaces stub Checks/PR cards) + coordinated merge UI | 320, 218 | 2 | web-ts |

**Phase 3 manual checklist (Tier-3):** sparse+blobless clone a real >10 GB monorepo and confirm <30 s p50 workspace creation; create a multi-repo workspace; run a coordinated PR-set merge against a real GitHub repo with a live webhook; confirm review threads sync; fetch a real Linear and Jira issue.

### Phase 4 — Maestro (08) (~15 tasks)

Wholly new. Depends on 03/04/05/07/13 (all present after P3) and 14 (`notify_user` stubbed until P5).

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| 401 | `concerto-maestro-mcp` in-process MCP server (distinct surface from `concerto-mcp`) | — | 1 | rust |
| 402 | Maestro-as-agent: long-lived agent under agent-host, `strict` mode, no fs/shell/net, scratch dir | 401 | 1 | rust |
| 403 | `maestro_state` table + `chats(kind=maestro)` singleton + daily-summary message tagging | — | 1 | rust |
| 404 | Per-workarea summary cache (`WorkareaSummary`/`SessionSummary`/`RepoSummary`) + refresh triggers + Haiku fallback summarizer | 402 | 2 | rust |
| 405 | Read-only tool set (the 11 read tools) | 404 | 1 | rust |
| 406 | Write tool set (5 tools, each gated by a UI confirmation chip) | 405 | 2 | rust |
| 407 | Side-channel tools: `notify_user` (stub against 14) + `propose_chip` (to 07) | 405 | 1 | rust |
| 408 | Deterministic routing pre-parser (`@workarea`, fanout, `@all`/`@idle`/`@blocked`, `/digest`/`/pause`/`/new`) | 402 | 1 | rust |
| 409 | Digest generation (<5 s p50, Sonnet, ≤600 tokens, grouped + 07 chips) | 404, 408 | 2 | rust |
| 410 | Daily history condensation (verbatim 24h + condensed older + weekly) | 403 | 1 | rust |
| 411 | `create_workspace_from_description` (issue parse → multi-repo detect → cone suggest → confirm chips → 03) | 406, 305, 313 | 2 | rust |
| 412 | Pluggable LLM provider (Claude/Codex/Gemini CLI + Direct API) + daily budget (200K/50K) + inert-on-exhaust | 402 | 2 | rust |
| 413 | Privacy enforcement (`exclude_from_maestro`, full-chat-access, enterpriseDataPrivacy disables if external) | 404 | 1 | rust |
| 414 | `Maestro` gRPC service (`SendToMaestro`/`GetDigest`/`SetWorkareaVisibility`) + events | 409 | 1 | rust |
| 415 | Desktop: Concerto chat top bar + digest rendering + routing UX + confirmation chips | 414, 218 | 2 | web-ts |

**Phase 4 manual checklist (Tier-3):** leave for >30 min across active workareas, return, judge digest quality + measure latency; route prompts via `@workarea` and fanout; create a workspace from a real issue link; confirm an excluded workarea leaks only hard facts; confirm budget-exhaust goes inert while routing still works.

### Phase 5 — Notifications, Mobile & Web (~22 tasks)

**Gated on the Iroh spike GO (V4).** Depends on the P2 transport/pairing spine.

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| 501 | Notification model + `notifications`/`notification_deliveries` tables + 6 kinds | — | 1 | rust |
| 502 | SQLite inbox + chronological feed + dedup window | 501 | 1 | rust |
| 503 | `PushBackend` trait + `ExpoPushBackend` (BYO Expo creds) + ID-only wakeup payload | 501 | 2 | rust |
| 504 | Post-wakeup `GetNotification` over E2EE + multi-device fan-out + first-to-approve-wins + active-viewing detection | 503, 209 | 2 | rust |
| 505 | `ActOnChip`→ResolveApproval/SendMessage + per-project opt-out + prefs | 504 | 1 | rust |
| 506 | Privacy property test: no PII in WakeupPayload; no body for enterprise-private projects | 503 | 1 | rust |
| 507 | `Notifications` gRPC service + `NotificationHandle`; wire Maestro `notify_user` live | 504, 407 | 1 | rust |
| 508 | `apps/mobile` Expo scaffold + EAS config + shared `packages/` proto-client extraction | — | 2 | rn-mobile |
| 509 | `ConcertoIroh` native module (Rust→C→JSI iOS / Rust→JNI Android) + XCFramework/.aar + CI | 212, 508 | 2 | rn-mobile |
| 510 | Connect-Web TS client adapted to call the native module | 509 | 2 | rn-mobile |
| 511 | Pairing QR scanner + `expo-secure-store` key/cert storage + multi-Core | 510, 207 | 2 | rn-mobile |
| 512 | Bottom-tab nav (Maestro default landing / Workspaces / Inbox) | 511 | 2 | rn-mobile |
| 513 | Workspaces drill-down + workarea detail (Sessions / Code & PRs swipe) | 512 | 2 | rn-mobile |
| 514 | Touch-first RN diff renderer (perf budget from spike 103; fallback noted) | 103, 513 | 2 | rn-mobile |
| 515 | Voice dictation input | 512 | 2 | rn-mobile |
| 516 | Push registration + post-wakeup fetch + lock-screen action chips + biometric gate | 503, 511 | 2 | rn-mobile |
| 517 | Localhost preview tunnel WebView (`StartLocalhostTunnel`) | 510 | 2 | rn-mobile |
| 518 | Lite-mode cellular streaming + cross-device handoff + background lifecycle | 510 | 2 | rn-mobile |
| 519 | `apps/web` SPA reusing Desktop renderer via shared package + `DataClient` abstraction | 218 | 2 | web-ts |
| 520 | Connect-Web data client (HTTP/2 + SSE fallback) + AckOffset polling | 204, 519 | 2 | web-ts |
| 521 | LAN-direct loopback TLS pinned to Core identity + remote WSS-via-relay Noise IK | 215, 520 | 2 | web-ts |
| 522 | Ephemeral pairing (phone-signed 8h session cert, IndexedDB, cleared on tab close) + "remember browser" | 511, 520 | 2 | web-ts |

**Phase 5 manual checklist (Tier-3):** install the EAS build on a real iPhone + Android; pair both; receive a real push and approve a tool call from the lock screen; confirm first-to-approve-wins across two devices; verify the RN diff renderer hits its budget on real hardware; open the web client on a borrowed laptop and on Linux (LAN-direct + relayed).

### Phase 6 — Desktop Split-Host, Windows & Thickening (~22 tasks)

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| 601 | First-launch Connect-to-Core picker + multi-Core switch (clean teardown/re-bootstrap) | 218 | 2 | web-ts |
| 602 | Remote-mode affordances (hide local-only, drag-drop→Files.Upload, artifact→Download, transport_kind conditional render) | 203, 601 | 2 | web-ts |
| 603 | Cmd+K command palette | — | 2 | web-ts |
| 604 | History pane | — | 2 | web-ts |
| 605 | Orchestrated one-shot actions (Fix Errors / Pull Latest / Open PR / Commit & Push) | 320 | 2 | web-ts |
| 606 | Inline-comment-to-composer in diff viewer + session deliberation chips | — | 2 | web-ts |
| 607 | Permission-mode UI + Workflow Explorer + Skill Explorer windows | 615 | 2 | web-ts |
| 608 | Windows Desktop build (WebView2 parity, `sc.exe` auto-spawn) | 113, 701 | 2 | web-ts |
| 609 | Persistent scheduled tasks (cron parse + jitter + run history) | — | 1 | rust |
| 610 | Budget guardrails (daily_budget_tokens, per-account cap, silent skip) | 609 | 1 | rust |
| 611 | Promote loop→scheduled (derive cron, preserve permission settings) | 609 | 1 | rust |
| 612 | Cloud-task sync (feature-detect, gray out if unsupported) | 609 | 2 | rust |
| 613 | 6 starter schedule templates bundled in the binary | 609 | 1 | rust |
| 614 | `SkillRegistrySource` trait + GitMarketplaceSource + LocalDirectorySource; add/refresh/remove marketplace | — | 2 | rust |
| 615 | Install/uninstall/update + version pinning + scheduled refresh + diff-to-upstream | 614 | 2 | rust |
| 616 | Sandboxed "Try this skill" (scratch workspace, strict, auto-archive 24h) | 615 | 2 | rust |
| 617 | Enterprise allow/deny/pinned skill lists + config writeback | 615 | 1 | rust |
| 618 | Per-user learning store (frequency+recency, score math, promote-after-5) | — | 1 | rust |
| 619 | Best-practice auto-prompts (severity styling, never auto-execute) + suppression (3 dismissals/7d→30d) | 618 | 1 | rust |
| 620 | `ChipRanker` trait (FrequencyRanker) + top-3 push-chip extraction (consumed by 14) | 618, 503 | 1 | rust |
| 621 | Per-user reset/disable + chip outcome events + enterpriseDataPrivacy halt | 618 | 1 | rust |
| 622 | Skill/Suggestion/Schedule mobile + web surfaces (parity passes on the new clients) | 508, 519 | 2 | web-ts |

**Phase 6 manual checklist (Tier-3):** smoke the Windows Desktop build end-to-end; install a skill from a real Git marketplace and run it; confirm learning promotes a chip after repeated use; confirm a persistent scheduled task fires across a reboot; confirm multi-Core switch on Desktop.

### Phase 7 — Platform Ports, Signing & Ship-Readiness (~13 tasks)

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| 701 | Windows Service + systemd user unit for Core | 113 | 2 | infra-ops |
| 702 | agent-host ConPTY (Windows PTY backend) | 701 | 2 | rust |
| 703 | Watchdog actor (auto-restart hung supervision tree) + RSS sampling + terminal crash policy | — | 1 | rust |
| 704 | OTLP exporter (opt-in, off by default, secret-scrubbed) | — | 1 | rust |
| 705 | Core auto-update (signed `updates.json` Ed25519) | 706 | 2 | rust |
| 706 | Release signing pipeline: Mac notarize, Windows EV, Ed25519 updates.json, SLSA L1; GHA matrix (Mac universal2, Win x64/arm64, Linux x64/arm64, EAS) | 113 | 3 | infra-ops |
| 707 | License gate full (cargo deny + `pnpm licenses`) + trait-seam registry completeness check | — | 1 | infra-ops |
| 708 | Self-host parity doc + fresh-machine walkthrough (CONTRIBUTING) + relay deploy recipe | 706 | 3 | doc |
| 709 | Diagnostics panel data source + health/diagnostics RPCs | 703 | 1 | rust |
| 710 | Performance-budget verification gates (all V1.0 `design/00 §7.7` budgets) | — | 2 | infra-ops |
| 711 | Full V1.0 smoke gate (co-located happy path covering all of V1.0) | 108 | 1 | infra-ops |
| 712 | README + getting-started + docs sync for V1.0 | — | doc | doc |
| 713 | `concerto pair` headless CLI (unicode-QR + base64 token) for tray-less Core | 207, 109 | 1 | rust |

**Phase 7 manual checklist (Tier-3):** install signed+notarized installers on Mac + Windows; run a Linux Core via systemd; deploy the relay on real infra and route a remote client through it; walk a fresh machine through the self-host doc; review the full perf-budget report against `§7.7`.

---

## 7. Task-file template (V1.0)

Identical to V0.1's (root `tasks/README.md` §6) **plus** a `Task type` field and a `Verification tier` field, and the `Verification` section names its tier/double explicitly. The file IS the prompt.

```markdown
# Task NNN — <Title>

| Field | Value |
|---|---|
| Phase | 1–7 |
| Task type | rust / web-ts / rn-mobile / infra-ops / spike / doc |
| Verification tier | 1 / 2 / 3 / spike |
| Size | small (≤4h) / medium (1–3d) / spike |
| Depends on | NNN, NNN, … |
| Touches subsystem(s) | 01, 09, … |
| Smoke gate | unchanged / extends:<capability> / new:<capability> |

## Goal
One paragraph. What this task makes true that wasn't true before.

## Inputs to read before starting
- design/<doc>.md §<section> — <why>
- docs/interfaces/<file>.md — <why>
- tasks/v1.0/<NNN-prev>-<slug>.md → "Handoff Notes" — drift from prior task

## Scope — in / Scope — out
- bullets

## Public interface this task locks
- proto / SQL / Rust trait / TS type — names + field numbers, FROZEN

## Implementation notes
Short, opinionated guidance on the non-obvious parts.

## Verification
Exact commands + expected outcomes. MUST state the tier and, for Tier 2,
the test double used and what it does NOT cover (→ becomes a phase-checklist line).
For spikes: the harness command + the numeric bar + the GO/NO-GO artifact path.

## Definition of Done
- [ ] All Verification commands pass on a clean checkout (Tier 1/2)
      — OR — findings doc committed with GO/NO-GO + measured numbers (spike)
- [ ] No TODO/FIXME/unimplemented!()/todo!() in new code (deliberate ones in Handoff)
- [ ] No files outside Outputs modified
- [ ] Interfaces regenerated + committed if any schema/contract changed
- [ ] Smoke gate green (if this task touches it)
- [ ] Single commit with the message below

## Outputs
- file paths (new / modified)

## Commit message
` ``
phase-N: <one-line summary>

<2–4 line body>

Refs: tasks/v1.0/NNN-<slug>.md
` ``

## Handoff Notes (filled in when finishing)
- Drift from plan / Open questions for next task / Deliberate debt / Smoke-gate state
```

The orchestrator and per-task prompts are in `tasks/v1.0/AUTO_EXECUTE_PROMPT.md` and `tasks/v1.0/PROMPT_TEMPLATE.md` (V1.0 variants that branch on `Task type` and tier). Tasks execute **sequentially** across a phase (the inventory `deps` are real), but a single task's **lead sub-agent may fan out helper sub-agents** to build independent sub-parts in parallel and integrate them into the one commit — see `PROMPT_TEMPLATE.md` → *Parallel build*.

---

## 8. Generation strategy (per decision V1)

- This README + the §6 inventory is the **complete contract** for V1.0.
- **Full task files are generated one phase at a time**, immediately before that phase executes, by a planning pass that reads: this README, the relevant `design/` sections, the *actual current state of `main`*, and the prior phase's Handoff Notes. This is why the inventory carries `deps`/`tier`/`type` but not full bodies — the body is most accurate when written against the code that exists when the phase starts.
- Phase 1's task files are generated **now** (they have no upstream V1.0 dependencies).
- The spike findings (101–104) may amend the design (`design/spikes/*` + design-doc notes) and may change later-phase tasks — that's expected and is the whole point of front-loading them.

---

## 9. Verification model recap & revising the plan

- **Tier discipline is non-negotiable.** A task may not downgrade its own tier to pass. If a Tier-1 task can only be made to pass by mocking something it shouldn't, that's a Stop-and-ask.
- **Phase gates are real gates.** The operator's Tier-3 checklist must be signed off before the next phase's task files are generated. A failed Tier-3 item is a revision task, not a silent skip.
- **Revising the inventory:** adding a task between `N` and `N+1` → insert `N.5` (no renumber). Changing a V0.1-locked or earlier-V1.0-locked interface → a new task titled "Revise <interface>" that re-locks at a new version; never edit a merged task's `Public interface this task locks`.
- **The design is canonical.** Drift between code and design is recorded in Handoff Notes; design changes are explicit tasks (e.g. 107), never silent edits.

---

*End of meta-document. Phase-1 task files (`101-…` through `113-…`) accompany this README. Later phases are generated just-in-time per §8.*
