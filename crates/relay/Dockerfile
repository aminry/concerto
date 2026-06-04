# Concerto self-hosted relay — single static binary image (Task 214,
# `design/11 §6.3`). The binary is the deliverable; this image just wraps it for
# the Fly.io / cloud-agnostic deploy. Build from the REPO ROOT so the whole
# workspace (Cargo.lock + the other crates the relay's path-deps need) is in
# context:
#
#   docker build -f crates/relay/Dockerfile -t concerto-relay .
#
# Configured by environment variables only (Twelve-Factor) — see `--help` or
# `design/11 §6.3`: RELAY_LISTEN_ADDR / WSS_LISTEN_ADDR (reserved, Task 215) /
# MAX_ROUTES / BANDWIDTH_CAP_PER_ENDPOINT / PROMETHEUS_LISTEN_ADDR.

# ---- builder ---------------------------------------------------------------
# Pin the Rust toolchain to the workspace MSRV floor's release line; the relay
# is pure-Rust (quinn/rustls/ring + prometheus), so a static musl target gives a
# self-contained binary with no glibc runtime dependency.
FROM rust:1.85-bookworm AS builder

# musl gives a fully static binary (no libc at runtime → scratch-able image).
RUN rustup target add x86_64-unknown-linux-musl \
    && apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
# Copy the whole workspace (the relay's path-deps + Cargo.lock for a reproducible
# dependency set). A .dockerignore at the repo root should exclude target/.
COPY . .

# Build only the relay binary, statically against musl, in release mode.
RUN cargo build --release --locked \
    --target x86_64-unknown-linux-musl \
    --bin concerto-relay -p concerto-relay

# ---- runtime ---------------------------------------------------------------
# `scratch` — nothing but the static binary. The relay opens no files, reads no
# config (env only), and forwards ciphertext (`design/11 §3.9`); it needs no
# shell, no libc, no CA bundle (the relay HTTP server is plain-HTTP behind the
# operator's TLS terminator / Fly anycast).
FROM scratch AS runtime

COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/concerto-relay /concerto-relay

# Documents the default ports (overridable via the env vars above). 80 = the
# iroh-relay HTTP server (RELAY_LISTEN_ADDR); 9090 = Prometheus
# (PROMETHEUS_LISTEN_ADDR); 443 reserved for the Task-215 WSS bridge.
EXPOSE 80
EXPOSE 9090

# Env-only config; no entrypoint args (Twelve-Factor strictness).
ENTRYPOINT ["/concerto-relay"]
