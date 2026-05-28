# Security Policy

Concerto handles user code, credentials, and agent-driven actions on behalf of developers. We take security disclosures seriously and ask the community to do the same.

## Reporting a vulnerability

**Please do not file a public GitHub issue for security problems.** Public reports put users at risk before a fix is available.

Email **security@concerto.app**.

You can encrypt your report with our PGP key:

- **Key ID:** *(to be published with the V0.1 release)*
- **Fingerprint:** *(to be published with the V0.1 release)*
- **Key URL:** https://concerto.app/security.asc

Include:

- A clear description of the issue and its potential impact.
- Steps to reproduce, including affected versions (Core, desktop, mobile, relay, web).
- Whether you've shared details with anyone else.
- Whether you'd like credit in the public advisory.

## Our commitment

- **Acknowledgement:** within **2 business days** of report.
- **Initial assessment:** within **7 business days**.
- **Public disclosure:** coordinated with the reporter, normally within **90 days** of initial report, sooner if the issue is actively exploited.
- **Credit:** named in the advisory and changelog if you'd like, anonymous if you'd prefer.
- **No legal action against good-faith research.** If you follow this policy and don't access data beyond what's needed to demonstrate the issue, we won't pursue you.

## In scope

- The `concerto-core` daemon and any code in this repository.
- The hosted relay at `relay.concerto.app` (operated by Concerto Inc).
- The published iOS and Android applications.
- The hosted web client at `relay.concerto.app/c/*`.
- The update-manifest signing pipeline.

## Out of scope

- Vulnerabilities in third-party software we depend on (please report those upstream; we'll patch when an upstream fix lands).
- Self-built or self-hosted forks operated by parties other than Concerto Inc — those are the operator's responsibility.
- Reports based solely on missing security headers or hypothetical attacks without a demonstrated impact.
- Denial-of-service attacks against the hosted relay or any third-party service (please don't test these against production).

## What we will and won't do

**Will:**
- Treat reasonable disclosures gratefully and act on them quickly.
- Publish a CVE when warranted.
- Backport fixes to recent stable releases when feasible.

**Won't:**
- Offer paid bug bounties at this time (V0.1 / V1.0 phase). We may add a structured program later.
- Negotiate over the embargo period in bad faith.

## Hall of fame

Researchers who report valid vulnerabilities will be acknowledged at https://concerto.app/security/credits (when published) and in the relevant CHANGELOG entries.

Thank you for helping keep Concerto users safe.
