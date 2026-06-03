# Task 208 — Noise IK Session Layer (AES-256-GCM, rekey 1 GB / 1 h) + `snow` Vectors + `validate_cert` Fuzz

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 205 |
| Touches subsystem(s) | 12 (Security & Identity), 11 (Transport — session crypto) |
| Smoke gate | unchanged |

## Goal
Implement the **inner Noise IK session layer** that wraps every Iroh connection in a second AEAD with a *different* authentication root than Iroh's TLS. After pairing, each connect runs a Noise **IK** handshake (initiator static = device key, responder static = Core identity key; the initiator already has the Core's static key from its `DeviceCert`), derives AES-256-GCM session keys + a BLAKE2b transport hash, and encrypts subsequent payloads — **rekeying every 1 GB or 1 h, whichever first**, dropping the connection on rekey/replay overflow. This is the exact layer **stubbed** in spike 102 (`design/spikes/tonic-iroh-findings.md` §3): the spike measured only Iroh's TLS pass and explicitly deferred the second-AEAD overhead measurement to this task — so this task must **benchmark** that overhead on both unary and streaming and confirm it does not breach the `> 1 MB/s session.io` bar (the spike's ~70–230 MB/s streaming headroom says it won't, but the spike did not measure it). The task also adds the `snow` known-answer test vectors and a `cargo-fuzz` target on `validate_cert`. After this task the session-crypto primitive is real and CI-proven against `snow` vectors over loopback; the live cross-device Noise session is exercised by Task 212/220 / Tier-3.

## Inputs to read before starting
- `design/12_Security_Identity.md` §3.4 — the **Noise IK design** (reproduce exactly): pattern `-> e, es, s, ss` / `<- e, ee, se`; **initiator static = device key**, **responder static = Core identity key**; the client has the Core's static from the `DeviceCert` (that is why IK, not XK — one round trip cheaper, `§12 R-4`); after the handshake derive **AES-256-GCM** session keys + a **BLAKE2b transport hash**; the **double-encrypt rationale** (Noise inside QUIC+TLS = two different auth roots; if Iroh's relay/endpoint is compromised the inner Noise still holds — the QR scan is the trust root).
- `design/12_Security_Identity.md` §6.3 — the **session-key lifecycle** (the FROZEN thresholds): new session per Iroh connection; **rekey every 1 GB OR 1 h, whichever first**; on rekey failure or **replay-counter overflow** drop the connection (client reconnects); **session keys never touch disk**.
- `design/12_Security_Identity.md` §5.1 (`establish_noise_session(transport: NoiseTransport) -> Result<NoiseSession>` — the `SecurityHandle` method this task realizes), §7.2 (the authenticated-remote-call sequence: `Noise IK init (device key, expect core_pub)` → `establish_noise_session` → `NoiseSession`, then gRPC frames flow Noise-encrypted), §10 (the **testing matrix** rows this task owns: unit "Noise IK against `snow` test vectors"; security "Fuzz `validate_cert` with malformed input — cargo-fuzz"; security "Replay attack … assert rejection"), §12 R-4 (IK over XK) and R-9 (rekey trigger: time 1 h AND data 1 GB, whichever first).
- `design/spikes/tonic-iroh-findings.md` §3 (**critical** — the Noise IK layer was STUBBED there; the second AEAD pass is "a few % of CPU on streaming, negligible on unary, but NOT measured here" and is "a line Task 208 must benchmark") and §7 ("Task 208 must benchmark the second Noise IK layer … measure it"). §4 (the measured baselines: UDS unary ~30 µs, Iroh-direct streaming ~70–96 MB/s, relay ~230 MB/s — the headroom your benchmark contextualizes against the 1 MB/s bar).
- `design/11_Remote_Transport_Relay.md` §3.3 (the QUIC-stream / transport model the Noise layer sits inside — for understanding where `NoiseTransport`/`NoiseSession` plug in; Task 212 does the actual Iroh wiring).
- `tasks/v1.0/205-identity-crypto-primitives.md` (+ Handoff) — the `crates/identity` layout, the `verify_cert(raw, core_pub)` signature the fuzz target hammers, the Ed25519 key types, and the canonical-CBOR shape (205 noted the crate is "shaped so Task 208 can drop a `cargo-fuzz` target on `verify_cert`").
- `tasks/v1.0/207-pairing-noise-xx.md` (+ Handoff, if merged) — the `snow` dependency is already added to `crates/identity` and `deny.toml` by 207 (Noise XX); the chosen `snow` version pin and its ratification. **If 207 is not yet merged when this runs, add `snow` per 207's notes and ratify it.** (208 depends on 205, not 207 — but both use `snow`; coordinate the single workspace pin.)
- `deny.toml` — for the `cargo-fuzz` toolchain deps (`libfuzzer-sys`/`arbitrary`) and any new SPDX; the dated-ratification comment style.

