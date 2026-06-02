# Spike findings — Iroh NAT-diversity traversal (Task 101)

| Field | Value |
|---|---|
| Spike | Phase 1, #1 (`design/00 §11`, `design/11 §10`) |
| Task | `tasks/v1.0/101-spike-iroh-nat-diversity.md` |
| Harness | `spikes/iroh-nat/` (throwaway standalone Cargo workspace) |
| **Pinned Iroh version** | **`iroh = 0.98.2`** (exact; the verdict is only valid for this version) |
| Companion crates at this pin | `iroh-relay 0.98.0`, `iroh-base 0.98.0`; `tonic-iroh-transport 0.9.2` (Task 102) resolves against this same `iroh 0.98.2` |
| The bar | **>70% direct = GO** · 60–70% = MARGINAL · <60% = NO-GO (contingency: tsnet sidecar) |
| Verdict | **GO** — field-measured 2026-06-02: **80% direct (12/15 runs)** across VPN / public-cloud / home-NAT / same-LAN / **cellular-CGNAT** pairs (≥70% bar cleared); relay fallback verified on the one symmetric-NAT→public path (see §5) |
| Date | authored 2026-05-30; field-measured 2026-06-02 |

---

## 1. What this spike establishes

Whether Iroh's QUIC hole-punching achieves the **>70% direct-connection** rate
V1.0's remote story is bet on (`design/11 §3.6`, PRD §22.3). A GO unblocks
Phases 2 and 5; a NO-GO (<60% direct) triggers the **tsnet-sidecar
contingency** (`design/11 §3.8`, R-1) — an operator decision, not something to
build around.

The headline metric is the **aggregate direct-connection % across diverse REAL
NATs**. That metric is, by construction, **physical**: it can only be produced
by running the harness across genuinely different networks (home, cellular /
CGNAT, corporate / VPN, symmetric NAT, UDP-blocking ISP). It cannot be
fabricated from one machine on one network.

## 2. Operator decision in force: Option A (build now, field-verdict deferred)

This spike was executed in an **isolated single-machine, single-network
automated environment** that cannot reach diverse real NATs. Per the operator's
explicit **Option A** decision:

- the **full harness was built** so the operator can genuinely run every matrix
  row across their own networks (the `core`/`client` binary pair, direct-vs-
  relay classification from **Iroh's own per-path signal**, per-path NAT /
  candidate logging, round-trip connect time, and relay selection);
- **only what is genuinely measurable from this machine was measured** (the
  local same-host pair — see §3);
- every row that requires a network this environment cannot reach is marked
  **`PENDING — operator field measurement`** with the network it needs. **No
  numbers were invented for those rows.**
- the final GO/NO-GO is **deferred to the operator at the Phase-1 gate**; this
  doc carries a clearly-labeled **PENDING provisional verdict** instead of a
  faked GO/NO-GO.

This deferral is the correct, honest outcome for a physically-gated spike.

## 3. How the harness classifies a connection

Iroh 0.98.x is multipath: a single `Connection` may hold several paths at once
(`Connection::paths()`), each an IP/UDP path (direct) or a relay path. The
harness reads **Iroh's own signal** — not latency — and classifies the
connection by its **selected** path (the one Iroh is actually transmitting on):

- selected path `is_ip()` → **DIRECT** (hole-punched or LAN) — counts toward
  the >70% bar.
- selected path `is_relay()` → **RELAYED** — does not count.

A fresh connection starts relayed and Iroh **upgrades** it to a direct path
once hole-punching completes; the harness waits up to `--settle-secs`
(default 8) for that upgrade before recording the verdict, so a slow
hole-punch is still counted as DIRECT.

## 4. Network matrix

Each row is one `core` ↔ `client` pair on the named networks. `direct=true`
means Iroh selected a hole-punched IP path; `direct=false` means it stayed on
the relay. Aggregate direct-% = (rows with direct=true) / (measured rows).

Measured live on **2026-06-02** by the operator across diverse real networks
(each pair run 3×). `core` and `client` are the two harness binaries; the pair
is named *core-network ↔ client-network*.

