# tonic-iroh-spike — throwaway Tonic-over-Iroh benchmark (Task 102)

A tiny harness that serves the **same production Tonic gRPC stack**
(`tonic 0.12` / `prost 0.13`) over three transports and benchmarks each, to
answer Task 102's two bars (`design/11 §10`): **unary within ~30% of UDS** and
**`session.io` streaming > 1 MB/s** over Iroh.

The numeric verdict lives in `design/spikes/tonic-iroh-findings.md`.

This crate is **throwaway** and a **standalone Cargo workspace** (its own empty
`[workspace]` table) on purpose — it must NOT join the repo-root workspace, so
its pinned Iroh dependency never leaks into other tasks' `cargo check
--workspace` / `cargo deny check`. Build and lint it from its own manifest.

> **Pins:** `tonic =0.12.3`, `prost =0.13.5` (production, root `Cargo.toml`),
> `iroh =0.98.2`, `iroh-relay =0.98.0` (Task 101). We deliberately do **not**
> use `tonic-iroh-transport` (it forces `tonic 0.14.6`, conflicting with the
> production tonic 0.12 pin) — the adapter is hand-rolled. See the findings doc.

---

## What it measures

Three transports, same `Bench` service (one unary `Echo`, one server-streaming
`Firehose`):

- **UDS** — bare Unix-domain socket (baseline).
- **Iroh-direct** — two Iroh endpoints on one host, **relays disabled**, so the
  only viable QUIC path is the direct (loopback) IP path.
- **Iroh-relay** — two Iroh endpoints with **IP transports cleared**, both
  pointed at a **local in-process `iroh-relay` dev instance**, forcing every
  byte through the relay. No external relay binary needed — the relay runs
  in-process (plain-HTTP, OS-assigned loopback port), the hermetic equivalent of
  `iroh-relay --dev`.

For each: unary round-trip **p50 / p95** and streaming **MB/s**.

## Running

```sh
# All three transports, table + GO/NO-GO:
cargo run --release --manifest-path spikes/tonic-iroh/Cargo.toml

# Heavier run (more iterations, more streamed bytes):
cargo run --release --manifest-path spikes/tonic-iroh/Cargo.toml -- \
    --unary-iters 5000 --warmup 500 --stream-mb 128

# Lint:
cargo clippy --manifest-path spikes/tonic-iroh/Cargo.toml -- -D warnings
```

Flags: `--unary-iters` (timed echo calls), `--warmup` (excluded calls),
`--unary-payload` (echo bytes), `--stream-mb` (total firehose MB),
`--chunk-bytes` (firehose chunk size, default 1 MiB to match `design/10 §5.2`).

The output prints a per-transport line, a results table with the
**p50 ÷ UDS ratio**, and a GO/NO-GO block. Note the table reports BOTH the raw
loopback ratio AND the **additive overhead** (Iroh p50 − UDS p50) — the additive
figure is what actually transfers to real networks, where it sits on top of LAN
(<100 ms) / WAN (<250 ms) RTT. See the findings doc for the interpretation.

## What it does and does NOT measure

- **Does:** the real production tonic-0.12 / prost-0.13 codegen over a
  hand-rolled Iroh-bidi-stream → tokio-duplex adapter; Iroh's built-in TLS 1.3
  encryption (it is ON in every Iroh number); a real relayed gRPC path through a
  local relay.
- **Does NOT:** the second Noise IK layer (`design/12` / Task 208 — stubbed
  here, `Scope — out`); a **real WAN / real relay** (the local relay has zero
  RTT and zero bandwidth limit — that row is PENDING operator field
  measurement); reconnect / ring-buffer semantics (Task 202).

## Layout

- `proto/bench.proto` — the throwaway `Bench` service (Echo + Firehose).
- `build.rs` — compiles it with `tonic-build 0.12`.
- `src/iroh_adapter.rs` — the hand-rolled Tonic-over-Iroh adapter (`IrohDuplex`,
  `IrohConnector`). The load-bearing part; friction notes in the findings doc.
- `src/lib.rs` — service impl, endpoint construction (direct / relay), the
  in-process dev relay, and the Iroh gRPC server.
- `src/bin/bench.rs` — the driver: UDS transport + measurement loop + table.
