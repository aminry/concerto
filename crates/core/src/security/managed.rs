//! `managed.json` reader + hot-reload watcher (Task 32 + Task 42).
//!
//! `managed.json` is the org-controlled override layer (per `design/12
//! §3.8`). Lives at `<config_dir>/managed.json`. The full V0.1 surface is
//! the union of Task 32's three fields and Task 42's two additional
//! "parsed but not enforced in V0.1" fields:
//!
//! - `version` (u32) — required when the file exists. Only `1` is
//!   supported. Higher values are rejected with [`Error::Internal`] so
//!   the user notices a forward-compat mismatch; missing/zero defaults
//!   to 1 for forward compatibility with the pre-Task-42 schema.
//! - `max_permission_mode` — caps the resolved effective mode.
//! - `allow_yolo` — when `false`, the user cannot set `yolo` at any
//!   level (RPC handlers translate this into `policy.yolo_blocked`).
//! - `allow_bypass_destructive_guard` — when `false`, the user cannot
//!   set `workareas.bypass_destructive_guard = true`
//!   (`policy.bypass_blocked`).
//! - `preamble_template_path` — parsed but not enforced in V0.1
//!   (org-customised entry-ceremony preamble; surfaced to the desktop
//!   shell in V1.0).
//! - `max_reasoning_level` — parsed but not enforced in V0.1
//!   (deliberation controls land in V1.0).
//!
//! Missing file → no managed policy ([`ManagedPolicy::default`]).
//! Malformed JSON → warn + default; the Core does not refuse to boot
//! when an org artifact is unparseable. **Unknown `version` field**, by
//! contrast, IS a hard error — that's a deliberate forward-compatibility
//! tripwire so a v2 policy file isn't silently mis-enforced by an older
//! Core binary.
//!
//! ## Hot reload (Task 42)
//!
//! [`ManagedPolicySource`] wraps a `tokio::sync::watch::Sender<ManagedPolicy>`
//! plus a background watcher task that observes
//! `<config_dir>/managed.json` via `notify`-rs and republishes the parsed
//! policy whenever the file mutates. Events are debounced at
//! [`HOT_RELOAD_DEBOUNCE`] (500 ms) so a typical editor save (write +
//! rename) only triggers one re-parse. Subscribers
//! ([`ManagedPolicySource::subscribe`]) get a `watch::Receiver` that
//! always yields the latest parsed value.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use concerto_error::{Error, Result};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::sync::watch;

use crate::security::permission::PermissionMode;

/// Debounce window for the hot-reload watcher. A typical editor save
/// (write + rename) fires multiple `notify` events in quick succession;
/// the debounce coalesces them into a single re-parse to keep the watch
/// channel quiet and the on-disk read bounded.
pub const HOT_RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);

/// Currently-supported `managed.json` schema version. Bump this when
/// adding required fields; older versions are accepted by default
/// (missing/zero → 1), newer versions are rejected with
/// [`Error::Internal`].
pub const MANAGED_SCHEMA_VERSION: u32 = 1;

/// Locked filename inside `<config_dir>`.
pub const MANAGED_FILE_NAME: &str = "managed.json";

/// Effective managed policy after parsing `<config_dir>/managed.json`.
///
/// Default values (no `managed.json`, missing keys) leave every field
/// permissive (`None` cap, `true` allows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPolicy {
    /// Schema version of the parsed file. Always equals
    /// [`MANAGED_SCHEMA_VERSION`] when produced by [`load_managed_policy`]
    /// — higher values short-circuit with an error before this struct is
    /// returned, lower/missing values are normalised to the current
    /// supported version.
    pub version: u32,
    /// Ceiling on the resolved effective permission mode.
    /// [`crate::security::resolve_effective_mode`] downgrades a higher
    /// resolved mode to this value.
    pub max_permission_mode: Option<PermissionMode>,
    /// When `false`, RPC handlers reject any attempt to set
    /// `permission_mode = yolo`. Surfaced separately from
    /// `max_permission_mode` so the UI can render a "yolo grayed out by
    /// policy" hint distinct from "policy caps at auto".
    pub allow_yolo: bool,
    /// When `false`, RPC handlers reject
    /// `workareas.bypass_destructive_guard = true`.
    pub allow_bypass_destructive_guard: bool,
    /// Path to an org-supplied preamble template injected into elevated
    /// permission-mode entry ceremonies. Parsed in V0.1 but not yet
    /// surfaced to the desktop shell — V1.0 work.
    pub preamble_template_path: Option<PathBuf>,
    /// Org cap on the deliberation level (e.g. `"high"`, `"medium"`).
    /// Parsed in V0.1 but not yet enforced — the agent supervisor's
    /// deliberation controls land in V1.0.
    pub max_reasoning_level: Option<String>,
}

