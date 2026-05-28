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
fn from_std_io_error() {
    let io_err = std::io::Error::other("oops");
    let err: Error = io_err.into();
    assert_eq!(err.wire_code(), "io");
}
