# 18 — Distribution & Operations

*Sub-system design doc. Inherits the locked decision from `00_Architecture_Overview.md` §6.11 (MIT for the whole monorepo, DCO contributions, hosted-vs-self-hosted parity). Where the other 17 docs describe **what Concerto does**, this one describes **how Concerto reaches users** and **which parts of the system are operated by the company** versus shipped to anyone who wants them.*

---

## 1. Purpose & scope

Concerto is two things at once:

1. **An open-source project** that any developer can clone, build, run, audit, fork, sideload, or self-host end-to-end with no Concerto Inc involvement.
2. **A company-operated offering** consisting of the hosted relay fleet, the published App Store / Play Store builds, the update server, and (V2.0+) a hosted skills marketplace and enterprise extension modules.

This doc enumerates exactly which artifacts belong to which side of that line, so that every other sub-system doc can answer "is this OSS or hosted?" without re-deriving the policy. It also defines the operational concerns that aren't owned by any single sub-system: release signing, code-signing certificates, the dependency-license CI gate, telemetry policy, the trademark, and the contribution workflow.

It owns:

- **The OSS scope** — what's in the monorepo, under what license, with what dependency rules.
- **The company scope** — what Concerto Inc operates in production, what it does *not* see, and what it sells.
- **Self-host parity** — the guarantee that every product capability except App Store / Play Store distribution under the Concerto name is self-hostable from source.
- **Release & signing** — how binaries get built, signed, notarized, and shipped to users; who holds the keys.
- **Telemetry policy** — what data may leave a user's machine and under what conditions.
- **Contribution model** — DCO sign-off, security disclosure, trademark guidance for contributors.
- **Enterprise-module seam** — the architectural pattern by which future BSL/FSL plugins extend the MIT Core without forking it.
- **Acqui-hire optionality** — what an acquirer would actually be buying that isn't already MIT-licensed and free.

It does **not** own: the OSS code itself (that's the other 17 docs); the business model (PRD-adjacent); the legal text of the license (that's `LICENSE` at the repo root).

---

## 2. Phase scope

| Phase | What ships |
|---|---|
| **V0.1** | `LICENSE` (MIT) + `NOTICE` + `THIRD_PARTY_LICENSES.md` (generated) + `CONTRIBUTING.md` (DCO) + `SECURITY.md` + `TRADEMARKS.md`. `cargo deny` license-gate in CI. Code-signing certs provisioned for macOS notarization. No hosted offering yet (alpha is local-only). |
| **V1.0** | + Concerto Inc operated relay fleet (Fly.io anycast, see `11 §3.2`). + Expo Push project operated by Concerto Inc. + iOS App Store + Android Play Store publishing under Concerto Inc developer accounts. + update server signing keys held by Concerto Inc. + opt-in OTLP telemetry shipped (off by default). + the "Concerto Pro" mobile subscription is the first paid surface (gates the *Concerto-published* mobile builds and the hosted relay tier; self-hosters and side-loaders bypass it entirely by definition). |
| **V2.0** | + first enterprise extension module (managed-CA, see `12` V2.0) loaded as a BSL plugin against MIT-defined trait surfaces. + SIEM forwarder plugin. + hosted skills marketplace as a separate Concerto Inc service. + reproducible-build attestations (SLSA-style) so self-hosters can verify they're building the same bits we publish. |

---

## 3. Key design decisions

### 3.1 What's open vs. what's operated — the boundary

The single most important table in this doc. Sub-system docs should be consistent with it.

