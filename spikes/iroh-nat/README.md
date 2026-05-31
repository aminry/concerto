# iroh-nat-spike — throwaway NAT-diversity harness (Task 101)

A tiny two-binary harness that measures whether **Iroh's QUIC hole-punching
reaches a DIRECT path** (vs falling back to a RELAYED path) between two
machines, across whatever real networks you can put them on. This is the
single biggest gate on Concerto V1.0's remote story (`design/00 §11`,
`design/11 §10`): the bet is **>70% direct connections across diverse real
NATs**.

This crate is **throwaway** and a **standalone Cargo workspace** (its own empty
`[workspace]` table) on purpose — it must NOT join the repo-root workspace, so
its pinned Iroh dependency never leaks into other tasks' `cargo check
--workspace` / `cargo deny check`. Build and lint it from its own manifest.

> **Pinned Iroh version: `0.98.2`** (see `Cargo.toml`). The GO/NO-GO verdict in
> `design/spikes/iroh-nat-findings.md` is only valid for this exact version.

---

## What it measures

For each connection, the `client` binary reads **Iroh's own per-path signal**
(`Connection::paths()`), not latency, and classifies the connection by its
**selected** path:

- selected path `is_ip()` → **DIRECT** (hole-punched, or LAN) — counts toward
  the >70% bar.
- selected path `is_relay()` → **RELAYED** — does not count.

It also logs every candidate path Iroh holds (direct candidates + the relay
standby), each path's RTT, and the round-trip connect time.

---

## Running a matrix row

Each row of the network matrix is one `core` ↔ `client` pair, with the two
binaries on two different machines / networks.

### 1. On the machine playing the **Core**

```sh
cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin core
```

It prints its `EndpointId` and the exact `client` command to run. Leave it
running (Ctrl-C to stop). It also logs the **home relay** it registered with —
confirm you see `home relay registered` so the relayed-fallback path is
actually reachable for this row.

### 2. On the machine playing the **client** (a different network)

Copy the `EndpointId` the core printed and run:

```sh
cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin client -- <EndpointId>
```

It prints a result block ending in a `MATRIX ROW` line:

```
PATH            : DIRECT
direct?         : YES
connect time    : 235ms
MATRIX ROW      : direct=true | relayed=false | connect_ms=235
```

Record the `direct=` / `relayed=` / `connect_ms=` values into the matrix in
`design/spikes/iroh-nat-findings.md`. Run each network pair a few times and
note the modal result.

### Relay options

- **default** (no flag): uses n0's public relays + n0 discovery. This is the
  zero-config path; discovery lets you dial a bare `EndpointId` with no
  pre-shared address — the realistic remote case. **Use this for the matrix.**
- `--relay <url>`: point both ends at your **own** throwaway relay (see below)
  so the relayed path is one you control. Pass the *same* `--relay` to both
  `core` and `client`.
- `--relay disabled`: direct-only, no relay assist. Note: with discovery's
  relay/DNS bootstrap gone, a bare `EndpointId` is not resolvable across hosts;
  this mode is only for confirming "no relay at all" behavior, not for the
  matrix.

### Settle window

Iroh starts a connection on the relay and **upgrades** to a direct path once
hole-punching completes. The harness waits up to `--settle-secs` (default 8)
for that upgrade before recording the verdict, so a slow hole-punch is still
counted as DIRECT. Bump it on high-latency links:

```sh
cargo run --manifest-path spikes/iroh-nat/Cargo.toml --bin client -- <EndpointId> --settle-secs 15
```

---

## Standing up your own throwaway relay (optional, for the relay-controlled row)

The default mode already exercises a real relay (n0's), so "relayed" is
measurable out of the box. If you want a relay **you** operate (e.g. to be sure
the relay fallback works against the binary V1.0 will self-host), run Iroh's
own relay server — the same `iroh-relay` the product will vendor (`design/11`
R-7):

```sh
# In a scratch checkout, not this workspace:
cargo install iroh-relay --version 0.98.0 --features server   # or run the published container
iroh-relay --dev                                              # dev relay on http://localhost:3340
```

Then pass `--relay http://<relay-host>:3340` to **both** `core` and `client`.
(Exact flags track the pinned `iroh-relay 0.98.x`; `iroh-relay --help` lists
them. The product relay binary is Task 214, out of scope here.)

---

## Verification (Task 101 tier: spike)

```sh
# Local pair connects (real measurement; same-host = DIRECT):
cargo run   --manifest-path spikes/iroh-nat/Cargo.toml --bin core
cargo run   --manifest-path spikes/iroh-nat/Cargo.toml --bin client -- <EndpointId>

# Harness builds clean:
cargo clippy --manifest-path spikes/iroh-nat/Cargo.toml -- -D warnings
```

The numeric verdict lives in `design/spikes/iroh-nat-findings.md`.