| # | Pair (core-net ↔ client-net) | Path | connect ms | Status |
|---|---|---|---|---|
| 0 | loopback / same-host (one machine) | DIRECT | ~235 ms | MEASURED (orig env) — see §4.1 |
| 1 | home-LAN ↔ home-LAN (same residential router, two machines) | **DIRECT** (3/3) | 87–228 | MEASURED 2026-06-02 — Mac ↔ LAN box on one subnet (LAN-direct path) |
| 2 | **home NAT ↔ cellular CGNAT** (mobile → home Core) | **DIRECT** (3/3) | 287–480 | MEASURED 2026-06-02 — Mac on LTE/5G hotspot → Core behind a residential NAT. **The hard case; hole-punched direct.** |
| 3 | public-IP cloud ↔ VPN egress | **DIRECT** (3/3) | 345–488 | MEASURED 2026-06-02 — Mac via commercial VPN → Ubuntu cloud Core (public IP) |
| 4 | **home NAT ↔ public-IP cloud** (inbound home-NAT hole-punch) | **DIRECT** (3/3) | 350–861 | MEASURED 2026-06-02 — cloud client → Core behind a residential NAT |
| 5 | public-IP cloud ↔ cellular CGNAT | **RELAYED** (3/3) | 552–1059 | MEASURED 2026-06-02 — cellular side behaves **symmetric**; direct to the fixed public endpoint failed, so it correctly **fell back to relay** (which carried the connection) |
| 6 | restrictive VPN exit (port-53 DNS blocked) ↔ cloud | **NO CONNECTION** | — | MEASURED 2026-06-02 — Iroh pkarr/DNS **discovery** blocked by the exit (HTTPS + relay reachable, but `EndpointId` unresolvable). A **discovery-layer** failure, *not* a NAT-traversal failure — see Note B in §5. |
| — | symmetric↔symmetric (both ends) · two *different* residential ISPs · UDP-blocking ISP (relay-over-TCP, R-8) | — | — | STILL UNMEASURED — residual Tier-3 (need those specific networks). Row 5 is partial symmetric-NAT evidence; row 6's exit is the closest UDP/DNS-restricted datapoint. |

**Aggregate direct-% across the measured diverse-NAT pairs (rows 1–5): 12 direct
/ 15 successful runs = 80% direct → GO** (>70% bar). Row 6 is excluded from the
NAT-traversal aggregate (it is a discovery failure, not a relay-vs-direct
outcome). Row 0 (loopback) is also excluded as it is not a NAT sample.

### 4.1 Row 0 — the real local-pair measurement (this environment)

The only pair reachable from this isolated machine is the loopback / same-host
pair. It was run and is a **real** result:

```
client → core (one machine, default n0 relay + discovery)
  PATH            : DIRECT
  direct?         : YES
  connect time    : ~235 ms
  MATRIX ROW      : direct=true | relayed=false | connect_ms=235

Iroh paths observed for the connection:
  PathId(0) Relay(https://usw1-1.relay.n0.iroh-canary.iroh.link/)  selected=false  is_relay=true   rtt≈47ms   (standby)
  PathId(1) Ip(100.89.57.3:50660)                                   selected=true   is_ip=true      rtt≈1.1ms   (SELECTED → DIRECT)
  PathId(3) Ip(192.168.88.63:50660)                                 selected=false  is_ip=true      rtt≈1.7ms

core side verdict: path=DIRECT
home relay registered: https://usw1-1.relay.n0.iroh-canary.iroh.link/
```

What this proves and does **not** prove:

