# Concerto — Technology evaluation

*Companion to the Concerto PRD. Comprehensive survey of stacks, libraries, and reference architectures, with pros/cons and recommendations.*

---

## 0. How to read this document

This is a long document because the surface area is large: a server in Rust (or Go), a desktop UI, two mobile apps, a web app, secure remote transport, git internals, agent process supervision, terminal emulation, diff rendering, scheduling, persistence, secrets, push notifications, build pipelines.

Each section follows the same shape:

- **What this is.** One paragraph framing the problem.
- **Options.** Each candidate with pros/cons.
- **Recommendation.** What I'd pick today, and the second-best fallback.

At the end (section 17) there's a one-page **assembled recommended stack** so you can see the whole thing in one view. If you want to skim, start there and dive into the sections that worry you.

---

## 1. Reference projects to study

This section catalogs adjacent tools in the agent-orchestration and developer-tooling space — not as competitors, but as engineering reference points for specific design choices.

### 1.1 Sculptor (Imbue, github.com/imbue-ai/sculptor)
Open source desktop app for concerted Claude Code agents in **Docker containers** (not git worktrees). Read it to understand the container-per-agent isolation pattern and the "pairing mode" sync between container and host repo. Imbue's blog post on dev containers + container snapshots is also good reading.

### 1.2 Happy Coder (slopus/happy, happy.engineering)
Open source Claude Code mobile companion. **This is the closest existing thing to what we want to build, just smaller scope.** Read this carefully:
- Three components: a CLI wrapper, an encrypted relay server, and Expo (React Native) mobile/web app.
- End-to-end encryption using **TweetNaCl** (X25519 ECDH + AES-256-GCM, ephemeral session keys).
- "Zero-knowledge" relay — server only sees ciphertext blobs.
- Push notifications: payload fetched after wakeup, same pattern we want.
- Voice input via the OS speech APIs.
- Pairing via QR code.
- Apache-licensed JS, easy to read.

If we don't use any of their code, we should at minimum mirror their crypto and pairing flow exactly. It works, it's been audited by usage, and matching it shortens our security review.

### 1.3 Vibe Kanban (BloopAI/vibe-kanban, sunsetted but open)
Kanban-board view of concerted agents on git worktrees. Multi-agent (Claude Code, Codex, OpenCode). Apache 2.0, written in TypeScript. Bloop shut down the company in April 2026 but the code is still there. Worth reading for their issue tracker integration and worktree lifecycle.

### 1.4 Nimbalyst (formerly Crystal, nimbalyst.com)
Closed source desktop + iOS companion. Multi-platform from the start. Visual editors (Excalidraw, mockups) embedded in workspaces. Useful as a UX reference, not for code.

### 1.5 Claude Squad
Terminal-based, uses tmux for the multiplexing. Lighter weight than dashboard-style orchestrators. Read for the terminal-only flow and how it shells out to Claude Code.

### 1.6 Tailscale + tsnet
Not an AI tool — but the gold-standard reference for "embedded peer-to-peer mesh with NAT traversal and a minimal relay (DERP) that sees only ciphertext." `tsnet` lets you embed a full Tailscale node inside a Go binary, no separate daemon. This is exactly the connectivity layer we want, and Tailscale's DERP relay design is the model for our minimal relay.

### 1.7 Iroh (iroh.computer)
A pure-Rust library for the same idea Tailscale does in Go: direct peer-to-peer QUIC connections, hole-punching, relay fallback, public-key addressing. If we're committed to Rust for the Core, **Iroh is probably the right transport layer** — see section 9 for the deep dive.

### 1.8 Anthropic Claude Agent SDK (anthropic-ai/claude-agent-sdk on npm/PyPI)
The same harness that powers Claude Code, exposed as a library. Available in Python and TypeScript. We don't have to spawn the `claude` CLI as a subprocess if we don't want to — we can embed the SDK and get programmatic control over the agent loop, tools, hooks, and sessions. This is a fairly recent addition (renamed from Claude Code SDK in March 2026) and the license is proprietary (not open source), but free for individuals/small teams. Worth using.

### 1.9 Codex (openai-codex npm package + CLI)
OpenAI's equivalent. Different architecture — they ship a separate desktop "Codex app" — but the CLI is what most users invoke. Treat as a subprocess to spawn.

---

## 2. The big architectural decision: language for the Core

Everything else flows from this. The Core daemon owns workspaces, git, agent subprocesses, secrets, and the local API. It will end up being the largest single piece of code in the system.

### 2.1 Rust

**Pros**
- Single static binary on Mac, Windows, Linux with `cross` or platform-matrix CI. No runtime to install.
- Memory safety without GC. Important because the Core runs for weeks at a time and supervises long-lived agent processes.
- Best-in-class libraries for everything we need: `tokio` (async runtime), `tonic` (gRPC), `quinn` (QUIC), `gix`/`gitoxide` (git), `rusqlite`/`sqlx` (SQLite), `keyring` (OS keychain), `snow` (Noise), `portable-pty` (pseudo-terminals).
- **Iroh** (peer-to-peer transport, see §9) is Rust-native. Using Iroh from anything other than Rust is awkward.
- **Tauri 2** (desktop shell, see §5) is Rust-native. If the desktop UI is Tauri, sharing types and protobuf with the Core is essentially free.
- Cargo workspaces make a monorepo of (core daemon, desktop shell, CLI, relay) very pleasant.
- The hire pool is smaller but the people who say "yes" to a Rust system project tend to be the ones we want.

**Cons**
- Slower compile times. A clean build of the Core will be minutes, not seconds. Mitigations: `mold` linker on Linux, `sccache`, split into many small crates, use `cargo-nextest`.
- More verbose for fast-changing internal logic. Async lifetimes can be painful.
- Two big ecosystems we depend on (`tonic` and `tokio`) move fast and occasionally break SemVer with each other. Pin versions carefully.

**Verdict.** Strong default. The combination of single-binary distribution, Iroh, Tauri, and the gix ecosystem make it the path of least resistance for a daemon that runs everywhere.

### 2.2 Go