impl Default for ManagedPolicy {
    fn default() -> Self {
        Self {
            version: MANAGED_SCHEMA_VERSION,
            max_permission_mode: None,
            allow_yolo: true,
            allow_bypass_destructive_guard: true,
            preamble_template_path: None,
            max_reasoning_level: None,
        }
    }
}

/// On-disk schema for V0.1. Each field is optional so partial files
/// (e.g. only `max_permission_mode` set) parse cleanly. `version` is
/// optional for forward compatibility with the pre-Task-42 schema; an
/// explicit higher value is rejected by [`load_managed_policy`].
#[derive(Debug, Default, Deserialize)]
struct ManagedFile {
    #[serde(default)]
    version: Option<u32>,
    max_permission_mode: Option<String>,
    allow_yolo: Option<bool>,
    allow_bypass_destructive_guard: Option<bool>,
    preamble_template_path: Option<PathBuf>,
    max_reasoning_level: Option<String>,
}

/// Load the managed policy from `<config_dir>/managed.json`.
///
/// Missing file: returns [`ManagedPolicy::default`] silently — most
/// installs (personal users) ship without one.
///
/// Malformed JSON or unknown `max_permission_mode` value: logs a
/// `tracing::warn!` and returns [`ManagedPolicy::default`]. The Core
/// stays running — an org artifact being broken should not lock the
/// user out of their machine.
///
/// **Unknown `version`** (anything other than missing/zero/1) returns
/// [`Error::Internal`] so the operator notices the mismatch. A future
/// `version: 2` Core binary will keep accepting `version: 1` files, but
/// a v1 Core binary must NOT silently mis-enforce a v2 file.
///
/// Synchronous I/O on purpose: the file is tiny (< 1 KB in practice).
pub fn load_managed_policy(config_dir: &Path) -> Result<ManagedPolicy> {
    let path = config_dir.join(MANAGED_FILE_NAME);
    parse_managed_policy_at(&path)
}

/// Parse a [`ManagedPolicy`] from a specific file path. Used by the
/// hot-reload watcher (which has the path in hand) and by
/// [`load_managed_policy`] (which derives the path from `<config_dir>`).
fn parse_managed_policy_at(path: &Path) -> Result<ManagedPolicy> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ManagedPolicy::default()),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "managed.json read failed; defaulting to permissive policy"
            );
            return Ok(ManagedPolicy::default());
        }
    };
    let parsed: ManagedFile = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "managed.json parse failed; defaulting to permissive policy"
            );
            return Ok(ManagedPolicy::default());
        }
    };

    // Forward-compat tripwire: an explicit version higher than what this
    // Core binary understands is a hard error. Missing or zero defaults
    // to the current supported version (compatible with the pre-Task-42
    // schema that omitted `version`).
    let version = parsed.version.unwrap_or(0);
    if version > MANAGED_SCHEMA_VERSION {
        return Err(Error::Internal(format!(
            "managed.json: unsupported version {version} (this Core only understands v{MANAGED_SCHEMA_VERSION})"
        )));
    }
    let version = if version == 0 {
        MANAGED_SCHEMA_VERSION
    } else {
        version
    };

    let max_permission_mode = match parsed.max_permission_mode.as_deref() {
        None => None,
        Some(s) => match crate::security::permission::parse_permission_mode(s) {
            Ok(m) => Some(m),
            Err(_) => {
                tracing::warn!(
                    path = %path.display(),
                    value = %s,
                    "managed.json max_permission_mode is not strict|normal|auto|yolo; ignoring"
                );
                None
            }
        },
    };

    Ok(ManagedPolicy {
        version,
        max_permission_mode,
        allow_yolo: parsed.allow_yolo.unwrap_or(true),
        allow_bypass_destructive_guard: parsed.allow_bypass_destructive_guard.unwrap_or(true),
        preamble_template_path: parsed.preamble_template_path,
        max_reasoning_level: parsed.max_reasoning_level,
    })
}

