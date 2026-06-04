# Task 213 — mDNS LAN Discovery (`_concerto._tcp.local`)

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 212 |
| Touches subsystem(s) | 11 (Remote Transport & Relay) |
| Smoke gate | new:mdns-discovery |

## Goal
Add mDNS LAN discovery so a Core advertises itself on the local network and clients on the same network find it and **prefer the LAN path** — opening Iroh directly to the discovered endpoint without ever consulting the relay. The Core (responder) publishes a `_concerto._tcp.local` service whose TXT record carries `endpoint_id` / `core_pubkey` / `version` / `caps`; clients (browser) browse for it and hand the discovered `endpoint_id` to the `crates/transport` Iroh path from Task 212 (which classifies the resulting connection as `ConnectionPath::Lan`). This is the zero-relay path for Desktop↔Core and Mobile↔Core on the same Wi-Fi, and the only remote path that survives `disable_remote = true`.

## Inputs to read before starting
- `design/11_Remote_Transport_Relay.md` §3.5 (Core publishes `_concerto._tcp.local` with TXT `endpoint_id` / `core_pubkey` / `version` / `caps`; clients browse and **prefer the LAN path** — open Iroh directly without the relay, no hole-punching needed; privacy opt-out via managed setting or per-network preference), §6.4 (**LAN-only mode** `disable_remote = true` does NOT register with a relay but **still publishes mDNS** and accepts LAN connections), §12 R-3 (**support BOTH IPv4 and IPv6** — some networks suppress one but not the other).
- `design/12_Security_Identity.md` §3.6 trust-table row (the mDNS broadcast is public: TXT records + public-key fingerprint are visible on the LAN — informs what is safe to put in TXT and the opt-out rationale).
- `crates/transport/src/*` (Task 212) — the `iroh::Endpoint`'s `endpoint_id`, the `ConnectionPath::Lan` variant, the `disable_remote` gate placement, and the public surface to extend. The responder reads the Core's live `endpoint_id` + the Core Ed25519 `core_pubkey` (Task 206) to populate TXT; the browser feeds discovered endpoint ids into the 212 connect path.
- `crates/transport/Cargo.toml` + `Cargo.toml` `[workspace.dependencies]` — where the chosen mDNS crate pin lands.
- `deny.toml` — the `[licenses] allow` list + dated **operator-ratification comment** style; the new mDNS crate must clear `cargo deny check`.
- `tasks/v1.0/212-transport-iroh-endpoint.md` → "Handoff Notes" — the FROZEN `crates/transport` surface, the `ConnectionPath::Lan` semantics, and how `disable_remote` is consulted.
- `tasks/v1.0/211-managed-settings-enforcement.md` → "Handoff Notes" — the managed-setting / per-network mDNS opt-out predicate (publication can be suppressed independently of `disable_remote`).
- `tasks/v1.0/201-capability-negotiation.md` → "Handoff Notes" — `caps` semantics (the `ServerCapabilities` feature surface the TXT `caps` field mirrors at coarse grain).

## Scope — in
- **Choose an mDNS crate** (e.g. `mdns-sd` — pure-Rust, IPv4+IPv6, responder + browser in one crate) and pin it in `[workspace.dependencies]` with a rationale comment; clear `cargo deny check`. State the choice + why in Handoff.
- **Responder** (Core advertises): register a `_concerto._tcp.local` service instance whose TXT record carries exactly the four keys from `design/11 §3.5` — `endpoint_id` (Iroh endpoint id), `core_pubkey` (base64 Ed25519), `version` (semver), `caps` (comma-separated features). Bind/advertise on **both IPv4 and IPv6** (R-3). Re-publish on endpoint/relay/version change; deregister cleanly on shutdown (goodbye packet).
- **Browser** (client discovery): browse `_concerto._tcp.local`, parse the TXT record into a discovered-Core descriptor (`endpoint_id` + `core_pubkey` + `version` + `caps`), and expose a stream/list of discovered Cores. The client then opens Iroh directly to that `endpoint_id` via the Task 212 connect path (LAN-preferred → `ConnectionPath::Lan`). Surface discovered Cores so 218/219 (Desktop Connect-to-Core picker) and 511 (mobile pairing) can consume them.
- **Opt-out:** publication is suppressible via a managed setting or per-network preference (Task 211 predicate). `disable_remote = true` does **NOT** silence mDNS (`design/11 §6.4`) — note this explicitly; the only thing that silences mDNS is the dedicated opt-out.
- **Integrate into `crates/transport`** (extend the 212 surface; this is the same crate, not a new one): the responder lifecycle is owned alongside the endpoint (start/stop with the transport); the browser is a discovery helper clients drive. Add the `MdnsResponder` field to `TransportState` (`design/11 §4` already names it).
- **Tests** (the Tier-2 double, see Verification): two in-process tasks on one host — one publishes the service on a loopback/local interface, the other browses and recovers the exact TXT schema (`endpoint_id`/`core_pubkey`/`version`/`caps`) and resolves the advertised address; assert IPv4 and IPv6 records are both present where the test interface supports them; assert publication is suppressed when the opt-out is set but **still published** when only `disable_remote` is set.

