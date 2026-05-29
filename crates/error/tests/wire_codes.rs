//! Tests for `Error` Display / Debug / wire_code(). One test per variant —
//! the wire codes are part of the cross-process protocol, so a typo here is
//! a breaking change.

use concerto_error::Error;

#[test]
fn io_wire_code_and_display() {
    let err = Error::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
    assert_eq!(err.wire_code(), "io");
    assert_eq!(err.to_string(), "io: missing");
    assert!(format!("{err:?}").contains("Io"));
}

#[test]
fn sqlx_wire_code_and_display() {
    let err: Error = sqlx::Error::RowNotFound.into();
    assert_eq!(err.wire_code(), "sqlx");
    assert!(err.to_string().starts_with("sqlx: "));
    assert!(format!("{err:?}").contains("Sqlx"));
}

#[test]
fn tonic_wire_code_and_display() {
    let err: Error = tonic::Status::unavailable("offline").into();
    assert_eq!(err.wire_code(), "tonic");
    assert!(err.to_string().starts_with("tonic: "));
    assert!(format!("{err:?}").contains("Tonic"));
}

#[test]
fn pairing_wire_code_and_display() {
    let err = Error::Pairing("bad token".to_string());
    assert_eq!(err.wire_code(), "pairing");
    assert_eq!(err.to_string(), "pairing: bad token");
    assert!(format!("{err:?}").contains("Pairing"));
}

#[test]
fn git_wire_code_and_display() {
    let err = Error::Git("clone failed: exit 128".to_string());
    assert_eq!(err.wire_code(), "git");
    assert_eq!(err.to_string(), "git: clone failed: exit 128");
    assert!(format!("{err:?}").contains("Git"));
}

#[test]
fn validation_wire_code_and_display() {
    let err = Error::Validation("name is required".to_string());
    assert_eq!(err.wire_code(), "validation");
    assert_eq!(err.to_string(), "validation: name is required");
    assert!(format!("{err:?}").contains("Validation"));
}

#[test]
fn not_found_wire_code_and_display() {
    let err = Error::NotFound("workspace abc not found".to_string());
    assert_eq!(err.wire_code(), "not_found");
    assert_eq!(err.to_string(), "not_found: workspace abc not found");
    assert!(format!("{err:?}").contains("NotFound"));
}

#[test]
fn internal_wire_code_and_display() {
    let err = Error::Internal("invariant violated".to_string());
    assert_eq!(err.wire_code(), "internal");
    assert_eq!(err.to_string(), "internal: invariant violated");
    assert!(format!("{err:?}").contains("Internal"));
}

#[test]
fn secrets_wire_code_and_display() {
    let err: Error = concerto_keychain::SecretsError::NotFound.into();
    assert_eq!(err.wire_code(), "secrets");
    assert!(err.to_string().starts_with("secrets: "));
    assert!(format!("{err:?}").contains("Secrets"));
}

#[test]
fn vcs_wire_code_and_display() {
    let err = Error::Vcs("gh exit 1: not authenticated".to_string());
    assert_eq!(err.wire_code(), "vcs");
    assert_eq!(err.to_string(), "vcs: gh exit 1: not authenticated");
    assert!(format!("{err:?}").contains("Vcs"));
}

#[test]
fn vcs_not_authenticated_wire_code_and_display() {
    let err = Error::VcsNotAuthenticated("run `gh auth login`".to_string());
    assert_eq!(err.wire_code(), "vcs.not_authenticated");
    assert_eq!(
        err.to_string(),
        "vcs.not_authenticated: run `gh auth login`"
    );
    assert!(format!("{err:?}").contains("VcsNotAuthenticated"));
}

#[test]
fn from_std_io_error() {
    let io_err = std::io::Error::other("oops");
    let err: Error = io_err.into();
    assert_eq!(err.wire_code(), "io");
}