## Scope — in
- **Noise IK wrapper** in `crates/identity` (e.g. `src/noise_ik.rs`, alongside 207's `noise_xx.rs`): a `snow`-based IK implementation with the `§3.4` roles (initiator static = device key, responder static = Core static; initiator pre-loads the responder's static key). Two-sided establishment over a caller-supplied byte channel yielding a `NoiseSession`. Document the **Noise protocol string** (e.g. `Noise_IK_25519_AESGCM_BLAKE2b` — match `snow`'s exact cipher/hash token names) and FREEZE it.
- **`NoiseSession`** type: the post-handshake transport with `encrypt`/`decrypt` (or `write_message`/`read_message`) over AES-256-GCM, exposing the derived BLAKE2b transport hash for channel binding. It owns the **rekey accounting**: a byte counter (rekey at **1 GB**) and a creation `Instant` (rekey at **1 h**), whichever trips first; on rekey it advances `snow`'s transport state (or signals the caller to re-handshake — pick per `snow`'s rekey API and document it). On **replay-counter overflow** or rekey failure it returns an error the caller treats as "drop + reconnect" (`§6.3`). **Session keys never touch disk** — no `Serialize`, no logging of key material; zeroize on drop.
- **`establish_noise_session`-shaped API**: a constructor matching `design/12 §5.1`'s `establish_noise_session(transport) -> Result<NoiseSession>` intent — initiator and responder entry points that take the static keys + the byte channel. Keep transport-agnostic (caller supplies the duplex/stream); Task 212 plugs the Iroh bidi stream in.
- **`snow` known-answer vectors**: commit a set of IK handshake test vectors (fixed static keys + ephemerals → fixed handshake messages + derived keys) and assert `snow`'s output matches, freezing the protocol against `snow`-version drift. If upstream published IK vectors are usable, use them and cite the source; otherwise generate-and-commit a deterministic vector with documented inputs (the same "committed known-answer vector" discipline 205 used for cert bytes).
- **`cargo-fuzz` target on `validate_cert`**: `crates/identity/fuzz/` (a `cargo-fuzz` workspace) with a `fuzz_targets/validate_cert.rs` that feeds arbitrary bytes (+ a fixed `core_pub`) to 205's `verify_cert`/206's `validate` and asserts **never panics** (always `Ok`/`Err`). Wire it so `cargo fuzz build` succeeds in CI as a *compile gate* (running the fuzzer to convergence is not a CI gate — document the smoke run command).
- **Replay-rejection test**: record a Noise IK transport message and replay it; assert the session rejects it (nonce/counter reuse) per `§10`.
- **Benchmark of the second-AEAD overhead** (the spike's deferred deliverable): a `criterion` (or equivalent) bench, or a `--release` harness test, that measures `NoiseSession` encrypt+decrypt throughput on (a) small unary-sized payloads and (b) a streaming bulk transfer (1 MiB chunks like `session.io`), reporting MB/s and the per-op overhead. **State the result against the `> 1 MB/s session.io` bar** and against the spike's ~70–230 MB/s Iroh-TLS numbers (i.e. confirm the second pass doesn't drag the combined path under 1 MB/s). Record the measured numbers in the Handoff (this is the line the spike said Task 208 must measure). The bench is **informational, not a hard CI gate** (loopback timing is environment-sensitive — Task 102 treats sub-ms / throughput numbers the same way).

## Scope — out
- The Noise **XX** *pairing* handshake — **Task 207** (different pattern; this task is the post-pairing *session* layer only). Reuse 207's `snow` dependency + protocol-string discipline; do not re-add the dep differently.
- The real Iroh transport that carries `NoiseSession` + the one-gRPC-conn-per-Iroh-bidi-stream wiring + acceptor priming — **Task 212** (it consumes this layer per the spike §7 handoff). This task's channel is a loopback duplex double.
- QUIC connection migration / NAT telemetry — **Task 216**.
- The split-host end-to-end loopback smoke (two Iroh endpoints, real RPC + stream + Files) — **Task 220**.
- Auth middleware / cert validation *policy* — **Tasks 206/210** (208 only fuzzes the existing `validate_cert`; it does not change validation logic).
- Post-quantum / ML-KEM — V2.0 (`§12 R-5`).

## Public interface this task locks
- **The `NoiseSession` establishment API** — FROZEN: the initiator/responder constructors (matching `establish_noise_session(transport) -> Result<NoiseSession>`), the IK role assignment (initiator=device, responder=Core), and the documented **Noise protocol string**.
- **The rekey thresholds** — FROZEN: **1 GB OR 1 h, whichever first** (`§6.3`/R-9); drop-on-overflow semantics.
- **The committed `snow` IK known-answer vectors** (the cross-`snow`-version protocol freeze).

## Implementation notes
- **IK key pre-loading.** In IK the initiator must set the responder's static public key *before* the handshake (`snow`'s `Builder::remote_public_key`). In production that key is the `core_pubkey` from the `DeviceCert`; in the loopback test, pass the responder's generated static directly. Keep the API so Task 212 can feed the cert-carried key.
- **Rekey mechanics.** `snow`'s `TransportState` exposes a `rekey`/`set_receiving`/`set_sending` style API depending on version; check the pinned `snow` and choose the supported rekey path. Track bytes on *both* send and receive; trip at 1 GB cumulative (document whether it's per-direction or combined — pick per `snow`'s model). The 1 h timer is wall-clock from session start. On trip, either rekey in place (if `snow` supports it cleanly) or return a `RekeyRequired` error that signals the transport to re-handshake — document which, and that the caller (212) drops+reconnects on hard failure.
- **No key material on disk or in logs.** No `Debug`/`Serialize` that prints keys; `zeroize` the session secrets on drop (mirror 205's private-key hygiene). The transport hash is fine to expose (it's a binding value, not a secret).
- **Fuzz target structure.** `crates/identity/fuzz/` is its own `cargo-fuzz` crate (`cargo fuzz init`-shaped) with `libfuzzer-sys` + `arbitrary`. Its only assertion is the panic-freedom + `Ok`/`Err` totality of `validate_cert`. Add a CI-friendly **compile gate** (`cargo fuzz build` or `cargo build` of the fuzz crate) and document the local run (`cargo fuzz run validate_cert -- -max_total_time=60`). Keep the fuzz crate out of the main workspace's default-member build if it complicates `cargo check --workspace` (cargo-fuzz crates are typically excluded) — document the exclusion.
- **Benchmark honesty.** Report measured MB/s on the test host, name the host class (as the spike does — e.g. Apple M-series), and interpret against the bar exactly like the spike: the bar is `> 1 MB/s`, the measured AEAD throughput will be orders of magnitude above it, so the conclusion is "second AEAD pass does not breach the session.io bar." Do not fabricate a real-WAN number — that remains the spike's PENDING Tier-3 line.
- **License.** `snow` is shared with 207 (resolved SPDX = MIT). `cargo-fuzz`'s `libfuzzer-sys`/`arbitrary` are MIT/Apache-2.0 (already on the allow-list) — confirm with `cargo deny check`; ratify anything new with a dated comment. A copyleft transitive = Stop-and-ask.
- **Cross-platform.** `snow`/AES-GCM are portable. `cargo-fuzz` requires a libFuzzer-capable toolchain — gate the fuzz *build* in CI to the lanes that support it (typically Linux); document that the fuzz target is not built on the Windows lane. No `std::os::unix` in `noise_ik.rs`/`NoiseSession`.

## Verification
Tier 2 — the double is a **loopback IK handshake (two in-process endpoints over `tokio::io::duplex`/in-memory) plus committed `snow` known-answer vectors**. It proves: the IK handshake + role assignment, AES-256-GCM session encrypt/decrypt, the rekey accounting (force a low threshold in a test to trip rekey deterministically), replay rejection, and the second-AEAD throughput characteristic. It does **NOT** cover: a real cross-device Noise session over a live Iroh QUIC connection across a real network — that is exercised by **Task 212 / Task 220** and is the **Phase-2 Tier-3 checklist** (split-host file transfer + real-NAT). The real-WAN-relayed throughput remains the spike's PENDING operator field line.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-identity` → IK loopback handshake, the committed `snow` known-answer vectors, AES-256-GCM round-trip, rekey-trip (forced low threshold), and replay-rejection tests pass.
4. `cargo fuzz build` (or the documented compile gate for `crates/identity/fuzz/`) → builds clean; the `validate_cert` target links. (Convergence run is not a CI gate.)
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → green (`snow` already ratified by 207; confirm `cargo-fuzz` deps clear).
7. The second-AEAD **benchmark** runs and its measured MB/s (unary + streaming) is recorded in Handoff with the explicit "does not breach `> 1 MB/s`" conclusion (informational, not a pass/fail gate).
8. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (the `NoiseSession`/`establish_noise_session` surface in `crates/identity/src/api.rs`, if exposed there).

## Definition of Done
- [x] Noise IK wrapper in `crates/identity` with `§3.4` roles + documented frozen protocol string
- [x] `NoiseSession` (AES-256-GCM) with rekey at 1 GB OR 1 h (whichever first) + drop-on-overflow; keys never persisted/logged, zeroized on drop
- [x] Committed `snow` IK known-answer vectors freezing the protocol
- [x] `cargo-fuzz` target on `validate_cert` in `crates/identity/fuzz/`; compile gate green; panic-freedom asserted
- [x] Replay-rejection test passes
- [x] Second-AEAD overhead benchmarked (unary + streaming); result vs the `>1 MB/s session.io` bar recorded in Handoff (the spike-deferred line)
- [x] Tier-2 double + uncovered (Tier-3) part stated in Verification
- [x] `cargo deny check` green; verification commands pass; interfaces regenerated + committed
- [x] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate ones in Handoff)
- [x] Single commit with the message below

