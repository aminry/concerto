# Task 315 — Webhook Receiver: relay route + `0x04` channel + HMAC verify + delivery-id idempotency

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 313, 215, 315.0 |
| Touches subsystem(s) | 13 (VCS Provider Integration), 11 (Transport & Relay), 12 (Security & Identity), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Make the Core receive push-driven GitHub events instead of poll-storming. Today there is **no inbound webhook path**: the relay (`crates/relay`) only routes Iroh + bridges browser WSS (`design/11 §3.4`), the transport demux (`crates/transport`) knows three channel tags (`0x01 Api`/`0x02 PushHint`/`0x03 Pairing`), and the V0.1 VCS code (`crates/core/src/vcs`) has no `ingest_webhook`. This task implements the full path Task 315.0 just froze in the design: (1) a **second relay HTTP route** `POST /webhook/github/<endpoint_id>` (sibling of the WSS bridge in `crates/relay`) that opens an ephemeral **`0x04` Webhook** Iroh bidi to the addressed Core and writes the FROZEN `WebhookEnvelope` (`design/11 §3.4.1`); (2) the **`0x04` `ChannelTag` arm** on the transport demux (`crates/transport`) that hands the duplex to a webhook handler — **not** the Noise gRPC path; (3) on the Core, **`VcsHandle::ingest_webhook(repo, WebhookPayload)`** (`design/13 §5.1`) which **constant-time-verifies HMAC-SHA256** over the body against the per-repo `VcsSecretSlot::WebhookSecret` (313/D4), dedupes replays via a **restart-surviving delivery-id idempotency cache** (migration **0013** `webhook_deliveries`), parses the event, targeted-invalidates the affected cache rows, and chains a 200/4xx ack back to GitHub through the relay. After this task the Core gets near-instant check/PR/deploy/thread updates over the relay; Task 318's `wait_for_check_runs` prefers these webhook updates over polling, and Task 316 emits the resulting `checks.<wa>.<repo>` events. The Tier-2 double is a loopback relay+Core (no real GitHub delivery); real GitHub→relay delivery is the Phase-3 Tier-3 checklist line.

