//! Inbound-webhook ingest: HMAC verify + delivery-id idempotency + event parse +
//! targeted cache invalidation (Task 315, `design/13 §3.2`/§6.2/§6.3/§8).
//!
//! The relay forwards an opaque GitHub POST body to the Core over the `0x04`
//! Webhook channel (`design/11 §3.4.1`); the Core verifies + processes it here.
//! The pipeline order is FROZEN (`design/13 §6.2`):
//!
//!   1. **Idempotency first** — dedupe on the `delivery_id` via the
//!      restart-surviving `webhook_deliveries` table (migration 0013). A replay
//!      is dropped with a `200` ack (so GitHub stops retrying the dupe) and
//!      never touches the secret or the parser.
//!   2. **HMAC verify** — recompute HMAC-SHA256 over the **raw body** with the
//!      per-repo `VcsSecretSlot::WebhookSecret` and **constant-time-compare**
//!      against `X-Hub-Signature-256` (`hmac::Mac::verify_slice`, never `==`). A
//!      mismatch / missing-secret / missing-signature is dropped + logged with
//!      **no sender-visible reason** (`design/13 §8`); ack `4xx`.
//!   3. **Parse** the event by `event_type` into the minimal shape the caches
//!      need; an unknown type is a no-op `200` (forward-compat).
//!   4. **Targeted cache invalidation** (`design/13 §6.3`) — drop just the
//!      affected PR / check / deployment cache rows so the next read (or 316's
//!      event emission) refreshes from origin; best-effort eager re-fetch + emit
//!      when a provider is available. A webhook-path failure here NEVER breaks
//!      the poll path (`design/13 §3.2`): a parse/invalidate error still acks
//!      `200` (the webhook was authentic; the accelerator just no-op'd).
//!
//! The HMAC secret material lives ONLY in the keychain (`VcsSecretSlot::
//! WebhookSecret`, D4); this module reads it through the [`WebhookSecretSource`]
//! seam so the Tier-2 tests inject a fake secret without touching the OS keychain.

use async_trait::async_trait;
use concerto_error::Result;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// The Core-side ingest result, mapped by the transport `WebhookSink` to a
/// [`WebhookAck`](concerto_transport-equivalent) byte. Kept proto/transport-free
/// so `crates/vcs` stays a leaf (the Core maps it to the transport ack).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOutcome {
    /// Accepted: HMAC verified + processed, OR idempotently deduped, OR an
    /// authentic-but-unparseable/unknown event (forward-compat). Ack `200`.
    Accepted,
    /// Rejected: HMAC mismatch, missing secret/signature, or a malformed
    /// envelope. Dropped + logged, **no sender-visible reason** (`design/13 §8`).
    /// Ack `4xx`.
    Reject,
    /// Core-internal error (e.g. the idempotency DB write failed) after an
    /// otherwise-valid frame. Ack `5xx`; GitHub redelivers.
    Error,
}

/// The Core-side webhook payload `VcsHandle::ingest_webhook` takes (`design/13
/// §5.1`). Carries the envelope fields the relay forwarded (`design/11 §3.4.1`);
/// `endpoint_id` is asserted by the transport layer before this is built, so it
/// is not re-carried here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookPayload {
    /// The `X-GitHub-Delivery` UUID — the idempotency key.
    pub delivery_id: String,
    /// The `X-Hub-Signature-256` header value (`sha256=<hex>`); empty when GitHub
    /// omitted it (a misconfigured hook) — treated as an HMAC failure.
    pub signature_256: String,
    /// The `X-GitHub-Event` type (`pull_request`/`check_run`/…).
    pub event_type: String,
    /// The raw GitHub POST body bytes (HMAC is computed over exactly these).
    pub body: Vec<u8>,
}

/// The keychain seam for the per-repo webhook secret (`VcsSecretSlot::
/// WebhookSecret`, D4). Production wires the OS keychain; the Tier-2 tests inject
/// a fixed secret. Returns `None` when no webhook secret is configured for the
/// repo (⇒ the webhook is dropped + logged; the hook is simply not set up).
#[async_trait]
pub trait WebhookSecretSource: Send + Sync + 'static {
    /// The raw HMAC secret bytes for `repo_id`, or `None` when unconfigured.
    async fn webhook_secret(&self, repo_id: &str) -> Result<Option<Vec<u8>>>;
}