## Outputs
- `crates/identity/src/noise_ik.rs` (new — IK wrapper + `NoiseSession`) + `crates/identity/src/lib.rs` / `src/api.rs` (modified — module + exposed surface)
- `crates/identity/tests/noise_ik_vectors.rs` (new — `snow` known-answer + handshake + rekey + replay tests)
- `crates/identity/fuzz/Cargo.toml`, `crates/identity/fuzz/fuzz_targets/validate_cert.rs` (new — cargo-fuzz crate)
- `crates/identity/benches/noise_aead.rs` (new — second-AEAD throughput bench) *(or a `#[cfg(test)] --release` harness; pick one and note it)*
- `Cargo.toml` (modified only if `snow`/fuzz pins not already present from 207)
- `deny.toml` (modified only if a new SPDX surfaces)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-2: Noise IK session layer (AES-256-GCM, rekey 1GB/1h) + fuzz

Implements the inner Noise IK session crypto (design/12 §3.4/§6.3): IK
handshake (device static initiator, Core static responder), AES-256-GCM
session keys, BLAKE2b transport hash, rekey at 1 GB or 1 h with
drop-on-overflow, keys never on disk. Adds committed snow known-answer
vectors and a cargo-fuzz target on validate_cert. Benchmarks the second
AEAD pass that spike 102 stubbed — confirmed far above the 1 MB/s
session.io bar.

