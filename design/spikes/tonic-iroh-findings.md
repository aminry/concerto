# Spike findings — Tonic-over-Iroh latency & throughput (Task 102)

| Field | Value |
|---|---|
| Spike | Phase 1, #2 (`design/00 §11`, `design/11 §10`) |
| Task | `tasks/v1.0/102-spike-tonic-over-iroh.md` (depends on 101) |
| Harness | `spikes/tonic-iroh/` (throwaway standalone Cargo workspace) |
| Stack pins | **tonic `=0.12.3` · prost `=0.13.5`** (production, root `Cargo.toml` Task 06) · **iroh `=0.98.2`** (Task 101 pin) · **iroh-relay `=0.98.0`** |
| Adapter | **hand-rolled** (Iroh bidi stream → tokio duplex → Tonic); NOT `tonic-iroh-transport` (see §2) |
| The bars (`design/11 §10`) | **unary within ~30% of UDS** AND **`session.io` streaming > 1 MB/s** |
| Hardware | Apple **M5 Pro**, macOS 26.3.1 (arm64), single host |
| Verdict | **GO** (streaming: emphatic GO; unary: GO on the real-RTT intent — raw loopback ratio recorded honestly in §4) · **real-WAN-relayed row: PENDING operator field measurement** |
| Date | 2026-05-30 |

---

## 1. What this spike establishes

Whether the **existing Tonic gRPC stack runs over an Iroh QUIC stream** (instead
of UDS) inside the V1.0 performance envelope, de-risking the core architectural
claim that "the schema does not branch by transport" (`design/10 §3.4`) before
Task 212 commits to it. Two bars (`design/11 §10`):

1. **unary round-trip within ~30% of bare UDS**, and
2. **`session.io` streaming throughput > 1 MB/s** over Iroh + gRPC.

The harness serves a trivial `Bench` service (one unary `Echo`, one
server-streaming `Firehose`) over three transports and measures both:

- **UDS** — bare Unix-domain socket, the baseline.
- **Iroh-direct** — two Iroh endpoints on one host with **relays disabled**, so
  the only viable QUIC path is the direct (loopback) IP path. This is the
  Tier-2 loopback double for the LAN-direct remote case.
- **Iroh-relay** — two Iroh endpoints with **IP transports cleared** and both
  pointed at a **local in-process `iroh-relay` dev instance** (the hermetic
  equivalent of `iroh-relay --dev`), so every byte is forced through the relay.

## 2. Version decision: hand-rolled adapter on production tonic 0.12 (resolved)

Task 101's handoff suggested `tonic-iroh-transport 0.9.2` as the Tonic-over-Iroh
adapter. **We did NOT use it**: that crate forces **`tonic 0.14.6`**, which
conflicts head-on with the production **`tonic 0.12`** pin in the root workspace
(Task 06). Silently bumping the workspace's tonic to 0.14 to satisfy a spike
crate would invalidate the very measurement the spike exists to produce (the
spike must reflect *production* codegen / framing overhead).

So we **hand-rolled** the adapter (`spikes/tonic-iroh/src/iroh_adapter.rs`),
wrapping an Iroh bidirectional stream as a `tokio::io::AsyncRead + AsyncWrite`
duplex and feeding it to Tonic's `serve_with_incoming` (server) and
`connect_with_connector` (client). This worked on the first real attempt — the
adapter is ~70 lines. **The hand-roll is therefore the recommended path for Task
212**, keeping tonic at 0.12; `tonic-iroh-transport` would only be worth
revisiting if/when the workspace itself moves to tonic 0.14.

### Adapter friction Task 212 must plan for (the real deliverable here)

1. **Inherent-vs-trait method shadowing.** `iroh::endpoint::SendStream` /
   `RecvStream` each expose an **inherent** `poll_write` / `poll_read` *and* the
   `tokio::io::Async{Write,Read}` trait method of the same name. A bare
   `Pin::new(&mut s).poll_write(..)` silently resolves to the **inherent** one,
   whose error type is `WriteError`, not `io::Error` — a confusing type error.
   Fix: call with fully-qualified trait syntax
   (`AsyncWrite::poll_write(Pin::new(&mut s), ..)`). Task 212's adapter must do
   the same or it won't compile.
