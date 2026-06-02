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
- [ ] mDNS crate chosen + pinned + `cargo deny` cleared; choice justified in Handoff
- [ ] Responder publishes `_concerto._tcp.local` with the exact 4-key TXT schema, IPv4+IPv6, clean (de)register lifecycle
- [ ] Browser discovers + parses TXT into a discovered-Core descriptor; feeds `endpoint_id` to the 212 LAN connect path
- [ ] Opt-out (managed / per-network) suppresses publication; `disable_remote = true` does NOT (asserted by test)
- [ ] FROZEN: the TXT record schema + service type + discovered-Core descriptor on `crates/transport`
- [ ] Tier-2 loopback discovery double tests pass; Verification commands pass; interfaces clean/regenerated; smoke `mdns-discovery` green
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate debt in Handoff)
- [ ] Single commit with the message below

## Outputs
- `Cargo.toml` (modified — `[workspace.dependencies]` += the mDNS crate pin with rationale)
- `crates/transport/Cargo.toml` (modified — mDNS dep)
- `crates/transport/src/mdns.rs` (new — responder + browser), `crates/transport/src/api.rs` (modified — discovered-Core descriptor + browse entry), `crates/transport/src/state.rs` (modified — `MdnsResponder` in `TransportState`)
- `crates/transport/tests/mdns_loopback.rs` (new — the two-task discovery double)
- `deny.toml` (modified only if a new SPDX needs ratification)
- `scripts/smoke.d/<NN>-mdns-discovery.sh` + `scripts/smoke.manifest` (new capability)
- `docs/interfaces/rust-api.md` (regenerated if the discovery surface changed)

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

## Handoff Notes (fill in when finishing)
- Drift from plan / mDNS crate choice + rationale / Open questions for next task / Deliberate debt / License ratifications / Smoke-gate state