Refs: tasks/v1.0/208-noise-ik-session.md
```

## Handoff Notes

- **Chosen Noise IK protocol string (FROZEN wire contract):**
  `Noise_IK_25519_AESGCM_BLAKE2b` — declared as
  `concerto_identity::NOISE_IK_PARAMS` in `crates/identity/src/noise_ik.rs`.
  `IK` gives the two messages `design/12 §3.4` names (`-> e, es, s, ss` /
  `<- e, ee, se`), initiator = device (pre-loads the responder's static),
  responder = Core. Cipher suite is X25519 / AES-256-GCM / BLAKE2b. **Key-type
  nuance (important for Task 212):** the Noise static is an **X25519 (DH) key**,
  *distinct* from the Ed25519 identity/signature key in the `DeviceCert`. This
  layer takes raw 32-byte X25519 statics (`NoiseStatic::generate` /
  `NoiseStatic::from_private`, which derives the pub via `snow`'s pure-Rust
  resolver). Task 212 owns deriving/storing the Core's and device's Noise
  statics and carrying the responder's public half so the initiator can
  pre-load it; the design says "device key"/"Core identity key" at the *role*
  level — at the *crypto* level these are the DH statics, not the cert's Ed25519
  keys. The establishment API is `establish_initiator(local, remote_static_pub,
  now, send, recv) -> NoiseSession` and `establish_responder(local, now, send,
  recv) -> NoiseSession` over a **caller-supplied byte channel** (the
  `establish_noise_session(transport) -> Result<NoiseSession>` intent of
  `design/12 §5.1`), plus the lower-level `NoiseIkHandshake` + `into_session`.

- **`snow` rekey mechanism (FROZEN thresholds 1 GB OR 1 h, whichever first):**
  `NoiseSession` owns a **combined-direction** byte counter (`REKEY_BYTES =
  1_000_000_000`) and a creation `Instant` (`REKEY_INTERVAL = 1 h`). Every
  `encrypt`/`decrypt` adds the plaintext length and, if either threshold is
  reached, **rekeys in place** via `snow`'s `TransportState::rekey_outgoing()` +
  `rekey_incoming()` (Noise spec §4.2 — deterministic key advance from the
  existing key, **no extra wire message**) and resets both counters. Each end
  rekeys symmetrically when its own accounting trips, so the two stay in
  lockstep without a re-handshake (per-Iroh-connection sessions; the byte
  budget is combined, time is wall-clock from session start). Clock-injected
  `encrypt_at`/`decrypt_at` make the rekey trip deterministic in tests. On
  **replay-counter (nonce) overflow** (`snow` `StateProblem::Exhausted` after
  2⁶⁴−1 msgs) or any AEAD auth failure, `decrypt` returns `IdentityError::Noise`
  → the caller (212) drops + reconnects (`design/12 §6.3`). Keys never touch
  disk: `NoiseSession` is not `Serialize`/`Debug`/`Clone`, logs no key
  material, and drops `snow`'s `TransportState` eagerly on `Drop` (snow zeroizes
  its cipher state). `NoiseStatic` zeroizes its private bytes on drop.

- **Frame-size limit (Task 212 must chunk):** a single Noise transport message
  is ≤ 65535 B incl. the 16 B AEAD tag, so `encrypt`'s payload must be ≤ 65519 B
  (oversized → `Err`, tested). A `session.io` 1 MiB chunk is split into ≤ 64 KiB
  Noise frames before encryption — exactly what the bench measures and what 212
  must do.

- **Measured second-AEAD throughput (the spike-deferred line) — Apple M-series
  (this dev host), `cargo bench -p concerto-identity`, informational not a CI
  gate:**
  - **unary (64 B encrypt+decrypt roundtrip):** ~2.1 µs/op (≈28 MiB/s — a 64 B
    payload is fixed-overhead-bound, so read it as per-op latency, not bytes/s).
  - **streaming (1 MiB transfer, chunked into 64 KiB Noise frames, encrypt+
    decrypt):** **~108 MiB/s**.
  **Conclusion vs the bar:** the `session.io` bar is **> 1 MB/s** (`design/11
  §10`). The second AEAD pass sustains ~108 MiB/s — **~100× the bar**, in the
  same order as the spike's ~70–230 MB/s Iroh-TLS streaming numbers — so the
  second AEAD pass **does NOT breach the `> 1 MB/s session.io` bar** and cannot
  plausibly drag the combined Iroh-TLS + Noise path under it. This confirms the
  spike's expectation (`tonic-iroh-findings.md` §3/§7). The **real-WAN-relayed**
  combined number remains the spike's PENDING operator Tier-3 field line.

- **Fuzz crate workspace-exclusion + CI lane:** `crates/identity/fuzz/` is a
  cargo-fuzz crate kept out of the stable workspace build **two ways** — it
  carries its own empty `[workspace]` table (cargo-fuzz convention) AND the root
  `Cargo.toml` lists `exclude = ["crates/identity/fuzz"]`. Verified: `cargo
  metadata --no-deps` lists no `*fuzz*` package, and `cargo check --workspace`
  does not compile it (nor `libfuzzer`/the fuzz crate in the main `Cargo.lock`).
  The compile gate is `cargo +nightly fuzz build` (or `cargo +nightly build`
  inside the fuzz dir) — to run on a libFuzzer-capable lane (typically Linux;
  not the Windows lane). **No nightly/cargo-fuzz on this dev host**, so I
  verified the target type-checks on stable via `cargo check` inside
  `crates/identity/fuzz` (passes); the libFuzzer link step itself needs nightly
  and is the operator/CI Linux-lane gate. The `validate_cert` target hammers
  both `verify_cert` (205) and `LocalCoreIssuer::validate` (206) with arbitrary
  bytes + a fixed Core pubkey and asserts panic-freedom + `Ok`/`Err` totality.
  Local convergence run (not a CI gate): `cargo +nightly fuzz run validate_cert
  -- -max_total_time=60`.
  - `libfuzzer-sys` (MIT/Apache-2.0/MIT-0) + `arbitrary` (MIT/Apache-2.0) are
    already on the `deny.toml` allow-list. Because the fuzz crate is excluded
    from the workspace these deps don't appear in workspace `cargo deny check`
    (which stays green); they're permissive regardless, so **`deny.toml` was NOT
    modified**.

- **Bench approach (criterion):** `crates/identity/benches/noise_aead.rs` uses
  **criterion** (dev-dep `criterion = "0.5"`, already in the lockfile via
  `concerto-gix-wrap`; `[[bench]] harness = false`), mirroring the existing
  gix-wrap bench convention. CI runs `cargo bench --no-run` (compile gate only),
  verified green.

- **Replay-rejection approach:** `tests/noise_ik_vectors.rs` records a
  legitimate transport frame, delivers it once (accepted), then re-delivers the
  **same** ciphertext — the receiver's AES-GCM nonce/counter has advanced, so
  the replayed frame fails to authenticate and `decrypt` returns `Err`
  (`design/12 §10`). A companion test flips a ciphertext bit and asserts the
  tampered frame is rejected.

- **Committed known-answer vectors:** `tests/noise_ik_vectors.rs` freezes a full
  IK handshake from fixed statics (`from_private`, deterministic pub derivation)
  + fixed ephemerals (`*_with_fixed_ephemeral`, `#[doc(hidden)]`, testing-only):
  the exact 96 B message 1, 48 B message 2, and 64 B BLAKE2b transport hash. A
  `snow` upgrade that perturbed the IK protocol fails these loudly (the
  cross-version freeze). Regen helper: `cargo test -p concerto-identity --test
  noise_ik_vectors -- --ignored --nocapture print_known_answer_vector`.