**Pros**
- Compile times are seconds. The whole Core would build in 10–20 seconds.
- **`tsnet`** lets you embed a full Tailscale node — this is a *complete* solution for the remote-control transport problem, including the relay (DERP) and managed NAT traversal. Hard to overstate how much engineering it saves.
- Massive standard library and ecosystem. Everything in this list has multiple mature implementations: gRPC, SQLite drivers, mDNS, keyring, PTY.
- Easier to hire for, easier to onboard new engineers, easier to keep code reviewable.
- Cross-compiles trivially — `GOOS=windows go build` from a Mac just works.

**Cons**
- No Tauri equivalent — desktop UI options in Go are weaker (Wails, Fyne, Gio). If we go Go for the Core, the desktop UI is almost certainly Electron or a separate Tauri/Rust UI process talking to a Go server.
- GC pauses are not a real concern for this workload but feel less elegant.
- The git story is **libgit2 via cgo bindings** (`git2go`) or shelling out to `git`. There is no native-Go git library at the level of gix.
- Less attractive to a certain class of engineer — but you can also argue this is fine because we want pragmatic people.

**Verdict.** Strong runner-up. **If you optimize for "ship faster", Go is the right choice.** If you optimize for "single language across daemon and desktop", Rust is.

### 2.3 Why not Node.js / Bun?

Happy Coder is built this way and it works fine for a small project. For something with this much surface area:
- Native add-ons (PTY, keyring, SQLite, native crypto) become friction points.
- Memory footprint of a Node process running for weeks is not great.
- Distributing a Node app cleanly on three OSes is solvable but unpleasant.
- We'd still want Rust or Go for one of the layers (transport, git), so we'd be in a polyglot setup anyway.

Don't pick Node for the Core. Use it inside the desktop UI (which is just a webview running our React code).

### 2.4 Why not Swift + Kotlin + Rust (no shared Core)?

Maximum native fidelity, but you write the agent supervisor and workspace manager three times (once per OS). That's a deal-breaker for a small team. Hard pass.

### 2.5 Recommendation

**Pick Rust if** you want a single language from Core through desktop, want Iroh for transport, want the type system to catch most of the agent-supervision bugs at compile time, and are willing to pay the compile-time tax.

**Pick Go if** you want maximum velocity for V0.1 and V1.0, want to use tsnet to skip building a custom transport entirely, and don't mind a separate UI process in TypeScript/Rust.

**My recommendation: Rust**, primarily because of Iroh and Tauri integration. The compile-time tax is real but bounded; the architectural cleanness is durable. Go is a sane fallback if the team has more Go experience than Rust experience.

---

## 3. Reference architectures we should explicitly copy

Three patterns from the ecosystem that resolve big design questions for us:

### 3.1 The Happy Coder pattern (for the remote control layer)
- A daemon on the developer's machine wraps each agent invocation.
- Encrypted blobs flow to and from a relay server.
- Mobile/web clients pair via QR code, exchange public keys, and decrypt blobs locally.
- Crypto: TweetNaCl (X25519 + AES-256-GCM + ephemeral session keys).
- Push notifications are wakeup-only; the payload is fetched from the daemon after wakeup over the E2EE channel.

**Steal this entirely.** It's the lowest-risk path to a working remote control layer. We'd extend it to add direct (non-relayed) peer connections when both ends are reachable.

