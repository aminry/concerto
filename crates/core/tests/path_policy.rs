//! Integration tests for Task 41 — filesystem allow-list + hard
//! deny-list (`crates/core/src/security/path_policy.rs`).
//!
//! Coverage:
//!
//! - **Allowed**: a path inside the workarea's `worktree_root`
//!   classifies as `Allowed`.
//! - **Outside**: an arbitrary tempdir outside every allow root
//!   classifies as `Outside`.
//! - **Denied**: writing to `<home>/.ssh/config` returns `Denied`
//!   regardless of the rest of the policy state.
//! - **Symlink escape**: a symlink under `<allow>/` pointing at
//!   `<home>/.ssh/` resolves via canonicalization to the deny prefix
//!   → `Denied`.
//! - **Missing path**: a path that doesn't exist on disk still
//!   classifies by prefix (lexical fallback via `path_clean`).
//!
//! These tests exercise the pure-Rust classifier (`classify`) directly
//! — the supervisor-side wiring is covered indirectly by Task 33's
//! tool-approval tests once the policy override path lands. Keeping the
//! integration test pure-Rust keeps it fast and deterministic.

#![cfg(unix)]

use std::path::PathBuf;

use concerto_core::security::{
    classify_path, path_policy::canonicalize_or_clean, AllowList, DenyList, PathDecision,
};
use tempfile::TempDir;

/// A path under `td`'s `worktree_root` classifies as Allowed.
#[test]
fn allowed_inside_workarea_worktree() {
    let td = TempDir::new().unwrap();
    let target = td.path().join("src/main.rs");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "fn main() {}").unwrap();

    let mut allow = AllowList::new();
    allow.push(td.path().to_path_buf());
    let deny = DenyList::new();

    assert_eq!(classify_path(&target, &allow, &deny), PathDecision::Allowed);
}

/// A path under an arbitrary tempdir (not the allow root) classifies as
/// Outside.
#[test]
fn outside_when_no_allow_match() {
    let allow_td = TempDir::new().unwrap();
    let other_td = TempDir::new().unwrap();
    let target = other_td.path().join("scratch.txt");
    std::fs::write(&target, "data").unwrap();

    let mut allow = AllowList::new();
    allow.push(allow_td.path().to_path_buf());
    let deny = DenyList::new();

    assert_eq!(classify_path(&target, &allow, &deny), PathDecision::Outside);
}

/// Writing to `<home>/.ssh/config` is Denied — the deny-list floor
/// applies even when the same prefix is also part of the allow-list.
#[test]
fn denied_for_ssh_config() {
    let fake_home = TempDir::new().unwrap();
    let ssh_dir = fake_home.path().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    let config_path = ssh_dir.join("config");
    std::fs::write(&config_path, "Host *").unwrap();

    // Allow everything under the fake-home — even though `.ssh/` is
    // syntactically inside the allow root, the deny floor still wins.
    let mut allow = AllowList::new();
    allow.push(fake_home.path().to_path_buf());
    let deny = DenyList::v0_1_default(fake_home.path());

    assert_eq!(
        classify_path(&config_path, &allow, &deny),
        PathDecision::Denied
    );
}

/// A symlink inside the allow-list pointing at `<home>/.ssh/` resolves
/// via `std::fs::canonicalize` to the deny prefix and classifies as
/// Denied. This is the V0.1 symlink-escape defense
/// (`design/12 §3.5`).
#[test]
fn symlink_escape_to_ssh_is_denied() {
    let fake_home = TempDir::new().unwrap();
    let ssh_dir = fake_home.path().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    std::fs::write(ssh_dir.join("id_rsa"), "secret").unwrap();

    let workarea = TempDir::new().unwrap();
    // Symlink: <workarea>/sneaky -> <home>/.ssh
    let symlink = workarea.path().join("sneaky");
    std::os::unix::fs::symlink(&ssh_dir, &symlink).unwrap();
    // Read through the symlink to <home>/.ssh/id_rsa.
    let through_symlink = symlink.join("id_rsa");

    let mut allow = AllowList::new();
    allow.push(workarea.path().to_path_buf());
    let deny = DenyList::v0_1_default(fake_home.path());

    assert_eq!(
        classify_path(&through_symlink, &allow, &deny),
        PathDecision::Denied
    );
}

/// A path that does not exist on disk still classifies correctly via
/// the `path_clean` lexical fallback. This is the V0.1 `Write` /
/// `Edit` case where the agent is creating a new file.
#[test]
fn missing_path_classified_by_prefix() {
    let td = TempDir::new().unwrap();
    // Canonicalize the allow root so the prefix match handles macOS's
    // `/var` → `/private/var` symlink the same way the classifier will
    // for the candidate. Without this the candidate's lexical fallback
    // would carry `/var/...` while the canonicalized allow root would
    // carry `/private/var/...`.
    let allow_root = canonicalize_or_clean(td.path());
    let mut allow = AllowList::new();
    allow.push(allow_root.clone());
    let deny = DenyList::new();

    // Brand-new path that doesn't exist yet.
    let new_path = allow_root.join("new-dir/new-file.txt");
    assert!(!new_path.exists());
    assert_eq!(
        classify_path(&new_path, &allow, &deny),
        PathDecision::Allowed
    );
}

/// A missing path under a missing deny root still classifies as Denied
/// — exercises the lexical-fallback prefix match on both sides.
#[test]
fn missing_path_in_deny_root_still_denied() {
    let mut allow = AllowList::new();
    allow.push(PathBuf::from("/tmp"));
    let mut deny = DenyList::new();
    // Use a path that does not exist; the deny root itself doesn't
    // exist so canonicalize falls back to the lexical cleaner.
    deny.push(PathBuf::from("/nonexistent-deny-root"));
    let candidate = PathBuf::from("/nonexistent-deny-root/inside/file");

    assert_eq!(
        classify_path(&candidate, &allow, &deny),
        PathDecision::Denied
    );
}