| Artifact | Source license | Who operates the production instance |
|---|---|---|
| `concerto-core` daemon binary | MIT | The user, on their machine |
| `concerto-relay` binary | MIT | Concerto Inc (hosted default) **or** the user (self-host) |
| `concerto-desktop` Tauri app | MIT | The user, on their machine |
| `concerto-tray` sidecar | MIT | The user, on their machine |
| `concerto-agent-host` helper | MIT | The user, on their machine (spawned by Core) |
| `concerto-cli` | MIT | The user, on their machine |
| `concerto-ios` source | MIT | n/a (App Store distribution is Concerto Inc's account; self-builds via TestFlight) |
| `concerto-android` source | MIT | n/a (Play Store distribution is Concerto Inc's account; sideload APK works) |
| Web client source | MIT | The user's Core serves it on `127.0.0.1` (LAN) **or** Concerto Inc serves the WSS bridge build at `relay.concerto.app` |
| protobuf `.proto` schemas | MIT | n/a (definitions) |
| Skills SDK / registry format | MIT | n/a (definitions); hosted marketplace (V2.0) is Concerto Inc operated |
| Hosted relay anycast fleet | (operates MIT binary) | **Concerto Inc only** |
| Expo Push project (V1.0) | (operates third-party) | **Concerto Inc only**; self-hosters bring their own Expo / APNs / FCM credentials |
| Update-manifest signing keys | n/a (keys) | **Concerto Inc only**; self-builds use the builder's own signing keys |
| macOS / Windows code-signing certs | n/a (keys) | **Concerto Inc only**; self-builds are ad-hoc-signed or use the builder's certs |
| App Store / Play Store listing | n/a (account) | **Concerto Inc only**; the "Concerto" name and icon are trademarked |
| Hosted skills marketplace (V2.0) | (operates MIT registry protocol) | **Concerto Inc only**; self-hosters can point at any registry URL |
| Enterprise extension modules (V2.0) | **BSL / FSL** | Concerto Inc (sold per-seat); not in the MIT monorepo |

The principle: **anything that costs Concerto Inc money to run** (anycast bandwidth, App Store fees, Expo quotas, code-signing certs, update infrastructure, marketplace moderation) is what justifies the company's existence and revenue. Anything that runs on a user's own machine is MIT, free, and self-hostable.

### 3.2 Self-host parity guarantee

A self-hoster who follows the build instructions can stand up the entire Concerto stack on their own infrastructure with **no Concerto Inc involvement**, with one specific exception:

> They cannot publish under the "Concerto" name on the iOS App Store or Google Play Store — the trademark and the developer accounts are Concerto Inc's. They can:
> - Distribute internal builds via TestFlight (iOS, 100-tester limit free; enterprise distribution unlimited with Apple Developer Enterprise Program).
> - Distribute internal builds via Play Console internal-testing tracks, internal app sharing, or direct APK sideload.
> - Publish a renamed fork on either store under their own developer account.

Everything else is parity:

| Capability | Concerto Inc operates | Self-hoster does |
|---|---|---|
| Hosted relay | Anycast, monitored, SLA | Run `concerto-relay` on Fly.io / their VPC / a Raspberry Pi |
| Push notifications | Concerto's Expo project | Bring own Expo account, or wire direct APNs/FCM creds in `managed.json` |
| Update server | `updates.concerto.app` | Self-host a static `update.json` + signed binaries, point Tauri updater at it |
| Skills marketplace | Hosted at `marketplace.concerto.app` | Point at any git URL exposing a `marketplace.json` (already supported in `06 §3.3`) |
| Mobile apps | Concerto Inc publishes to App Store / Play Store | Build from source, TestFlight or sideload under your own name |
| Audit forwarding | Optional SIEM endpoint (V2.0 module) | OSS syslog forwarder ships in `09` |

The audit-log SIEM forwarder is a good worked example: the basic syslog/HTTPS forwarder ships in the MIT codebase (`09 §3.5`). The *managed* SIEM module (multi-tenant, encrypted at rest, compliance-ready) is a V2.0 BSL extension. Both consume the same `AuditLogSubscriber` trait.

### 3.3 Release & signing

**Where binaries come from:**

- **CI builds.** GitHub Actions matrix produces binaries for Mac (universal2), Windows (x64 + arm64), Linux (x64 + arm64). Mobile builds via EAS Build.
- **Signing.** macOS binaries notarized with Apple Developer ID held by Concerto Inc. Windows binaries signed with an EV code-signing cert held by Concerto Inc. iOS and Android via their respective store signing.
- **Update manifest.** A `updates.json` signed with an Ed25519 key held by Concerto Inc (`tauri-plugin-updater` verifies). Self-builds disable auto-update or point at the builder's own update server.
- **Reproducibility (V2.0).** Aim for SLSA Level 3: deterministic builds where possible, attestations published, source-to-binary auditable. This is the moat against supply-chain compromise *and* the proof that the binaries match the OSS source.

**What an acquirer would actually get** (relevant to the "acqui-hire" path discussed in business-model planning):

1. The "Concerto" trademark.
2. The Apple Developer Program ID and the iOS app's App Store presence.
3. The Google Play Developer account and Android listing.
4. The macOS code-signing Developer ID and the Windows EV cert.
5. The update-manifest signing private key (controls what existing installs auto-update to).
6. The relay anycast operation (Fly.io account, anycast IP allocations, monitoring).
7. The `concerto.app` (or equivalent) domain.
8. The Expo Push project ID and any device-token registrations.
9. Any V2.0+ enterprise extension modules (BSL crates, customer contracts, support relationships).
10. The contributor pool's contact list and the GitHub org admin rights.

The MIT code itself is, of course, already free. The "company" is everything in this list that isn't.

### 3.4 Telemetry policy

**Locked:** No telemetry leaves a user's machine by default. There is no analytics SDK linked into any binary. No crash reporting service phones home.

The only data Concerto Inc ever sees in production V1.0:

| Data | Source | Purpose | Justification |
|---|---|---|---|
| Source IP, ciphertext byte counts, NAT-traversal success/fail | Hosted relay (when used) | Operate the relay fleet; debug hole-punching | Per `11` — necessary to run the service; never includes payload |
| Wakeup IDs | Expo Push (when used) | Deliver push notifications | Per `14 §3.2` — payload is wakeup-only, no content |
| Anonymous app version pings | Tauri updater (when used) | Compute update rollout health | Aggregated; no device ID, no user ID |
| Voluntary OTLP traces | OTLP exporter (only when user enables it) | Diagnostics; SIEM integration for enterprise | Off by default; endpoint configurable in `managed.json` |
| App Store / Play Store crash reports | Apple / Google (for the Concerto-Inc-published builds only) | Bug triage | The user accepted this by installing from the store; not in the source-built version |

Anything else — and especially anything resembling product analytics, behavioral telemetry, or usage tracking — is **out of scope forever**, locked at the architecture level. Adding it would invalidate the security pitch and is treated as a one-way door.

### 3.5 Dependency-license gate (CI-enforced)

`00 §6.11` locks the permitted dependency licenses. This sub-system owns the *enforcement*:

- **Rust:** `cargo deny check licenses` runs on every PR. Allowed: MIT, Apache-2.0 (with NOTICE), BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, Unicode-DFS-2016, Zlib. Denied (CI fails): GPL-2.0+, LGPL-2.1+, AGPL-3.0+, SSPL-1.0, BUSL-1.1, MS-RL, anything in the `cargo-deny` "copyleft" or "restricted" categories.
- **JS / TS (clients):** `pnpm licenses list --json` → script gates the same allow-list.
- **Swift / Kotlin (native mobile bridges):** manual review at PR time; small surface so manageable.
- **Generated artifact:** `THIRD_PARTY_LICENSES.md` regenerated by CI via `cargo about generate` + pnpm-licenses script, committed back to main on release tags.

The Claude Agent SDK opt-in path (`04 §3.3`, V1.5) requires a separate license review at the time it's added, because it ships as a Node runtime sidecar (different license context than a Rust crate). Per `04`, subprocess remains the primary backend regardless — the SDK is never the only path.

### 3.6 Contribution model — DCO, no CLA

- **DCO sign-off required.** Every commit signs off via `git commit -s`, attesting that the contributor has the right to contribute the change. Linux kernel uses this; well-understood; doesn't require Concerto Inc to hold any rights the contributor didn't grant explicitly.
- **No CLA.** Concerto Inc does not collect a Contributor License Agreement. This means we **cannot unilaterally relicense** the MIT codebase — a future BSL/FSL flip would require all contributors' consent or rewriting their contributions.
  - This is **intentional** and matches the locked decision in `00 §6.11`. It signals long-term commitment to MIT for the existing code, and removes the "they'll BSL us eventually" suspicion that CLAs increasingly trigger.
  - The escape valve, if we ever need it, is to ship new enterprise modules as **separate crates** under BSL/FSL (per `00 §6.11`'s "Future enterprise modules" row). Existing MIT crates stay MIT.
- **Security disclosure:** `SECURITY.md` lists a security email, PGP key, and 90-day disclosure SLA.
- **Code of conduct:** Contributor Covenant; standard, well-known.
- **Trademark guidance for contributors:** explained in `TRADEMARKS.md`. Contributors can use "Concerto" in factual references (e.g., "compatible with Concerto 1.4") but cannot ship forks under the Concerto name.

### 3.7 Enterprise-module seam — the BSL plugin pattern

Future enterprise features (managed-CA, SIEM, advanced policy, SSO) are designed to ship as **separate crates** loaded at runtime by the MIT Core. The MIT Core only knows about trait surfaces; the BSL crates implement them.

**Pattern:**

1. Define the trait in the MIT crate (e.g., `audit::AuditLogSubscriber` in `crates/core/src/audit/mod.rs`).
2. Ship a default OSS implementation in the MIT crate (e.g., `SyslogSubscriber`, `JsonlSubscriber`).
3. The enterprise crate (e.g., `crates/enterprise-siem`, BSL-licensed) ships a richer implementation (e.g., `SiemForwarderSubscriber` with at-rest encryption, multi-tenancy, retry logic).
4. The Core loads enterprise crates via a `cargo` feature flag at build time, or via a dynamic-library path at runtime (V2.0+), gated by license-key verification *only inside the BSL crate* — the MIT Core is never license-gated.

**Trait surfaces designed to be plugin seams** (each sub-system doc should call out which it owns):

| Trait | Owner | OSS impl | Enterprise impl (V2.0+) |
|---|---|---|---|
| `AuditLogSubscriber` | `09 §3.5` | Stdout, JSONL, syslog | SIEM forwarder, encrypted-at-rest writer |
| `DeviceCertIssuer` | `12 §3.2` | Self-issued by local Core | Org-managed CA, MDM-integrated |
| `SkillRegistrySource` | `06 §3.3` | Git-URL marketplaces | Hosted enterprise marketplace, allow-list-enforced |
| `SuggestionRuleSource` | `07 §3.2` | Local TOML + bundled defaults | Org-shared rule distribution |
| `VcsProvider` | `13 §3.8` | GitHub (octocrab + gh) | GitLab, Bitbucket, Gerrit, GitHub Enterprise variants |
| `PushBackend` | `14 §3.6` | Expo Push | Direct APNs/FCM, on-prem push gateway |
| `IdentityIssuer` | `12 §3.2` | Ed25519 self-signed | SAML / SCIM / OIDC bridge |

The cost of designing these traits up front (~2 days each, mostly bikeshedding the function signatures) is dramatically less than retrofitting them. Sub-system docs should treat these as required architectural elements, not optional polish.

### 3.8 Web-client trust note

The web client deserves a specific call-out because its trust model differs from desktop/mobile:

- **LAN-direct (Core serves on `127.0.0.1`):** entirely on the user's machine. No Concerto Inc surface.
- **Remote via WSS bridge:** the bridge is the relay (Concerto Inc operated in the default deployment, or self-hosted). The relay still sees only ciphertext per `11 §3.4`. The web client's JavaScript is also served by the relay in this mode — meaning the relay operator (Concerto Inc, or the self-hoster) **can in principle modify the JS bundle they serve**.

For the hosted Web client, this means Concerto Inc has the technical ability to push a backdoored JS bundle to a user's browser. We mitigate by:
- Publishing Subresource Integrity hashes for every released web build.
- Pinning the web build to a specific release tag at the relay; rollouts go through the same signing key as desktop updates.
- (V2.0) Reproducible builds + transparency log so users can verify the served JS matches a tagged release.

Self-hosters running their own relay control their own web bundle. This is one of the strongest reasons enterprises will choose self-hosted relay for sensitive deployments.

---

## 4. Repo-root artifacts (the operational truth)

The following files live at the monorepo root and codify the policies above. They are mentioned here so sub-system docs can reference them without redefining their content.

| File | Content | Lifecycle |
|---|---|---|
| `LICENSE` | MIT license text, copyright "Concerto Contributors" | Committed; never modified except for copyright-year roll |
| `NOTICE` | Aggregate Apache-2.0 attribution + Concerto's own copyright line | Committed; regenerated on release |
| `THIRD_PARTY_LICENSES.md` | Generated full list of dependency licenses | Regenerated by CI on every release tag |
| `CONTRIBUTING.md` | DCO instructions, branch / PR norms, build setup, code-of-conduct pointer | Committed; updated when workflow changes |
| `SECURITY.md` | Disclosure policy, security email + PGP fingerprint, SLA | Committed; updated when contacts rotate |
| `TRADEMARKS.md` | "Concerto" trademark policy; nominative use OK, brand use requires permission | Committed; rarely changes |
| `CODE_OF_CONDUCT.md` | Contributor Covenant | Committed; rarely changes |
| `.github/workflows/*` | CI, including license gate (`cargo deny`), build matrix, release signing | Versioned with the code |
| `deny.toml` | `cargo deny` config (allowed licenses, denied licenses, advisory policy) | Versioned with the code |

---

## 5. Interfaces

### 5.1 To sub-system docs

This doc is mostly policy, but it does define one normative interface: the **enterprise-module trait registry** in `§3.7`. Each sub-system doc that owns one of those trait surfaces is expected to:

1. Define the trait in its own crate (so it's MIT and usable by the OSS Core).
2. Document the trait in the doc's §5 ("Interfaces") section.
3. Implement at least one OSS variant.
4. Reserve the right to ship one or more BSL/FSL variants in V2.0+ without rewriting the trait.

Sub-systems that currently own a seam: `06 Skills`, `07 Suggestions`, `09 Persistence` (audit subscriber), `12 Security` (cert issuer, identity issuer), `13 VCS` (provider), `14 Notifications` (push backend).

### 5.2 To the build & release pipeline

No emitted events; this is a build-time concern, not a runtime one. Relevant CI surface lives in `.github/workflows/`:

- `ci.yml` — type/lint/test on every PR
- `licenses.yml` — `cargo deny` + JS license check (separate job for clear failures)
- `release.yml` — signed builds, notarization, update-manifest signing
- `licenses-regen.yml` — regenerate `THIRD_PARTY_LICENSES.md` on release tags

### 5.3 To Concerto Inc operations (out-of-band)

Operational concerns documented separately (not in this repo):

- Anycast Fly.io account credentials
- Apple Developer Program ID + signing certs
- Google Play Developer account
- Expo project tokens
- Update-manifest signing private key
- Domain registrations (`concerto.app`, etc.)
- Trademark registration documents

These are the "everything that isn't free" inventory from `§3.3`. Treat them as the durable assets of Concerto Inc.

---

## 6. Internal architecture

There is no internal architecture for this sub-system in the Core. It's policy + repo-root files + CI + Concerto Inc's external operational accounts. Sub-systems that *implement* the policy (the trait seams in `§3.7`) live in their own docs.

---

## 7. Sequence diagrams — none required

Distribution is a build-and-release flow, not a runtime one. The release flow is documented in `.github/workflows/release.yml` rather than here, since CI YAML is more useful to release engineers than prose.

---

## 8. Error handling & failure modes

| Failure | Detection | Response |
|---|---|---|
| Contributor opens PR without DCO sign-off | CI bot (DCO check) | Comment with instructions; block merge until rebased with `-s` |
| New dependency uses GPL/AGPL/SSPL | `cargo deny` CI gate | Build fails; PR author must find a replacement or justify (rare exceptions discussed in PR) |
| Code-signing cert expires | Renewal calendar reminder; release build fails | Pre-rotate 30 days before expiry; release pipeline blocks if cert is invalid |
| Update-signing key compromised | Manual incident detection | Out-of-band: revoke key, ship a fresh signed installer with new key embedded, blog post, security disclosure |
| App Store rejection of mobile build | Apple/Google review feedback | Address per their guidelines; may delay release; never blocks self-build / sideload path |
| Self-hoster reports breaking change | Issue tracker | Treat as a P1 — self-host parity is the architectural guarantee |
| Hosted relay outage | Monitoring | Status page + LAN/self-host paths still work — every Concerto deployment that has wired up a self-hosted relay is unaffected |
| Trademark misuse (fork shipping as "Concerto") | Community report | Friendly request to rename per `TRADEMARKS.md`; escalate only if necessary |

---

## 9. Dependencies on other sub-systems

| Sub-system | How |
|---|---|
| **00 Architecture** | This doc derives from `00 §6.11`'s locked decision |
| **All sub-systems** | Each owns part of the "what's MIT vs. operated" boundary (`§3.1`) and possibly a trait seam (`§3.7`) |
| **09 Persistence** | The audit-log subscriber trait is the prototypical example seam |
| **11 Transport** | The hosted vs. self-hosted relay distinction is the largest operational concern |
| **12 Security** | Future managed-CA is the prototypical enterprise extension module |
| **14 Notifications** | Expo Push project is operated by Concerto Inc; self-hosters bring own creds |

This doc has no upward dependencies; it is consumed (read) by every other sub-system.

---

## 10. Testing strategy

| Layer | What | How |
|---|---|---|
| Policy compliance | License gate works | Inject a test dep with a denied license; assert CI fails |
| Build reproducibility (V2.0) | Same source produces same binary | Build twice from clean state; diff |
| Signing | Notarization passes; updater verifies signature | Per-release smoke test |
| Self-host parity | Following `CONTRIBUTING.md` `# Self-hosting` section produces a working stack | Periodically (once per release cycle) walk a fresh machine through the instructions |
| Trait seam contract | Each `§3.7` trait has at least one OSS impl and a test fixture for swap | Trait-level unit tests in each owning crate |

---

## 11. Open questions / deferred

| # | Question | Decision | Where |
|---|---|---|---|
| O-1 | Should the relay binary's source ship under MIT or BSL? | **MIT.** Per `00 §6.11`. Letting competitors run our relay code is fine; they'd still have to operate it, and Concerto Inc's relay fleet is the moat (uptime, anycast, monitoring), not the bits. | §3.1 |
| O-2 | Mobile apps: free or paid? | **Free to install in V1.0; "Concerto Pro" subscription gates the Concerto-published mobile builds at $8–12/mo** per the recommended hybrid business model. Self-hosters who sideload bypass entirely. Final pricing is a PRD/marketing concern, not a design one. | §2 (V1.0) |
| O-3 | Should we publish a Helm chart / Docker Compose for self-hosters? | **V1.5.** V1.0 documents a manual binary install; V1.5 adds a Compose file for the relay + a Helm chart for org self-hosts. | (V1.5) |
| O-4 | SLSA level target | **Level 3 by V2.0.** Reproducible builds + attestations. V1.0 ships Level 1 (signed but not reproducible). | §3.3 (V2.0) |
| O-5 | When (if ever) to introduce a CLA | **Never for the existing MIT codebase.** New enterprise crates ship under BSL/FSL with their own contribution model if needed. | §3.6 |
| O-6 | Donations / sponsors program | **Open GitHub Sponsors page from V1.0.** Cost is zero; signals OSS legitimacy. Funds go to a designated account, not founder personal. | (operational) |
| O-7 | Reproducible builds in V1.0 | **Best-effort, not blocking.** Real reproducibility is V2.0. | §3.3 |

---

*End of `18_Distribution_and_Operations.md`. The locked source-of-truth is `00 §6.11`; the trait seams referenced in §3.7 are owned by their respective sub-system docs.*