/// The seam that resolves a [`VcsProvider`](crate::provider::VcsProvider) for a
/// repo so the targeted-invalidation path can **eagerly re-fetch + emit** the
/// fresh state (`design/13 §6.3`). Production wires the keychain-PAT-backed
/// octocrab provider; tests inject a `FakeGitHub`-backed one. Returning `None`
/// (no token / no provider) is fine — the cache rows are still dropped, so the
/// next poll/read refreshes (the webhook is a strict accelerator; the poll path
/// never depends on it).
#[async_trait]
pub trait WebhookProviderSource: Send + Sync + 'static {
    /// A provider for `repo_full_name`, or `None` when no credential is wired.
    async fn provider_for(
        &self,
        repo_full_name: &str,
    ) -> Result<Option<std::sync::Arc<dyn crate::provider::VcsProvider>>>;
}

/// Verify the GitHub `X-Hub-Signature-256` header against `body` keyed by
/// `secret`, in **constant time** (`hmac::Mac::verify_slice`). The header is the
/// `sha256=<hex>` form GitHub sends; a missing/empty/malformed header or a
/// non-hex tag is a verification failure (returns `false`), never a panic.
///
/// This is the security-load-bearing primitive (`design/13 §3.2`/§8): the compare
/// is the RustCrypto `verify_slice` (a constant-time `subtle::ConstantTimeEq`),
/// NOT a `==` on the digest.
pub fn verify_signature(secret: &[u8], body: &[u8], signature_256: &str) -> bool {
    // GitHub sends `sha256=<hex>`. Reject anything else.
    let Some(hex) = signature_256.strip_prefix("sha256=") else {
        return false;
    };
    let Some(expected) = decode_hex(hex) else {
        return false;
    };
    // `new_from_slice` accepts any key length (HMAC pads/hashes as needed) and
    // never fails for HMAC; treat an error defensively as a verification failure.
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    // Constant-time compare (the whole point — never `==` on the tag).
    mac.verify_slice(&expected).is_ok()
}

/// Decode a lowercase/uppercase hex string to bytes; `None` on any non-hex
/// char or an odd length.
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}

/// The minimal parsed shape a GitHub webhook body yields, by `event_type`
/// (`design/13 §6.2`): just enough to locate the affected cache rows for targeted
/// invalidation (`design/13 §6.3`). Unknown / unparseable events map to
/// [`ParsedEvent::Unhandled`] (a no-op `200`, forward-compat).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedEvent {
    /// A `pull_request` / `pull_request_review` / `pull_request_review_thread`
    /// event: invalidate the PR's review-thread cache.
    PullRequest { number: i64 },
    /// A `check_run` / `check_suite` / `status` event: invalidate the
    /// `(repo, sha)` check cache.
    CheckRun { sha: String },
    /// A `deployment` / `deployment_status` event: invalidate the `(repo, ref)`
    /// deployment cache.
    Deployment { ref_: String },
    /// An event type 315 does not act on (`ping`, `push`, …) or a body that did
    /// not carry the field we key on. A no-op `200` (`design/13 §6.2`
    /// forward-compat).
    Unhandled,
}

