# Spike findings — Iroh NAT-diversity traversal (Task 101)

| Field | Value |
|---|---|
| Spike | Phase 1, #1 (`design/00 §11`, `design/11 §10`) |
| Task | `tasks/v1.0/101-spike-iroh-nat-diversity.md` |
| Harness | `spikes/iroh-nat/` (throwaway standalone Cargo workspace) |
| **Pinned Iroh version** | **`iroh = 0.98.2`** (exact; the verdict is only valid for this version) |
| Companion crates at this pin | `iroh-relay 0.98.0`, `iroh-base 0.98.0`; `tonic-iroh-transport 0.9.2` (Task 102) resolves against this same `iroh 0.98.2` |
| The bar | **>70% direct = GO** · 60–70% = MARGINAL · <60% = NO-GO (contingency: tsnet sidecar) |
| Verdict | **PENDING OPERATOR FIELD MEASUREMENT** (see §5) |
| Date | 2026-05-30 |

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

| # | Pair (core ↔ client) | Path (DIRECT/RELAYED) | connect ms | Status |
|---|---|---|---|---|
| 0 | **loopback / same-host** (client→core, one machine) | **DIRECT** | **~235 ms** | **MEASURED (this env)** — see §4.1 |
| 1 | home NAT ↔ home NAT (same router, two machines) | — | — | **PENDING — operator field measurement** (two machines behind one residential router) |
| 2 | home NAT ↔ home NAT (two *different* residential ISPs) | — | — | **PENDING — operator field measurement** (two homes / two ISPs) |
| 3 | home ↔ cellular (phone hotspot / CGNAT) | — | — | **PENDING — operator field measurement** (LTE/5G hotspot, carrier-grade NAT) |
| 4 | home ↔ corporate / VPN | — | — | **PENDING — operator field measurement** (corporate Wi-Fi or VPN egress) |
| 5 | behind **symmetric NAT** | — | — | **PENDING — operator field measurement** (a network whose NAT is symmetric; the hardest case for hole-punching) |
| 6 | **UDP-blocking ISP** (relay-over-TCP fallback, R-8) | — | — | **PENDING — operator field measurement** (an ISP/network that blocks UDP; validates Iroh's relay-over-TCP / port-443 fallback) — **unmeasured-with-reason: this env has no UDP-blocking network** |

**Aggregate direct-% across diverse real NATs: PENDING** — not computable until
rows 1–6 are measured. (Row 0 alone is not a NAT-diversity sample and must not
be treated as the aggregate.)

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
  hole-punches. The >70% bar is about diverse real NATs (rows 1–6) and stays
  **PENDING**.

## 5. Provisional verdict — **PENDING OPERATOR FIELD MEASUREMENT**

> **This is NOT a final GO / NO-GO / MARGINAL.** The aggregate direct-%
> bar — **>70% GO · 60–70% MARGINAL · <60% NO-GO** — **can only be decided
> once the operator runs the harness across the real network matrix (rows 1–6
> above).** From this single-machine / single-network environment the field
> metric is not obtainable, and no field numbers were invented.

What is established now:

- The harness is real and runnable by the operator across their networks
  (see `spikes/iroh-nat/README.md` for per-row invocation).
- On the one pair this environment can reach (loopback), Iroh selects a
  **DIRECT** path and a real relay registers as fallback — i.e. the mechanism
  the bar depends on (direct path preferred, relay available as fallback) is
  functioning at the pinned version.
- Iroh `0.98.2` is pinned; the verdict, once measured, is valid only for this
  version.

**Operator action at the Phase-1 gate:** run rows 1–6, fill the matrix, compute
the aggregate direct-%, then record the final verdict here:

- **≥70% direct → GO.** Iroh is the sole non-browser transport; Phases 2 and 5
  proceed as planned.
- **60–70% → MARGINAL.** Proceed but ship the "relayed" indicator prominently
  and keep the contingency on the table.
- **<60% → NO-GO → contingency (operator decision).** Add a **tsnet Go sidecar**
  the Rust Core supervises for stubborn networks (`design/11 §3.8`, R-1). This
  costs a Go process + Tailscale/Headscale account friction (against the
  "no accounts" principle, PRD §16.1) and is a follow-on the operator triggers,
  **not** something to pre-build. Building the sidecar is explicitly out of
  scope for this spike (`tasks/v1.0/101 §Scope — out`).

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
