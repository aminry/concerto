# Phase 5 (Notifications, Mobile & Web) — Planning Addendum

*Read this AFTER `README.md` §4–§6 and BEFORE any Phase-5 task file. It records the
decisions the Phase-5 planning conversation (2026-06-14) locked on top of the README
inventory, the cross-task **frozen contracts** the task files must agree on, the
**migration-number reservation**, the **execution-order / verification-tier strategy**, and the
**machine-consumable task graph** (`PHASE5_DAG.json` + §8) the auto-execute loop reads.*

| Field | Value |
|---|---|
| Status | Approved (2026-06-14) |
| Scope | Phase 5 only (tasks 501–522 + inserts 500, 507.5, 509.5, 523; reframed 508/519) |
| Supersedes | Nothing. Amends `README.md §6` Phase-5 inventory (4 insert rows + refined deps + reframed scopes). |
| Authority | These decisions are FIXED for the Phase-5 task files exactly as `README.md §4` decisions are fixed; revising one is a new planning conversation. |
| Gate | **Iroh-NAT spike = GO** (`design/spikes/iroh-nat-findings.md:11`, 80% direct field-measured 2026-06-02). The README V4 gate that blocks Phase 5 is **CLEARED**; tsnet contingency NOT triggered. **The relay is load-bearing** (spike Note A) — relayed paths are tested, not just direct. |

The single most load-bearing rule (inherited from Phase 4): **every interface in §4 below is FROZEN
by the task named as its owner; later tasks CONSUME it, never re-lock it.** If a task author finds
the design contradicts a §4 contract, that's a Stop-and-ask, not a silent re-lock.

**Phase 5 is greenfield in three trees over prepared seams.** There is no notifications code
(no `notifications.proto`, no `notifications`/`notification_deliveries` tables, no `PushBackend`,
no `NotificationHandle`), no `apps/web`, no `apps/mobile`, no `packages/`, and **no TypeScript
proto codegen at all** (the desktop hand-mirrors proto types). But every seam is ready: `notify_user`
is a typed stub (`crates/core/src/maestro/tools/side.rs`, drained via `NotifyRecorder::snapshot()`);
`WakeupPayload` is the FROZEN ID-only carrier (`crates/transport/src/api.rs:912`, arg 2 of
`send_wakeup_hint`); the **Connect-Web bridge is built and live** (`crates/core/src/connect_bridge.rs`,
env-gated `CONCERTO_CONNECT_BRIDGE`, default OFF); `MaestroProvider` (`crates/core/src/maestro/provider.rs:150`)
is the exact template for `PushBackend`; `devices.push_token`/`push_platform` columns already exist
(`0001_initial_schema.sql:248`); `SecretKind::PushExpoApiKey` is reserved; `Devices.UpdateDevicePushToken`
is explicitly **deferred to Phase 5** in `devices.proto:173-175`. The biggest divergences from
`design/14`/`16`/`17` are reconciled by **insert 500** (read it first). The canonical product spec is
`design/14`/`16`/`17`; where the built code diverges, 500's amendment + this addendum govern, and the
task author transcribes the **built** signatures.

---

## 0. Execution order — verifiable-first (operator decision, 2026-06-14)

Phase 5 spans Rust (fully CI-verifiable), Web (fully UI-E2E + screenshot verifiable via Playwright
against a real Core), and Mobile (a hard **Tier-3 physical ceiling**: real iPhone/Android EAS
installs, real push to a locked phone, biometric, lock-screen chips, 60fps-on-hardware, and native
XCFramework/.aar **on-device** load cannot be driven by a CI sub-agent). The operator chose
**verifiable-first** execution so everything provable in CI lands first and the physical surface is
isolated to the end:

| Track | Tasks | Verifiability |
|---|---|---|
| **A — Notifications** | 500, 501–507 | **Tier 1/2, fully CI-green.** Rust + gRPC; loopback-Iroh + `MockPushBackend` doubles; zero physical deps. |
| **B — Foundation + Web** | 507.5, 519–523 | **Tier 2, fully CI-green incl. UI-E2E + screenshots.** Playwright drives headless Chromium against a real `concerto-core` with the live Connect-Web bridge. Notifications become **UI-accessible + end-to-end-tested** here (523). |
| **C — Mobile** | 508, 509, 509.5, 510–518 | **Tier 2 ceiling** (jest + RN-Testing-Library + simulator/Detox) **with explicit Tier-3 device deferrals** the operator signs at the phase gate. |

Tracks run in order A → B → C. Within a track, the DAG (§8) defines the partial order. **`deps`
honor the cross-track reality** (e.g. mobile 510 consumes Track-B's 507.5 `DataClient`; web 522's
real phone-mediated pairing needs mobile 511 but its **Tier-2 path uses a stub-phone signer** so 522
completes in Track B — the 415-style "frozen-consumer overlaps the producer" pattern).

---

## 1. The locked decisions