- **Drift from plan:**
  - **`api.rs` NOT modified / no `rust-api.md` diff.** The Outputs line said
    "`src/lib.rs` / `src/api.rs` (modified — module + exposed surface)". I
    exposed the IK surface as **re-exports from `lib.rs`** (the
    `NoiseSession`/`establish_*`/`NoiseStatic`/`NoiseIkHandshake` + consts) and
    did **not** relocate the struct definitions into `api.rs` — exactly
    mirroring how Task 207 handled the parallel `noise_xx` surface (its
    `NoiseHandshake`/`NoiseTransport` are lib-level re-exports, not in `api.rs`).
    Consequence: `regen-interfaces.sh` (which scans only `api.rs` for
    `pub struct/trait/enum`) produces **no `docs/interfaces/rust-api.md` diff**,
    so the `git diff --exit-code docs/interfaces/` gate passes with the regen
    committed and unchanged. `api.rs` is therefore NOT in the final touched set.
  - **`NoiseStatic` is an X25519 keypair, taken raw** (not derived from the
    Ed25519 cert key). The IK static is a DH key; converting Ed25519→X25519 is
    out of this layer's scope and a Task 212 wiring concern (see the protocol
    note above). The API is shaped so 212 feeds whichever bytes it derives.
  - **Transport hash is 64 bytes** (`TRANSPORT_HASH_LEN`), because the
    `BLAKE2b` Noise suite's handshake hash is BLAKE2b-512 — distinct from the
    32-byte BLAKE2b-256 `device_id` digest. Exposed as `[u8; 64]`.
  - **`crates/identity/fuzz/Cargo.lock`** is committed with the fuzz crate (the
    cargo-fuzz convention for reproducible fuzz builds); it is a separate lock
    inside the excluded crate and does not affect the main workspace lock.
  - **`Cargo.toml` modified** to add the `exclude` entry; `crates/identity/
    Cargo.toml` modified to add the criterion dev-dep + `[[bench]]`. `snow` was
    already pinned by 207 (reused, NOT bumped — 0.9.6, MSRV-safe). `deny.toml`
    NOT modified.

