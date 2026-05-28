//! Round-trip tests for the typed keychain wrapper.
//!
//! Gated behind `#[cfg(target_os = "macos")]`:
//! - macOS ships the Security framework natively; the `apple-native`
//!   feature on `keyring` 3 talks to it directly with no daemon required.
//! - Linux needs a running Secret Service (GNOME keyring / KWallet) which
//!   CI doesn't provide; until V1.0 adds the `linux-native` or
//!   `sync-secret-service` backend wiring, Linux test runs would either
//!   block on a missing service or false-positive on a misconfigured one.
//! - Windows isn't a V0.1 target (design/00 §10).
//!
//! On Linux/Windows the file still compiles (zero tests run); the
//! `cargo test -p concerto-keychain` build still succeeds.
//!
//! Per the task spec, each test uses a unique `service` name to avoid
//! colliding with real `concerto` entries on developer machines.
//! Cleanup runs on best-effort: if a test panics mid-way, the orphan
//! entry sits under `concerto-test-<n>` and is harmless.

#![cfg(target_os = "macos")]

use concerto_keychain::{Provider, SecretKind, SecretValue, Secrets};

/// Generate a unique service name per test invocation so concurrent runs
/// (e.g., `cargo test`'s default thread pool) don't trample each other.
fn unique_service(tag: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "concerto-test-{}-{}-{}-{}",
        std::process::id(),
        nanos,
        seq,
        tag
    )
}

#[tokio::test]
async fn set_then_get_returns_same_value() {
    let secrets = Secrets::with_service_for_test(unique_service("rt"));
    let kind = SecretKind::ProviderToken(Provider::Anthropic);

    secrets
        .set(kind, SecretValue::new("sk-test-12345".to_string()))
        .await
        .expect("set");

    let got = secrets.get(kind).await.expect("get").expect("Some");
    assert_eq!(got.expose(), "sk-test-12345");

    secrets.delete(kind).await.expect("cleanup");
}

#[tokio::test]
async fn missing_key_returns_none() {
    let secrets = Secrets::with_service_for_test(unique_service("missing"));
    // Nothing ever written under this service.
    let got = secrets.get(SecretKind::GithubPat).await.expect("get");
    assert!(got.is_none());
}

#[tokio::test]
async fn delete_then_get_returns_none() {
    let secrets = Secrets::with_service_for_test(unique_service("del"));
    let kind = SecretKind::GithubPat;

    secrets
        .set(kind, SecretValue::new("ghp_xyz".to_string()))
        .await
        .expect("set");
    secrets.delete(kind).await.expect("delete");

    let got = secrets.get(kind).await.expect("get");
    assert!(got.is_none(), "expected None after delete, got Some");
}

#[tokio::test]
async fn delete_missing_is_idempotent() {
    let secrets = Secrets::with_service_for_test(unique_service("del-missing"));
    // No entry exists; delete should still succeed.
    secrets
        .delete(SecretKind::PushExpoApiKey)
        .await
        .expect("delete missing");
}

#[tokio::test]
async fn overwrite_returns_new_value() {
    let secrets = Secrets::with_service_for_test(unique_service("overwrite"));
    let kind = SecretKind::CoreIdentityPrivateKey;

    secrets
        .set(kind, SecretValue::new("v1".to_string()))
        .await
        .expect("set v1");
    secrets
        .set(kind, SecretValue::new("v2".to_string()))
        .await
        .expect("set v2");
    let got = secrets.get(kind).await.expect("get").expect("Some");
    assert_eq!(got.expose(), "v2");

    secrets.delete(kind).await.expect("cleanup");
}