| # | Decision | Choice (locked) | Consumed by |
|---|---|---|---|
| **D1** | Execution order | **Verifiable-first** (§0): Track A (Notifications) → Track B (Foundation+Web) → Track C (Mobile). Front-loads CI-provable work; isolates the physical Tier-3 mobile surface. | all |
| **D2** | Notifications wire contract frozen early (contract-first) | **501 freezes `notifications.proto` message types** (`NotificationKind`, `subject_kind`, `NotificationPayload`, `ToolApprovalContext`, `InboxFilter`) + the two tables. **507 adds the `Notifications` gRPC *service* + `NotificationHandle` + wires `notify_user` live.** Mirrors 401/401.5's contract-first split so the inbox UI (523) builds against frozen types while the Rust service is still in flight. | 502–507, 523 |
| **D3** | `subject_kind` taxonomy reconciliation | `design/14 §4` lists `{workspace\|agent_session\|pr\|schedule_run}` but `§3.3` + the FK columns make **workarea-scoped the common case**. LOCK the enum = **`{workspace, workarea, session, pull_request, schedule_run}`** (`workarea` first-class; `session` not `agent_session`). 501 freezes it as a proto enum + the `notifications.subject_kind` CHECK. | 501, 504, 523 |
| **D4** | Chip identity & dispatch reconciliation | `design/14`'s `SuggestionChip`/`chip_id`/`ChipId` **do not exist**. The real `Chip` (`suggestions.proto:29`) is `rule_id=1 / workarea_id=2 / title=3 / priority=4 / created_at_ms=5 / action=6` (free-form `action` token, no `chip_id`). LOCK: notifications persist chips as the **`Chip` shape** (`chips_json`); `ActOnChip` identifies a chip by **`rule_id`** within the notification; the **`action` token → dispatch kind** map (`approval`/`resolve_* ⇒ Sessions.ResolveApproval`; `message`/`send_* ⇒ Sessions.SendMessage`; `open_*`/`navigate ⇒ navigate event`) is a documented table 505 owns. | 501, 505, 523 |
| **D5** | First-wins single source of truth | The atomic guard for first-to-approve-wins is the **EXISTING `tool_approvals` row / `Sessions.ResolveApproval` idempotency** (`crates/persist/src/tool_approvals.rs`, `crates/core/src/agent_supervisor/approval.rs`), **NOT** a second guard on `notifications.action_taken`. `notifications.action_taken` is a **denormalized UI marker** set *after* the underlying `ResolveApproval` succeeds; `ActOnChip` delegates to `ResolveApproval`'s existing `AlreadyResolved` semantics and broadcasts `approval.cancelled`. Avoids a cross-table double-resolve race. | 504, 505 |
| **D6** | ID-only `WakeupPayload` fields | `WakeupPayload` (`transport/src/api.rs:912`) is FROZEN to *exist* + be arg 2 of `send_wakeup_hint`; its **fields are not yet frozen** (left to `design/14`). **503 freezes the wire shape**: the opaque `bytes` carry CBOR/JSON `{ notification_id, kind, source }` and **nothing else**. 506's property test asserts no other field ever appears (the privacy invariant). | 503, 506, 507, 516 |
| **D7** | `PushBackend` trait + `MockPushBackend` | **503 freezes `PushBackend`** (modeled on `MaestroProvider`): `send_wakeup(target, payload) -> DeliveryReport` / `register_device(device, token, platform)` / `revoke_device(device)`. **`ExpoPushBackend`** is LIVE (BYO creds from `managed.json.push_backend_config` + `SecretKind::PushExpoApiKey`); **`MockPushBackend`** (records calls, no network) is the **Tier-2 double** for fan-out / first-wins / retry-on-Expo-down tests. **`DirectApnsFcmBackend`** is a FROZEN-unwired V1.5 seam (typed `unimplemented`, not the macro). | 503, 504, 506, 516 |
| **D8** | `Devices.UpdateDevicePushToken` + `push_platform` widen | **503** lands the deferred `Devices.UpdateDevicePushToken` RPC (`devices.proto:173-175`; appends a NEW rpc number after `GetCoreInfo`, never reorders) AND **widens `devices.push_platform` CHECK to add `'expo'`** (migration **0018**, in-place `PRAGMA writable_schema` rewrite — the `0010` precedent, because **CHECK-widening is BANNED** as a `DROP`+recreate under `foreign_keys=ON`). | 503, 516 |
| **D9** | `notification.events` subject (two-site) | **507** adds **`Subject::NotificationEvents`** + `parse_subject("notification.events")` + `StreamsHandler::with_notification_events(sender)` — the EXACT `with_maestro_events`/`with_vcs_events` pattern (`crates/core/src/handlers/streams.rs:534`, registered at **BOTH** `api_server.rs:746` + `connect_bridge.rs:337`). Carries `notification.created/updated/read/acted` on the opaque **`Event.checks_opaque=17`** carrier — **no new `Event.body` oneof arm** (the oneof is FROZEN through 16). `approval.cancelled` rides the existing `session.events.<sid>`. Missing the second registration site is the single easiest Phase-5 bug (the D8-of-Phase-4 precedent). | 507, 523 |
| **D10** | TS proto codegen toolchain + `DataClient` seam | **507.5** stands up the repo's FIRST TS codegen: **buf + `@connectrpc/connect-web` + `@bufbuild/protobuf`** (the `design/17 §3.2` named choice; pairs with the live gRPC-Web bridge). The web transport uses **gRPC-Web BINARY framing** to the bridge (avoids the prost-serde snake_case vs connect-es camelCase JSON mismatch entirely; `build.rs:88-90`). It defines the **`DataClient` interface** (`rpc(method, msg)` + `subscribe(subject, filter, cb)`) and refactors desktop `client.ts` onto **`createTauriDataClient`** (desktop's existing 39 vitest files staying green = the seam's regression proof). Desktop keeps its hand-written `api/*.ts` (low-risk; the refactor is internal to `client.ts`); generated types live in `@concerto/client` for **web** to consume. Adding `buf`/codegen tooling is a `pnpm`/devDep change (no cargo-deny). | 508, 510, 519, 520 |
| **D11** | Renderer extraction is web-only; mobile shares only `@concerto/client` | **519** extracts the React-DOM tree (`components`/`hooks`/`state`/`theme`) into **`@concerto/ui`**, consumed by desktop (imports it back) + web. **Mobile (RN) builds its OWN component tree** and consumes ONLY `@concerto/client` (proto-client + `DataClient`, with 510's native-module transport). `design/16`'s `Monaco`-free RN diff (514) is a rewrite, never a port. | 508, 510, 519 |
| **D12** | Native module via `iroh-ffi` first (operator decision) | **509 evaluates `iroh-ffi`** (iroh's official uniffi Swift/Kotlin bindings) as the base; hand-rolls Rust→C→JSI / Rust→JNI **only if** `iroh-ffi` cannot carry our `connect_channel` gRPC-over-Iroh + Noise-IK + `0x03` pairing (`tools/pair-dial` is the working exemplar). 509 = the Rust **cdylib** + uniffi scaffolding + **host build** + a **loopback Rust integration test** (Tier-2 CI-green). **509.5** = XCFramework/.aar packaging + Expo config plugin + cross-compile CI lane (Tier-2 "builds"; **on-device run = Tier-3**). The generic `rpcUnary/rpcStream(method, bytes)` surface (`design/16 §3.2`) is built on a **tonic `Grpc<Channel>` passthrough codec**, not per-service stubs. Adding `iroh-ffi`/`uniffi` = **cargo-deny Stop-and-ask** (the 313/401 precedent). | 509, 509.5, 510, 511, 516 |
| **D13** | The robust test/CI harness (operator decision) | Net-new, distributed per track (§7): **(a) `proptest`** dev-dep for 506's no-PII-in-`WakeupPayload` property test (cargo-deny vet ⇒ Stop-and-ask on any advisory); **(b) Playwright** for web + desktop-renderer **UI-E2E + screenshot baselines**, driven against a real `concerto-core` with `CONCERTO_CONNECT_BRIDGE=1` (upgrading the `97-connect-web-bridge.sh` curl double to a browser); **(c) jest + RN-Testing-Library** + simulator/Detox for mobile (Tier-2 ceiling); **(d) NEW CI jobs that actually RUN** vitest (desktop+web), Playwright (web-e2e), and jest (mobile) — **none run in CI today** (only `pnpm build`) — wired as additive `smoke.d` capabilities + workflow jobs. **Every Phase-5 feature is reachable through a UI surface and exercised by a UI-E2E test** (web/desktop in CI; mobile to the simulator ceiling + the operator's Tier-3 checklist). | 506, 507, 508, 519, 523 |
| **D14** | User-facing "Concerto" naming; drop project-grouping | Mobile/web user-facing copy uses **"Concerto"** for the chat (Maestro is the internal service; desktop already renamed it). **513** drops the stale **project-grouping** level (Workspace→Workarea; the `Project` collapse is done in code) — `design/16 §3.6`'s "grouped by project" is obsolete. | 512, 513, 523 |
| **D15** | Connect-Web bridge exposure posture | The bridge stays **default-OFF** (`CONCERTO_CONNECT_BRIDGE`) for co-located installs; web/mobile deployments turn it ON. **520** documents the bind/exposure model; **the auth-less + TLS-less bridge is NEVER exposed on a non-loopback interface** — 521 adds **LAN-direct TLS pinned to the Core identity**, 522 adds **ephemeral session-cert auth**, and 210's auth middleware gates it. Loopback-only until 521/522 land. | 520, 521, 522 |

---

## 2. Resolved sub-decisions (smaller forks — locked so the task authors stay consistent)

| Area | Question | Locked answer |
|---|---|---|
| 501 | crate vs module placement | A **new core module `crates/core/src/notifications/`** (`mod.rs`, `model.rs`, `dedup.rs`, `fanout.rs`, `push/{mod,expo,mock}.rs`, `handle.rs`), **not** a new workspace crate — it must reach the 03/04/05/13 handles + `tool_approvals` + `StreamsHandler` in-process, like every other subsystem. The persist repo module is `crates/persist/src/notifications.rs`. |
| 501 | `notifications.proto` location | `crates/proto/proto/concerto/v1/notifications.proto` (auto-discovered by `build.rs:collect_proto_files`; no manual list edit). 501 freezes the **messages**; 507 adds the **service**. |
| 502 | dedup window + retention | Dedup key `(workarea_id, kind, subject_id)` (or `(workspace_id, …)` when no workarea), **5-min** unread window (R-2, per-workspace configurable via `settings_json`). Retention (R-9): **90-day default, auto-archive not delete** — a tiny prune helper exposed for the scheduler; **no scheduler wiring in P5** (a `Notifications.PruneArchive`-style helper + a unit test; the cron hook is a P6 note). |
| 503 | push config source | `ExpoPushBackend` reads `managed.json.push_backend_config` via the existing `crates/core/src/security/managed.rs` watcher + the Expo API key from `SecretKind::PushExpoApiKey` (keychain). Self-host = BYO Expo creds (V3); no Concerto-Inc project needed for the dev loop. |
| 504 | active-viewing signal | "Actively viewing a workarea" = a client subscription to `workarea.events{id=X}` **or** any `session.events.<sid>` for a session in that workarea within the **last 30 s**, read from the `StreamsHandler` subscription registry. The fan-out planner subtracts these devices before pushing. |
| 505 | preferences storage | Per-workspace opt-out = **`workspaces.settings_json`** key (`notify_*`, the `exclude_from_maestro` RMW precedent — **no migration**); per-event-kind global defaults + per-device DND = `0018` adds a `devices.dnd_until INTEGER` column (additive) + a `notification_prefs` JSON in user settings. Resolver order: event-kind default → per-workspace → per-device → per-schedule (`design/14 §3.8`). |
| 506 | property-test tool | **`proptest`** (the `design/14 §10` "property-based" requirement). Hand-rolled exhaustive table is the fallback if `proptest` trips cargo-deny (Stop-and-ask). The invariant: for ANY notification (incl. enterprise-private + tool-approval), `WakeupPayload` decodes to exactly `{notification_id, kind, source}` and the body/subject/title never serialize into it. |
| 507 | `NotificationHandle` shape | `design/14 §5.1` verbatim: `notify(NotifyRequest)->NotificationId` (called by 04/13/05), `get_inbox`/`get_notification`/`mark_read`/`act_on_chip(id, chip_id, by)`/`update_workspace_settings`/`register_device_push_token`. The `notify_user` live sink (507) implements the `NotifySink` trait (`maestro/tools/side.rs`) over `NotificationHandle.notify(kind=agent_completed_with_message)` — **no change to the FROZEN MCP schema** (the 407 handoff contract). `read_inbox_summary` (`maestro/tools/read.rs:431`) goes live over `get_inbox`. |
| 507.5 | package boundary | Root `pnpm-workspace.yaml` + `packages/`: **`@concerto/client`** (generated proto types + `DataClient` interface + `createTauriDataClient`) consumed by desktop+web+mobile; **`@concerto/ui`** (the extracted renderer) created in **519**, consumed by desktop+web only. Desktop keeps its `apps/desktop` Vite app; the refactor proves the seam by keeping all 39 desktop vitest files green. |
| 509 | generic dispatch | The native module exposes `openSession/rpcUnary(method,bytes)/rpcStream(method,bytes,onEvent)/closeSession/natStats` (`design/16 §3.2`) over a **tonic `Grpc<Channel>` passthrough/identity codec** so proto bytes cross the FFI opaquely (510 owns connect-es encode/decode). Device-keypair gen + the `0x03` Noise-XX pairing **primitive** live in 509; 511 drives the **UX**. `natStats` = client-side `ConnectionPath` classification (not the Core's counters). |
| 514 | RN diff perf | Budget from spike 103 (`1000-line diff <1.5 s, 60 fps on iPhone13+/Pixel6+`). The spike's **on-device verdict is PENDING** operator field measurement (`design/spikes/rn-diff-findings.md`) — 514 ships the RN renderer behind the **documented V1.5 native-diff fallback**; the GO/NO-GO is a **Tier-3 checklist line**, not a phase-entry blocker. |
| 522 | ephemeral pairing double | Tier-2 uses a **stub-phone signer** (a test helper that signs the 8h `web_ephemeral` session cert, `device_kind="web_ephemeral"`, IndexedDB-stored, cleared on tab close). Real **phone-mediated** pairing (mobile 511 signs for the browser) is the **Tier-3** line. So 522's hard dep is 520 (+ 521 TLS); the 511 dep is for the real cross-device flow only. |
| all web-ts | verification dir | The README `web-ts` set targets `apps/web`. Track-B tasks run `pnpm -C apps/web …`; the **desktop-renderer screenshot** coverage (since `@concerto/ui` is shared) runs in the same Playwright suite against the web shell. 519 establishes the harness; 520–523 each add their own E2E + screenshots. |

---

## 3. Migration-number reservation

Current last shipped migration is **`0016_chat_messages_metadata.sql`** (Phase 4). Phase-5 migrations
are reserved **in task order**. A task with NO row here adds **no** migration (it uses an existing
column, a `settings_json` JSON key, an in-memory cache, or the keychain).

> **Author check (do this first):** confirm the actual highest `crates/persist/migrations/NNNN_*.sql`
> on `main` before writing. If a task landed a migration above 0016, **shift this whole block up by
> the same offset, preserving order** — and note it in your Handoff.

| Migration | Owner task | Adds |
|---|---|---|
| `0017` | **501** | `notifications` + `notification_deliveries` tables (`design/14 §4`) + the two partial unread indexes. `notifications.subject_kind` CHECK = the D3 enum; `severity` CHECK `IN ('low','medium','high')`; self-FK `superseded_by`; `action_taken_by_device_id` FK→`devices`. |
| `0018` | **503** | **In-place `push_platform` CHECK widen** to `IN ('apns','fcm','expo')` (the `0010` `writable_schema` rewrite — CHECK-widening is BANNED otherwise) **+** additive `devices.dnd_until INTEGER` column (505 per-device DND). |

- 502 (inbox/dedup/retention) = queries over `0017`, **no migration**.
- 505 (per-workspace opt-out) = `workspaces.settings_json` JSON key, **no migration** (global/per-device prefs ride `0018`'s `dnd_until` + user settings JSON).
- 504/506/507 = no schema change (fan-out logic / property test / service + in-process subject).
- Track B (web) + Track C (mobile) add **no migrations** (TS/RN only; `UpdateDevicePushToken` writes existing columns).

**CHECK-widening is BANNED** (`foreign_keys=ON` + per-migration transactions ⇒ `DROP` cascade-deletes
children). 0018 is the only widen and MUST use the `0010` in-place rewrite.

---

## 4. Cross-cutting FROZEN contracts (owner → consumers)

**4.1 `notifications.proto` messages + the two tables — FROZEN by 501 (D2/D3/D4).** The
`NotificationKind` enum (the 6 kinds, `design/14 §3.1`), `subject_kind` (the D3 5-value enum),
`NotificationPayload` (`id/kind/subject_kind/subject_id/title/body/at_ms/chips/approval`), the new
`ToolApprovalContext` message, `InboxFilter`, and the `notifications`/`notification_deliveries`
schema (`0017`). Chips reuse the `Chip` shape (`suggestions.proto:29`, D4). **All timestamps are
`int64` unix-ms** (the Maestro `generated_at_ms` precedent — no `google.protobuf.Timestamp`). 502–507
+ 523 consume; never re-shape.

**4.2 `Notifications` gRPC service + `NotificationHandle` + `notification.events` — FROZEN by 507
(D2/D9).** `service Notifications { GetNotification / GetInbox / MarkRead / ActOnChip /
UpdateWorkspaceSettings / RegisterDevicePushToken }` (mirrors `NotificationHandle`, `design/14 §5`),
the `Subject::NotificationEvents` arm + `parse_subject` + `with_notification_events`, the **two-site
`NotificationsServer` registration** (`add_core_services` + `connect_bridge.rs`), and the live
`notify_user` `NotifySink`. Events ride `Event.checks_opaque=17`; **no new oneof arm**. 523 consumes
the proto/TS surface; the inbox UI subscribes `notification.events`.

**4.3 `PushBackend` + `WakeupPayload` fields + `UpdateDevicePushToken` — FROZEN by 503 (D6/D7/D8).**
The trait (`send_wakeup`/`register_device`/`revoke_device`), `ExpoPushBackend` LIVE +
`MockPushBackend` double + `DirectApnsFcmBackend` frozen seam, the `WakeupPayload` JSON/CBOR shape
(`{notification_id, kind, source}` ONLY), the `Devices.UpdateDevicePushToken` RPC (new rpc number),
and the `push_platform`+`'expo'` widen (`0018`). 504/506/507/516 consume.

**4.4 The `DataClient` interface + `@concerto/client` codegen + `pnpm` monorepo — FROZEN by 507.5
(D10/D11).** The `DataClient` TS interface (`rpc`/`subscribe`), the buf+connect-es codegen output
shape (gRPC-Web **binary** framing), `createTauriDataClient`, the root `pnpm-workspace.yaml` +
`packages/@concerto/client` boundary. 508/510 (mobile), 519/520 (web) all consume; the desktop
refactor onto `createTauriDataClient` is the regression proof (39 vitest files green).

**4.5 `@concerto/ui` shared renderer — FROZEN by 519 (D11).** The extracted React-DOM tree
(`components`/`hooks`/`state`/`theme`) consumed by desktop (re-imported) + web. Mobile does NOT
consume it (D11). 523 adds the inbox/notification surface here so desktop + web both inherit it.

**4.6 `ConcertoIroh` native-module JS surface — FROZEN by 509 (D12).** `openSession` /
`rpcUnary(method, Uint8Array)->Uint8Array` / `rpcStream(method, Uint8Array, onEvent)->SubId` /
`closeSession` / `natStats` + the `0x03` Noise-XX pairing primitive + device-keypair gen. 510 adapts
the connect-es `DataClient` to call it; 511 drives the pairing UX; 516 drives push wakeup → fetch.

---

## 5. The four inserts + two reframes (amend `README.md §6` Phase-5 inventory)

| Task | Goal | Deps | Tier | Type |
|---|---|---|---|---|
| **500** | **Phase-5 architecture reconciliation** — amend `design/14`/`16`/`17`: (a) `subject_kind` taxonomy (D3), (b) chip identity & `action`→dispatch map (D4), (c) first-wins single-guard (D5), (d) `WakeupPayload` shape + `push_platform` widen (D6/D8), (e) Connect-Web bridge default-OFF + auth/TLS posture (D15), (f) Maestro→Concerto naming + drop project-grouping (D14), (g) `iroh-ffi`-first native module (D12), (h) the rn-diff PENDING note (514). Runs **first** (doc, `design/` only — zero code collision), like 200 / 315.0 / 400. | — | 3 | doc |
| **507.5** | **JS monorepo + TS proto codegen + `DataClient` seam** — root `pnpm-workspace.yaml` + `packages/@concerto/client` (buf + `@connectrpc/connect-web` + `@bufbuild/protobuf` codegen from `crates/proto`, gRPC-Web binary), the `DataClient` interface, `createTauriDataClient`, and the desktop `client.ts` refactor onto it (all 39 desktop vitest files stay green). **The foundation 508/510/519/520 depend on** (replaces the README's implicit "shared `packages/` extraction"). | 218 | 2 | web-ts |
| **509.5** | **Native-module packaging + cross-compile lane** — XCFramework (iOS) + `.aar` (Android) from 509's cdylib, the Expo config plugin, and a **cross-compile-only CI lane** (`aarch64-apple-ios` / `aarch64-linux-android` link-check via `cargo-ndk` + `rust-toolchain` targets). Tier-2 = **builds**; **on-device load/run = Tier-3** (phase checklist). Split out of the README's monolithic 509 (`+CI` clause). | 509 | 2 | rn-mobile / infra-ops |
| **523** | **Inbox / notification-center / prefs UI** (shared `@concerto/ui` renderer; desktop + web) — the chronological inbox feed + unread badges, the `notification.events` live subscription, the notification-derived confirmation-chip surface + toasts, and per-workspace/per-event opt-out settings. **Wired to 507's LIVE `Notifications` service** and **end-to-end UI-E2E + screenshot tested via Playwright against a real Core** (the strongest verification of Track A through the UI). | 507, 519, 520 | 2 | web-ts |
| **508** *(reframed)* | **Expo/EAS scaffold + mobile jest/RN-TL harness + mobile CI lane** — `apps/mobile` Expo app + EAS config + bottom-tab shell, consuming **507.5's `@concerto/client`** (the README's "shared packages/ proto-client extraction" now lives in 507.5). Stands up jest + RN-Testing-Library + a mobile CI job. | 507.5 | 2 | rn-mobile |
| **519** *(reframed)* | **`apps/web` SPA + `@concerto/ui` extraction + Playwright harness** — extract the desktop renderer to `@concerto/ui`, build the web shell + `createConnectWebDataClient` skeleton, and **establish the Playwright UI-E2E + screenshot harness** (real Core + `CONCERTO_CONNECT_BRIDGE=1`) + the web-e2e/run-vitest CI jobs. | 507.5, 218 | 2 | web-ts |

---

## 6. Refined dependencies (the task-graph edge-list)

These deps refine the README inventory rows; they (and the README rows) MUST appear in each task
file's `Depends on`. The machine-consumable form is `PHASE5_DAG.json` + §8.

| Task | Depends on | Why (beyond the README row) |
|---|---|---|
| 500 | — | doc root (runs first) |
| 501 | 500 | implements 500's taxonomy/chip/payload amendments; freezes `notifications.proto` messages + `0017` |
| 502 | 501 | inbox/dedup/retention over 501's tables |
| 503 | 501 | `PushBackend`+Expo+Mock, `WakeupPayload` shape, `UpdateDevicePushToken`, `0018` widen |
| 504 | 503, 209 | fan-out + first-wins (over `tool_approvals`, D5) + active-viewing; 209 provides the eligible-device set |
| 505 | 504 | `ActOnChip` dispatch (D4 map) + prefs hierarchy + per-workspace opt-out |
| 506 | 503 | property test over 503's `WakeupPayload` (proptest, D6) |
| 507 | 504, 407 | `Notifications` service + `NotificationHandle` + `notification.events` + live `notify_user` (fills 407's sink) + live `read_inbox_summary` |
| 507.5 | 218 | TS codegen + `DataClient` + monorepo; desktop refactor proven against 218's vitest setup |
| 519 | 507.5, 218 | `@concerto/ui` extraction + web shell + Playwright harness over 507.5's `DataClient` |
| 520 | 204, 519 | Connect-Web data client (HTTP/2 + SSE + AckOffset) against the live bridge (204) |
| 521 | 215, 520 | LAN-direct TLS pinned to Core identity + remote WSS-via-relay Noise IK (215's relay/WSS) |
| 522 | 520 *(real flow: +511)* | ephemeral pairing — Tier-2 stub-phone signer (520 only); real phone-mediated pairing needs 511 (Tier-3) |
| 523 | 507, 519, 520 | inbox/notification UI in `@concerto/ui`, live against 507's service, E2E via 519's harness |
| 508 | 507.5 | Expo scaffold consumes `@concerto/client`; jest/RN-TL + mobile CI |
| 509 | 212, 508 | `ConcertoIroh` native module (iroh-ffi cdylib + host build + loopback test) over 212's Iroh endpoint |
| 509.5 | 509 | XCFramework/.aar packaging + Expo plugin + cross-compile lane |
| 510 | 509, 509.5 | connect-es `DataClient` adapted to the native module (`createNativeDataClient`) |
| 511 | 510, 207 | pairing QR + `expo-secure-store` + multi-Core (207's Noise-XX pairing) |
| 512 | 511 | bottom-tab nav (Concerto/Workspaces/Inbox, D14) |
| 513 | 512 | workspaces drill-down + workarea detail (no project tier, D14) |
| 514 | 103, 513 | touch-first RN diff (spike-103 budget + fallback) |
| 515 | 512 | voice dictation |
| 516 | 503, 511 | push registration + post-wakeup fetch + lock-screen chips + biometric (over 503's push + 511's keystore) |
| 517 | 510 | localhost preview tunnel WebView |
| 518 | 510 | lite-mode cellular + handoff + background lifecycle |

> **Headline overlaps:** (1) **507.5 unblocks both web and mobile** — it is the shared monorepo
> foundation, sequenced first in Track B. (2) **523 proves notifications through the UI** with a real
> Core — the Track-A → Track-B verification bridge. (3) **522 completes in Track B** behind a
> stub-phone signer (the 511 dep is the Tier-3 real-device flow only).

---

## 7. Verification model & the robust test/CI harness (operator's primary ask)

The operator's explicit requirement: **every feature is unit + integration + E2E tested, every
feature is reachable through a UI, and the UI is exercised by UI-E2E with screenshots.** This section
is the contract for that. It is realized **distributed across tasks** (not one mega-task), with the
holistic picture here.

### 7.1 What gets built (net-new test infra)

| Layer | Tool | Owner task | Runs in CI? |
|---|---|---|---|
| Rust unit | `#[test]` / `#[tokio::test]` | every Track-A task | yes (existing nextest) |
| Rust integration | `crates/test-harness` `CoreUnderTest::spawn` + new `notifications_client()`/`devices_client()` accessors | 501–507 | yes (nextest) |
| Push fan-out / first-wins / retry | **`MockPushBackend`** double | 503/504 | yes |
| Privacy property | **`proptest`** (no-PII-in-`WakeupPayload`) | 506 | yes |
| Notifications smoke | new `scripts/smoke.d/NN-notifications.sh` (create → fetch over loopback-Iroh → `ActOnChip`; real-Expo legs `ci_skip` behind `--ci-mode`) | 507 | yes (smoke.yml) |
| Web/desktop unit | **vitest** (existing, extended) | 507.5/519/523 | **NEW job** (vitest never runs in CI today) |
| **Web/desktop UI-E2E + screenshots** | **Playwright** vs real Core + `CONCERTO_CONNECT_BRIDGE=1` | 519 (harness) + 520/521/522/523 (coverage) | **NEW `web-e2e` job** |
| Mobile unit | **jest + RN-Testing-Library** | 508 + every Track-C task | **NEW mobile job** |
| Mobile UI-E2E | **Detox / simulator** (Tier-2 ceiling) | 508/512/513/516 | best-effort (macOS runner); else Tier-3 |
| Native build | host cdylib + loopback (509); cross-compile link-check (509.5) | 509/509.5 | yes (host) / NEW cross-compile lane |

### 7.2 The Tier boundary — what CI proves vs. what the operator signs

**Fully CI-provable (Tracks A + B): every notification + every web feature.** Playwright drives a
real browser against a real `concerto-core` (the bridge is already built); screenshots are asserted
against per-OS baselines on a **single pinned runner (ubuntu)** to avoid font/AA drift, with a
tolerance threshold. Notifications are exercised **through the inbox UI** end-to-end (523).

**Tier-3 — operator signs at the phase gate (the README Phase-5 checklist, unchanged):** real EAS
build on a physical iPhone + Android; pair both; real push to a **locked** phone + approve from the
lock screen; first-to-approve-wins across **two physical devices**; the RN diff renderer at **60 fps
on real hardware** (the spike-103 verdict); native XCFramework/.aar **on-device** load; the web
client on a **borrowed laptop + Linux** (LAN-direct + relayed). Mobile native-module *compilation*
needs Xcode/Android-NDK toolchains that are not assumed present in CI — 509.5's cross-compile lane is
a **link-check**, not an on-device run.

### 7.3 The standing CI gap this phase closes

Today `.github/workflows/ci.yml` runs **only `pnpm build`** for the frontend — **vitest never runs in
CI** and there is **no Playwright/jest/Detox**. Track B (519) adds the `web-e2e` + `run-vitest` jobs;
Track C (508) adds the mobile jest job; 509.5 adds the cross-compile lane. After Phase 5 the README
§5.3 `web-ts`/`rn-mobile` command sets are **actually enforced in CI**, not aspirational.

---

## 8. Concurrency / wave map (pipelined + bounded-parallel, K = 3)

Phase 5 keeps the orchestrator default **K = 3** (lower than Phase 4's K=4: Track A serializes on the
new `notifications` module + proto + migrations, and the monorepo scaffold 507.5 is a single
high-churn root). **The merge invariant is unchanged: dependency-ordered serialized merges; `main`
always green; in-flight branches rebase onto each new `main`; a substantive rebase conflict →
re-dispatch the later task fresh.** Eligibility = **dependency-ready (per §6 / `PHASE5_DAG.json`) AND
write-set-disjoint on a hard seam from every in-flight task.** Tracks run **A → B → C** (§0);
within a track the DAG defines overlap.

**Completion state (update as you go):** ✅ Track A backend logic complete & green — **500** (design
reconciliation), **501** (model + 0017 + frozen proto), **502** (de-dup + retention), **503** (PushBackend
+ Expo/Mock + WakeupBody + UpdateDevicePushToken + 0018), **504** (fan-out + post-wakeup fetch +
active-viewing seam), **505** (ActOnChip + prefs), **506** (privacy proptest), **507a** (NotificationHandle
+ notify() orchestration), **507b-1** (notification.events subject + producer bridge), **507b-2**
(Notifications gRPC service + handler), **507b-3 b3-i** (live Notifications service registered on
UDS+Iroh + `notification.events` producer — verified end-to-end over a real Core), **507b-ii a** (live
`read_inbox_summary`). **Track B started:** **507.5** (pnpm monorepo + buf/connect-es TS codegen + `@concerto/client`
`DataClient` seam), **519** (`apps/web` notifications-inbox SPA + Playwright UI-E2E + screenshots +
web CI workflow). **520** (Notifications on the connect-web bridge — D9 site 2, verified gRPC-Web `GetInbox`), **523 core**
(live web inbox vs a running Core — real notifications rendered in a browser, Playwright-screenshotted
via `scripts/web-live-demo.sh`). **507b-ii + b-iii** (`notify_user` LIVE sink — Maestro `notify_user`
routes through `LiveNotifySink`→`NotificationHandle.notify` (kind=`AgentCompletedWithMessage`), wired in
`boot.rs`; `scripts/smoke.d/92-notifications.sh` + `smoke-client get-inbox`, `smoke --only notifications`
PASS). **Track A is now functionally complete.** **523 full** (`@concerto/ui` extracted — shared
React-DOM `Inbox` consumed by **both** `apps/web` and `apps/desktop`; desktop folded into the root pnpm
workspace, RightRail Inbox tab, 227 desktop tests green). **Track C started:** **508** (`apps/mobile`
Expo SDK 54 + RN + TS scaffold: expo-router bottom-tab shell Concerto/Workspaces/Inbox, Inbox wired to
`@concerto/client` notif types, jest + RN-Testing-Library harness + mobile CI lane; native module /
`expo prebuild` / EAS native builds are Tier-3). **Combined-tree re-verify after integrating the three
worktree branches — all green:** TS (client 4, ui 5, web typecheck+build, mobile 2, desktop 227) + Rust
(clippy `-D warnings` clean, fmt clean, notifications lib 27, `maestro_notify_user` 2). **520 full** (live
web inbox — `@concerto/client` `subscribeNotificationsLive` decodes the FROZEN opaque `Event.checks_opaque`
frames, stream-first → AckOffset polling fallback with a Live/Polling badge; `@concerto/client/testing`
Core-free mock; web Playwright proves a streamed item appears with no refresh + the polling-fallback path,
+ screenshots; client 11 tests, web e2e 5-pass). **513** (mobile workspaces drill-down + workarea detail
— `WorkspacesScreen` over a `WorkspacesClient` seam with real `@concerto/client` proto types via a
fixture-backed mock, Workspace→Workarea per D14 (no project tier), JS-only `SegmentedControl`
Sessions/Code&PRs to stay Tier-2; 22 tests/5 suites). **Post-520/513 combined re-verify — all green:**
client 11, ui 5, web build + e2e (5 pass / 1 live-Core skip), mobile 22, desktop 227. ⏳ Track B: **521**
(LAN TLS/relay), **522** (ephemeral pairing — Tier-2 stub signer). **509 done** (`crates/concerto-iroh-ffi`
— **hand-rolled `uniffi` cdylib over `concerto-transport`, the D12 fallback**: iroh-ffi is unusable — git-only,
no 0.98.x release, forces a colliding second iroh. Thin facade over the spike-validated seam `tools/pair-dial`
proves: `connect_channel` Noise-IK API channel, `pair_over_iroh` ported verbatim (0x03 tag), generic
byte-passthrough `rpcUnary`/`rpcStream` via an identity tonic codec, client-side `natStats`. uniffi 0.28
MPL-2.0 already allow-listed → cargo-deny clean, no new advisory; single iroh 0.98.2, no iroh-ffi; out of
default-members. Adversarial review verdict **solid**. Verified: clippy clean, unit 12/12 + **live loopback**
(real Core: 0x03 pair→openSession→rpcUnary `GetServerCapabilities`==IROH→rpcStream event→natStats==Lan→close),
fmt clean).

**✅ ALL 26 PHASE-5 TASKS IMPLEMENTED & INTEGRATED (Waves 1–2 landed; 42 commits ahead of `main`).**
**Track B finished:** **521** (LAN-direct TLS — `connect_bridge_tls.rs` derives a rustls cert deterministically
from the Core identity pubkey, publishes the SPKI fingerprint for pinning, opt-in `CONCERTO_CONNECT_BRIDGE_TLS`
default-OFF; `connect_bridge_tls` 3/3 incl. impostor-pin reject; relay WSS Noise-IK `relay_noise_ik` 2/2; no new
external crate). **522** (ephemeral pairing — `@concerto/client` session machinery: Web-Crypto Ed25519 stub-phone
signer, 8h `web_ephemeral` cert, IndexedDB store, clear-on-tab-close + remember-browser, connect interceptor;
client 27 tests, web e2e session 4/4 + screenshots). **Track C finished:** **509.5** (uniffi-bindgen emits Swift+Kotlin
on the host = Tier-2 proof; XCFramework + .aar packaging scripts (shellcheck-clean); `.github/workflows/native.yml`
cross-compile lane (actionlint-clean); off-by-default `cli` feature keeps the shipped cdylib lean — local target
link-check is an honest Tier-3 deferral). **510** (`createNativeDataClient` adapts the connect-es `DataClient` over
the 509 opaque-bytes module — toBinary→rpcUnary→fromBinary unary + rpcStream→async-iterable subscribe). **511**
(pairing QR scanner `expo-camera` + connect-blob parse + `expo-secure-store` device-seed/cert persistence +
multi-Core add/list/switch). **512** (Concerto chat landing tab — real generated `maestro_pb` types via a `ChatClient`
seam, streaming assistant bubbles, composer, paired/empty/error states; live `getHistory`/`sendToMaestro` Tier-2,
`maestro.events` token stream a Tier-3 seam). **514** (pure-RN touch-first unified-diff renderer — virtualized
FlatList, collapsible hunks, spike-103 perf budget documented; surfaced in the Code&PRs segment + a demo route).
**515** (voice-dictation composer mic over a `SpeechRecognizer` seam; real STT Tier-3). **516** (push registration →
`Devices.UpdateDevicePushToken` `expo`, ID-only D6 wakeup → post-wakeup fetch, lock-screen Approve/Dismiss chips →
`ActOnChip`, fail-closed biometric gate). **517** (localhost preview tunnel WebView over a `TunnelClient` seam,
`react-native-webview`). **518** (AppState background→`closeSession` / foreground→reopen+re-subscribe with
`since_offset`; `expo-network` cellular→lite-mode; cross-device handoff token round-trip).
**Combined verification (merged tree, all green):** Rust — clippy `-D warnings` clean, fmt clean, cargo-deny ok
(no new advisory), bridge-TLS 3/3, relay Noise-IK 2/2, iroh-ffi 12 unit + live loopback, bindgen emits Swift+Kotlin;
TS — `@concerto/client` 27, `@concerto/ui` 5, web build + Playwright (session 4/4 + live 2/2), **mobile 25 suites /
154 tests**, desktop 227. **Remaining: Wave 3** — comprehensive adversarial review across the whole Phase-5 surface
→ fix every finding → final full verification. **Tier-3** (real on-device run, native-bindings load, real camera /
Keychain / biometric / push delivery, NAT diversity, on-device diff perf, live cross-device handoff, the Core trusting
the web_ephemeral cert) is deferred to the operator phase gate by design (Track C's Tier-2 ceiling + the D15 posture).

### 8.1 Per-task write-sets (the disjointness oracle)

Hard seams: any `*.proto`, a shared `mod.rs`/`lib.rs`, `crates/core/src/boot.rs`, `api_server.rs`,
`connect_bridge.rs`, a migration, `crates/core/src/handlers/streams.rs`, the same source module, the
root `pnpm-workspace.yaml`/`packages/@concerto/*` index. Trivially-mergeable: `Cargo.lock`,
`pnpm-lock.yaml`, `docs/interfaces/*`, `scripts/smoke.manifest`, distinct test files, distinct
`apps/*` vs `crates/*` trees.

| Task | Write-set (globs) | Hard seams shared with |
|---|---|---|
| 500 | `design/14_*.md`, `design/16_*.md`, `design/17_*.md` | — (doc only) |
| 501 | `crates/proto/proto/concerto/v1/notifications.proto`, `crates/persist/migrations/0017_*.sql`, `crates/persist/src/{notifications,lib,api}.rs`, `crates/core/src/notifications/{mod,model}.rs` | 502–507 (notifications module + proto); 503 (migrations dir) |
| 502 | `crates/core/src/notifications/{dedup,mod}.rs`, `crates/persist/src/notifications.rs` | 501/504 (module + persist) |
| 503 | `crates/core/src/notifications/push/{mod,expo,mock}.rs`, `crates/proto/proto/concerto/v1/devices.proto`, `crates/persist/migrations/0018_*.sql`, `crates/core/src/handlers/devices.rs`, `crates/transport/src/api.rs` (WakeupPayload doc only) | 501 (migrations dir); `devices.proto` (no other P5 task writes it) |
| 504 | `crates/core/src/notifications/{fanout,mod}.rs` | 501/502/505 (module) |
| 505 | `crates/core/src/notifications/{chip_dispatch,mod}.rs`, `crates/persist/src/workspaces.rs` (settings key) | 504 (module) |
| 506 | `crates/core/tests/notifications_privacy.rs`, `Cargo.toml`/`deny.toml` (proptest) | — (test + dep only) |
| 507 | `crates/core/src/notifications/handle.rs`, `crates/proto/proto/concerto/v1/notifications.proto` (service), `crates/core/src/handlers/{mod,notifications,streams}.rs`, `api_server.rs`, `connect_bridge.rs`, `crates/core/src/maestro/tools/side.rs` (live sink), `scripts/smoke.d/NN-notifications.sh` | 501 (proto); 414-era streams/api_server/connect_bridge (none in-flight in P5) |
| 507.5 | `pnpm-workspace.yaml`, `package.json` (root), `packages/@concerto/client/**`, `buf.gen.yaml`, `apps/desktop/src/api/client.ts` (refactor) | 519/508 (packages root); 520 (DataClient) |
| 519 | `apps/web/**`, `packages/@concerto/ui/**`, `apps/desktop/src/**` (import @concerto/ui), `.github/workflows/*` (web-e2e/vitest jobs) | 520/521/522/523 (apps/web + @concerto/ui); 507.5 (packages root) |
| 520 | `apps/web/src/data/**`, `packages/@concerto/client/transport/**` | 519/521/522 (apps/web) |
| 521 | `apps/web/src/data/{tls,relay}.ts`, `crates/relay/**` (stub seam, read) | 520 (data dir) |
| 522 | `apps/web/src/pairing/**`, `apps/web/src/data/**` | 520/521 (data dir) |
| 523 | `packages/@concerto/ui/notifications/**`, `apps/web/e2e/notifications.spec.ts`, `apps/desktop/src/**` (mount) | 519 (@concerto/ui) |
| 508 | `apps/mobile/**`, `.github/workflows/*` (mobile job) | 509–518 (apps/mobile) |
| 509 | `crates/concerto-iroh-ffi/**`, `Cargo.toml`, `rust-toolchain.toml`, `deny.toml` | 509.5 (crate); `Cargo.toml`/toolchain |
| 509.5 | `crates/concerto-iroh-ffi/build/**`, `apps/mobile/modules/**`, `.github/workflows/*` (cross-compile lane) | 509 (crate); 508 (apps/mobile) |
| 510 | `apps/mobile/src/data/**`, `packages/@concerto/client/transport/native.ts` | 511–518 (apps/mobile) |
| 511–518 | `apps/mobile/src/**` (distinct screen/feature dirs) | each other (apps/mobile — serialize on shared nav/index) |

### 8.2 Suggested waves (illustrative — recompute eligibility each tick from `PHASE5_DAG.json`)

- **Wave 1 (Track A start):** `500` (doc) — runs first, alone (blocks 501).
- **Wave 2:** `501` (proto messages + tables) — the contract-first root; serializes the module/proto seam.
- **Wave 3:** `502` ∥ `503` ∥ `506` — disjoint-ish over 501 (502=dedup, 503=push+`devices.proto`+`0018`, 506=test+proptest); watch the `notifications/mod.rs` soft seam.
- **Wave 4:** `504` (←503/209) → `505` (←504); `507` (←504/407) once 502/503/505 land (it touches the proto service + two-site registration + streams).
- **Wave 5 (Track B start):** `507.5` (monorepo foundation — single high-churn root, run alone) → then `519` (web + @concerto/ui + Playwright harness).
- **Wave 6:** `520` (←204/519) ∥ then `521` (←215/520), `522` (←520, stub-phone), `523` (←507/519/520 — the live notifications UI E2E).
- **Wave 7 (Track C start):** `508` (Expo scaffold ←507.5) → `509` (native module) → `509.5` (packaging) → `510` (native DataClient).
- **Wave 8:** `511`→`512`→`513`/`515`, `514` (←103/513), `516` (←503/511), `517`/`518` (←510) — Track-C screens, serializing on the shared mobile nav/index.

**Cluster summary (parallelize across, serialize within):**

| Cluster | Tasks | Shared hot files |
|---|---|---|
| **N — notifications backend** | 500→501→{502,503,506}→504→505→507 | `notifications/mod.rs`, `notifications.proto`, persist migrations, `streams.rs` |
| **F — foundation** | 507.5 | `pnpm-workspace.yaml`, `packages/@concerto/client`, desktop `client.ts` |
| **W — web** | 519→{520,521,522}, 523 | `apps/web`, `@concerto/ui` |
| **M — mobile** | 508→509→509.5→510→{511…518} | `apps/mobile`, `concerto-iroh-ffi` |

**If unsure whether two tasks are disjoint → serialize them.** A green `main` and correct interfaces
outrank the speedup.

*End of Phase-5 planning addendum. The task files (500, 501–507, 507.5, 519–523, 508, 509, 509.5,
510–518) are written against this document, `README.md`, the `PHASE5_DAG.json` graph, and the
`design/14`/`16`/`17` sections each cites.*