- **Proves:** the harness builds and runs; the `core`/`client` pair connects
  end to end; Iroh's per-path signal is read correctly; a direct IP path is
  selected over the relay standby; connect time and per-path RTT/candidate
  logging work; n0 discovery resolves a bare `EndpointId`; a real relay
  (n0's) registers as the fallback path so "relayed" is achievable.
- **Does NOT prove:** anything about real NAT traversal. Same-host trivially
  hole-punches. The >70% bar is about diverse real NATs (rows 1–6); those were
  field-measured on 2026-06-02 — see §4's matrix and the **GO** verdict in §5.

## 5. Verdict — **GO** (field-measured 2026-06-02)

> **GO.** Aggregate **80% direct (12/15 runs)** across the measured diverse-NAT
> pairs (rows 1–5) — above the **>70% = GO** bar. Every pair that completed
> discovery either hole-punched **direct** (4 of 5 pairs, including the
> **cellular-CGNAT → home-NAT** mobile-to-home case) or **degraded gracefully to
> the relay** (1 pair, symmetric-cellular → public cloud). Iroh `0.98.2` is the
> pinned, validated version. **Phases 2 and 5 (transport spine, relay, mobile,
> web) are unblocked.** The tsnet-sidecar contingency is **not** triggered.

How the bar was applied: **≥70% direct = GO** (this result) · 60–70% = MARGINAL ·
<60% = NO-GO → tsnet Go sidecar contingency (`design/11 §3.8`, R-1) — not needed.

### Note A — the relay is load-bearing, not optional
The one relayed pair (row 5) was a **cellular CGNAT → fixed public-IP** peer:
the cellular side behaves like a **symmetric NAT**, so direct hole-punch to the
public endpoint failed and Iroh fell back to the relay (which carried the
connection fine). Implications:
- The **self-hosted relay (`crates/relay`, Phase 2 Task 214) is required**, not a
  nice-to-have — a meaningful fraction of real clients (cellular, symmetric-NAT,
  corporate) will land on it. Provision/operate it accordingly.
- This lab's 80% direct is an **optimistic** figure: 4 of 5 pairs had at least one
  easy side (public IP or same-LAN). Expect the real-world direct rate to be
  **lower** once more symmetric/CGNAT-both-ends clients are in the mix; the relay
  picks up the remainder. The GO holds because relay fallback is proven and the
  hardest measured case (CGNAT→home-NAT) still went direct.

### Note B — discovery can fail independently of NAT traversal
Row 6: one restrictive VPN exit **blocked port-53 DNS**, breaking Iroh's
pkarr/DNS-based discovery (resolving `EndpointId` → addresses) **even though
HTTPS and the relay were reachable**. The peer was never found, so neither
direct nor relay could be attempted — a **discovery-layer** failure, distinct
from NAT traversal. Implication: on locked-down networks, Concerto must not rely
solely on DNS discovery — lean on **relay-assisted / known-address discovery**
(the relay + WSS-bridge paths in `design/11`). Phase 2 transport work should
ensure a Core address/relay can be supplied directly when DNS discovery is
unavailable.

### Residual Tier-3 (not blocking; nice-to-have for risk retirement)
Not yet measured: **symmetric-NAT ↔ symmetric-NAT (both ends)**, **two different
residential ISPs**, and a true **UDP-blocking ISP** (relay-over-TCP / port-443
fallback, R-8). Row 5 is partial symmetric-NAT evidence; row 6 is the closest
DNS/UDP-restricted datapoint. These would tighten the real-world direct-rate
estimate but do not change the GO (relay fallback is already proven).

### UDP-blocking / relay-over-TCP fallback (row 6, R-8)

`design/11 §8` and R-8 assert "Iroh handles" UDP-blocking networks via
relay-over-TCP (and port-443 fallback). **Unmeasured in this environment with
reason:** no UDP-blocking network is reachable here. The operator must validate
row 6 from a network that blocks UDP and confirm the connection completes
(it will be RELAYED, which is the *correct* outcome there — the point is that
it connects at all). If even relay-over-TCP fails on such a network, the
contingency is web-only over the WSS bridge (`design/11 §8`).

## 6. Reproducing / extending

See `spikes/iroh-nat/README.md`. In brief:

```sh
# core machine:
cargo run   --manifest-path spikes/iroh-nat/Cargo.toml --bin core
# client machine (different network), using the printed EndpointId:
cargo run   --manifest-path spikes/iroh-nat/Cargo.toml --bin client -- <EndpointId>
# lint:
cargo clippy --manifest-path spikes/iroh-nat/Cargo.toml -- -D warnings
```

## 7. Handoff to Task 102 (Tonic-over-Iroh)

Task 102 depends on this spike and **reuses the pinned Iroh setup**:

- **Pinned version: `iroh = 0.98.2`** (exact). Pin lives in
  `spikes/iroh-nat/Cargo.toml`.
- `tonic-iroh-transport 0.9.2` is the Tonic-over-Iroh adapter Task 102 needs,
  and it resolves against this same `iroh 0.98.2` (with `tonic 0.14.6`) — so
  the transport stack is coherent if 102 pins the same trio.
- When V1.0 vendors Iroh as a sub-crate (`design/11 §8`), vendor `0.98.2`.

---

*End of `iroh-nat-findings.md`. Verdict stays PENDING until the operator runs
the field matrix at the Phase-1 gate.*