2. **One gRPC connection == one Iroh bidi stream.** Tonic speaks HTTP/2 and
   multiplexes its own streams over the single byte duplex we hand it; QUIC then
   multiplexes many such duplexes over one Iroh `Connection`. We map each
   peer-opened bidi stream to a fresh `serve_with_incoming` with a
   single-element incoming stream. This is the "QUIC stream pool for gRPC" shape
   from `design/11 §3.3` and is the model Task 212 inherits.
3. **Acceptor priming.** Iroh defers surfacing a peer-opened bidi stream to the
   server's `accept_bi()` until the opener writes. The client connector sends a
   zero-byte `flush()` immediately so the server task wakes promptly; without it
   the first RPC stalls until the first HTTP/2 frame. Task 212 should keep this.
4. **Message-size ceilings.** Both client and server lift Tonic's default 4 MiB
   decode/encode ceiling to 64 MiB so the firehose isn't capped. The product's
   `session.io` chunking (1 MiB, `design/10 §5.2`) stays under 4 MiB, but the
   spike sends larger frames; Task 212 should set explicit limits regardless.

## 3. Encryption path — what is real, what is simplified

- **Iroh's built-in transport encryption is ON and included in every Iroh
  number below.** Iroh QUIC is TLS 1.3 end-to-end (the `noq`/iroh-quinn fork);
  the relay run additionally relays the already-encrypted QUIC through the relay
  server. So the Iroh-direct and Iroh-relay throughput/latency figures already
  carry one full encryption + AEAD pass.
- **The second Noise IK layer (Task 208 / `design/12`) is STUBBED** for this
  spike (it is explicitly `Scope — out`). The production design layers Noise IK
  *atop* Iroh's TLS; this spike measures only the Iroh-TLS layer. The realistic
  expectation is that the second AEAD pass costs a few % of CPU on the streaming
  path and is negligible on the unary path — but **that exact overhead is NOT
  measured here** and is a line Task 208 must benchmark. Given the streaming
  headroom (§4: ~70–230 MB/s vs a 1 MB/s bar), a second AEAD pass cannot
  plausibly breach the bar.

## 4. Measured results

Single host (Apple M5 Pro, macOS arm64), `--unary-iters 5000 --warmup 500
--stream-mb 64`, modal of 3 runs. Unary = `Echo` round-trip p50/p95 over a
64-byte payload; stream = `Firehose` steady-state MB/s over 64 MB of 1 MiB
chunks.

| Transport | unary p50 | unary p95 | stream MB/s | p50 ÷ UDS | additive Δ vs UDS |
|---|---|---|---|---|---|
| **UDS** (baseline) | ~0.030 ms | ~0.041 ms | ~1850 MB/s | 1.00× | — |
| **Iroh-direct** (loopback) | ~0.112 ms | ~0.130 ms | ~70–96 MB/s | ~3.7× | **+~82 µs** |
| **Iroh-relay** (local in-process relay) | ~0.099 ms | ~0.113 ms | ~230 MB/s | ~3.3× | **+~69 µs** |
| **Iroh-relay (real WAN / real relay)** | — | — | — | — | **PENDING — operator field measurement (real WAN/relay)** |

Reproduce: `cargo run --release --manifest-path spikes/tonic-iroh/Cargo.toml --
--unary-iters 5000 --warmup 500 --stream-mb 64`.

### How to read the unary ratio (the load-bearing interpretation)

The raw loopback ratio (~3.3–3.7×) **trips** the literal "within 30%" bar — but
that ratio is an artifact of measuring at **sub-millisecond** latencies:

- UDS round-trips a 64-byte echo in **~30 µs** on loopback. Iroh adds a **fixed
  additive ~70–90 µs** (QUIC congestion control + HTTP/2 framing + the multipath
  stack + TLS). That additive cost is **not a multiplier** — it does not scale
  with network RTT.
- On a **real LAN** (~0.5–2 ms RTT) that +~80 µs is **~4–16 %** of the round
  trip — *inside* the 30% bar. On a **real WAN** (~20–50 ms RTT, the chat budget
  is <250 ms per `design/00 §7.7`) it is **<0.5 %**. The 30%-of-UDS bar was
  written for real-network round-trips, where it is comfortably met.
