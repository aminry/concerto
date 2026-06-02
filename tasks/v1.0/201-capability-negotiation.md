# Task 201 — Per-Connection Transport-Kind Negotiation

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | — |
| Touches subsystem(s) | 10 (Client API Protocol), 11 (Transport — seam consumer) |
| Smoke gate | unchanged |

## Goal
Make `ServerCapabilities.transport_kind` reflect the **actual transport a connection arrived on**, instead of the V0.1 hardcode. Today `RuntimeHandler::get_server_capabilities` always returns `TransportKind::Uds` (`crates/core/src/handlers/runtime.rs` — see the `transport_kind: TransportKind::Uds as i32` line with its "later tasks will branch on a stored mode" comment). This task builds the **per-connection tagging seam**: each transport listener injects its `TransportKind` into the request's extensions, and the handler reports the tagged value. UDS stays the only *live* value (Iroh arrives in 212, WSS bridge in 204), but after this task those tasks set their kind by tagging their listener — never by editing the handler. This is the "connect-time capability negotiation" the rest of Phase 2's clients (and `design/15 §3.11`'s remote-mode affordance suppression) depend on.

## Inputs to read before starting
- `design/10_Local_API_Protocol.md` §3.4 (two equally-supported auth paths into the **same** handlers; `GetServerCapabilities` returns the negotiated transport kind so clients suppress affordances), §4.2 (`ServerCapabilities` + `TransportKind`), §7.1 (capability-negotiation sequence).
- `design/15_Desktop_Client.md` §3.11 (the Desktop consumer — it reads `transport_kind` to hide "Reveal in Finder" etc. in remote mode; you are building the field it keys off).
- `crates/proto/proto/concerto/v1/runtime.proto` — the `TransportKind` enum + `ServerCapabilities` fields **already exist** (V0.1 scaffolded them ahead); this task does **not** change the proto.
- `crates/core/src/handlers/runtime.rs` — the current hardcode + the existing `capabilities_advertise_uds_transport` test you must keep green.
- `crates/core/src/api_server.rs` — where the Tonic server + the UDS listener are built; this is where the per-connection extension gets injected.
- `tasks/v1.0/200-adapter-reconciliation.md` → "Handoff Notes" — the hand-rolled-adapter decision; task 212 will consume this seam to tag `IROH`.

## Scope — in
- A request-extension carrier for the inbound transport, e.g. `pub struct ConnTransport(pub TransportKind)` (name + place it where the api server builds listeners; keep it in `crates/core`).
- Wire the **UDS listener** to insert `ConnTransport(TransportKind::Uds)` into every request's extensions (via Tonic's `Connected` connection-info → a small `tower`/interceptor layer, or the `serve_with_incoming` connect-info path — whichever matches how `api_server.rs` already constructs the server).
- `RuntimeHandler::get_server_capabilities` reads `request.extensions().get::<ConnTransport>()`; reports that kind, **defaulting to `Uds`** when absent (back-compat / direct-construction in tests).
- Document the **contract** (module doc on the carrier): *every* transport listener tags its connections — UDS now (this task), Iroh in 212, WSS bridge in 204. The handler never branches on transport; it only reads the tag.
- Tests: (a) the existing UDS test stays green; (b) a request carrying an injected `ConnTransport(Iroh)` extension makes the handler report `IROH` — proves the seam end-to-end without a live Iroh listener.

## Scope — out
- The Iroh listener / WSS bridge themselves (212 / 204) — they only need to *exist as tag sites*; not built here.
- Any capability **gating** of services (`optional_services` stays empty until a task actually disables a service, e.g. Maestro in P4).
- `optional_streams` / `default_stream_buffer` proto fields — **not added speculatively**; their owning tasks (202 for stream buffers) add them additively when needed.
- On Windows the co-located transport is a named pipe; it maps to `TRANSPORT_KIND_UDS` ("co-located, peer-attested") semantically — no new enum variant. Note this in the carrier doc.

## Public interface this task locks
- Rust: the `ConnTransport` request-extension type (name + that it carries a `TransportKind`) — the seam every listener writes and the handler reads. FROZEN.
- `ServerCapabilities.transport_kind` semantics: now reflects the **live** connection — `UDS` = co-located peer-UID/named-pipe; `IROH` = device-cert split-host/mobile; `WSS_BRIDGE` = browser. (Proto field numbers unchanged — already frozen in `runtime.proto`.)

## Implementation notes
- Tonic surfaces per-connection info through request extensions when the server is built with a connect-info source. `tokio::net::UnixStream` implements `tonic::transport::server::Connected`; the cleanest seam is a thin layer that, knowing which listener it wraps, inserts the right `ConnTransport`. Since UDS is the only listener today, inject it there and document the seam so 212/204 do the same in their listener setup — do **not** try to infer transport from socket internals in the handler.
- Keep `core_host_os` / `core_hostname` exactly as they are (already real: `std::env::consts::OS`, `hostname::get()`).
- The carrier is internal to `crates/core` (not a published-crate `api.rs` surface), so `regen-interfaces.sh` will likely produce **no diff** — confirm with step 6 and note it in Handoff (cf. Task 112's regen note).
- Cross-platform: no `std::os::unix`-only types in the carrier or handler signatures so the Windows CI lane (Task 113) stays green; gate any UDS-specific listener glue under `#[cfg(unix)]` as the existing api server already does.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core runtime` → the kept UDS test + the new injected-`IROH` seam test pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `scripts/smoke.sh` → unchanged; `GetServerCapabilities` over the live UDS Core still reports `transport_kind = UDS`. Exits 0.
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → clean (or commit the regen if the carrier is surfaced in an `api.rs`).

## Definition of Done
- [ ] `ConnTransport` request-extension carrier defined + documented with the all-listeners-tag contract
- [ ] UDS listener tags every request; handler reports the tagged kind (default `Uds`)
- [ ] Existing UDS capability test green; new injected-`IROH` seam test passes
- [ ] Proto unchanged; `core_host_os`/`core_hostname` behavior preserved
- [ ] Verification commands pass; smoke green; interfaces clean (or regenerated)
- [ ] Single commit with the message below

## Outputs
- `crates/core/src/handlers/runtime.rs` (modified — read the tag)
- `crates/core/src/api_server.rs` (modified — inject the tag on the UDS listener) + the carrier type (here or a small new module)
- `crates/core/tests/capability_negotiation.rs` (new — the injected-kind seam test) *(or extend the handler's `#[cfg(test)]` module)*

## Commit message
```
phase-2: per-connection transport-kind negotiation seam

Replaces the hardcoded ServerCapabilities.transport_kind with a
ConnTransport request-extension that each listener tags. UDS stays the
only live kind; 212 (Iroh) and 204 (WSS) tag their listeners without
touching the handler.

Refs: tasks/v1.0/201-capability-negotiation.md
```

## Handoff Notes (fill in when finishing)
- Drift from plan / Open questions for next task / Deliberate debt / Smoke-gate state