/// Parse the webhook `body` for `event_type` into a [`ParsedEvent`]. Never
/// errors — a malformed/short body for a known type degrades to
/// [`ParsedEvent::Unhandled`] (the HMAC already proved authenticity; a body we
/// cannot project is just an accelerator no-op, not a reject).
pub fn parse_event(event_type: &str, body: &[u8]) -> ParsedEvent {
    let json: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return ParsedEvent::Unhandled,
    };
    match event_type {
        "pull_request"
        | "pull_request_review"
        | "pull_request_review_thread"
        | "pull_request_review_comment" => json
            .get("pull_request")
            .and_then(|pr| pr.get("number"))
            .and_then(|n| n.as_i64())
            .or_else(|| json.get("number").and_then(|n| n.as_i64()))
            .map(|number| ParsedEvent::PullRequest { number })
            .unwrap_or(ParsedEvent::Unhandled),
        "check_run" => json
            .get("check_run")
            .and_then(|c| c.get("head_sha"))
            .and_then(|s| s.as_str())
            .map(|sha| ParsedEvent::CheckRun {
                sha: sha.to_string(),
            })
            .unwrap_or(ParsedEvent::Unhandled),
        "check_suite" => json
            .get("check_suite")
            .and_then(|c| c.get("head_sha"))
            .and_then(|s| s.as_str())
            .map(|sha| ParsedEvent::CheckRun {
                sha: sha.to_string(),
            })
            .unwrap_or(ParsedEvent::Unhandled),
        "status" => json
            .get("sha")
            .and_then(|s| s.as_str())
            .map(|sha| ParsedEvent::CheckRun {
                sha: sha.to_string(),
            })
            .unwrap_or(ParsedEvent::Unhandled),
        "deployment" => json
            .get("deployment")
            .and_then(|d| d.get("ref"))
            .and_then(|s| s.as_str())
            .map(|r| ParsedEvent::Deployment {
                ref_: r.to_string(),
            })
            .unwrap_or(ParsedEvent::Unhandled),
        "deployment_status" => json
            .get("deployment")
            .and_then(|d| d.get("ref"))
            .and_then(|s| s.as_str())
            .map(|r| ParsedEvent::Deployment {
                ref_: r.to_string(),
            })
            .unwrap_or(ParsedEvent::Unhandled),
        // ping, push, anything else: forward-compat no-op.
        _ => ParsedEvent::Unhandled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A known-good HMAC fixture: secret + body + the matching `sha256=<hex>`
    /// header verifies; a flipped byte (and a missing header) fails.
    #[test]
    fn hmac_good_and_bad() {
        let secret = b"itsasecret";
        let body = br#"{"action":"completed"}"#;
        // Compute the reference signature the same way GitHub does.
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(body);
        let tag = mac.finalize().into_bytes();
        let hex: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        let good = format!("sha256={hex}");

        assert!(verify_signature(secret, body, &good));

        // Flip one hex nibble → fail.
        let mut bad_chars: Vec<char> = good.chars().collect();
        let last = bad_chars.len() - 1;
        bad_chars[last] = if bad_chars[last] == '0' { '1' } else { '0' };
        let bad: String = bad_chars.into_iter().collect();
        assert!(!verify_signature(secret, body, &bad));

        // Missing/empty header → fail (a misconfigured hook).
        assert!(!verify_signature(secret, body, ""));
        // Wrong prefix → fail.
        assert!(!verify_signature(secret, body, "sha1=abcdef"));
        // Wrong secret → fail.
        assert!(!verify_signature(b"wrong", body, &good));
    }

    #[test]
    fn parse_known_and_unknown_events() {
        let pr = br#"{"action":"opened","pull_request":{"number":42}}"#;
        assert_eq!(
            parse_event("pull_request", pr),
            ParsedEvent::PullRequest { number: 42 }
        );

        let cr = br#"{"action":"completed","check_run":{"head_sha":"abc123"}}"#;
        assert_eq!(
            parse_event("check_run", cr),
            ParsedEvent::CheckRun {
                sha: "abc123".into()
            }
        );

        let dep = br#"{"deployment":{"ref":"main"}}"#;
        assert_eq!(
            parse_event("deployment", dep),
            ParsedEvent::Deployment {
                ref_: "main".into()
            }
        );

        // Unknown type → no-op.
        assert_eq!(
            parse_event("ping", br#"{"zen":"hi"}"#),
            ParsedEvent::Unhandled
        );
        // Known type, missing field → no-op (not a reject).
        assert_eq!(
            parse_event("check_run", br#"{"action":"completed"}"#),
            ParsedEvent::Unhandled
        );
        // Garbage body → no-op.
        assert_eq!(
            parse_event("pull_request", b"not json"),
            ParsedEvent::Unhandled
        );
    }
}