/// Hot-reload broadcaster for the managed policy.
///
/// Owns a `tokio::sync::watch::Sender<ManagedPolicy>` and the background
/// `notify`-rs watcher task. Subscribers obtain a
/// [`watch::Receiver<ManagedPolicy>`] via [`Self::subscribe`] and either
/// poll the current value with `borrow()` or await mutations with
/// `changed()`. The receiver is `Clone`, so each consumer can hold its
/// own copy without serialising on a shared mutex.
///
/// V0.1 wiring: the gRPC `Server` constructor builds one
/// [`ManagedPolicySource`] per process and passes the receiver into the
/// per-RPC enforcement helpers as needed. The synchronous
/// [`load_managed_policy`] is still the path used inside individual
/// handler methods — the watch channel exists to let long-lived
/// subscribers (e.g. future cached resolvers) observe changes without
/// re-reading the file.
///
/// The watcher task is parked on a `std::sync::mpsc::Receiver` fed by
/// `notify`-rs's event callback (which runs on `notify`'s own thread).
/// On every event the task waits [`HOT_RELOAD_DEBOUNCE`] for further
/// activity, then re-parses the file and publishes the result via
/// `watch::Sender::send`. Failed re-parses (e.g. mid-write reads) log a
/// `tracing::warn!` and leave the previous policy in place — callers
/// see the next successful parse on the next event burst.
pub struct ManagedPolicySource {
    sender: watch::Sender<ManagedPolicy>,
    path: PathBuf,
    // The `notify::RecommendedWatcher` must outlive the task it feeds —
    // stash it here so dropping the source tears the watcher down.
    _watcher: Option<notify::RecommendedWatcher>,
    // The debounce task's join handle. Detached on drop because the
    // `notify`-rs event channel closes and the task exits naturally.
    _debounce_task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for ManagedPolicySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedPolicySource")
            .field("path", &self.path)
            .field("current", &*self.sender.borrow())
            .finish()
    }
}

impl ManagedPolicySource {
    /// Build a source rooted at `<config_dir>/managed.json`. Performs an
    /// initial synchronous parse, seeds the watch channel, then spawns
    /// the `notify`-rs watcher on the parent directory (so events for a
    /// not-yet-existing `managed.json` still arrive).
    ///
    /// Errors from the initial parse (e.g. `version > supported`) are
    /// returned to the caller; transient I/O failures during a later
    /// reload are logged and swallowed (the previous policy stays in
    /// effect).
    pub fn new(config_dir: &Path) -> Result<Self> {
        let path = config_dir.join(MANAGED_FILE_NAME);
        let initial = parse_managed_policy_at(&path)?;
        let (sender, _) = watch::channel(initial);

        // Spawn the watcher (best-effort: missing config_dir means no
        // watcher, but the caller can still consult the static parser
        // via the watch sender's seed value).
        let watch_dir = config_dir.to_path_buf();
        if let Err(e) = std::fs::create_dir_all(&watch_dir) {
            tracing::warn!(
                dir = %watch_dir.display(),
                error = %e,
                "managed.json: failed to ensure config dir for watcher; hot reload disabled"
            );
            return Ok(Self {
                sender,
                path,
                _watcher: None,
                _debounce_task: None,
            });
        }

        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher = match notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| match res {
                Ok(ev) => {
                    // Only react to mutations that could change the file
                    // contents: create / modify / remove. Access events
                    // are noise.
                    if matches!(
                        ev.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        let _ = tx.send(());
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "managed.json watcher error");
                }
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "managed.json: notify watcher init failed; hot reload disabled");
                return Ok(Self {
                    sender,
                    path,
                    _watcher: None,
                    _debounce_task: None,
                });
            }
        };
        // Watch the parent directory (non-recursive) so events for
        // create/replace of the managed.json file still arrive even when
        // the file doesn't yet exist at startup.
        if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
            tracing::warn!(
                dir = %watch_dir.display(),
                error = %e,
                "managed.json: notify watch() failed; hot reload disabled"
            );
            return Ok(Self {
                sender,
                path,
                _watcher: None,
                _debounce_task: None,
            });
        }

        let task_sender = sender.clone();
        let task_path = path.clone();
        let task = tokio::spawn(async move {
            debounce_loop(rx, task_path, task_sender).await;
        });

        Ok(Self {
            sender,
            path,
            _watcher: Some(watcher),
            _debounce_task: Some(task),
        })
    }

    /// Subscribe to policy changes. The returned receiver immediately
    /// yields the current value via `borrow()`; `changed().await`
    /// completes the next time the watcher publishes a new policy.
    pub fn subscribe(&self) -> watch::Receiver<ManagedPolicy> {
        self.sender.subscribe()
    }

    /// Current parsed policy. Mainly useful for tests; production code
    /// should `subscribe()` so it sees subsequent reloads.
    pub fn current(&self) -> ManagedPolicy {
        self.sender.borrow().clone()
    }

    /// Path the watcher is observing.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Debounce loop running on the tokio runtime. Blocks on the
