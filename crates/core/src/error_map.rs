//! Convert internal [`concerto_error::Error`] values into gRPC
//! [`tonic::Status`] responses (Task 13).
//!
//! ## Why a free function, not a `From` impl
//!
//! The orphan rule forbids `impl From<concerto_error::Error> for
//! tonic::Status` outside the crate that defines one of the two types
//! — and both are foreign to `concerto-core`. We therefore expose a
//! plain function [`error_to_status`]; callers in
//! [`crate::handlers`] use `.map_err(error_to_status)?` to bridge.
//!
//! ## Mapping
//!
//! The mapping reuses the stable wire-code strings declared on
//! [`concerto_error::Error::wire_code`]. Each wire code maps to a
//! [`tonic::Code`]; unmapped categories fall through to `Internal`.
//! The serialized [`concerto_proto::v1::ConcertoError`] is attached as
//! the status details payload so clients can inspect structured
//! fields beyond the human-readable message.

use concerto_error::Error;
use concerto_proto::v1::ConcertoError;
use prost::Message;
use tonic::{Code, Status};

/// Convert a Concerto error into a gRPC [`Status`].
///
/// The returned status:
/// - Has [`Status::code`] mapped from `err.wire_code()` (see the
///   match below).
/// - Has [`Status::message`] set to the `Display` impl of the error.
/// - Has [`Status::details`] populated with a Prost-encoded
///   [`ConcertoError`] proto. The `fields` member is left empty in
///   V0.1; the audit-log path will populate it once it lands.
pub fn error_to_status(err: Error) -> Status {
    let code = match err.wire_code() {
        // I/O failures are typically retryable transport-level issues;
        // surface them as `Unavailable` so clients with retry policies
        // back off appropriately.
        "io" => Code::Unavailable,
        // SQL errors are server-side state problems; the caller cannot
        // recover by retrying with different inputs.
        "sqlx" => Code::Internal,
        // A nested `tonic::Status` already carries its own code — but
        // we have no way to recover it here without re-parsing, so
        // collapse to `Internal`. Handlers that wrap a foreign Status
        // should rebuild the Status directly rather than round-trip
        // through this mapping.
        "tonic" => Code::Internal,
        // Authentication/authorization failure during pairing.
        "pairing" => Code::Unauthenticated,
        // Keychain / secret-store failure.
        "secrets" => Code::FailedPrecondition,
        // Git shell-out / gix operation failure (Task 18). Surface as
        // `Internal` — clients have no recourse but to log + report.
        "git" => Code::Internal,
        // Caller-facing input validation failure (Task 19).
        // `ConcertoError.code` carries the specific subcode in the
        // message body (e.g. `workspace.v0_single_repo_only`).
        "validation" => Code::InvalidArgument,
        // Caller-facing missing-entity failure (Task 19).
        "not_found" => Code::NotFound,
        // Policy precondition (Task 32 — wrong acknowledgement string
        // on a permission-mode elevation).
        "policy" => Code::FailedPrecondition,
        // Org-managed policy lockout (Task 32 — `managed.json` caps
        // `max_permission_mode` below the requested mode).
        "policy.locked" => Code::PermissionDenied,
        // VCS provider failure (Task 45 — `gh` shell-out non-zero exit
        // or JSON parse failure). Internal because clients can't
        // recover by retrying with different inputs.
        "vcs" => Code::Internal,
        // VCS not authenticated (Task 45 — `gh auth status` failure).
        // FailedPrecondition so the UI can guide the user through
        // `gh auth login` without treating it as a hard bug.
        "vcs.not_authenticated" => Code::FailedPrecondition,
        // Catch-all for invariants the type system can't capture.
        "internal" => Code::Internal,
        // Future variants get logged so we notice an unmapped code in
        // the wild, but still surface as `Internal` to clients.
        other => {
            tracing::warn!(
                wire_code = other,
                "unmapped wire code; defaulting to Internal"
            );
            Code::Internal
        }
    };

    let proto = ConcertoError {
        code: err.wire_code().to_string(),
        message: format!("{err}"),
        fields: None,
        transaction_id: String::new(),
    };
    let mut buf = Vec::with_capacity(proto.encoded_len());
    // `encode` only fails if the buffer has insufficient capacity —
    // impossible here because we just sized it from `encoded_len()`.
    proto
        .encode(&mut buf)
        .expect("prost encode into pre-sized Vec");

    Status::with_details(code, format!("{err}"), buf.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use concerto_error::Error;

    #[test]
    fn pairing_maps_to_unauthenticated() {
        let s = error_to_status(Error::Pairing("nope".into()));
        assert_eq!(s.code(), Code::Unauthenticated);
        assert!(s.message().contains("nope"));
        assert!(
            !s.details().is_empty(),
            "details must include ConcertoError"
        );
    }

    #[test]
    fn internal_maps_to_internal() {
        let s = error_to_status(Error::Internal("boom".into()));
        assert_eq!(s.code(), Code::Internal);
    }

    #[test]
    fn io_maps_to_unavailable() {
        let s = error_to_status(Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "x",
        )));
        assert_eq!(s.code(), Code::Unavailable);
    }

    #[test]
    fn details_decode_back_to_proto() {
        let s = error_to_status(Error::Internal("hello".into()));
        let decoded = ConcertoError::decode(s.details()).expect("decode details");
        assert_eq!(decoded.code, "internal");
        assert!(decoded.message.contains("hello"));
    }
}