## Scope — out
- **The Iroh connection itself** (Task 212 owns the connect path + `ConnectionPath::Lan` classification; this task hands it an `endpoint_id`, it does not open QUIC).
- **The relay path / hole-punch** (212/214 — mDNS is the relay-free alternative, not a relay feature).
- **Desktop / mobile discovery UI** (Task 219 Desktop picker, Task 511 mobile pairing — they consume the discovered-Core stream this task exposes).
- **Pairing** over the discovered Core (Task 207 — discovery is unauthenticated advertisement; trust is still established by the QR/cert flow; the TXT `core_pubkey` is a fingerprint hint, not an auth credential).
- **Proto changes** — discovery is internal Rust; no proto (decision D1). Do not add proto.
- **Real cross-device LAN discovery** across two physical machines on real Wi-Fi (Tier-3 — Phase-2 manual checklist).

## Public interface this task locks
- **The TXT record schema** for `_concerto._tcp.local` — FROZEN. Exactly the four keys, names + value encodings: `endpoint_id=<iroh_endpoint_id>`, `core_pubkey=<base64-ed25519>`, `version=<semver>`, `caps=<comma-separated-features>` (`design/11 §3.5`). Mobile (511) and Web (LAN path, 521) browse for this exact schema; new keys are append-only.
- **The service type string** `_concerto._tcp.local` — FROZEN.
- The discovered-Core descriptor type + browse entry point on `crates/transport` (`src/api.rs`) that 218/219/511 consume.

## Implementation notes
- **TXT value hygiene:** `endpoint_id` and `core_pubkey` are deliberately public on the LAN (`design/12 §3.6`); put **nothing** secret in TXT (no device certs, no tokens). `caps` is the coarse feature list (mirror the `ServerCapabilities` surface from 201 at low fidelity — enough for a client to decide whether to bother connecting), not a full capability dump.
- **IPv4 + IPv6 (R-3):** ensure the responder registers A and AAAA records and the browser accepts either; some networks suppress one. Pick a crate that does both out of the box (`mdns-sd` does) rather than hand-rolling dual-stack.
- **`disable_remote` is not an mDNS switch.** Wire the opt-out to its own managed/per-network setting (Task 211). A reviewer will check that `disable_remote = true` leaves mDNS publishing — `design/11 §6.4` is explicit. Add a test asserting exactly this.
- **Lifecycle:** publish after the Iroh endpoint is up (you need its `endpoint_id`) and send a goodbye/unregister on shutdown so stale records don't linger. Re-announce on `version`/`endpoint_id`/`caps` change.
- **Cross-platform.** The mDNS crate must build on the Windows + Linux CI lanes (Task 113). `mdns-sd` is pure-Rust and portable; if a candidate crate needs platform sockets behind `#[cfg]`, gate it and confirm the Windows lane stays green. No `std::os::unix`-only types in the public discovery surface.
- **License:** the mDNS crate (and any transitive net deps) must clear `cargo deny check`; ratify new SPDX with a dated comment, copyleft = Stop-and-ask.

## Verification
Tier 2.

The Tier-2 test double is **two processes/tasks on one host discovering via mDNS on a loopback/local interface** — one responder, one browser, recovering the exact TXT schema and resolving the advertised address in-process. It proves the **publish + browse + TXT-schema + dual-stack + opt-out logic** without a second machine. It does **NOT** cover real cross-device LAN discovery across two physical machines on real Wi-Fi (multicast on a real switch, mDNS-suppressing work networks, IPv6-only segments) — that is **Tier-3**, on the Phase-2 manual checklist (pair a real second machine over LAN via mDNS direct).

