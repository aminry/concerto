# Task 218 — Desktop Dual Transport: `CoreClient` trait + UDS/Iroh impls + connected-Core registry

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | web-ts |
| Verification tier | 2 |
| Size | medium (1–3d) — **spans Rust (`src-tauri`) + TS (`src/api`, `src/state`)** |
| Depends on | 217 |
| Touches subsystem(s) | 15 (Desktop Client), 11 (Transport — client consumer) |
| Smoke gate | unchanged |

## Goal
Turn the Desktop's single, hard-wired UDS gRPC client into the **transport-agnostic `CoreClient`** abstraction `design/15 §3.2` specifies, behind a **connected-Core registry** so the Desktop can hold many paired Cores and switch the active one. Today the Tauri shell dials exactly one UDS socket (`apps/desktop/src-tauri/src/core_client.rs` — a process-wide `tonic::Channel` over `~/.concerto/core.sock`, no notion of "which Core" or "how"). This task introduces (in `src-tauri`) a `CoreClient` trait with two impls — `UdsCoreClient` (peer-UID, co-located) and `IrohCoreClient` (device-cert split-host, over Task 217's `TransportHandle` client side) — plus a `cores.json` + OS-keychain registry of `PairedCore` rows and the "active Core" pointer. It surfaces a minimal TS binding in `src/api`/`src/state` so the renderer can read which Core is active and its `transport_kind` (Task 201) without learning the transport. This is the substrate Task 219's pairing UI and every Phase-3/4/6 remote-mode affordance build on; it does **not** ship the pairing UX (219) or the picker UI (601) — only the data layer + dispatch routing.

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.2 (the renderer never speaks gRPC; the Tauri command proxy wraps a single `CoreClient` trait with `UdsCoreClient`/`IrohCoreClient` impls — the exact trait shape is quoted there). **Note:** the `IrohCoreClient` struct comment + the §3.10/§6 "Split-host" bullet still cite `tonic-iroh-transport`; Task 200 amends those in place to the **hand-rolled tonic-0.12 ↔ Iroh-bidi adapter** decision (B1) — if 200 has landed, read the amended text; either way **the adapter is the hand-roll, never `tonic-iroh-transport`** (it forces `tonic 0.14`, which collides with the `tonic 0.12` workspace pin — see `design/spikes/tonic-iroh-findings.md §2`). The `IrohCoreClient` here consumes Task 212's adapter via Task 217's `TransportHandle`; it does **not** re-implement the adapter.
- `design/15_Desktop_Client.md` §3.10.1 (the `PairedCore` / `ActiveCore` struct layout — `core_id = BLAKE2b(core_pubkey)`, `transport`, `uds_socket_path`, `iroh_endpoint_id`, `core_pubkey`, `device_cert`, `last_connected_at`; **storage split: `cores.json` is cleartext metadata, device certs + device private keys live in the OS keychain keyed by `core_id`**), §3.10.2 (the launch decision tree incl. the embedded-Core step-0 amendment), §3.11 (the renderer reads `ServerCapabilities.transport_kind` and conditionally renders affordances — you build the field plumbing it keys off, not the affordances).
- `design/11_Remote_Transport_Relay.md` §3.1 (Iroh is the single non-browser remote transport), §5.2 (the three client SDK surfaces; the Desktop "uses local UDS co-located, Iroh when remote" — same Tonic services either way; the client's only transport knowledge is "how do I open a session").
- `apps/desktop/src-tauri/src/core_client.rs` — the **current** UDS-only client to refactor into `UdsCoreClient`: the lazy process-wide `tonic::Channel`, `default_socket_path`, `get_or_connect`/`reset_channel`, the `CoreClientError` enum (`#[serde(tag = "kind", content = "message")]` — a renderer wire contract; keep it), and `set_socket_override` (the embedded-mode hook).
- `apps/desktop/src-tauri/src/commands.rs` — the Tauri command surface (`concerto_rpc`/`concerto_subscribe`/`concerto_unsubscribe`/…) and the per-service `tonic` client construction in `dispatch`; this is what routes through `CoreClient` instead of `get_or_connect` after this task.
- `apps/desktop/src-tauri/src/embedded.rs` — embedded Core installs the UDS socket via `set_socket_override`; the registry must promote that in-process UDS as a `PairedCore { transport: Uds, display_name: "This machine" }` (§3.10.2 step 2) without fighting the embedded path.
- `apps/desktop/src-tauri/Cargo.toml` — the current dep set (`tonic`/`tower`/`hyper-util` UDS stack; **no `concerto-transport`/`concerto-identity`/`concerto-keychain` yet**). Adding the Iroh client pulls these in — see Implementation notes for the feature-gating decision.
- `apps/desktop/src/api/client.ts` + `apps/desktop/src/api/runtime.ts` — the TS data-layer conventions: every call goes through `invoke("concerto_rpc", …)`; `errorMessage` reads the `{kind,message}` envelope; `ServerCapabilities` is currently an opaque `Record`. `apps/desktop/src/state/useUiStore.ts` — the Zustand UI-only store pattern (server-canonical state lives in React Query; Zustand holds selection/ephemera only, per `design/15 §3.3`).
- `crates/keychain/src/api.rs` — the keychain crate's public surface + `version.workspace`/`[lib] name` conventions; this is where device certs + device keys go, keyed by `core_id`.
- `tasks/v1.0/217-transport-handle-api.md` → "Handoff Notes" — the `TransportHandle` client-side surface `IrohCoreClient` consumes (start/stop, the connection open path, how a `SignedDeviceCert` is presented). **217 is a hard dependency; do not start until its handoff is readable.** If 217's client surface differs from `design/11 §5.1`, follow the handoff.
- `tasks/v1.0/201-capability-negotiation.md` → "Handoff Notes" — `transport_kind` is now per-connection; the registry's `transport` field and the renderer's `transport_kind` read must agree.

## Scope — in
**Rust (`src-tauri`):**
- Define `pub trait CoreClient` (async) with **exactly** the `design/15 §3.2` shape: `async fn dispatch(&self, method: &str, payload: Value) -> Result<Value, CoreClientError>` and `async fn start_stream(&self, subject: &str, filter: Value, sink: StreamSink) -> Result<SubscriptionId, CoreClientError>` (adapt `StreamSink`/`SubscriptionId` to the existing `commands.rs` event-bus forwarder shape — keep the dot→slash subject mapping locked in `client.ts`).
- `UdsCoreClient`: refactor the existing `core_client.rs` channel logic behind the trait (peer-UID auth, the lazy channel + reset-on-error strategy preserved). No behavior change for the co-located path.
- `IrohCoreClient`: dial the active Core's Iroh endpoint via Task 217's `TransportHandle` client side, present the stored `SignedDeviceCert` in request metadata (per `design/12 §3.3` / `design/11 §3.3`), and route the same Tonic service calls. Reuse Task 212's hand-rolled adapter via 217 — **do not** re-implement it.
- The **connected-Core registry**: load/save `cores.json` (the `PairedCore` metadata fields from §3.10.1, cleartext, no secrets) + read/write device certs and device private keys in the OS keychain keyed by `core_id` (via `crates/keychain`). CRUD: list, get-active, set-active, upsert, remove (remove deletes the keychain entries too). `core_id = BLAKE2b(core_pubkey)`.
- Wire `commands.rs` to resolve the **active** `CoreClient` from the registry and dispatch through the trait (replacing the direct `get_or_connect`). The embedded/co-located UDS is promoted into the registry as the implicit "This machine" `PairedCore` (§3.10.2 step 2), preserving the `set_socket_override` embedded hook.
- A Tauri command pair to read registry state for the renderer: list paired Cores + the active one + its `transport_kind` (enough for 219 to build on). **Mutating pairing commands (StartPairing/CompletePairing) are Task 219/207/209 — not here**; this task may stub the registry-write side they call, but the pairing ceremony is out.

**TS (`src/api`, `src/state`):**
- A typed `src/api/cores.ts` binding over the read commands above (list paired Cores, active Core, its `transport_kind`) following the `client.ts`/`runtime.ts` convention.
- Type `ServerCapabilities.transport_kind` properly in `src/api/runtime.ts` (it is currently an opaque `Record`) so the renderer can branch on `UDS | IROH | WSS_BRIDGE` (Task 201's enum).
- A small Zustand slice (or extend `useUiStore`) holding the **active-Core id** for UI-only selection — server-canonical registry data stays in React Query keyed off the read commands (per `design/15 §3.3`). No domain state duplicated into Zustand.

## Scope — out
- The **pairing ceremony + pairing UI** (QR show/scan, StartPairing/CompletePairing) — Task 219 (UI) + Tasks 207/209 (the RPCs). This task exposes the registry the ceremony writes into; it does not drive the ceremony.
- The **Connect-to-Core picker / first-launch flow / multi-Core switch UX** — Task 601 (`design/15 §3.10.2`/§3.10.4). This task ships the registry + active-Core read/set commands the picker will call, not the picker.
- The **remote-mode affordance suppression** (hide "Reveal in Finder", drag-drop→`Files.Upload`, etc.) — Task 602 (`design/15 §3.11`). This task only types `transport_kind` so 602 can branch.
- Building the **Iroh endpoint in Core / the adapter / the `TransportHandle`** — Tasks 212/217. This task is the **client consumer** only.
- Real cross-machine split-host (two physical machines) — **Tier-3** phase-checklist line; the Tier-2 double here is a loopback Iroh endpoint on one host.
- Auto-update / signing / launchd (already shipped or Phase 7).

## Public interface this task locks
- **Rust (FROZEN):** the `CoreClient` trait — method set + signatures (`dispatch`, `start_stream`) exactly per `design/15 §3.2`. Every transport impl (UDS now, Iroh now, WSS-bridge-on-desktop never in V1.0) implements this; `commands.rs` only ever talks to the trait. The `CoreClientError` `{kind,message}` serde envelope is preserved (renderer wire contract — see `core_client.rs` test).
- **`cores.json` registry schema (FROZEN):** the on-disk JSON shape mirroring `PairedCore` (`core_id`, `display_name`, `transport`, `uds_socket_path?`, `iroh_endpoint_id?`, `core_pubkey`, `last_connected_at?`) **plus** the active-Core pointer. **Secrets (device cert, device private key) are NEVER in `cores.json`** — they live in the keychain keyed by `core_id`. New fields append-only; document a `version` field.
- **TS (FROZEN):** the `src/api/cores.ts` binding surface (the read-command return types) + the `transport_kind` typing in `runtime.ts` — the contract Task 219's UI and Task 601/602 consume.

## Implementation notes
- **The Rust/TS boundary (read this twice).** The dual-transport `CoreClient`, both impls, and the registry are **Rust in `src-tauri`** — the renderer is forbidden from speaking gRPC/keychain/fs directly (`apps/desktop/src-tauri/capabilities/main.json` grants no `http`/`shell`/`fs`). The TS side is **thin**: typed bindings over new Tauri read-commands + the active-Core UI selection. Do not push transport logic into TS. Split Outputs accordingly.
- **Feature-gate the Iroh client** the way the crate already gates `embedded-core`: the Iroh path (and its `concerto-transport`/`concerto-identity`/`concerto-keychain` deps) should be behind a Cargo feature so the lean co-located build doesn't grow the Iroh dependency tree unnecessarily — but the **`CoreClient` trait + registry are always present** (UDS is the always-available impl). Decide the exact feature name + default in-task and record it in Handoff; mirror the `#[cfg(feature = …)]` discipline in `main.rs`/`embedded.rs`.
- **Keychain on non-mac.** `crates/keychain` is the abstraction; V1.0 Desktop ships mac (Windows is Task 608). Keep keychain calls behind the crate's API so the Windows build (608) swaps the backend without touching this code. No raw Security-framework calls here.
- **Don't regress the co-located happy path.** The smoke gate boots a co-located UDS Core and dials it through the shell; after this refactor that path must still resolve the implicit "This machine" `PairedCore` and dispatch identically. The embedded `set_socket_override` hook must still install the in-process socket as that implicit Core. Keep `reset_channel`-style reconnect for UDS.
- **`core_id` derivation** must match `crates/identity`'s `device_id`/BLAKE2b convention applied to `core_pubkey` (`design/15 §3.10.1` says `BLAKE2b(core_pubkey)`); reuse the identity crate's hash, don't hand-roll BLAKE2b.

## Verification
**Tier 2.** Two command sets — this task spans Rust + TS.

**Rust (`src-tauri`) — the `cargo` gate:**
1. `cargo check -p concerto-desktop --all-features` clean (and with default features — the lean build).
2. `cargo clippy -p concerto-desktop --all-targets --all-features -- -D warnings` clean.
3. `cargo test -p concerto-desktop` → unit tests pass, including: registry round-trips `cores.json` (load/save/upsert/remove) with secrets kept out of the JSON; `UdsCoreClient` dials a loopback `UnixListener` (keep the existing `connect_uds_*` tests green); `IrohCoreClient` dispatches against a **loopback Iroh endpoint** (two endpoints on one host, relays disabled — the Tier-2 double) and carries the device cert in metadata; the `{kind,message}` error envelope test stays green.

**TS (`src/api`/`src/state`) — the `web-ts` §5.3 set, against the REAL `apps/desktop` scripts:**
> ⚠️ `apps/desktop/package.json` today defines **only** `dev` / `build` (`tsc --noEmit && vite build`) / `preview` / `tauri` — **no `typecheck`, `lint`, or `test` script, and no vitest/eslint dep.** README §5.3's `web-ts` set assumes `pnpm -C apps/<app> typecheck|lint|test|build`. This task must **add the missing scripts + devDeps** (a `typecheck` alias for `tsc --noEmit`, `vitest` for `test`, and a `lint` — eslint or `tsc`-only if eslint is not yet configured) as part of its TS work, then satisfy them. Record exactly what you added in Handoff.
4. `pnpm -C apps/desktop typecheck` clean (add the script: `tsc --noEmit`).
5. `pnpm -C apps/desktop lint` clean (add it; if no eslint config exists yet, scope this task to add a minimal one or alias to typecheck and note it).
6. `pnpm -C apps/desktop test` → vitest unit tests for `src/api/cores.ts` (the binding shape) + the active-Core Zustand slice pass (mock `@tauri-apps/api` `invoke`). Add `vitest` + config.
7. `pnpm -C apps/desktop build` → `tsc --noEmit && vite build` clean.

**Tier-2 double + what it does NOT cover.** The double is a **loopback Iroh endpoint pair on one host with relays disabled (direct)** for the `IrohCoreClient` dispatch test, plus a loopback `UnixListener` for `UdsCoreClient`, plus mocked `invoke` for the TS bindings. It proves: trait routing, registry persistence (secrets-out-of-JSON), device-cert-in-metadata, and the TS read path. It does **NOT** cover: real cross-machine split-host (Desktop on a laptop, Core on a workstation/VM), real NAT traversal/relay fallback, or real OS-keychain prompts on a signed build — those are the **Tier-3 Phase-2 checklist** lines ("pair a real second machine over LAN", "transfer a file split-host"). Task 220's loopback smoke is the end-to-end Tier-2 capstone; this task's tests are unit/component scope.

## Definition of Done
- [x] `CoreClient` trait defined with the FROZEN `design/15 §3.2` signatures; `UdsCoreClient` + `IrohCoreClient` impls; `commands.rs` dispatches through the trait
- [x] `IrohCoreClient` consumes Task 217's `TransportHandle` + Task 212's hand-rolled adapter (NO `tonic-iroh-transport`); presents the device cert in metadata
- [x] `cores.json` registry (cleartext metadata) + keychain (certs/keys keyed by `core_id`); co-located/embedded UDS promoted as the implicit "This machine" Core
- [x] TS `src/api/cores.ts` binding + typed `transport_kind` + active-Core Zustand slice; server-canonical data stays in React Query
- [x] Missing `apps/desktop` pnpm scripts/devDeps (typecheck/lint/test) added; all `web-ts` §5.3 commands pass
- [x] Both the `cargo` set and the `web-ts` set pass; co-located smoke path unaffected (`scripts/smoke.sh` still green — unchanged gate)
- [x] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate seams for 219's pairing writes documented in Handoff)
- [x] Single commit with the message below

## Outputs
**Rust (`src-tauri`):**
- `apps/desktop/src-tauri/src/core_client.rs` (UNCHANGED — kept as the UDS dial primitives + the FROZEN `CoreClientError` `{kind,message}` envelope + its tests; the trait was split into a new `src/transport.rs` instead — see Drift)
- `apps/desktop/src-tauri/src/transport.rs` (new — the `CoreClient` trait + `UdsCoreClient`; ADDED to Outputs, see Drift)
- `apps/desktop/src-tauri/src/rpc.rs` (new — the per-RPC `dispatch_over_channel`/`subscribe_over_channel` shared by both impls, extracted from `commands.rs`; ADDED to Outputs, see Drift)
- `apps/desktop/src-tauri/src/iroh_client.rs` (new — `IrohCoreClient`, feature-gated + the Tier-2 loopback double)
- `apps/desktop/src-tauri/src/cores_registry.rs` (new — `cores.json` + keychain registry)
- `apps/desktop/src-tauri/src/commands.rs` (modified — dispatch through the active `CoreClient`; new registry read/set commands)
- `apps/desktop/src-tauri/src/main.rs` (modified — register the new commands + modules; manage registry state)
- `apps/desktop/src-tauri/Cargo.toml` (modified — `concerto-keychain`/`concerto-identity` + `async-trait`/`base64` always-on; `concerto-transport`/`iroh` behind the `iroh-transport` feature)
- `crates/keychain/src/api.rs` + `crates/keychain/src/lib.rs` (modified — added `CoreSecretSlot` + per-`core_id` `get/set/delete_core_secret`; ADDED to Outputs, see Drift)
- `docs/interfaces/rust-api.md` (regenerated — gains the keychain `CoreSecretSlot` enum; ADDED to Outputs)
- `Cargo.lock` (modified — new desktop deps; `wmi`→`windows 0.62.2` unchanged)

**TS (`src/api`, `src/state`):**
- `apps/desktop/src/api/cores.ts` (new — typed registry read binding)
- `apps/desktop/src/api/runtime.ts` (modified — typed `transport_kind` enum + `isRemoteTransport`)
- `apps/desktop/src/state/useCoresStore.ts` (new slice — UI-only pending active-Core selection)
- `apps/desktop/src/api/cores.test.ts` + `src/state/useCoresStore.test.ts` + `src/api/runtime.test.ts` (new — vitest)
- `apps/desktop/package.json` (modified — `typecheck`/`lint`/`test` scripts + `vitest` devDep)
- `apps/desktop/pnpm-lock.yaml` (modified — `vitest` added)
- `apps/desktop/vitest.config.ts` (new — node env, `src/**/*.test.ts`)

## Commit message
```
phase-2: desktop dual transport (CoreClient + cores.json registry)

Refactors the desktop's UDS-only client into a transport-agnostic
CoreClient trait (UdsCoreClient + IrohCoreClient over Task 217's
TransportHandle, hand-rolled tonic-0.12 adapter — no tonic-iroh-
transport) behind a cores.json + keychain connected-Core registry.
Adds the TS binding + typed transport_kind and the missing apps/desktop
typecheck/lint/test scripts. Pairing UX is Task 219.

Refs: tasks/v1.0/218-desktop-dual-transport.md
```

## Handoff Notes

**FROZEN `CoreClient` trait (`design/15 §3.2`, `apps/desktop/src-tauri/src/transport.rs`).**
```rust
#[async_trait::async_trait]
pub trait CoreClient: Send + Sync {
    async fn dispatch(&self, method: &str, payload: Value) -> Result<Value, CoreClientError>;
    async fn start_stream(&self, subject: &str, filter: Value, sink: StreamSink)
        -> Result<StreamSubscription, CoreClientError>;
}
```
`StreamSink` is `design/15 §3.2`'s `StreamSink` adapted to the existing event-bus forwarder: a cloneable `Arc<dyn Fn(&Value) -> bool + Send + Sync>` (return `false` to end the stream). `SubscriptionId = String`; `start_stream` returns `StreamSubscription { id, join }` (the id + the forwarder `JoinHandle` so `commands.rs`'s `SubscriptionRegistry` aborts it on unsubscribe — `design/15 §3.2` returns just the id, the handle is the desktop's existing abort mechanism). Impls: `UdsCoreClient` (always present) + `IrohCoreClient` (feature `iroh-transport`). `commands.rs` only ever talks to `Box<dyn CoreClient>`. The `CoreClientError` `{kind,message}` serde envelope is preserved verbatim in `core_client.rs` (its renderer-wire-contract test stays green).

**FROZEN `cores.json` schema (`apps/desktop/src-tauri/src/cores_registry.rs`).** Cleartext doc `{ version: u32 (=1), cores: [PairedCore], active_core_id: Option<String> }`. `PairedCore = { core_id (BLAKE2b(core_pubkey) hex), display_name, transport ("uds"|"iroh"), uds_socket_path?, iroh_endpoint_id?, core_pubkey ([u8;32]), core_noise_pubkey? ([u8;32]), last_connected_at? }`. **Secrets (device cert + device private key) are NEVER in `cores.json`** — they live in the OS keychain keyed by `core_id` via `concerto-keychain`'s new `CoreSecretSlot::{DeviceCert,DevicePrivateKey}` (account string `cores.<core_id>.<slot>`). The implicit co-located UDS is promoted as `PairedCore { core_id: "local-machine", display_name: "This machine", transport: Uds }` (`§3.10.2` step 2). `core_id` reuses `concerto_identity::device_id` (no hand-rolled BLAKE2b).

**Drift from plan.**
- **Trait split, not in-place.** Per the Outputs' "or split into `src/transport.rs` — decide and note": `core_client.rs` is **unchanged** (kept as UDS dial primitives + the FROZEN `CoreClientError` + its tests); the trait + `UdsCoreClient` live in new `src/transport.rs`, and the per-RPC service mapping moved to new `src/rpc.rs` (`dispatch_over_channel`/`subscribe_over_channel`, generic over the gRPC transport `T` so the plain UDS `Channel` and the Iroh `InterceptedService<Channel, DeviceCertInterceptor>` both route through one place). Both are ADDED to Outputs.
- **`crates/keychain` touched (ADDED to Outputs + flagged).** The frozen Task-10 `SecretKind` enum is closed/`Copy` and cannot key by `core_id`. Rather than break its `Copy` derive or its frozen variants, I added a **parameterized** accessor — `CoreSecretSlot` + `Secrets::{get,set,delete}_core_secret(core_id, slot)` (account `cores.<core_id>.<slot>`) — append-only, no change to any existing variant/account string (the Task-10 account-string tests stay green). `docs/interfaces/rust-api.md` regenerated (gains `CoreSecretSlot`).
- **`PairedCore.core_noise_pubkey` appended to the schema (flagged).** The Iroh Noise IK handshake needs the Core's **X25519** Noise static public key, which is a *distinct* key from the Ed25519 `core_pubkey` in `design/15 §3.10.1`'s struct — the dial cannot proceed without it. Added as an append-only `Option<[u8;32]>` (None for UDS), captured at pairing from Task 217's `core_noise_public()` companion. This is the one field beyond the design's literal struct; the schema clause permits append-only additions.
- Did **not** touch `core_client.rs`, `embedded.rs`, or `tray.rs` logic (embedded's `set_socket_override` still works: `resolve_active_client` promotes the override'd default socket on first dispatch).

**Rust↔TS boundary as built.** All transport/keychain/registry logic is Rust (`src-tauri`). TS is thin: `cores.ts` (read bindings over `list_paired_cores`/`get_active_core`/`set_active_core`), typed `transport_kind` in `runtime.ts` (numeric enum matching the proto ordinals + `isRemoteTransport`), and `useCoresStore.ts` (UI-only **pending** active-Core id; the committed active Core is React-Query-canonical, never duplicated into Zustand).

**Cargo feature name + default for the Iroh path.** `iroh-transport` (default **OFF**). It gates `dep:concerto-transport` + `dep:iroh` (and the `IrohCoreClient` module). The `CoreClient` trait + the `cores.json`/keychain registry are **always present** (UDS is the always-available impl); `concerto-keychain`/`concerto-identity` + `async-trait`/`base64` are non-optional (the registry derives `core_id` + stores per-Core secrets regardless of flavour). The Tier-2 `cargo` gate runs both `--all-features` and default.

**Exact pnpm scripts + devDeps added.** `package.json` scripts: `"typecheck": "tsc --noEmit"`, `"test": "vitest run"`, `"lint": "tsc --noEmit"` (**lint is aliased to typecheck — no eslint config added**, to avoid a large eslint dep tree for a thin data-layer task; a real eslint pass is a later DX task if wanted). devDep added: `vitest ^2.1.8` (resolved 2.1.9; `pnpm-lock.yaml` committed). `vitest.config.ts` uses the `node` environment (`src/**/*.test.ts`; no jsdom — the tests mock `@tauri-apps/api`'s `invoke` and touch no DOM). All four `web-ts` §5.3 commands pass (typecheck/lint/test/build).

**How the loopback-Iroh `IrohCoreClient` test is structured** (`iroh_client.rs` `#[cfg(test)]`, feature `iroh-transport`). Two `iroh::Endpoint`s on one host, **relays disabled** (`RelayMode::Disabled` client; `IrohTransport::start` with `disable_remote:true` server) → the only path is direct loopback. The server runs a minimal `RuntimeServer` over the transport's `ApiDispatcher` (mirroring `crates/transport/tests/loopback.rs`); its `get_server_capabilities` **asserts the `concerto-device-cert` metadata is present + base64-decodable** and echoes `server_version="iroh-probe"` + `transport_kind=2`. The client dials via `connect_channel` (Task 212's hand-rolled adapter + Noise IK initiator), wraps the channel in `InterceptedService<Channel, DeviceCertInterceptor>` (stamps `base64(cert_bytes||signature)`), and routes `dispatch("Runtime.GetServerCapabilities")`. A second test proves an unmapped method returns `NotImplemented` before touching the wire. (No `dev-relay` feature needed — that gates only the *relayed*-path subtests in the transport crate.)

**Registry-write seams left for 219/207/209/601.** The pairing ceremony writes are stubbed-but-present: `CoresRegistry::{upsert, remove, set_active, get, get_secret, set_secret, delete_secret}` + `core_id_for` + the `iroh_client::IrohCoreClient::connect` constructor are all implemented and unit-tested but not yet driven from the live command path (marked `#[cfg_attr(not(test), allow(dead_code))]` where only tests + future tasks call them). 219 builds the pairing UI + Connect-to-Core picker on `cores.ts` + the active-Core slice; 601 wires the live Iroh dial (`resolve_active_client` currently returns `NotImplemented` for an active Iroh Core — the connect flow must build the client `Endpoint` + resolve the cert/key from the keychain + the server `EndpointAddr` from `iroh_endpoint_id`, then construct `IrohCoreClient` and hold it as managed state rather than rebuilding per call).

**Open questions for next task.**
1. **Iroh client lifetime/caching.** `IrohCoreClient` holds a persistent multiplexed channel; the live connect flow (601) must cache it in Tauri state (per active Core) rather than rebuild per dispatch — `resolve_active_client` builds a fresh `UdsCoreClient` per call (cheap, reuses the process-wide channel) but an Iroh client must not be re-dialed per call. The seam returns `NotImplemented` until 601 supplies the cached-client resolution.
2. **`iroh_endpoint_id` → `EndpointAddr`.** The Tier-2 double uses `direct_endpoint_addr` (loopback). Real dial must parse `PairedCore.iroh_endpoint_id` into an `iroh::EndpointAddr` (via Iroh discovery / the `relay_hint` captured at pairing). That resolution + relay-hint storage is 601/219's wiring.
3. **`core_noise_pubkey` capture.** 219's pairing must persist Task 217's `core_noise_public()` into `PairedCore.core_noise_pubkey` (and the device's own Noise static into the keychain alongside the cert/key) — without it the Iroh handshake can't complete.

**Deliberate debt.** — (none; no `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code. `Status::unimplemented(...)` in the test double is a runtime gRPC status for unused probe methods, not the macro.)

**Smoke-gate state.** unchanged — added no smoke check. `scripts/smoke.sh` PASSED (exit 0, "all checks PASSED", including the co-located UDS happy path that now resolves through the registry + `CoreClient` trait).