### 3.2 The Tailscale / Iroh pattern (for the transport)
- Each device has a long-lived keypair. The public key is the device identity.
- A minimal coordination server (DERP for Tailscale, n0's relays for Iroh) helps with NAT traversal and falls back to relaying ciphertext if direct connection fails.
- The protocol is QUIC + TLS 1.3 end-to-end.

**Use this for the transport layer specifically**, on top of the Happy Coder pattern's pairing model. Iroh has effectively packaged this as a Rust library.

### 3.3 The orchestration-dashboard UX pattern
A warm off-white aesthetic with a restrained accent color, status communicated by glanceable colored dots, and a three-panel layout:
- Workspaces in a sidebar with status dots.
- A chat + terminal column in the middle.
- A diff + checks column on the right.
- Diff viewer with inline comments that become composer attachments.
- Chat + Diff + Checks + Terminal tabs.
- Plan mode, Fast mode, agent selector.

**Adopt this layout** in V0.1, then add Concerto-specific surfaces (multi-repo session, skill explorer, workflow explorer) in V1.0.

---

## 4. Core daemon — internal building blocks

Concrete library picks for the Core daemon. Assume Rust unless noted; I'll call out the Go equivalents.

### 4.1 Async runtime

**Tokio.** No real alternative for production async Rust. `async-std` is effectively dead, `smol` is too minimal for our needs. Tokio is the standard.

- ✅ Mature, vast ecosystem, well-documented.
- ✅ Works with every other library in this stack (tonic, quinn, sqlx, hyper, reqwest, snow).
- ❌ Compile times are part of the Rust compile-time tax.

Use this. There's no decision to make.

### 4.2 Git library

**Three options**, ranked by my preference:

#### gix / gitoxide
Pure-Rust git implementation under active development by Sebastian Thiel and contributors. Sponsored partly by GitHub.

- ✅ Pure Rust, no C bindings, no libgit2 system dependency.
- ✅ Async-first APIs. Fits cleanly into Tokio.
- ✅ Active development; new features (sparse checkout, partial clone) are landing.
- ✅ Used in cargo internally for git operations.
- ❌ **Not feature-complete.** Push, full merge workflows, rebase, hooks are still under development as of 2026. The crate status pages are honest about this.
- ❌ Some operations require dropping to `git2` (libgit2 bindings) or shelling out to `git`.

#### git2 (libgit2 bindings)
The mature option. Wraps libgit2.

- ✅ Feature-complete. Everything works.
- ✅ Stable for years.
- ❌ Requires libgit2 C library at build time. Solvable but adds friction on Windows.
- ❌ libgit2 has **partial** sparse-checkout support (see the open PRs), and **no** `core.sparseCheckoutCone` support yet. This is a real blocker for our monorepo story.
- ❌ Not async; blocking calls have to be wrapped in `spawn_blocking`.

#### Shell out to `git`
Spawn the `git` binary for every operation.

- ✅ Always feature-complete. Sparse-checkout, blobless clones, sparse index — all the cutting-edge git features just work because we use git itself.
- ✅ Trivial to debug — copy/paste the command into a terminal.
- ✅ No build dependency on libgit2.
- ❌ Slower for high-frequency operations (each spawn is ~5–20 ms).
- ❌ Parsing porcelain output is fragile.
- ❌ Requires git on the user's PATH (almost always true, but not for sandboxed environments).

**Recommendation: hybrid.** Shell out to `git` for clone, fetch, sparse-checkout configuration, partial clones, blobless conversions — operations where we want git's behavior exactly and they're infrequent. Use `gix` for the hot path: status, diff, log, ref reading. Use `git2` only for things that gix doesn't cover yet and that shelling out is too slow for.

**For Go:** the calculus is simpler — `git2go` (libgit2 cgo bindings) plus shelling out for sparse-checkout-cone is the path. No equivalent of gix exists.

### 4.3 Persistence — SQLite

**Pick one of:**

#### `rusqlite`
Synchronous, bundled-SQLite, very direct mapping to the C API.
- ✅ Smallest dependency footprint.
- ✅ Easy to reason about.
- ❌ Blocking; you wrap each call in `tokio::task::spawn_blocking`.

#### `sqlx`
Async, supports SQLite (and Postgres, MySQL). Compile-time-checked queries with the right macros.
- ✅ Async-first.
- ✅ Compile-time query validation — your SQL is checked against a real schema at build time. Catches whole classes of bugs.
- ❌ Heavier dependency.
- ❌ The compile-time check requires a populated DEV database at build time, which is friction in CI.

#### `libsql` / Turso
Open-source SQLite fork (libSQL) with embedded replicas, plus the new Rust-rewrite Turso database. The Turso team is pushing aggressively in 2026 — but Turso's main wins (embedded sync to a cloud primary, multi-writer, etc.) don't apply to us because we're local-first. Skip Turso.

**Recommendation: sqlx.** Async fits Tokio. Compile-time checks are valuable for a long-lived schema. Worth the friction.

**Connection pattern:** one writer connection, multiple reader connections, WAL mode. Standard SQLite-as-app-database playbook.

### 4.4 Pseudo-terminal (PTY) — for agent subprocesses

We launch Claude Code / Codex / Gemini CLI as child processes. They want a TTY (some of their UI is escape-sequence based). We need to be a PTY master.

**Pick:**

#### `portable-pty` (Wez Furlong, part of wezterm)
Mature, used in wezterm. Wraps native PTY APIs on Linux/macOS (`posix_openpt`) and ConPTY on Windows.
- ✅ Battle-tested in wezterm, which is a production terminal emulator.
- ✅ Cross-platform — ConPTY on Windows 10/11 works correctly.
- ✅ Reader/writer split, resize handling (SIGWINCH equivalent), child process supervision.
- ❌ Not async-native. Wrap reads in `spawn_blocking` or use the included async helpers.

#### `rust-pty`
Newer crate, async-first via Tokio.
- ✅ Async API.
- ❌ Newer, less battle-tested.

**Recommendation: portable-pty.** It powers wezterm; if it's good enough for that, it's good enough for us.

### 4.5 OS credential store (secrets)

API tokens (Anthropic, OpenAI, GitHub PATs), pairing keys, etc.

**Pick: `keyring-rs`** (the modern v4+ design, with separate per-platform store crates):
- Keychain on macOS
- Credential Manager on Windows
- Secret Service / libsecret on Linux

It's the standard. Used by everything. No real alternative.

### 4.6 mDNS / Bonjour for local-network discovery

When the desktop and the phone are on the same Wi-Fi, we should discover the Core without going through any server.

**Pick: `mdns-sd`.** Pure Rust, no async-runtime dependency (uses flume channels), supports both the responder side (Core broadcasts itself as `_concerto._tcp.local`) and the browser side (clients find it). Compatible with Avahi (Linux), Bonjour (macOS/iOS), and dns-sd (Windows).

Alternative: `zeroconf` crate wraps the platform-native APIs. More polished but pulls in `libavahi-client` on Linux. Skip in favor of `mdns-sd`.

### 4.7 Process supervisor

Each agent is a child process. We need: start, restart on crash, capture stdio, send signals, detect zombies, propagate exit codes.

**Pick: roll our own on top of `tokio::process` + `portable-pty`.** Process supervision is small and the libraries that exist (`supervisor`, `proc-actor`) don't add much over the raw primitives. Wrap each agent in a struct with: pid file, restart policy, last-N-seconds restart history, an output ring buffer for clients reconnecting mid-stream.

### 4.8 Logging / tracing

**Pick: `tracing` + `tracing-subscriber`.** Standard for async Rust. Pipe events to a rotating log file and (optionally) to OpenTelemetry. Off by default per the local-first principle; opt-in only.

---

## 5. Desktop UI framework

Big decision. Three options.

### 5.1 Tauri 2

**What it is.** Rust core + native OS WebView (WebKit on macOS, WebView2 / Edge on Windows, WebKitGTK on Linux) + your choice of frontend framework (React, Vue, Svelte). Tauri 2 reached stable October 2024; current version is 2.10.1 (March 2026).

**Pros**
- **Tiny bundles:** 5–15 MB typical, vs. 80–200 MB for Electron.
- **Low memory:** 30–80 MB idle vs. 150–450 MB for Electron.
- **Fast startup:** ~1.8 seconds cold vs. 4–12 seconds for Electron in benchmarks.
- **Single language with Core:** Shared Rust types, shared protobuf, no FFI gymnastics.
- **Capability-based permission system** — the frontend can't call native APIs without explicit declarations. Security-by-default.
- **Mobile support since v2.0 (October 2024).** Same Rust core can target iOS and Android. (See §6.)
- Active development, ~$22M funding.

**Cons**
- **Three different WebViews to support.** WebKitGTK on Linux is the weak link — historical bugs in font rendering, video playback, and CSS edge cases. Less of an issue in 2026 than it was in 2022 but still real.
- **Smaller plugin ecosystem than Electron.** You'll write some native code where Electron would have a battle-tested npm package.
- **Auto-updater is full-binary download** (no differential updates yet). Fine because our bundles are small.
- **Mobile is "stable API, not finished story."** The Tauri team has been explicit that v2.0 mobile is a foundation, not the production-ready state. Not all desktop plugins work on mobile yet. (More in §6.)

### 5.2 Electron

**What it is.** Chromium + Node.js bundled together. Industry standard. Used by VS Code, Slack, Discord, Notion, GitHub Desktop.

**Pros**
- **Most mature ecosystem.** Anything you need has 3+ npm packages.
- **Consistent WebView** across platforms (it's Chromium everywhere).
- **electron-updater** is the gold standard for desktop auto-update: differential updates, staged rollouts, code-signed releases.
- **Hire pool is enormous.** Any web developer can contribute.
- **Native integrations** (system tray, native menus, notifications, file dialogs) are extremely well-trodden.

**Cons**
- **180 MB bundle, 300–450 MB RAM idle.** This is the cost of bundling a browser.
- **No code sharing with a Rust Core** — you have to bridge between Node and Rust via N-API or a separate IPC layer.
- **No mobile.** If we go Electron, we write mobile separately. (Almost certainly we'd write mobile separately anyway, but if we go Tauri we *could* unify.)
- **Security posture** requires care (context isolation, disabled Node integration in renderers, content security policies). The footguns are well-known but you have to remember them.

### 5.3 Wails (Go) / Native (SwiftUI / WinUI / GTK)

Out of consideration for V1. Wails has the same WebView issues as Tauri without the ecosystem. Three native UIs is too much engineering for a small team.

### 5.4 Recommendation

**Tauri 2.** The bundle/memory wins matter for a daemon-companion app that users keep open all day. Single-language with the Core is a meaningful architectural simplification.

The WebKitGTK risk on Linux is real but bounded — most of the UI is plain React, and the bits that break are usually fixable with CSS tweaks. Worst case, ship Electron as a fallback Linux build.

**Fallback: Electron.** If the team has zero Rust appetite for the UI layer, Electron is fine. We pay the bundle/memory tax but ship faster. Choose this if Tauri's WebKitGTK quirks bite during V0.1 prototype.

---

## 6. Mobile clients

This is the most contested decision. Five real options.

### 6.1 Native iOS (Swift + SwiftUI) + Native Android (Kotlin + Compose)

Two codebases. Maximum platform fidelity.

**Pros**
- Native everything: push, voice, haptics, navigation, accessibility, App Store / Play Store integration.
- Best performance, smoothest animations.
- Best long-term maintenance — Apple and Google support their own platforms forever.
- Hires are easy in both ecosystems.

**Cons**
- You write everything twice. For a touch-optimized diff viewer alone, that's a real cost.
- Two release cycles to coordinate.
- Bug fixes are 2× the work.
- Hiring two separate skill sets.

### 6.2 React Native (with Expo)

JavaScript / TypeScript on top of native components. **This is what Happy Coder uses.**

**Pros**
- **Massive ecosystem,** the largest of any cross-platform mobile framework.
- Hot reload, fast iteration.
- Shares ~80% of code with the web client if structured right.
- **Expo** handles the painful parts: APNs/FCM credentials, build infra, native module integration, EAS for cloud builds.
- Hire pool is huge — any React dev can ramp.
- New Architecture (Fabric + JSI + TurboModules) is now mandatory and stable.
- **Existence proof: Happy Coder works well and ships features fast.** They've shipped voice, push, multi-session, diff viewing in this stack.

**Cons**
- Performance gap vs. native on intricate animations and large list rendering. Probably fine for our use case (chat + diff + status) but worth knowing.
- Native modules sometimes break on RN upgrades. Less common with the New Architecture but still a tax.
- Some platform features lag — new iOS APIs sometimes take 6 months to land in RN.

### 6.3 Flutter

Dart language. Renders via Skia, owns its own UI pipeline.

**Pros**
- Pixel-perfect identical UI across iOS and Android.
- Strong animation performance.
- Single binary.

**Cons**
- **Dart is a niche language.** Smaller hire pool than JS or Kotlin.
- UI doesn't feel quite native on iOS — close, but uncanny-valley close.
- No code sharing with the web/desktop client we're already building in React.
- Mature but Google's commitment level has been questioned a few times.

**Verdict:** strong for greenfield mobile-only products, weak for us because of the lack of web shared code.

### 6.4 Kotlin Multiplatform (KMP) + Compose Multiplatform

KMP for shared business logic (data layer, networking, state). Compose Multiplatform (now stable on iOS since May 2025) for shared UI.

**Pros**
- Fastest-growing cross-platform option — from 7% to 23% market share in 18 months.
- **Best architecture for sharing only business logic** — use KMP for the network + state layer, write SwiftUI on iOS, write Compose on Android. Maximum native UI fidelity with shared logic.
- Now battle-tested at Netflix, Cash App, McDonald's.

**Cons**
- Compose Multiplatform on iOS is technically stable but the production-fidelity story (animations, list performance, navigation) is still maturing.
- Smaller library ecosystem than RN or Flutter.
- Requires Kotlin expertise on the team.
- **No code sharing with the web/desktop React app.** This is the big one for us.

### 6.5 Tauri 2 Mobile

Same Rust + WebView stack as the desktop, targeting iOS and Android.

**Pros**
- One codebase across desktop and mobile.
- Rust shared all the way down.

**Cons**
- **Not yet production-ready for our use case.** Tauri team has explicitly said v2 mobile is "a foundation, not the finished story." Many desktop plugins don't work on mobile yet. App Store / Play Store distribution patterns are still being formalized.
- WebView-based mobile apps feel non-native in subtle ways (no native navigation gestures, sluggish scroll on certain content types).
- A user wanting an iOS app expects an iOS app — not a "web app pretending to be iOS." For something users will rely on daily, this matters.

**Verdict for V1:** too immature. Worth revisiting for V1.5.

### 6.6 Recommendation

**Three rankings depending on your priorities:**

#### Optimize for shipping speed: **React Native + Expo**
Happy Coder proved this stack works for this exact use case. You get:
- ~70–80% code share with the web client (which is already React).
- Expo handles APNs/FCM, no separate certificates to manage.
- Largest hire pool.
- Fastest iteration during V1.0 beta.
- Worst-case mobile performance is "good enough" for chat + diff + status.

This is my actual recommendation for V1.0.

#### Optimize for native fidelity: **SwiftUI + Compose + KMP for shared logic**
If we're hiring an iOS specialist anyway (which is plausible at V2 if Concerto becomes a real product), this becomes attractive. KMP for the network/state layer + native UIs for the most touch-critical surfaces (diff viewer especially).

Punt this decision to V1.5 — by then we'll know if the mobile UX is the bottleneck.

#### Don't pick: Flutter (no web sharing), Tauri Mobile (too immature).

---

## 7. Web client

The web client is mostly a re-skin of the desktop's React tree. Three sub-decisions:

### 7.1 React vs alternatives
Use **React**. The desktop UI is React (inside Tauri's webview). Reuse the components. SolidJS, Svelte, Vue are all fine in isolation but the win of sharing >80% of components with desktop is the dominant factor.

### 7.2 Build tool
**Vite.** Fastest dev server, smallest config burden, used in essentially every new React project in 2025–2026. Webpack and esbuild-config-directly are slower in DX or harder to configure.

### 7.3 State management
**Zustand or Jotai.** Skip Redux — overkill for client-side state when the server holds canonical state. The clients are largely stateless renderers; useState + Zustand for the few cross-cutting pieces (current workspace, sidebar collapse) is plenty.

### 7.4 Component library
**shadcn/ui (Radix-based) + Tailwind.** Owned components (you copy them into your repo), full control over styling, accessible by default. The de-facto choice for modern, calm-aesthetic developer-tool apps in 2026.

Alternative: build from scratch on Radix Primitives + Tailwind. More work but more design control. shadcn is faster to start.

---

## 8. Local API protocol (Core ↔ Clients on the same machine)

The Core daemon exposes an API on a Unix socket (named pipe on Windows). Clients talk to it.

### 8.1 gRPC (Tonic in Rust)

**Pros**
- Schema-first (protobuf). Generates type-safe clients in every language we care about (TypeScript via `ts-proto`, Swift, Kotlin, Rust, Go).
- **Bidirectional streaming is first-class** — exactly what we need for agent I/O.
- Mature: Tonic is widely deployed (Bottlerocket, Linkerd, Tonic-based microservices everywhere).
- Versioning is rigorous via proto field numbers.
- Backpressure and flow control are built into HTTP/2.

**Cons**
- Browsers can't speak native gRPC. The web client needs gRPC-Web (proxy translation) or a parallel HTTP/WebSocket API. **gRPC-Web has limitations** — no client streaming, only server streaming.
- Protobuf adds a build-time step.
- Heavier than a custom JSON protocol for trivial RPCs.

### 8.2 JSON-RPC over WebSocket

**Pros**
- Trivial to implement, trivial to debug (curl, browser devtools).
- Browser support is trivial.
- No build step.

**Cons**
- No schema discipline. Easy to drift between client and server.
- No code generation; you write request/response types twice.
- Backpressure is fiddly.

### 8.3 Cap'n Proto RPC

**Pros**
- Even faster than gRPC for trivial RPCs.
- Time-travel RPC (compose calls without round-trips).
- Schema-first.

**Cons**
- Smaller ecosystem. Tonic has 10× the production deployments.
- Less library support in mobile (no first-class Swift or Kotlin support).

### 8.4 Recommendation

**gRPC (Tonic) as the primary protocol for desktop and mobile clients**, **plus a gRPC-Web bridge for the web client**, **plus an HTTP/SSE fallback** for the few cases where gRPC-Web's limitations bite (mostly: pure client streaming).

Define the schema once in `.proto`. Generate:
- Rust server (Tonic).
- Rust client for the Tauri desktop's renderer.
- TypeScript client (via `ts-proto` or `connect-web`).
- Swift client (via `swift-protobuf` + `grpc-swift`).
- Kotlin client (via `grpc-kotlin`).

The web client uses **Connect-Web (buf.build/connect)** rather than vanilla gRPC-Web — Connect handles the gRPC-Web vs. HTTP fallback negotiation automatically and the DX is significantly better.

This is the same pattern Linkerd, Buf, and many large gRPC deployments use.

---

## 9. Remote transport (Core ↔ Clients across the internet)

The hardest individual layer. Three candidate approaches.

### 9.1 Iroh

A pure-Rust library for peer-to-peer QUIC connections with NAT traversal and relay fallback. Built by the team behind IPFS (n0). Public-key-addressed endpoints.

**How it works**
- Each Core has an Ed25519 keypair. Public key is the endpoint ID.
- Iroh ships a **relay protocol called n0relays** for NAT traversal assistance and ciphertext-only relay fallback. Default relays are run by n0; self-hostable.
- Connection is QUIC + TLS 1.3 end-to-end. Hole-punching with QUIC Address Discovery (QAD).
- About 70% of consumer networks succeed at direct hole-punching; the rest fall back to relayed QUIC.
- `tonic-iroh-transport` exists — you can run **gRPC over Iroh**. This is huge for us because it means the local API and the remote API are the same protocol, just different transports.

> **V1.0 forward-pointer (2026-06-02):** Phase-1 spike 102 (`design/spikes/tonic-iroh-findings.md` §2) resolved the adapter to a **hand-rolled `tonic 0.12` ↔ Iroh-bidi-stream duplex adapter**, not the off-the-shelf `tonic-iroh-transport` crate (which forces `tonic 0.14`, conflicting with the workspace pin). The "gRPC over Iroh" thesis below holds exactly as written — only the adapter implementation differs. See `00 §6.6` / `11 §3.1.1` for the canonical decision.

**Pros**
- Solves NAT traversal completely. Hole-punch with relay fallback, all in one library.
- Public-key addressing = device identity is the key, no DNS, no certificate authorities.
- E2EE by default via TLS 1.3 with pinned device keys.
- **gRPC tunnels cleanly over it** via `tonic-iroh-transport`.
- Open source (Apache + MIT), self-hostable relays.
- Active development by a well-resourced team.

**Cons**
- Newer than the alternatives. Less battle-tested than Tailscale.
- Rust-only (with FFI for other languages, but they're less polished). If the Core were Go, this would be a problem.
- The relay is run by n0 by default. We'd want to either run our own or pay them for hosted relays for production.

### 9.2 Tailscale tsnet

Embed a full Tailscale node inside the Core. The Core "joins" a tailnet. Every paired client is a node on the same tailnet.

**Pros**
- **The gold standard for this problem.** Years of production use.
- DERP (Tailscale's relay) is more battle-tested than n0's relays.
- ACLs, exit nodes, MagicDNS, Funnel — features we'd never build ourselves.
- Self-hostable via **Headscale** (open-source Tailscale control plane).
- Go-native. If we picked Go for the Core, this is the obvious choice.

**Cons**
- **Requires the user to log in to Tailscale.** Even with our own control plane (Headscale), the user needs an account. For our "no account creation" principle, this is a friction point.
- The full Tailscale model is heavier than what we need. We don't want a mesh between *all* devices on the user's tailnet — we want a star: Core ↔ paired devices.
- Go-only — `tsnet` does not have a Rust port. If the Core is Rust, this means a separate Go sidecar process.
- More opinionated about identity and ACLs than we want.

### 9.3 Roll-our-own WireGuard tunnels

Use `wireguard-rs` or `boringtun` directly. Build the control plane ourselves.

**Pros**
- Maximum control.
- WireGuard itself is rock solid.

**Cons**
- We have to build the control plane, key distribution, NAT traversal, relay infrastructure ourselves. **Don't.**

### 9.4 WebRTC (data channels)

What Happy Coder effectively uses (their server proxies blobs; in a pure WebRTC version, data channels would carry blobs P2P).

**Pros**
- Browser support is universal.
- Mature NAT traversal (STUN + TURN, ICE).
- Native libraries on iOS (WebKit) and Android (libwebrtc).

**Cons**
- WebRTC is a media-first stack. Its data channel API is awkward, the JS bindings are stateful and weird, and Safari support has edge cases.
- Setting up a WebRTC signaling server is still on us.
- Native WebRTC libraries on mobile are very heavy.
- Less elegant than QUIC; you give up backpressure semantics you'd want.

### 9.5 Recommendation

**Iroh, with our own self-hosted relay for production.**

Reasoning:
- It solves NAT traversal completely.
- `tonic-iroh-transport` lets us run gRPC over it, so the local socket and the remote tunnel use the same RPC schema.
- Public-key addressing matches our "Core is its own identity provider" principle.
- The crypto is QUIC + TLS 1.3 + Ed25519 device keys, which is exactly what we'd build ourselves (worse) if we tried.
- If we self-host the relay, no third party sees any of our traffic.

**Fallback if Rust is dropped:** Go + tsnet + Headscale. This is the "we copy Tailscale entirely" path. Works, but Tailscale account friction is a thing.

**For the web client specifically:** Iroh's browser story is in development but not as mature as the native story. For V1.0 web, I'd consider a separate path: the web client connects to the Core over a small WebSocket bridge that the Core opens for browser clients. This is fine because the web client is the lower-traffic surface; the bridge is server-only and the data stays on the Core.

---

## 10. End-to-end encryption (crypto primitives)

Independent of the transport, we want a clear answer to "what protects the bytes."

### 10.1 Noise Protocol Framework (via `snow` crate)

**What it is.** A framework for building cryptographic protocols. Used by WireGuard, used by Lightning, used by I2P. The Noise IK pattern is a one-RTT mutually-authenticated handshake.

- ✅ Audited, deployed at scale (WireGuard).
- ✅ Multiple Rust crates: `snow` (most popular), `noise-protocol`, `clatter` (no-std + post-quantum).
- ✅ Composable — pick your DH, cipher, and hash primitives.
- ❌ Lower level than MLS. You build the group/multi-device story yourself.

### 10.2 Messaging Layer Security (MLS, RFC 9420)

**What it is.** IETF-standardized E2EE for groups. **OpenMLS** is the Rust implementation.

- ✅ Standardized, designed for multi-device + multi-user from day one.
- ✅ Forward secrecy + post-compromise security.
- ✅ Audited, used by Wire, Cisco, AWS Wickr.
- ❌ Overkill for our pairing-and-streaming model. We don't have groups of 50,000.
- ❌ More complex to integrate.

### 10.3 TweetNaCl-style (X25519 + AES-256-GCM)

**What Happy Coder uses.**

- ✅ Simple, well-understood primitives.
- ✅ Tons of audited implementations.
- ❌ You're hand-rolling the protocol on top, which is where mistakes happen.

### 10.4 Recommendation

**Snow (Noise IK pattern), for the pairing handshake and session establishment.** Inside an Iroh QUIC stream this is partially redundant (QUIC has TLS 1.3 already), but the redundancy is intentional: Iroh's TLS is authenticated by the Iroh endpoint key, ours is authenticated by the **device pairing key** (which is what the user actually trusted via the QR scan). Two layers of authentication, two different secrets, makes the relay-compromise scenario robust.

Inside the Noise tunnel, use AES-256-GCM for symmetric encryption and BLAKE2b for hashing — the Noise spec's defaults.

For storage encryption (the local audit log, archived chat history): **AES-256-GCM with keys derived from the OS keychain.**

Skip MLS for V1.0 — revisit only if we add group/team features in V2.

---

## 11. Agent integration

Two main models — and we should support both depending on what the user wants.

### 11.1 Spawn the CLI as a subprocess

`claude` and `codex` are CLIs. We spawn them in a PTY (via `portable-pty`), feed them stdin, read stdout/stderr, parse output.

**Pros**
- Works with whatever authentication the user already has set up (Claude Pro, Max, API key).
- We track the upstream CLI's features automatically.
- Same binary the user would run by hand — easy to debug.

**Cons**
- We're parsing terminal output, which is fragile. The CLI's UI changes between releases.
- Permission approval flow goes through the CLI's UI, not ours, unless we intercept it.

### 11.2 Embed the Claude Agent SDK (Anthropic's library)

`@anthropic-ai/claude-agent-sdk` (Node) or `claude-agent-sdk` (Python). Same agent loop, tools, and context management that powers Claude Code, exposed as a programmable library.

**Pros**
- We control the loop. Tool approvals happen via our UI directly, no CLI intercept.
- Structured outputs — we get typed messages, not parsed text.
- Hooks, subagents, sessions are first-class APIs.

**Cons**
- **Proprietary license.** Free for individuals but commercial use is governed by Anthropic's commercial terms. We'd need to talk to Anthropic before shipping.
- Node or Python only — no Rust SDK. We'd need a sidecar process.
- Locks us to Claude — for Codex / Gemini we still need to spawn the CLI.
- Authentication is its own API key (or shared with Claude Code's auth on the same machine).

### 11.3 Recommendation

**Support both, with subprocess as the default and SDK as an opt-in advanced mode.**

- V0.1 / V1.0: subprocess the CLI. Works for Claude Code, Codex, Gemini CLI, and any future CLI. No Anthropic licensing concerns.
- V1.5+: add an opt-in "Embedded agent" mode for Claude that uses the Agent SDK. Better UX (cleaner tool approvals, structured streams) but adds a sidecar process.

The subprocess model is the well-trodden path for CLI-backed agent orchestrators.

---

## 12. MCP integration

MCP servers (Model Context Protocol) extend agents. The agents (Claude Code, Codex) already understand MCP — they read `~/.claude/mcp.json` / `~/.codex/config.toml` / project-level `.mcp.json`. Our job is to surface MCP server status, allow/deny, and discovery in our UI.

**No new library needed for the protocol itself** — MCP runs inside the agent process. Our involvement is:
- Read the agent's MCP config to display which servers are active.
- Add a panel where the user can install, configure, and toggle MCP servers.
- Forward project-level `.mcp.json` to the agent's working directory.

The official MCP SDK is at github.com/modelcontextprotocol — TypeScript, Python, Rust, Go, Java, C# clients are all available. If we ever ship our own MCP server (e.g., a `concerto_link_pr` tool the agent calls), use the SDK in whichever language we pick for the Core.

---

## 13. Terminal emulator (web side)

When the user opens the Terminal tab on the desktop or in the workspace's chat view, we need to render the agent's PTY output.

### 13.1 xterm.js

The standard. Used by VS Code, Replit, GitHub Codespaces, Cloudflare workers dashboard, Linode, basically every browser-based terminal.

- ✅ Mature, MIT-licensed, well-maintained.
- ✅ Add-ons for fit (auto-resize), search, web-links, WebGL renderer.
- ✅ `react-xtermjs` wrapper for clean React integration.

**Pick this.** There's no real alternative.

For the mobile diff viewer, **do not use xterm.js** on touch — the touch UX is wrong for terminals. Either render the terminal as plain text (read-only) on mobile, or build a custom touch-first terminal view.

---

## 14. Diff viewer

Two sub-decisions: the desktop diff and the mobile diff.

### 14.1 Desktop — Monaco Editor (Microsoft, VS Code's editor) in diff mode

- ✅ The VS Code diff experience, embedded.
- ✅ Excellent performance on large diffs.
- ✅ Inline comments via custom decorations.
- ✅ Side-by-side and unified views built in.
- ❌ Big bundle (~2 MB minified).
- ❌ Customizing the inline comment UI requires Monaco-internals familiarity.

**Alternative: CodeMirror 6 + `@codemirror/merge`.** Lighter, more modern API, smaller bundle. Strong choice if Monaco's bundle size is a concern.

**Recommendation: Monaco.** Familiarity with VS Code's diff is a UX win — users immediately know how to use it.

### 14.2 Mobile — custom SwiftUI / Compose / React Native components

Don't try to embed Monaco on mobile. Touch-first diff has its own pattern: swipe between files, pinch to zoom hunks, long-press a line to comment. Custom components per platform are the right call.

If we go React Native, **`react-native-syntax-highlighter`** + a custom diff renderer is the path. The diff parsing (unified diff format → hunks → annotated lines) is platform-independent — write it once in TypeScript, render it per-platform.

---

## 15. Push notifications

Same problem on both mobile platforms: we want to send the user a wakeup that fetches content from the Core (no payload in the push itself).

### 15.1 Apple Push Notification service (APNs)

- iOS only.
- HTTP/2 API. Send wakeup pushes (`content-available: 1`) and the OS wakes the app silently to fetch content.
- Authenticated via JWT signed with an Apple-issued P-256 key.

### 15.2 Firebase Cloud Messaging (FCM)

- Android (and works on iOS, but proxies through APNs).
- HTTP API or admin SDKs.
- Authenticated via OAuth + service account.

### 15.3 Expo Push Notifications

If we use React Native + Expo, **Expo's push service wraps APNs and FCM**, hiding credential management. The wakeup contains an Expo push token; Expo translates to APNs/FCM behind the scenes.

**Pros:** Massively simpler ops — no APNs certificates, no FCM service account JSON.
**Cons:** Adds Expo as a third party in the wakeup path. They can see "device X has a notification pending" but not the payload.

### 15.4 Recommendation

**For V1.0:** Expo push. Saves weeks of credential ops, fits the wakeup-only model fine (Expo sees the wakeup, not the payload). Document this clearly in the security model.

**For V1.5+:** Switch to direct APNs / FCM if enterprise customers require it. The expo-notifications library actually supports direct APNs/FCM as a backend swap, so this is mostly an ops change.

---

## 16. Build, distribution, signing

### 16.1 Desktop builds

**Tauri**'s own CLI produces:
- macOS: `.app` + `.dmg`. Sign with an Apple Developer ID certificate, notarize via Apple's notary service.
- Windows: `.exe` / `.msi`. Sign with a code-signing certificate (Sectigo, DigiCert, etc.); EV certs avoid SmartScreen warnings.
- Linux: `.AppImage`, `.deb`, `.rpm`, plus an `.flatpak` if we want Flathub distribution.

CI: GitHub Actions with platform-matrix builds. Each platform on a runner of that OS.

### 16.2 Auto-update

**Tauri's built-in updater** for V1.0. Full-binary download, signed manifest. Works.

**For V1.5 differential updates:** `electron-updater` if we're on Electron. For Tauri, the `tauri-plugin-updater` does the job; differential isn't yet supported but the bundles are small enough that it doesn't matter much.

### 16.3 Mobile builds

**Expo EAS Build.** Cloud-builds iOS and Android binaries from CI. Handles credentials, signing, App Store / Play Store submission via EAS Submit. Saves us from running a Mac in CI for iOS builds.

Alternative: GitHub Actions with `xcodebuild` and `gradle`. Cheaper at scale, more work to maintain.

### 16.4 Relay

**Single static Rust binary**, distributed as a Docker image and a raw binary. Run in our cloud (Fly.io or similar anycast provider for low-latency hole-punching). Self-hostable for enterprise.

---

## 17. The recommended stack (one page)

For people who skip to the end.

### Core daemon
- **Language:** Rust
- **Async runtime:** Tokio
- **Git:** `gix` for hot path, `git2` for gaps, shell out to `git` for clone / sparse-checkout / blobless
- **Storage:** SQLite via `sqlx`
- **Process supervisor:** Custom on top of `tokio::process` + `portable-pty`
- **Local IPC:** Tonic gRPC over Unix socket / named pipe
- **Secrets:** `keyring-rs` v4
- **Service discovery on LAN:** `mdns-sd`
- **Logging:** `tracing` + `tracing-subscriber`

### Remote transport
- **Library:** Iroh
- **Crypto on top:** Noise IK via `snow` (atop Iroh's own QUIC+TLS for defense in depth)
- **gRPC tunnel:** `tonic-iroh-transport`
- **Relay:** Self-hosted, deployed as a Rust binary on anycast hosts. Run our own fleet for V1; offer self-hosted for enterprise.
- **Pairing model:** QR code, Ed25519 device keys, copied from Happy Coder's flow

### Desktop client
- **Shell:** Tauri 2
- **Frontend:** React + TypeScript + Vite
- **State:** Zustand
- **UI components:** shadcn/ui + Tailwind
- **Diff viewer:** Monaco Editor (read-only with custom decoration layer)
- **Terminal:** xterm.js with `react-xtermjs`
- **Auto-update:** `tauri-plugin-updater`

### Mobile clients (V1.0)
- **Stack:** React Native + Expo
- **Push:** Expo Push (wraps APNs / FCM)
- **Voice:** `expo-speech` + native fallback (Apple Speech Recognition / Android SpeechRecognizer for advanced cases)
- **Diff viewer:** Custom React Native component, parses unified diffs server-side
- **Build:** EAS Build + EAS Submit

### Web client
- **Stack:** Same React + TypeScript code as the desktop client, served by the Core
- **gRPC transport:** Connect-Web (buf.build/connect) with HTTP/SSE fallback
- **Hosting:** Served by the Core itself when on LAN; through the relay when remote

### Agent integration (V1.0)
- **Default:** Spawn Claude Code / Codex / Gemini CLI as subprocesses in PTY
- **V1.5 stretch:** Add an "Embedded Claude" mode using the Claude Agent SDK (Node sidecar)

### MCP
- Read existing MCP configs (~/.claude/mcp.json, ~/.codex/config.toml, project .mcp.json)
- Surface in UI
- Ship our own server for "concerto_link_pr" (and similar Concerto-specific tools) using `modelcontextprotocol/typescript-sdk`

### Build & distribution
- **CI:** GitHub Actions, platform-matrix
- **Mac signing:** Apple Developer ID + notarytool
- **Windows signing:** Sectigo or DigiCert standard code-sign (consider EV later)
- **Linux:** AppImage as primary, .deb and Flatpak as secondary
- **Mobile:** EAS Build → App Store / Play Store
- **Relay:** Docker image + raw binary, deployed on Fly.io anycast

---

## 18. Decisions deferred to a prototype

A handful of choices I'd punt to a 1–2 week prototype rather than decide on a document:

1. **Tauri vs. Electron on Linux specifically.** Build the same 10-screen prototype on both. If WebKitGTK's quirks are visible, Electron for Linux is fine.
2. **Iroh's NAT-traversal success rate on real corporate networks.** Spin up the relay, get 10 engineers in 10 different network environments to pair, measure direct-connection rate. If it's below 60%, reconsider tsnet.
3. **React Native diff viewer performance** on a 1000-line diff. If scrolling chops, switch the mobile diff path to per-platform native (SwiftUI + Compose).
4. **gix vs. shell-out latency** for the operations we hit most. Benchmark `gix status` on a worktree of a 40 GB monorepo with a sparse cone. If gix is faster than 100 ms, it's the hot-path winner. If not, more shell-out usage.
5. **Tonic over Iroh in practice.** Build a Hello World and stream 10 MB of agent stdout across a real coffee-shop Wi-Fi to a real phone. Measure latency and throughput. If it's under 200 ms p50 and over 1 MB/s, ship it.

Each of these is a 2–5 day spike. Do them in week 1–2 of V0.1 before committing to anything that's hard to change later.

---

## 19. What we're explicitly not building

Some "obvious" things we're not building, with reasons:

- **Our own LLM.** We orchestrate over Claude Code, Codex, Gemini CLI. We are not in the model business.
- **Our own MCP protocol.** MCP is the industry standard; we use it.
- **Our own git server.** Push goes to GitHub (or GitLab, Bitbucket, etc.) through the normal git protocol.
- **Our own VPN.** Iroh handles the encrypted tunnel.
- **Our own keyring.** OS keychain via `keyring-rs`.
- **Our own diff algorithm.** Git computes diffs; we render them.
- **Our own terminal protocol.** ConPTY / Unix PTY via `portable-pty`.
- **Our own auth provider.** Device pairing is our identity model. No accounts.
- **Our own analytics.** OpenTelemetry, off by default, opt-in.

Each "no" is a place we save engineering time.

---

*End of document. The actual stack is the one in section 17 unless a prototype in section 18 surfaces a reason to change it.*