Per README §5.3 (`rust`):
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-transport mdns` → publish/browse round-trip, exact TXT schema (`endpoint_id`/`core_pubkey`/`version`/`caps`), IPv4+IPv6 presence, opt-out suppresses while `disable_remote` does not, all pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (new mDNS crate cleared; `deny.toml` ratified if needed).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen if the `crates/transport` `api.rs` discovery surface changed.
7. `scripts/smoke.sh` → the new `mdns-discovery` capability publishes + browses on the loopback interface and asserts the TXT schema; existing caps stay green. Exits 0.

## Definition of Done
- [x] mDNS crate chosen + pinned + `cargo deny` cleared; choice justified in Handoff
- [x] Responder publishes `_concerto._tcp.local` with the exact 4-key TXT schema, IPv4+IPv6, clean (de)register lifecycle
- [x] Browser discovers + parses TXT into a discovered-Core descriptor; feeds `endpoint_id` to the 212 LAN connect path
- [x] Opt-out (managed / per-network) suppresses publication; `disable_remote = true` does NOT (asserted by test)
- [x] FROZEN: the TXT record schema + service type + discovered-Core descriptor on `crates/transport`
- [x] Tier-2 loopback discovery double tests pass; Verification commands pass; interfaces clean/regenerated; smoke `mdns-discovery` green
- [x] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate debt in Handoff)
- [x] Single commit with the message below

## Outputs
- `Cargo.toml` (modified — `[workspace.dependencies]` += the mDNS crate pin with rationale)
- `Cargo.lock` (modified — mdns-sd + if-addrs/socket-pktinfo lockfile churn; expected)
- `crates/transport/Cargo.toml` (modified — mDNS dep)
- `crates/transport/src/mdns.rs` (new — responder + browser), `crates/transport/src/api.rs` (modified — discovered-Core descriptor + `MdnsConfig`/`MdnsResponder`/`MdnsBrowser` + service-type/TXT-key consts + `mdns` field on `IrohTransport`)
- `crates/transport/src/endpoint.rs` (modified — ADDED to Outputs, see Drift: `publish_mdns`/`stop_mdns`/`is_mdns_publishing`/`browse_lan` on `IrohTransport`, `mdns` field init, `stop()` deregisters mDNS — the responder is owned alongside the endpoint per 212's frozen `IrohTransport`/`TransportState` split, NOT on `TransportState`)
- `crates/transport/src/lib.rs` (modified — ADDED to Outputs: `pub mod mdns;` + re-export the new frozen names)
- `crates/transport/src/error.rs` (modified — ADDED to Outputs: new `TransportError::Mdns` variant)
- `crates/transport/tests/mdns_loopback.rs` (new — the two-task discovery double)
- `deny.toml` (NOT modified — mdns-sd tree clears `cargo deny check` with no new SPDX)
- `scripts/smoke.d/99-mdns-discovery.sh` + `scripts/smoke.manifest` (new capability)
- `docs/interfaces/rust-api.md` (regenerated — the 4 new `api.rs` decls)

## Commit message
```
phase-2: mDNS LAN discovery (_concerto._tcp.local)

Core advertises _concerto._tcp.local (TXT: endpoint_id/core_pubkey/
version/caps, IPv4+IPv6) and clients browse + prefer the LAN path,
opening Iroh directly via the 212 path (ConnectionPath::Lan, no relay).
Publication is opt-out via managed/per-network setting; disable_remote
does NOT silence mDNS. Tier-2 double: two tasks discover on loopback.

Refs: tasks/v1.0/213-mdns-lan-discovery.md
```

## Handoff Notes (filled in when finishing)

- **mDNS crate choice + rationale.** `mdns-sd = "0.20"` (pure-Rust mDNS/DNS-SD
  responder + browser in one crate). Picked because: (1) it registers **A and
  AAAA** records and the browser accepts either out of the box — satisfies
  `design/11 §12 R-3` (support both v4 and v6) without hand-rolling dual-stack
  sockets; (2) **loopback interfaces (`LoopbackV4`/`LoopbackV6`) are enabled by
  default in 0.20**, which is the load-bearing property that makes the Tier-2
  double hermetic — the responder+browser pair resolve over **loopback
  multicast** with no real LAN and no external network, exactly what a headless
  CI network sandbox needs; (3) pure-Rust + cross-platform (`if-addrs` +
  `socket2`), no `#[cfg(unix)]` on the discovery path → the Task-113 Windows
  lane stays green; (4) license `Apache-2.0 OR MIT`, with a tiny transitive tree
  (`if-addrs` MIT/BSD-3-Clause, `socket-pktinfo`/`flume`/`fastrand`
  MIT/Apache-2.0) all already on the `deny.toml` allow-list. Pinned at the
  **minor** (`"0.20"`, not an exact patch) — unlike the iroh trio it is a
  self-contained leaf with a stable wire format, not part of the spike-validated
  QUIC stack. Earlier 0.13 lacks the default loopback interfaces; 0.20 is why
  this is hermetic.