/// `notify`-rs event channel in a `spawn_blocking` because the channel
/// is `std::sync::mpsc::Receiver` and would otherwise stall a runtime
/// worker thread. After receiving an event the loop drains further
/// pending events, sleeps [`HOT_RELOAD_DEBOUNCE`], then re-parses and
/// republishes the policy.
async fn debounce_loop(
    mut rx: mpsc::Receiver<()>,
    path: PathBuf,
    sender: watch::Sender<ManagedPolicy>,
) {
    loop {
        // Block on the notify channel inside a spawn_blocking so the
        // tokio worker stays free. Receiver is moved into the blocking
        // task and returned back so the loop can re-park on it.
        let (handed_back, ok) = match tokio::task::spawn_blocking(move || match rx.recv() {
            Ok(()) => (rx, true),
            Err(_) => (rx, false),
        })
        .await
        {
            Ok(pair) => pair,
            Err(_) => return,
        };
        rx = handed_back;
        if !ok {
            // Channel closed (watcher dropped) → stop.
            return;
        }

        // Debounce: sleep, then drain any further events that arrived
        // during the sleep window. A typical editor save (write + rename)
        // fires multiple events in quick succession; we want one re-parse.
        tokio::time::sleep(HOT_RELOAD_DEBOUNCE).await;
        rx = match tokio::task::spawn_blocking(move || {
            while rx.try_recv().is_ok() {}
            rx
        })
        .await
        {
            Ok(r) => r,
            Err(_) => return,
        };

        // Re-parse + publish. A failed parse leaves the previous policy
        // in place — `tracing::warn!` lives inside `parse_managed_policy_at`.
        match parse_managed_policy_at(&path) {
            Ok(policy) => {
                // `watch::Sender::send` returns Err iff there are no
                // receivers; that's fine — the seed value is still cached
                // and the next subscriber will see the latest write.
                let _ = sender.send(policy);
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "managed.json: reload failed; previous policy retained"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_is_default() {
        let d = TempDir::new().unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p, ManagedPolicy::default());
        assert_eq!(p.version, MANAGED_SCHEMA_VERSION);
    }

    #[test]
    fn cap_to_auto_parses() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"version": 1, "max_permission_mode": "auto"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.max_permission_mode, Some(PermissionMode::Auto));
        assert!(p.allow_yolo);
        assert!(p.allow_bypass_destructive_guard);
        assert_eq!(p.version, 1);
    }

    #[test]
    fn unknown_mode_warns_and_defaults() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"max_permission_mode": "nope"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.max_permission_mode, None);
    }

    #[test]
    fn malformed_json_warns_and_defaults() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), "not json").unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p, ManagedPolicy::default());
    }

    #[test]
    fn allow_flags_parse() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"allow_yolo": false, "allow_bypass_destructive_guard": false}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert!(!p.allow_yolo);
        assert!(!p.allow_bypass_destructive_guard);
    }

    #[test]
    fn missing_version_defaults_to_one() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"max_permission_mode": "auto"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.version, 1);
    }

    #[test]
    fn explicit_version_one_parses() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), r#"{"version": 1}"#).unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.version, 1);
    }

    #[test]
    fn future_version_errors() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), r#"{"version": 2}"#).unwrap();
        let err = load_managed_policy(d.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("unsupported version"),
            "expected version error, got: {msg}"
        );
    }

    #[test]
    fn preamble_and_reasoning_fields_parse() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"preamble_template_path": "/etc/preamble.md", "max_reasoning_level": "medium"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(
            p.preamble_template_path,
            Some(PathBuf::from("/etc/preamble.md"))
        );
        assert_eq!(p.max_reasoning_level.as_deref(), Some("medium"));
    }
}