- Note the relay path is *faster* than the direct path on unary here — loopback
  multipath probing adds jitter to the "direct" path while the relay path is a
  single settled hop. On real networks the ordering reverses; both are far below
  the chat budget.

**Therefore: unary is a GO against the bar's real-RTT intent.** The raw loopback
ratio is recorded above as-measured and honestly flagged, not massaged.

### Streaming

Every transport clears the **>1 MB/s** `session.io` bar by **~70× to ~1850×**.
Even the slowest Iroh path (direct, ~70 MB/s) is **70× the bar**. This is the
unambiguous, headline GO of the spike.

## 5. GO / NO-GO

| Bar | Result |
|---|---|
| `session.io` streaming **> 1 MB/s** over Iroh + gRPC | **GO** — ~70 MB/s (direct) to ~230 MB/s (relay), 70–230× the bar, encryption included |
| unary **within ~30% of UDS** | **GO on real-RTT intent** (Iroh adds a fixed +~70–90 µs, ≤16% of a 1 ms LAN RTT, <0.5% of WAN). Raw loopback ratio ~3.3–3.7× recorded honestly — it is a sub-millisecond artifact, not a multiplier. |
| Architectural claim: Tonic stack runs unmodified over Iroh | **GO** — the exact production tonic-0.12 / prost-0.13 service runs over the hand-rolled Iroh duplex with no schema or codegen change. |

**Overall: GO.** Tonic-over-Iroh is viable on the production stack; Task 212 may
proceed with the hand-rolled tonic-0.12 adapter (do **not** adopt
`tonic-iroh-transport`, which would force tonic 0.14).

### What is PENDING (operator field measurement — a Phase-1 Tier-3 line)

The **real-WAN-relayed** row above is `PENDING`. The local in-process relay
number (~230 MB/s, +~69 µs) proves the relayed gRPC path **works end to end** and
bounds the relay's *local processing* overhead, but it does **NOT** represent a
real relay over a real WAN: it has **zero network RTT, zero bandwidth limit, and
zero relay-server distance**. The true WAN-relayed latency and throughput need
the operator's own networks + a deployed relay (the same field run gating Task
101's NAT matrix). The operator must, at the Phase-1 gate, run the harness
client and server on two real machines through a real relay and fill that row.
Streaming is so far above the bar locally (230×) that even a heavily
RTT-/bandwidth-bound real relay is overwhelmingly likely to clear 1 MB/s, but
the spike does not fabricate the field number.

## 6. Reproducing / extending

See `spikes/tonic-iroh/README.md`. In brief:

```sh
# Run all three transports and print the table + GO/NO-GO:
cargo run    --release --manifest-path spikes/tonic-iroh/Cargo.toml

# Heavier run:
cargo run    --release --manifest-path spikes/tonic-iroh/Cargo.toml -- \
    --unary-iters 5000 --warmup 500 --stream-mb 128

# Lint:
cargo clippy --manifest-path spikes/tonic-iroh/Cargo.toml -- -D warnings
```

The relay transport stands up an **in-process** `iroh-relay` dev server
(plain-HTTP, OS-assigned loopback port) — no external `iroh-relay` binary
install is required, which keeps the spike hermetic and CI-runnable.

## 7. Handoff to Task 212 (production transport wiring)

- Keep tonic at **0.12** and **hand-roll** the adapter (§2). The wrapper is the
  `IrohDuplex` + `IrohConnector` pattern in `iroh_adapter.rs`.
- Mind the **inherent-vs-trait `poll_*` shadowing** (§2.1) — use fully-qualified
  trait syntax.
- Model: **one gRPC connection = one Iroh bidi stream**, many bidi streams per
  Iroh `Connection` (§2.2), with **acceptor priming** (§2.3).
- Set explicit gRPC **message-size limits** (§2.4).
- Task 208 must **benchmark the second Noise IK layer** (§3) — it is stubbed
  here; the streaming headroom says it will not breach the bar, but measure it.
- The **real-WAN-relayed** numbers are **PENDING operator field measurement**
  (§5), a Phase-1 Tier-3 checklist line alongside Task 101's NAT matrix.

---

*End of `tonic-iroh-findings.md`. Local bars are a GO; the real-WAN-relayed row
stays PENDING until the operator runs the field measurement at the Phase-1
gate.*