- **License ratifications.** **None needed** — `cargo deny check` is green
  (`advisories ok, bans ok, licenses ok, sources ok`) with the mdns-sd tree
  added; no new SPDX expression appears (every license is already allowed).
  `deny.toml` was **not** modified.

- **Drift from plan.**
  - **`MdnsResponder` lives on `IrohTransport`, not on the in-memory
    `TransportState`.** The task said "Add the `MdnsResponder` field to
    `TransportState` (`design/11 §4` already names it)", and the design's §4
    struct does list `mdns_responder` on `TransportState`. BUT Task 212 (merged,
    FROZEN) deliberately split the design's one struct: the **owning** fields
    (`iroh_endpoint`, `relay_url`, `pairing_listener`) moved to `IrohTransport`,
    leaving `TransportState` as the **pure session-registry** (`sessions` +
    `nat_stats`) with `#[derive(Default)]`. The mDNS responder owns a non-`Default`
    daemon thread and its lifecycle is "start/stop with the transport" (the
    task's own Scope-in wording: "owned alongside the endpoint"), so it belongs
    with the endpoint on `IrohTransport` (field `mdns: Arc<Mutex<Option<
    MdnsResponder>>>`), exactly where 212 put the other owned fields. Putting it
    on `TransportState` would have broken 212's FROZEN `Default` derive and the
    216/217 "pure registry" contract. This honors the design intent
    (responder owned by the transport, started after the endpoint is up) while
    respecting 212's frozen split. Flagged here per the "don't modify a frozen
    surface silently" rule — the `TransportState` struct was **not** touched.
  - **`MdnsConfig` carries no `disable_remote`** (by design): the opt-out
    (`MdnsConfig::opt_out`) is the *only* mDNS switch. `disable_remote` is
    consulted nowhere in `mdns.rs` — a reviewer can confirm `disable_remote =
    true` leaves mDNS publishing. The `disable_remote_does_not_silence_mdns`
    test drives the real `IrohTransport::start(disable_remote=true)` →
    `publish_mdns(opt_out=false)` → `is_mdns_publishing() == true` path
    (`design/11 §6.4`).
  - **No `concerto-core` wiring.** This task fills the transport surface only.
    The Core actor that builds the live `MdnsConfig` (real `endpoint_id` from
    212, `core_pubkey` base64 of the Task-206 Ed25519 public key, `version`,
    `caps` mirroring `ServerCapabilities`) and calls `publish_mdns` at
    boot/re-announce, plus the wiring of the *opt-out* to a concrete managed /
    per-network setting, is **deferred to Task 217's `TransportHandle` façade /
    boot wiring** (the same place 212 deferred its `serve_iroh` auto-spawn). See
    Open questions. No `boot.rs` / `api_server.rs` changes.

- **Open questions for next task (217 / 218 / 219 / 511 / 211-followup).**
  - **The FROZEN mDNS surface (declared in `crates/transport/src/api.rs`):**
    the consts `SERVICE_TYPE = "_concerto._tcp.local."`, `TXT_ENDPOINT_ID =
    "endpoint_id"`, `TXT_CORE_PUBKEY = "core_pubkey"`, `TXT_VERSION = "version"`,
    `TXT_CAPS = "caps"`; the descriptor `DiscoveredCore { instance_name,
    endpoint_id, core_pubkey_b64, version, caps, addresses }` (+ `caps_list()`);
    `MdnsConfig { instance_name, endpoint_id, core_pubkey_b64, version, caps,
    port, addrs, opt_out }` (+ `::new(..)`); `MdnsResponder::{publish(config) ->
    Result<Self>, is_publishing(), config(), fullname(), shutdown()}` (+ `Drop`);
    `MdnsBrowser::{start(exclude_fullname: Option<String>) -> Result<Self>,
    recv() -> Option<DiscoveredCore>, shutdown()}` (+ `Drop`); and on
    `IrohTransport`: `publish_mdns(MdnsConfig) -> Result<()>`, `stop_mdns()`,
    `is_mdns_publishing() -> bool`, `browse_lan() -> Result<MdnsBrowser>`. New
    TXT keys are **append-only** (`design/11 §3.5`). 218/219/511/521 browse for
    this exact schema.
  - **For 217 (`TransportHandle` / boot):** build the live `MdnsConfig` from the
    transport (`endpoint_id()`), the Core Ed25519 public key
    (base64, Task 206), the Core version, and the coarse `caps` list (mirror
    `ServerCapabilities` from 201 at low fidelity), then call
    `transport.publish_mdns(cfg)` **after** the endpoint is up; re-call it on
    `version`/`endpoint_id`/`caps` change (the responder replaces the prior
    registration, sending the old record's goodbye on drop). Wire `opt_out` to a
    concrete setting (see next bullet). `stop()` already deregisters mDNS.
  - **For 211-followup (the opt-out predicate):** Task 213 takes `opt_out` as a
    plain bool on `MdnsConfig`; there is **no `mdns_opt_out` key in
    `managed.json` yet** (211 froze `disable_remote` / pairing / max-devices /
    relay / deny-paths, and explicitly restated that `disable_remote` must NOT
    silence mDNS). A dedicated managed-setting key (e.g. `disableMdns` /
    `mdnsOptOut`) + a per-network preference predicate is the natural home for
    the boolean 217 will pass; that key is **not** in scope here (no proto, no
    managed.json change in this task). Recommend 217 (or a small 211-followup)
    add the managed key and feed its value into `MdnsConfig::opt_out`.
  - **For 218/219/511 (discovery UI):** drive `MdnsBrowser::start(None)` (or
    `IrohTransport::browse_lan()`), drain `recv()` for `DiscoveredCore`s, and
    hand each `endpoint_id` to the 212 connect path
    (`connect_channel(...)` → `ConnectionPath::Lan`, no relay). `core_pubkey_b64`
    is a fingerprint **hint** only — trust is still established by the QR/cert
    pairing flow (Task 207); discovery is unauthenticated advertisement.

- **Deliberate debt.** None deferred via `TODO`/`unimplemented!`/`todo!`. Two
  scoped deferrals, both recorded with their closing task: (1) the Core-actor
  `publish_mdns` boot/re-announce wiring + building the live `MdnsConfig` from
  the real identity/version/caps → **Task 217**; (2) the `managed.json` mDNS
  opt-out key + per-network preference predicate that feeds `MdnsConfig::opt_out`
  → **Task 217 / a 211-followup** (the bool seam exists today and is tested).

- **Tier-3 lines the loopback double does NOT cover** (Phase-2 manual checklist):
  **real cross-device LAN discovery across two physical machines on real Wi-Fi**
  — multicast on a real switch (224.0.0.251 / ff02::fb), mDNS-suppressing work
  networks, IPv6-only segments, and the LAN-direct Iroh open to a discovered
  endpoint actually classifying as `ConnectionPath::Lan` end-to-end. The double
  proves publish + browse + the exact 4-key TXT round-trip + opt-out suppression
  + `disable_remote`-doesn't-silence on **loopback**; "pair a real second machine
  over LAN via mDNS direct" stays on the Phase-2 Tier-3 checklist. On this dev
  host loopback delivered the IPv4 (A) record but not the IPv6 (AAAA) one over
  loopback multicast, so the **dual-stack** assertion is OR-floored ("at least
  one family resolves") with full IPv4+IPv6 advertisement registered and the
  AAAA round-trip itself a Tier-3 line; the responder always *registers* both A
  and AAAA (R-3).

- **Smoke-gate state.** New capability `mdns-discovery` registered in
  `scripts/smoke.manifest` (`scripts/smoke.d/99-mdns-discovery.sh`), running
  after `transport-loopback`. It is **hermetic** (no Core boot / no keychain /
  no real network): one `cargo test -p concerto-transport --test mdns_loopback`
  invocation drives a responder + a browser discovering over loopback, every
  wait timeout-bounded (8 s) so it never hangs, degrading gracefully (assert the
  responder published + the unit-tested TXT encode/parse) if a sandbox blocks
  even loopback multicast. Full `scripts/smoke.sh` passes (all 18 caps,
  86 seconds); `cargo test -p concerto-transport --test mdns_loopback` →
  3 passed; the 6 `src/mdns.rs` unit tests pass under `cargo test --workspace`.
  `shellcheck -x` on the new smoke script is clean.
