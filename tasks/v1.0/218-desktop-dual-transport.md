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
- [ ] `CoreClient` trait defined with the FROZEN `design/15 §3.2` signatures; `UdsCoreClient` + `IrohCoreClient` impls; `commands.rs` dispatches through the trait
- [ ] `IrohCoreClient` consumes Task 217's `TransportHandle` + Task 212's hand-rolled adapter (NO `tonic-iroh-transport`); presents the device cert in metadata
- [ ] `cores.json` registry (cleartext metadata) + keychain (certs/keys keyed by `core_id`); co-located/embedded UDS promoted as the implicit "This machine" Core
- [ ] TS `src/api/cores.ts` binding + typed `transport_kind` + active-Core Zustand slice; server-canonical data stays in React Query
- [ ] Missing `apps/desktop` pnpm scripts/devDeps (typecheck/lint/test) added; all `web-ts` §5.3 commands pass
- [ ] Both the `cargo` set and the `web-ts` set pass; co-located smoke path unaffected (`scripts/smoke.sh` still green — unchanged gate)
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate seams for 219's pairing writes documented in Handoff)
- [ ] Single commit with the message below

## Outputs
**Rust (`src-tauri`):**
- `apps/desktop/src-tauri/src/core_client.rs` (modified — `CoreClient` trait + `UdsCoreClient` refactor; or split the trait into a new `src/transport.rs` — decide and note)
- `apps/desktop/src-tauri/src/iroh_client.rs` (new — `IrohCoreClient`, feature-gated)
- `apps/desktop/src-tauri/src/cores_registry.rs` (new — `cores.json` + keychain registry)
- `apps/desktop/src-tauri/src/commands.rs` (modified — dispatch through the active `CoreClient`; new registry read-commands)
- `apps/desktop/src-tauri/src/main.rs` (modified — register the new commands; manage registry state)
- `apps/desktop/src-tauri/Cargo.toml` (modified — feature-gated `concerto-transport`/`concerto-identity`/`concerto-keychain` deps)

**TS (`src/api`, `src/state`):**
- `apps/desktop/src/api/cores.ts` (new — typed registry read binding)
- `apps/desktop/src/api/runtime.ts` (modified — typed `transport_kind`)
- `apps/desktop/src/state/` (new slice or `useUiStore.ts` modified — active-Core selection)
- `apps/desktop/src/api/cores.test.ts` + the slice test (new — vitest)
- `apps/desktop/package.json` (modified — add `typecheck`/`lint`/`test` scripts + `vitest`/lint devDeps)
- `apps/desktop/vitest.config.ts` (new, if needed) + any `eslint`/`tsconfig` lint glue

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

## Handoff Notes (fill in when finishing)
- Drift from plan / Rust↔TS boundary as built / the Cargo feature name + default for the Iroh path / exact pnpm scripts + devDeps added / registry-write seams left for 219 / Open questions / Smoke-gate state
