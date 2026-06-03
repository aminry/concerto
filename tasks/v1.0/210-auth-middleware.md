# Task 210 — Auth Middleware: Device-Cert Path (Iroh) + Peer-UID Fast Path (UDS) → Identical Handlers

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 206, 209 |
| Touches subsystem(s) | 10 (Client API Protocol), 12 (Security & Identity) |
| Smoke gate | unchanged |

## Goal
Add the **authentication middleware** the whole remote surface depends on, so that **both** auth paths land in the **identical** Tonic handlers with an identically-shaped request-scoped identity. Today `crates/core/src/api_server.rs` builds the UDS server with **no auth gating** — it trusts whoever can open the socket (V0.1's "trusted box" model). This task adds two things without ever making a handler branch on transport: (a) on the **UDS listener**, a peer-UID check (`SO_PEERCRED` on Linux / `LOCAL_PEERPID`→`SO_PEERCRED`-equivalent on macOS) confirming the connecting UID matches the Core's owning UID → implicit admin, populating a `DeviceContext` from an implicit **"local-uds" pseudo-cert** (no real cert); and (b) a cert-validation tower-layer/interceptor that reads the `concerto-device-cert` metadata header, calls Task 206's `LocalCoreIssuer::validate` (signature + expiry + revoked-set — the set Task 209 populates on revoke), and on success injects the resulting `DeviceContext` as a request extension. Which path runs is chosen off the **Task-201 `ConnTransport`** tag, not by sniffing sockets. Until Task 212's Iroh listener exists, the cert path is wired + unit-tested with **injected** certs/metadata; the live Iroh exercise arrives with 212/220. After this task every handler can read a uniform `DeviceContext` and the Core rejects invalid/expired/revoked certs (`UNAUTHENTICATED` / `PERMISSION_DENIED`) and non-owner UDS peers.

## Inputs to read before starting
- `design/10_Local_API_Protocol.md` §3.4 — the **two-auth-paths table** (reproduce faithfully): every gRPC call carries a `concerto-device-cert` metadata header validated against the Core pubkey + `devices` table; UDS adds an `SO_PEERCRED` (Linux) / `LOCAL_PEERPID` (macOS) check that the connecting UID matches the Core's owning UID → **implicit admin, no cert**; both paths land in the **same** handlers; the request-scoped `DeviceContext { device_id, capabilities }` is populated **identically**, with an implicit **"local-uds" pseudo-cert** for the UDS path so handlers don't branch on transport.
- `design/10_Local_API_Protocol.md` §6 (the `AuthMiddleware → AuthzScope → handlers` pipeline + handler thinness — middleware does auth, handlers stay logic-free), §6.3 (UDS vs Iroh = same Tonic server; "the auth middleware sees the difference: UDS connections have peer-uid; Iroh connections have device-cert metadata"), §7.1 (capability-negotiation: cert verified → `DeviceContext` set before the first RPC), §7.3 (mobile RPC: `validate device-cert` → `DeviceContext` → dispatch).
- `design/10_Local_API_Protocol.md` §8 — the **error mapping** you implement: invalid/expired cert → `UNAUTHENTICATED` + `ConcertoError{code="auth.invalid_cert"}`; revoked device → `PERMISSION_DENIED` + `ConcertoError{code="auth.revoked"}` + emit revocation event. (Missing-cert on the Iroh path = `UNAUTHENTICATED` `auth.invalid_cert` too — no cert is an invalid cert.)
- `design/12_Security_Identity.md` §5.1 (`validate_cert(raw) -> Result<DeviceContext>` is the method the auth middleware calls), §6.1 (the **< 200 µs** in-memory hot path — middleware must not add a DB hit), §3.10 (validate via the `DeviceCertIssuer` from 206 — the middleware depends on the trait, not a concrete validator), §3.2 (the 4 validation steps — already inside 206's `validate`; the middleware just maps its `Err` variants to Status), §8 (the same failure-mode table from the security side: signature invalid / expired / wrong-core / revoked).
- `tasks/v1.0/206-device-cert-issuer.md` (+ **Handoff Notes**) — the **FROZEN `DeviceContext { device_id: [u8;32], device_name, capabilities: Vec<String> }`** shape (the middleware's output extension), the `DeviceCertIssuer::validate(raw) -> Result<DeviceContext>` signature + its error variants (`Expired`/`Revoked`/`WrongCore`/`Malformed`), and where the `LocalCoreIssuer` is constructed at boot so the middleware can hold an `Arc<dyn DeviceCertIssuer>`.
- `tasks/v1.0/209-devices-service.md` (+ **Handoff Notes**) — confirms the revoked set the validator reads is populated on revoke, so the `auth.revoked` test exercises the same path; and that revocation closes streams (the middleware is the *reconnect* rejection, not the active close).
- `tasks/v1.0/201-capability-negotiation.md` (+ **Handoff Notes**) — the **`ConnTransport(pub TransportKind)`** request-extension seam every listener tags; this task **chooses the auth path off that tag** (`Uds` → peer-uid, `Iroh`/`WssBridge` → cert), keeping the handler transport-blind. Read how `api_server.rs` injects the tag on the UDS listener — you add the peer-uid check at that same injection site.
- `crates/core/src/api_server.rs` — the UDS server build (`UnixListener` + `serve_with_incoming_shutdown`), the `#[cfg(unix)]` gating, and the (currently absent) auth layer; this is where both the peer-uid check and the cert tower-layer attach.
- `crates/core/src/handlers/sessions.rs` + `crate::error_map` — the `#[async_trait]` handler pattern + the existing Status/`ConcertoError` mapping helpers to extend with `auth.invalid_cert` / `auth.revoked`.
- `tasks/v1.0/README.md` §5.3 (`rust` command set) + §6 row 210.

## Scope — in
- **`DeviceContext` as a request extension** consumed by handlers: the middleware inserts the Task-206 `DeviceContext` into `request.extensions_mut()`; a small accessor (e.g. `fn device_context(req: &Request<_>) -> &DeviceContext`) handlers call. (Handlers don't *have* to read it yet — the seam is what this task locks; per-RPC authz consumption thickens later.) Define an `AuthzScope`/capability check helper stub that reads `DeviceContext.capabilities` (V1.0 always `["admin"]` → always allow) so §6's `AuthzScope` box exists as a seam.
- **UDS peer-UID fast path** (`#[cfg(unix)]`): at the UDS listener (where Task 201 tags `ConnTransport(Uds)`), read the connecting peer's UID from the `UnixStream` (`SO_PEERCRED` on Linux; the macose equivalent — `getpeereid` / `LOCAL_PEERCRED` — yields uid; `LOCAL_PEERPID` is the pid variant named in the design, but the **UID** is what we compare). Compare against the Core process's own UID (`nix`/`libc` `geteuid`, or a crate already in the tree). **Match** → build the implicit **"local-uds" pseudo-cert** `DeviceContext { device_id = <fixed local-uds sentinel>, device_name = "local-uds", capabilities = ["admin"] }` and inject it. **Mismatch** → reject the connection (`UNAUTHENTICATED`; this is the "a local non-Concerto process tries to connect" threat-row in `12 §6.4`). Document the sentinel `device_id` for the local-uds pseudo-cert and FREEZE it.
- **Cert-validation layer** (a Tonic `tower` layer / interceptor, transport-agnostic): for connections tagged non-UDS (`Iroh`/`WssBridge`), read the `concerto-device-cert` metadata header, base64/raw-decode it (match the wire encoding pairing uses), call `issuer.validate(raw)`, and on `Ok` inject the `DeviceContext`. Map `Err` → Status: `Malformed`/`Expired`/`WrongCore`/missing-header → `UNAUTHENTICATED` + `auth.invalid_cert`; `Revoked` → `PERMISSION_DENIED` + `auth.revoked` (+ emit the revocation/`security.violation` event per §8). The layer holds an `Arc<dyn DeviceCertIssuer>` from the 206 boot wiring.
- **The `concerto-device-cert` metadata key constant** — define once (e.g. `pub const DEVICE_CERT_METADATA_KEY: &str = "concerto-device-cert";`) and FREEZE it; clients (218/511/520) key off this exact string.
- **Wire both into `api_server.rs`** so the live UDS Core enforces peer-uid today and the cert layer is mounted (exercised by injected metadata in tests until 212's Iroh listener lands). The cert layer attaches to the shared `Server::builder()` so it applies to every service uniformly.
- **Tier-1 tests**: cert path with an **injected** `Arc<dyn DeviceCertIssuer>` stub — valid cert → `DeviceContext` populated + RPC dispatches; expired → `UNAUTHENTICATED`/`auth.invalid_cert`; revoked → `PERMISSION_DENIED`/`auth.revoked`; missing/garbage header → `UNAUTHENTICATED`/`auth.invalid_cert`. UDS path: peer-uid **match** → implicit-admin `DeviceContext`; peer-uid **mismatch** → connection rejected. The `ConnTransport` tag drives which path runs (inject `ConnTransport(Iroh)` to exercise the cert layer without a live Iroh listener — the Task-201 seam pattern).

## Scope — out
- The **live Iroh listener** that tags `ConnTransport(Iroh)` + carries real cert metadata over the wire — **Task 212**; the end-to-end split-host auth exercise — **Task 220**. This task wires + unit-tests the cert path with injected certs/metadata; the real Iroh exercise is the **Phase-2 Tier-3 checklist**.
- The cert **issuance**/**validation crypto** itself — **Tasks 205/206** (this task *calls* `validate`, never reimplements it).
- Revocation **persistence** + the **active** mid-stream stream-close — **Task 209** (this task is the *reconnect-time* rejection that reads the revoked set 209 populates).
- The WSS-bridge's cert re-presentation from a browser's stored pairing — **Tasks 215/521** (the layer is transport-agnostic and will serve `WssBridge`-tagged connections; not built here).
- Per-RPC **authz scoping** beyond the V1.0 binary `["admin"]` allow-all (read/write/admin scopes are V2.0) — only the `AuthzScope` seam is stubbed here.
- `managed.json` enforcement of allowed/max devices — **Task 211** (consumes auth but is a separate policy layer).
- Windows **named-pipe peer identity** — see Implementation notes: a **gated TODO** in V1.0 (the Windows co-located transport maps to the `UDS` kind per Task 201; real named-pipe peer attestation lands with the Windows Service work, Task 701-adjacent).

## Public interface this task locks
- **The `concerto-device-cert` metadata key** string constant — FROZEN; every remote client presents the cert under this exact header.
- **The `DeviceContext` request-extension contract** — handlers/middleware read the Task-206 `DeviceContext { device_id, device_name, capabilities }` from `request.extensions()`; populated **identically** on both paths. FROZEN shape (owned by 206; this task locks that it rides as a request extension and the accessor signature).
- **The "local-uds" pseudo-cert** `DeviceContext` shape + its sentinel `device_id` — FROZEN, so anything that later inspects `device_id` can recognize the local path.
- **The auth error mapping**: invalid/expired/missing cert → `UNAUTHENTICATED`/`auth.invalid_cert`; revoked → `PERMISSION_DENIED`/`auth.revoked`. FROZEN (matches `design/10 §8`).

## Implementation notes
- **Choose the path off `ConnTransport`, never off socket internals.** Task 201 made `ConnTransport(TransportKind)` the per-connection tag every listener writes. The auth layer reads it: `Uds` → expect peer-uid already established at the listener (or do the check in the layer if the `UnixStream` peer cred is threaded through connect-info); `Iroh`/`WssBridge` → require + validate the cert metadata. A handler must still never see the transport — only the resulting `DeviceContext`.
- **Peer-uid mechanics.** On Linux, `tokio::net::UnixStream` exposes `peer_cred()` → `UCred { uid, .. }`. On macOS, `peer_cred()` is also available (backed by `LOCAL_PEERCRED`/`getpeereid`) and yields the uid — the design's `LOCAL_PEERPID` wording refers to the pid sibling; compare **uid** against `geteuid()`. Prefer `UnixStream::peer_cred()` over raw `libc` if it's available in the pinned tokio. Do the check at the UDS connect-info site so a non-owner connection is refused before any RPC dispatches.
- **The cert layer is one tower layer for all services.** Attach it to the shared `Server::builder()` (the `builder` in `run_uds`) so every `add_service` is covered uniformly — do not duplicate per-service. The layer's `poll_ready`/`call` injects the extension then forwards; on auth failure it short-circuits with the mapped Status without calling the inner service.
- **Hold the issuer as `Arc<dyn DeviceCertIssuer>`.** 206 constructs the `LocalCoreIssuer` at boot; thread that `Arc` into both the cert layer and (already) the revoke path. The middleware adds **no** DB hit — `validate` is the in-memory < 200 µs path.
- **`#[cfg(unix)]` + Windows gap.** Gate the peer-uid glue under `#[cfg(unix)]` exactly as `api_server.rs` already does. For Windows, leave a clearly-commented gated TODO: the named pipe maps to `TransportKind::Uds` (Task 201's note), but named-pipe peer attestation (`GetNamedPipeClientProcessId` + token UID) is not implemented in V1.0 — on Windows the co-located path currently has **no** peer check (documented limitation; the Windows Core lands later). State this gap loudly in Handoff.
- **Error/event wiring.** Extend `crate::error_map` (or add an auth-error helper) for the two new `ConcertoError` codes. The `auth.revoked` path should emit the `security.violation`/revocation event per §8 if the audit/event handle is in reach at the middleware; if not, note it in Handoff (don't force a fragile wiring).
- **Determinism.** Tests inject an `Arc<dyn DeviceCertIssuer>` stub returning canned `Ok(DeviceContext)`/`Err(Expired)`/`Err(Revoked)`/`Err(Malformed)`; no real keys needed (a real-key happy path can additionally compose 206 if cheap). The peer-uid mismatch test fakes a non-owner uid via the check's injection seam (or a same-process socketpair where the uids match for the positive case + a documented negative-path unit on the comparison function).

## Verification
Tier 1 — the test double is an **injected `Arc<dyn DeviceCertIssuer>` stub** (canned validate results) plus an **injected `ConnTransport` tag** to select the path without a live Iroh listener. It proves: cert valid → `DeviceContext` injected + dispatch; expired/garbage/missing → `UNAUTHENTICATED`/`auth.invalid_cert`; revoked → `PERMISSION_DENIED`/`auth.revoked`; UDS peer-uid match → implicit-admin context; UDS peer-uid mismatch → rejected. It does **NOT** cover: a **real** Iroh connection presenting a real cert over the wire (Task 212) or the full split-host auth round-trip (Task 220), nor Windows named-pipe peer attestation — those are the **Phase-2 Tier-3 checklist** lines.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core` (auth middleware) → valid/expired/revoked/missing-cert (stubbed issuer) + peer-uid match/mismatch tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new external deps expected beyond a peer-cred helper if `tokio`'s isn't enough; confirm + ratify if any).
6. `scripts/smoke.sh` → the live co-located UDS Core still serves every RPC under the new peer-uid gate (same-UID Desktop/CLI connect succeeds; the gate adds no regression). Exits 0.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → the middleware/`DeviceContext`-extension surface is internal to `crates/core` (depth-4 / no `core` `api.rs`) → expect **no** `docs/interfaces/` diff (confirm, cf. Task 112's regen note); commit if any surfaces.

## Definition of Done
- [x] UDS peer-uid check (`#[cfg(unix)]`) at the listener: owner-UID match → implicit-admin "local-uds" `DeviceContext`; mismatch → rejected
- [x] Cert-validation tower layer: reads `concerto-device-cert`, calls 206's `validate`, injects `DeviceContext`; maps errors to `auth.invalid_cert` / `auth.revoked`
- [x] Path chosen off the Task-201 `ConnTransport` tag; handlers never branch on transport; `AuthzScope` seam stubbed (binary `["admin"]`)
- [x] `concerto-device-cert` key constant + "local-uds" pseudo-cert sentinel defined + FROZEN; both layers wired into `api_server.rs`
- [x] Windows named-pipe peer-identity gap left as a documented gated TODO (Handoff)
- [x] Tier-1 valid/expired/revoked/missing + peer-uid match/mismatch tests pass; the Tier-3 uncovered part stated in Verification
- [x] Verification commands pass; smoke green; interfaces clean (or regenerated)
- [x] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate gated ones — e.g. Windows peer-id — in Handoff)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/security/auth.rs` (new — the cert tower-layer + peer-uid check + `DeviceContext` extension accessor + `AuthzScope` stub + metadata-key constant) + `crates/core/src/security/mod.rs` (modified — `pub mod auth;`)
- `crates/core/src/api_server.rs` (modified — attach the cert layer to the shared builder; add the peer-uid gate at the UDS connect-info site)
- `crates/core/src/error_map.rs` *(or the auth error helper home)* (modified — `auth.invalid_cert` / `auth.revoked` codes)
- `crates/core/tests/auth_middleware.rs` (new — Tier-1 cert + peer-uid tests with the injected issuer stub + `ConnTransport` tag)
- `docs/interfaces/*` (regenerated only if a surface appears) — **no diff** (the auth surface is internal to `crates/core`).
- **ADDED to Outputs (flagged in Handoff):** `crates/core/src/boot.rs` (modified — construct the auth `Arc<dyn DeviceCertIssuer>` + thread it into the api-server factory + run the Task-209 startup revoked-set mirror before the auth path goes live) and `crates/core/Cargo.toml` (modified — promote the already-in-tree `base64` to a direct dep for the metadata-header codec). `Cargo.lock` churn (one line) as expected.

## Commit message
```
phase-2: auth middleware — device-cert layer + UDS peer-uid into identical handlers

Adds the design/10 §3.4 two-path auth: a UDS peer-uid check (owner-UID =
implicit admin via a local-uds pseudo-cert) and a cert-validation tower
layer that reads the concerto-device-cert header, calls the Task 206
DeviceCertIssuer::validate, and injects a uniform DeviceContext as a
request extension. The path is chosen off the Task 201 ConnTransport tag
so handlers never branch on transport. Errors map to auth.invalid_cert
(UNAUTHENTICATED) / auth.revoked (PERMISSION_DENIED). Cert path is
unit-tested with an injected issuer until the Task 212 Iroh listener
lands; Windows named-pipe peer-id is a gated TODO.

Refs: tasks/v1.0/210-auth-middleware.md
```

## Handoff Notes (filled in when finishing)

**Frozen surface (for clients 218/511/520 and the wiring tasks 212/217):**
- **Metadata key constant** — `concerto_core::security::auth::DEVICE_CERT_METADATA_KEY = "concerto-device-cert"` (ASCII key). The value is **base64(STANDARD) of the on-wire signed cert `cert_bytes || signature`** — exactly the bytes `complete_pairing` returns (proto `signed_device_cert`, D1 opaque CBOR) and the device stores verbatim. Helper `encode_cert_metadata(&[u8]) -> String` produces the value; the layer base64-decodes then calls `issuer.validate(raw)`. Clients MUST use STANDARD base64 under this exact ASCII key.
- **local-uds pseudo-cert** — `LOCAL_UDS_DEVICE_ID = [0xED; 32]` (FROZEN sentinel, deliberately not a BLAKE2b fingerprint), `LOCAL_UDS_DEVICE_NAME = "local-uds"`, `capabilities = ["admin"]`; built by `local_uds_context()`.
- **DeviceContext accessor** — `concerto_core::security::auth::device_context(&Request<T>) -> Option<&DeviceContext>` (FROZEN signature). Populated **identically** on both paths. `AuthzScope::allows(&DeviceContext) -> bool` is the stubbed `design/10 §6` seam (V1.0: any context with the `"admin"` token → allow).
- **Error mapping (FROZEN, `design/10 §8`)** — invalid/expired/wrong-core/malformed/**missing** cert → `UNAUTHENTICATED` + `ConcertoError{code="auth.invalid_cert"}`; revoked → `PERMISSION_DENIED` + `ConcertoError{code="auth.revoked"}`. Built directly in `error_map::auth_invalid_cert_status` / `auth_revoked_status` (no new `concerto_error::Error` variants — that crate is not in Outputs); `error_map::concerto_code(&Status)` decodes the code from the details payload.

**Where it attaches (api_server.rs):** one tonic interceptor on the shared `Server::builder().layer(...)` covers every service uniformly. On the UDS listener it first inserts the Task-201 `ConnTransport(Uds)` tag, then calls `AuthInterceptor::authenticate`. `authenticate` chooses the path off the tag: `Uds` → peer-uid; `Iroh`/`WssBridge`/`Unspecified` → cert. Task 212's Iroh listener / Task 204's WSS bridge tag their own kind the same way and inherit the cert path with **no edit** to this interceptor or any handler.

**Issuer Arc threading from 206 boot:** `boot.rs` constructs an `Arc<dyn DeviceCertIssuer>` for the auth layer that shares the **SAME `revoked_set` handle** as the pairing/device-manager issuer (so a revoke is observed on the auth path with no DB hit). Because Task-207's `PairingCoordinator::new` takes `LocalCoreIssuer` **by value** and needs its `LocalCoreIssuer`-specific `core_public_key()` (not on the trait), and `KeyPair` is `ZeroizeOnDrop` (not `Clone`), boot builds a **second** issuer by calling `load_or_create_core_identity` again (the keychain reload returns the same key; `created==false` so no second `CoreIdentityCreated` audit). It is threaded `boot → ApiServerActor::with_managers(..., auth_issuer) → run_uds → AuthInterceptor::new`. `None` when no keychain identity exists → cert path refuses every remote (`auth.invalid_cert`); the UDS peer-uid path still works (needs no issuer).

**Peer-cred mechanism per-OS:** `#[cfg(unix)]` uses **tonic's `UdsConnectInfo` (tokio `UCred`) `peer_cred.uid()`** — tonic 0.12 inserts `UdsConnectInfo` into request extensions before the interceptor runs (verified in `tonic-0.12.3/src/transport/server/mod.rs`), so no raw `libc` socket calls are needed; the Core's own UID is `libc::geteuid()` (`libc` already a core dep). A request tagged `Uds` lacking `UdsConnectInfo` (an unattested in-process request) is **refused**, never granted implicit admin.

**Closed the Task-209 startup-mirror gap:** YES. `auth::mirror_revoked_devices(&Persistence, &RevokedSet)` runs `SELECT id FROM devices WHERE revoked_at IS NOT NULL` and inserts each decoded `device_id` into the shared set; `boot.rs` calls it **before the gRPC server (auth path) goes live**, unconditionally (even with no keychain identity — it only touches the DB + set, and only ever *adds* trust-removal, never fail-open). The test `revoked_device_stays_revoked_across_a_restart` proves a revoked device, after a fresh empty set rebuilt only from the DB, is rejected `Revoked`/`auth.revoked` (and asserts it would be *accepted* without the mirror — pinning the bug). I put the mirror in `auth.rs` (an Output) rather than editing the Task-209 `devices.rs` (not an Output).

**Windows named-pipe peer-id (gated gap, loud):** On Windows the co-located named pipe maps to `TransportKind::Uds` (Task 201) but peer attestation (`GetNamedPipeClientProcessId` + token UID) is **NOT** implemented in V1.0. The peer-uid glue is `#[cfg(unix)]`; the `#[cfg(not(unix))]` `authenticate_uds` grants the local-uds context **unconditionally** (no peer check) — but the whole UDS gRPC server is itself unsupported on non-Unix in V1.0 (`api_server` errors on non-unix), so this is unreachable today. Real named-pipe attestation lands with the Windows Core (Task 701-adjacent). Recorded under Deliberate debt below.

**Auth event-emission wiring state:** the `auth.revoked` status is returned correctly, but the `design/10 §8` "emit revocation/`security.violation` event" on the revoked-reconnect path is **NOT** wired here — the interceptor does not hold an audit/event handle (threading one into the tonic interceptor closure would be fragile, and the per-listener auth layer is the wrong place to own an audit writer). Revocation itself is already audited by Task 209's `revoke_device` (`AuditKind::DeviceRevoked`). The reconnect-rejection event is best emitted where an audit handle naturally lives; flagged as an Open question for 211/212.

**Drift from plan:** Added `crates/core/src/boot.rs` + `crates/core/Cargo.toml` to Outputs (flagged above): the cert layer needs a real issuer at runtime, which only boot can supply, and the task explicitly scopes the startup mirror "at boot (or when the auth layer / DeviceManager is constructed)". No other prior-task files touched (`devices.rs`/`pairing.rs`/`identity.rs` unchanged). `base64` promoted from transitive to direct dep — **no new external crate** enters the graph (already vendored via tonic/tonic-web); `cargo deny` unchanged.

**Open questions for next task:**
- **211** enforces `managed.json` (disable_remote / allowed / max devices) on THIS path: it should layer on top of `AuthInterceptor` (e.g. consult policy after `validate` succeeds, before injecting the context) — the `AuthzScope` seam is the natural home, or a sibling policy check in the same interceptor. The `device_context` accessor + the metadata-key constant are the stable hooks.
- **212/217** wire the real transport + active session closing: 212's Iroh listener tags `ConnTransport(Iroh)` and presents the cert under `DEVICE_CERT_METADATA_KEY` (base64) — the cert path is already mounted and unit-tested, so 212 only adds the listener. 217 replaces `NoopSessionCloser`; the auth layer is the *reconnect* rejection (revoked-set read), not the active close.
- The **revoked-reconnect `security.violation` event** (see above) wants an audit handle reachable from the auth layer — decide its home in 211/212.

**Deliberate debt:** Windows named-pipe peer attestation is a gated `#[cfg(not(unix))]` no-peer-check path (unreachable in V1.0 since non-unix UDS serving is itself unsupported) — closes with the Windows Core, **Task 701-adjacent**. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` markers in new code.

**Smoke-gate state:** **unchanged.** No new smoke check added. `scripts/smoke.sh` passes (exit 0) — the same-UID smoke client connects through the new peer-uid gate with no regression.