## Inputs to read before starting
- `tasks/v1.0/315.0-webhook-relay-framing-design.md` → its Outputs are the `design/11 §3.4.1` + `design/13 §3.2`/§6.2/§8 amendments this task **implements to the letter**. Read `design/11 §3.4.1` (the `WebhookEnvelope` framing — five fields, the pinned encoding, the body ceiling, the ack→HTTP-status mapping) and `design/11 §3.3` (the `0x04 Webhook` channel, **no Noise**). 315.0 removed every design choice; do not re-invent the framing.
- `design/13_VCS_Provider_Integration.md` §3.2 (webhook design + relay route, per-pairing-rotated secret, relay forwards opaque bytes, Core verifies + falls back to polling on failure), §6.2 (the `GH→Relay→Trans→Vcs→verify HMAC→parse→cache→200` sequence — this is the exact pipeline), §6.3 (cache invalidation: targeted on webhook receipt), §8 (the failure rows you implement: HMAC mismatch → **drop + log + do NOT inform sender**; replay → **drop on delivery-id**; spoofed source → reject; offline Core → relay 5xx + drop). §4 (`VcsState.webhook_secrets: HashMap<RepoId, [u8;32]>` — the in-memory secret cache loaded from the keychain).
- `design/11_Remote_Transport_Relay.md` §3.2 (the relay's job list — webhook forwarding is now a named bullet), §3.4 + **§3.4.1** (the route + envelope, sibling of the WSS bridge), §3.9 (the relay-visibility carve-out: it forwards the GitHub body opaquely but never holds the HMAC secret + does no verification).
- `crates/relay/src/wss.rs` — the **precedent to mirror, not copy-paste**: `WssBridgeServer::start` builds a `RelayMode::Custom` Iroh client `Endpoint`; `serve_connection` parses `<endpoint_id>` *before* work (`parse_endpoint_id`, `MAX_ENDPOINT_ID_LEN`, `WSS_PATH_PREFIX`), dials `EndpointAddr::new(id).with_relay_url(...)`, `conn.open_bi()`, and the `TRANSPORT_ALPN = b"concerto/transport/1"` constant. The webhook route reuses this client endpoint (or builds a sibling one) but does a **plain HTTPS POST → one `0x04` bidi → read ack → HTTP status**, not a WebSocket pump. Reuse the path-parse + the relay-forced dial; note `wss.rs`'s flagged drift (no dialable endpoint on `iroh-relay::Server`; the route holds its own client endpoint).
- `crates/relay/src/api.rs` + `crates/relay/src/state.rs` + `crates/relay/src/config.rs` — the FROZEN relay surface (`RelayConfig` env vars, `RelayState`, the `WSS_LISTEN_ADDR`/`WSS_PATH_PREFIX` precedent). The webhook route is **additive** behind a new env var (decide + freeze the name, e.g. `WEBHOOK_LISTEN_ADDR`; mirror the `wss_listen_addr: Option<SocketAddr>` shape) — do not regress 214/215's frozen config surface. If the webhook route can share the WSS HTTPS listener (one TLS server, two paths `/wss/` + `/webhook/github/`) decide that in-task and freeze it; otherwise a second listener.
- `crates/transport/src/channels.rs` (`tag_from_byte` — add the `0x04 => Ok(ChannelTag::Webhook)` arm + update the doc-comment table + the frozen-tag tests), `crates/transport/src/api.rs` (`ChannelTag` enum — add `Webhook = 0x04`; the `from_byte`/`as_byte` round-trip), `crates/transport/src/endpoint.rs` (the serve-loop `match tag { … }` at ~line 580 — add the `ChannelTag::Webhook` arm that reads the `WebhookEnvelope` off the **raw** duplex (no `handshake_responder` — `0x04` is not Noise) and hands it to the Core's VCS webhook ingest seam, then writes the ack back on the same duplex).
- `crates/transport/src/lib.rs` — `ChannelTag` is re-exported; the new variant flows out automatically. Confirm the `ApiDispatcher`/seam wiring: the webhook ingest needs a Core-side callback the transport invokes (mirror how `ApiDispatcher`/`AuthObserver` are injected into the serve loop — add a small `WebhookSink`-style seam the Core supplies at `serve_iroh`).
- `crates/core/src/vcs/actor.rs` + `mod.rs` — the V0.1 `VcsProviderActor`/`VcsHandle` (its `gh()`/`create_pr`/`get_check_runs`/`fetch_issue` methods); after **Task 313** these are the `concerto-vcs` crate's `VcsProvider`/`VcsHandle`. Add `ingest_webhook(repo, payload)` to the `VcsHandle` (the FROZEN `design/13 §5.1` method). Read 313's Handoff for the exact `VcsHandle`/`crates/vcs` shape — **313 is a hard dependency** (the trait, the `testkit`, and `VcsSecretSlot`).
- `crates/keychain/src/api.rs` — the `CoreSecretSlot`/`get_core_secret(core_id, slot)` parameterized-accessor precedent. **313 freezes `VcsSecretSlot::WebhookSecret`** keyed by `scope_id = repo_id` (account `vcs.<repo_id>.webhook_secret`, `PHASE3_PLANNING §4.1`/D4); this task **reads** it for HMAC verify, never re-defines it. Confirm 313's Handoff names `VcsSecretSlot::WebhookSecret`.
- `crates/persist/migrations/0008_pull_requests.sql` — confirm the **highest shipped migration is 0008** (per `PHASE3_PLANNING §3` author-check). This task's migration is **0013** (`webhook_deliveries`); 0009–0012 are owned by 306/307/310/313 and may or may not be on `main` when 315 runs. **Author check (do first):** read the actual highest `crates/persist/migrations/NNNN_*.sql`; if the reserved block shifted, shift 0013 by the same offset preserving order, and note it in Handoff.
- `crates/persist/src/` — the migration + reader/writer convention (a `webhook_deliveries.rs` module mirroring an existing small table like `tool_approvals`/`schedules`): an `insert_delivery_if_absent(delivery_id, repo_id, received_at) -> bool` (true = newly inserted = process; false = replay = drop) + a TTL-cleanup sweep.
- `tasks/v1.0/313-vcs-provider-github.md` → "Handoff Notes" — the `crates/vcs` crate name, the `VcsProvider` method set, the `#[cfg(feature = "testkit")]` `FakeGitHub` builders + fixtures under `crates/vcs/tests/fixtures/`, and `VcsSecretSlot`. **315 enables `concerto-vcs/testkit` as a dev-dep** (`PHASE3_PLANNING §4.3`).
- `tasks/v1.0/215-relay-wss-bridge.md` → "Handoff Notes" — the as-built relay HTTP/TLS + Iroh-dial surface the webhook route extends; whether the WSS TLS listener can host a second path.

## Scope — in
**Relay (`crates/relay`):**
- A **second HTTP route** `POST /webhook/github/<endpoint_id>` behind a new FROZEN env var (`WEBHOOK_LISTEN_ADDR`, `Option<SocketAddr>`, additive to `RelayConfig`; or the shared-WSS-listener decision, frozen). Parse `<endpoint_id>` (reuse `parse_endpoint_id`/`MAX_ENDPOINT_ID_LEN`) and reject malformed with HTTP 400 **before** dialing. Read the GitHub headers (`X-GitHub-Delivery`, `X-Hub-Signature-256`, `X-GitHub-Event`) + the raw body (bounded to the §3.4.1 ceiling — reject oversize with 413).
- Open an ephemeral **`0x04` Webhook** Iroh bidi to the addressed Core (relay-forced dial, `RelayMode::Custom`, `TRANSPORT_ALPN`), write the channel tag byte + the FROZEN `WebhookEnvelope`, await the Core's ack frame, map it to the HTTP status returned to GitHub (200/4xx/5xx). The relay does **no** HMAC verify, **no** parse, **no** persistence — it forwards the body opaquely (`design/11 §3.9`).
- **Offline Core** (dial fail / no route): return 502/503 to GitHub + drop + **log** (metadata only); **no buffering** (`design/11 §3.2`, `design/13 §8`). A new relay metric is fine (`concerto_relay_webhooks_forwarded_total` / `_dropped_total`, additive to the frozen metric set).

**Transport (`crates/transport`):**
- `ChannelTag::Webhook = 0x04` on the enum + `tag_from_byte`'s `0x04` arm + the doc-comment table; the frozen-tag tests gain the `0x04` round-trip assertion.
- The serve-loop `ChannelTag::Webhook` arm: read the `WebhookEnvelope` off the **raw** duplex (no Noise handshake — `0x04` is the deliberate non-Noise channel per §3.4.1), invoke the Core-supplied webhook ingest seam, write the ack frame back on the duplex. Bound the read (the body ceiling) and the wait (a timeout) so a malformed/stalled frame can't pin the loop.
- A small `WebhookSink`-style seam injected at `serve_iroh` (mirror `ApiDispatcher`/`AuthObserver`) so the transport stays Core-agnostic and the Core wires its VCS ingest in.

**Core VCS (`crates/vcs` / `crates/core`):**
- `VcsHandle::ingest_webhook(repo: RepositoryId, payload: WebhookPayload) -> Result<()>` (FROZEN `design/13 §5.1`). `WebhookPayload` carries the envelope fields (delivery_id, signature_256, event_type, body). The Core path:
  1. **Idempotency first:** `insert_delivery_if_absent(delivery_id, repo_id, now)` → `false` ⇒ **replay, drop** (no error to caller; ack 200 so GitHub stops retrying a dupe).
  2. **HMAC verify:** recompute HMAC-SHA256 over the raw `body` with the per-repo `VcsSecretSlot::WebhookSecret` (loaded into `VcsState.webhook_secrets` from the keychain); **constant-time compare** against `signature_256`. Mismatch ⇒ **drop + log, do NOT surface to the sender** (`design/13 §8`); ack 401 (the relay maps to a 4xx GitHub sees, but no human-facing error path). Missing secret for repo ⇒ drop + log (webhook not configured) ⇒ ack 4xx.
  3. **Parse** the event by `event_type` (`pull_request`/`check_run`/`deployment`/`pull_request_review_thread`) into the minimal shape the caches need. Unknown event types ⇒ ack 200 + no-op (forward-compat).
  4. **Targeted cache invalidation** (`design/13 §6.3`): invalidate just the affected PR / check / thread cache rows so the next read (or 316's event emission) refresh from origin. Emit `vcs.webhook_received` (broadcast, low-rate, informational — `design/13 §5.3`). The per-`checks.<wa>.<repo>` event emission is **Task 316**; 315 invalidates + leaves a hook 316 consumes.
  5. **Ack 200** chained back through the transport → relay → GitHub.
- **Migration 0013** `webhook_deliveries(delivery_id TEXT PRIMARY KEY, repo_id TEXT NOT NULL, received_at INTEGER NOT NULL)` + a TTL-cleanup (1h, `design/13 §6.2`) that survives restart (so a redelivery after a Core restart is still deduped within the window). A persist module `webhook_deliveries.rs` with `insert_delivery_if_absent` + `prune_expired`.
- Load the per-repo webhook secret from the keychain into `VcsState.webhook_secrets` (lazy on first webhook for a repo, or at boot for configured repos). **Polling fallback** (`design/13 §3.2`): the existing/poll path remains the default; webhooks are an accelerator, so a webhook-path failure must never break the poll path.
- Tests (Tier 2): HMAC verify with known good/bad fixtures (`design/13 §10` "Known good/bad fixtures"); a replay (same delivery_id) → dropped, no double-update; "webhook arrives during a poll → no double-update" against a fixture relay + the `testkit` `FakeGitHub`; an oversize body → rejected; an unknown `event_type` → no-op 200; a missing-secret repo → drop; the relay route parses + forwards the envelope to a loopback Core and chains the ack back (the loopback relay+Core double).

## Scope — out
- **Real GitHub→relay delivery** — Tier-3 (Phase-3 manual checklist: "run a coordinated PR-set merge against a real GitHub repo with a live webhook"). This task proves the path with a **loopback relay+Core** + `testkit` fixtures.
- The **`checks.<wa>.<repo>` event emission + review-thread/check-run/deploy aggregation** — **Task 316**. 315 invalidates caches + leaves the invalidation hook; 316 fetches + emits.
- **`wait_for_check_runs` webhook subscription** — **Task 318** (it subscribes to the events 316 emits; 315 just makes them possible). 318 also degrades to pure polling when no webhook is wired.
- **Defining `VcsSecretSlot::WebhookSecret`** + the `crates/vcs` crate + the `VcsProvider` trait + the `testkit` — **Task 313** (`PHASE3_PLANNING §4.1`/§4.3). 315 consumes them.
- The **`WebhookEnvelope` framing decisions** — **Task 315.0** froze them in `design/11 §3.4.1`. 315 transcribes, never re-chooses.
- **Webhook registration on GitHub** (creating the webhook via the API with the secret) — out for V1.0 here; the secret is stored (313's keychain slot) and the user/operator registers the URL. (If `design/13` implies auto-registration, that is a separate concern; keep 315 to receipt.)
- **Linear/Jira webhooks** — V1.0 is GitHub-only here; the route is `/webhook/github/`. Other providers are future.

## Public interface this task locks
- **`ChannelTag::Webhook = 0x04`** on the transport demux (`crates/transport/src/api.rs` + `channels.rs`) — FROZEN wire byte, joining `0x01/0x02/0x03`. The `0x04` arm reads the `WebhookEnvelope` off the **raw** (non-Noise) duplex.
- **Relay route `POST /webhook/github/<endpoint_id>`** + the new `WEBHOOK_LISTEN_ADDR` env var (or the frozen shared-WSS-listener decision) — additive to the FROZEN `RelayConfig`.
- **`WebhookPayload`** (the Core-side struct `ingest_webhook` takes) + the on-wire `WebhookEnvelope` framing exactly per `design/11 §3.4.1` (delivery_id, signature_256, event_type, endpoint_id, body + the pinned encoding + ack frame). FROZEN.
- **Migration 0013 `webhook_deliveries`** schema (`delivery_id` PK, `repo_id`, `received_at`) — FROZEN columns. (Shift the number per the §3 author-check if 0009–0012 are not yet on `main`; record in Handoff.)
- **`VcsHandle::ingest_webhook(repo, WebhookPayload) -> Result<()>`** (the FROZEN `design/13 §5.1` method) — idempotency-first, then constant-time HMAC, then parse + targeted-invalidate.
- **The HMAC contract:** HMAC-SHA256 over the **raw** body bytes, keyed by the per-repo `VcsSecretSlot::WebhookSecret`, constant-time compare against `X-Hub-Signature-256`'s `sha256=<hex>`. Mismatch/missing ⇒ drop + log, **no sender-visible error**.

## Implementation notes
- **Idempotency before HMAC, HMAC before parse.** Order matters: dedupe on delivery-id first (cheapest, drops replays without touching the secret), then constant-time HMAC (anti-spoof), then parse. A failed HMAC must **never** reach the parser. Use the `hmac` + `sha2` crates' constant-time `verify_slice` (or `subtle`) — never `==` on the digest.
- **The `0x04` channel is deliberately non-Noise.** Do not call `handshake_responder` on the webhook duplex (that is the `0x01/0x02` Noise path). `0x04` reads the envelope off the raw `IrohDuplex`. The authenticity floor is the HMAC, not Noise (the carve-out 315.0 wrote into `design/11 §3.9`). The Iroh/QUIC layer still encrypts the relay→Core hop; the inner Noise is intentionally absent because the peer is GitHub-via-relay, not a paired device. Re-read `design/11 §3.4.1` before wiring the arm.
- **Reuse the relay's WSS machinery; don't fork it.** The webhook route's Iroh-client-endpoint + relay-forced dial + path-parse are the same primitives `wss.rs` already built. Factor the shared bits (the `RelayMode::Custom` endpoint, `parse_endpoint_id`, the dial) so the webhook route and the WSS bridge don't drift. Decide whether one HTTPS listener serves both `/wss/` and `/webhook/github/` (cleaner, one TLS cert) and freeze it.
- **Restart-surviving idempotency.** The design says "1h TTL cache"; the `current-code-state` reader noted in-memory would suffice, but `PHASE3_PLANNING §3`/D9 reserves **migration 0013 `webhook_deliveries`** explicitly so the dedup **survives restart** (a GitHub redelivery seconds after a Core bounce is still deduped). Implement the table; the 1h TTL is the prune window, not an in-memory-only cache.
- **Polling never breaks.** A webhook-path failure (relay down, secret missing, parse error) must leave the poll path intact (`design/13 §3.2` "falls back to polling"). The webhook path is strictly additive acceleration. Do not make any existing read depend on a webhook arriving.
- **Tier-2 double = loopback relay + Core + `testkit` `FakeGitHub`.** Stand up the relay route + a Core transport endpoint on one host (mirror `crates/transport/tests/loopback.rs` / `wss.rs`'s loopback double), POST a fixture webhook (a recorded `check_run` body + a correct `X-Hub-Signature-256` computed with a test secret) at the route, assert the Core dedupes/verifies/parses and acks 200; flip one signature byte → assert drop + 4xx + no cache mutation. **What it does NOT cover:** real GitHub computing the real signature + real redelivery + real network to a real relay — that is the Tier-3 phase-checklist line.
- **Cross-platform:** the relay route is plain TCP/HTTPS + an Iroh bidi; the Core path is `hmac`/`sha2`/`sqlx` — nothing `#[cfg(unix)]`. Builds on the Windows CI lane (Task 113), same as the WSS bridge.
- **Reuse-not-reinvent:** `repository_id → owner/repo` resolution already exists in the VCS layer (the `repositories` row); the HMAC secret accessor is 313's `VcsSecretSlot::WebhookSecret`; the broadcast emit is the existing events machinery. Add only the webhook-specific glue.
- **Parallel build hint:** three independent Outputs a lead sub-agent can fan out + integrate into one commit — (a) the relay route (`crates/relay`), (b) the `0x04` `ChannelTag` + serve-loop arm + `WebhookSink` seam (`crates/transport`), (c) the Core `ingest_webhook` + HMAC + migration 0013 + `webhook_deliveries` persist (`crates/core`/`crates/vcs`/`crates/persist`). They meet at the FROZEN `WebhookEnvelope` (315.0) + the `WebhookSink` seam.

## Verification
**Tier 2.** The `rust` §5.3 set + the loopback relay+Core double.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-vcs -p concerto-core -p concerto-transport -p concerto-relay webhook` → HMAC good/bad fixtures, replay-drop (no double-update), poll-vs-webhook no-double-update (fixture relay + `testkit` `FakeGitHub`), oversize-body reject, unknown-event no-op, missing-secret drop, the `0x04` tag round-trip, and the loopback relay→Core envelope-forward + ack-chain-back all pass.
4. `cargo test --workspace --no-fail-fast` → all pass (the new migration applies cleanly; existing tests unaffected).
5. `cargo deny check` → green (no new workspace pins beyond 313's `octocrab`/`graphql_client`/`wiremock`; `hmac`/`sha2` are already in-tree or MIT/Apache-clean — verify and pin once if introduced).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen if `ingest_webhook` / `ChannelTag::Webhook` / the new relay config field change the generated Rust-API surface; no `.proto` change is expected (the webhook path is internal, not a new gRPC RPC).
7. `scripts/smoke.sh` → **unchanged** gate (this task adds no smoke capability; the webhook path needs a real GitHub delivery to smoke end-to-end, which is Tier-3). Confirm the existing smoke stays green.

**Tier-2 double + what it does NOT cover.** The double is a **loopback relay route + a loopback Core transport endpoint on one host** (relays disabled / direct, mirroring `wss.rs`'s loopback) + the `concerto-vcs/testkit` `FakeGitHub` for the parsed-event side. It proves: the relay route parses + opens the `0x04` bidi + writes the envelope + chains the ack; the transport demuxes `0x04` to the webhook handler (no Noise); the Core dedupes (delivery-id), constant-time-verifies HMAC, parses, targeted-invalidates, and acks. It does **NOT** cover: real GitHub computing a real `X-Hub-Signature-256`, real webhook delivery over a real relay on real infra, or GitHub's real redelivery policy — those are the **Phase-3 Tier-3 checklist** lines ("coordinated PR-set merge against a real GitHub repo with a live webhook"; "confirm review threads sync").

## Definition of Done
- [ ] Relay route `POST /webhook/github/<endpoint_id>` added (new `WEBHOOK_LISTEN_ADDR` or frozen shared-listener decision), parses `<endpoint_id>` before work, forwards the `WebhookEnvelope` over an ephemeral `0x04` bidi, chains the ack to the HTTP status, drops+logs+5xx on offline Core (no buffering)
- [ ] `ChannelTag::Webhook = 0x04` on the transport demux + `tag_from_byte` arm + doc-table + frozen-tag test; serve-loop `0x04` arm reads the envelope off the **raw** duplex (no Noise) via the new `WebhookSink` seam and writes the ack
- [ ] `VcsHandle::ingest_webhook(repo, WebhookPayload)` implemented: idempotency-first (delivery-id) → constant-time HMAC-SHA256 vs `VcsSecretSlot::WebhookSecret` → parse → targeted cache-invalidate → ack; HMAC mismatch/missing ⇒ drop+log, no sender-visible error
- [ ] Migration **0013 `webhook_deliveries`** (delivery_id PK / repo_id / received_at) + `insert_delivery_if_absent` + 1h TTL prune; restart-surviving dedup (number shifted per §3 author-check if needed, noted in Handoff)
- [ ] `concerto-vcs/testkit` enabled as a dev-dep; Tier-2 tests (HMAC good/bad, replay-drop, poll-vs-webhook, oversize, unknown-event, missing-secret, loopback relay→Core forward+ack) pass
- [ ] Polling path unaffected; webhook path is strictly additive; builds on the Windows CI lane
- [ ] All §5.3 `rust` commands pass; interfaces regenerated if the Rust-API surface changed; smoke unchanged + green
- [ ] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams for 316/318 documented in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/relay/src/webhook.rs` (new — the `/webhook/github/<endpoint_id>` route + envelope-forward + ack-chain)
- `crates/relay/src/api.rs` / `crates/relay/src/config.rs` / `crates/relay/src/state.rs` / `crates/relay/src/main.rs` (modified — `WEBHOOK_LISTEN_ADDR` config + route registration + optional metric)
- `crates/transport/src/api.rs` / `crates/transport/src/channels.rs` / `crates/transport/src/endpoint.rs` / `crates/transport/src/lib.rs` (modified — `ChannelTag::Webhook = 0x04` + serve-loop arm + the `WebhookSink` seam)
- `crates/core/src/vcs/` (or `crates/vcs/`, per 313's crate move) — `ingest_webhook` + HMAC verify + cache invalidation + the `VcsState.webhook_secrets` load (modified/new)
- `crates/persist/migrations/0013_webhook_deliveries.sql` (new — number per §3 author-check)
- `crates/persist/src/webhook_deliveries.rs` (new — `insert_delivery_if_absent` + `prune_expired`) + `crates/persist/src/lib.rs` (modified — module export)
- `crates/core/src/boot.rs` (modified — wire the Core's webhook ingest seam into `serve_iroh`'s `WebhookSink`)
- `crates/relay/tests/webhook_forward.rs` + `crates/core/tests/webhook_ingest.rs` (new — the Tier-2 loopback + HMAC/replay tests, using `concerto-vcs/testkit` fixtures)
- `Cargo.toml` (root + the touched crates — `hmac`/`sha2` pin if newly introduced; `concerto-vcs` dev-dep with `testkit`)
- `docs/interfaces/rust-api.md` (regenerated if the Rust-API surface changed)

## Commit message
```
phase-3: webhook receiver — relay route + 0x04 channel + HMAC + idempotency

GitHub posts to a new relay route POST /webhook/github/<endpoint_id>;
the relay opens an ephemeral 0x04 Webhook Iroh bidi (no Noise) and pumps
the FROZEN WebhookEnvelope (design/11 §3.4.1) to the Core, which dedupes
on delivery-id (migration 0013 webhook_deliveries, restart-surviving),
constant-time-verifies HMAC-SHA256 against VcsSecretSlot::WebhookSecret,
parses, and targeted-invalidates caches. Mismatch/replay drop silently;
offline Core → relay 5xx + drop, no buffering. Real GitHub delivery is
the Phase-3 Tier-3 line; proven here against a loopback relay+Core.

Refs: tasks/v1.0/315-webhook-receiver.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan — —
- Open questions for next task — —
- Deliberate debt — —
- Smoke-gate state — —