- **Open questions for Task 212 (transport wiring):**
  - **Where do the X25519 Noise statics come from?** This layer takes raw
    statics; 212 must decide how the Core's persistent Noise static is
    generated/stored (keychain alongside the Ed25519 identity?) and how the
    device carries the Core's Noise *public* static (alongside `core_pubkey` in
    the cert flow, or as a separate pairing-time field). If the design intends
    the Noise static to be *derived* from the Ed25519 identity (Ed25519→X25519
    birational map), that conversion belongs in 212/the identity-loader, not
    here — flag for the operator.
  - **Chunking:** 212 must split `session.io`/gRPC frames into ≤ 64 KiB Noise
    frames before `encrypt` (the bench's `NOISE_FRAME` shows the shape).
  - **Rekey is in-place, no wire signal:** both ends rekey deterministically
    when their own byte/time accounting trips; 212 must keep the accounting on
    *both* peers consistent (count plaintext on the same `encrypt`/`decrypt`
    boundary this layer does) so they stay in lockstep. A hard `decrypt` error
    (exhausted/auth-fail) is 212's "drop + reconnect" signal.

- **Deliberate debt:** — (none; no `TODO`/`FIXME`/`unimplemented!()`/`todo!()`
  in new code).

- **Smoke-gate state:** **unchanged.** No smoke check added; `scripts/smoke.sh`
  untouched. The IK layer is pure in-crate crypto with an in-process loopback
  double — no Core boot, no keychain, no network — so it adds no smoke
  capability (the live cross-device session is Task 212/220 Tier-3).

- **Tier-3 not covered by the loopback double** (→ Phase-2 manual checklist):
  a real cross-device Noise IK session inside a live Iroh QUIC stream across a
  real network (NAT, relay, real RTT) — exercised by **Task 212 / Task 220**
  (split-host file transfer + real-NAT). The real-WAN-relayed throughput of the
  combined second-AEAD path remains the spike's **PENDING** operator field line.
  Stated in the `Verification` section and the test-module doc.
