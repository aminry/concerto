//! Per-workarea edit mutex registry (Task 308, `design/04 §3.5`).
//!
//! A single workarea can host **multiple concurrent agent sessions**
//! (e.g. Claude alongside Codex on the same worktrees + `.context/`).
//! Those sessions share the same files, so two of them writing at once
//! could clobber each other mid-edit. `design/04 §3.5` / R-5 specifies
//! the contract: **at most one session writes files at a time within a
//! workarea**, enforced by a per-workarea `Mutex<()>` with a 10s
//! acquisition timeout. The loser's write **fails fast** with a clear
//! "blocked on `<other session>`" error rather than queuing
//! indefinitely (indefinite queuing would deadlock multi-agent flows).
//! Reads (status, diff, `git log`) stay concurrent — they acquire
//! nothing.
//!
//! ## Placement: a neutral module, two holders
//!
//! The registry lives under `workspace_manager/` because the edit lock
//! is **workarea-scoped state**, next to the workarea owner. But the
//! type is held by `Arc` in **both** subsystems that need it
//! (`PHASE3_PLANNING §2`):
//!
//! - the **Agent Supervisor (04)** acquires the lock around a session's
//!   write-class tool execution, and
//! - the **Workarea Manager (03)** reads [`EditMutexRegistry::holder`]
//!   for UI / diagnostics ("blocked on `<session>`").
//!
//! `boot.rs` constructs **exactly one** `Arc<EditMutexRegistry>` and
//! `Arc::clone`s it into each via the `with_edit_mutex_registry`
//! builders. Constructing two registries would defeat the cross-session
//! lock, so the single-instance discipline is load-bearing.
//!
//! ## Scope of the lock (FROZEN)
//!
//! The lock is acquired around **write-class tool execution only** —
//! `Write`, `Edit`, `MultiEdit`, `NotebookEdit`, and the Concerto-driven
//! commit. Read-class operations (`Read`, `Grep`, diff, status, `git
//! log`) acquire nothing. There is exactly one lock per [`WorkareaId`],
//! shared across all of that workarea's sessions. See
//! [`is_write_class`].
//!
//! **Boundary note (`Scope — out`):** a `Bash` tool call that *happens*
//! to write files is **not** gated — the mutex guards the explicit edit
//! tools above + the Concerto commit path, not arbitrary shell. Gating
//! `Bash` writes is the agent's responsibility, exactly as it is today.
//! Per-file mutex granularity is a V2.0 maybe (`design/04 R-5`); V1.0 is
//! one serial write lock per workarea.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use concerto_persist::{SessionId, WorkareaId};
use tokio::sync::Mutex;

/// Wire-code carried on the typed error a blocked write surfaces. Rides
/// the existing `session.events` / `workarea.events` stream as a typed
/// error string (no new proto field — Task 308 adds no wire surface).
pub const EDIT_MUTEX_BLOCKED_WIRE_CODE: &str = "workarea.edit_mutex.blocked";

/// Default acquisition timeout for the per-workarea edit lock
/// (`design/04 §3.5` / R-5). Tests inject a shorter value through the
/// `timeout` argument of [`EditMutexRegistry::acquire`].
pub const DEFAULT_EDIT_MUTEX_TIMEOUT: Duration = Duration::from_secs(10);

/// The write-class tool set the per-workarea edit mutex serializes
/// (FROZEN, `design/04 §3.5`). Mirrors the path-bearing-write tools the
/// resolver's [`crate::agent_supervisor::tool_args`] extractor knows
/// about, **plus** `MultiEdit` (which the V0.1 extractor doesn't parse a
/// path out of but which is still a write). `Read` is deliberately
/// absent — reads acquire nothing.
///
/// Returns `true` for the tool names whose execution must hold the
/// workarea's edit lock; the Concerto-driven commit path acquires the
/// lock directly (it is not a tool name).
pub fn is_write_class(tool_name: &str) -> bool {
    matches!(tool_name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit")
}

/// Returned when [`EditMutexRegistry::acquire`] times out: another
/// session on the same workarea is holding the edit lock. Carries the
/// holding [`SessionId`] when known so the blocked session's event
/// stream can name the current writer ("blocked on `<session>`").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditBlocked {
    /// The session currently holding the lock, or `None` if the holder
    /// could not be determined (a benign race: the holder released
    /// between the timeout and the bookkeeping read).
    pub holder: Option<SessionId>,
}

impl EditBlocked {
    /// Human-readable "blocked on `<session>`" description for the
    /// session-event error surface. Names the holder when known,
    /// otherwise "another session".
    pub fn describe(&self) -> String {
        match &self.holder {
            Some(sid) => format!("blocked on session {sid}"),
            None => "blocked on another session".to_string(),
        }
    }
}

impl std::fmt::Display for EditBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{EDIT_MUTEX_BLOCKED_WIRE_CODE}: {}", self.describe())
    }
}

/// Per-workarea lock state: the inner async mutex (held across the
/// write's `.await`) plus the holder bookkeeping the registry exposes
/// via [`EditMutexRegistry::holder`].
struct WorkareaLock {
    /// Inner serial write lock. `Arc` so [`EditGuard`] can own a guard
    /// whose lifetime is decoupled from the outer map lock.
    inner: Arc<Mutex<()>>,
    /// The session currently holding `inner`, set on a successful
    /// acquire and cleared in [`EditGuard::drop`]. Behind the same outer
    /// map lock so reads + writes of the holder never race the map.
    holder: Arc<Mutex<Option<SessionId>>>,
}

/// Shared, cheap-to-clone (`Arc` inner) registry of per-workarea edit
/// locks. Held by `Arc` in both the Agent Supervisor (acquires) and the
/// Workarea Manager (reads [`Self::holder`]); see the module docs.
#[derive(Clone, Default)]
pub struct EditMutexRegistry {
    /// Outer lock guarding the `WorkareaId → WorkareaLock` map. Held
    /// only long enough to look up / lazily insert the inner lock —
    /// never across the long write `.await` (the inner lock is cloned
    /// out, then the outer guard is dropped before awaiting).
    map: Arc<Mutex<HashMap<WorkareaId, WorkareaLock>>>,
}

impl EditMutexRegistry {
    /// Construct an empty registry. `boot.rs` wraps this in an `Arc` and
    /// hands the clone to both subsystems.
    pub fn new() -> Self {
        Self {
            map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Acquire the workarea's edit lock for `session_id`, lazily creating
    /// the per-workarea lock on first touch. Holds the inner lock across
    /// the returned [`EditGuard`]'s lifetime; the guard releases on drop
    /// (including on panic) and clears the holder.
    ///
    /// Returns [`EditBlocked`] (naming the current holder, when known) if
    /// the lock can't be acquired within `timeout` — the caller MUST then
    /// **reject** the blocked session's write (fail fast), not queue.
    ///
    /// Reads must NOT call this — they acquire nothing.
    pub async fn acquire(
        &self,
        workarea: &WorkareaId,
        session_id: &SessionId,
        timeout: Duration,
    ) -> Result<EditGuard, EditBlocked> {
        // 1. Look up / lazily insert the per-workarea lock under the
        //    outer map lock, then clone the inner `Arc`s out and DROP the
        //    outer guard before awaiting the inner lock — never hold the
        //    map lock across the long write.
        let (inner, holder) = {
            let mut map = self.map.lock().await;
            let entry = map.entry(workarea.clone()).or_insert_with(|| WorkareaLock {
                inner: Arc::new(Mutex::new(())),
                holder: Arc::new(Mutex::new(None)),
            });
            (Arc::clone(&entry.inner), Arc::clone(&entry.holder))
        };

        // 2. Race the inner-lock acquisition against the timeout. On
        //    success we own the guard for the duration of the write; on
        //    timeout we read the holder for the "blocked on <session>"
        //    message and fail fast.
        match tokio::time::timeout(timeout, inner.clone().lock_owned()).await {
            Ok(guard) => {
                *holder.lock().await = Some(session_id.clone());
                Ok(EditGuard {
                    _guard: guard,
                    holder,
                })
            }
            Err(_elapsed) => {
                let current = holder.lock().await.clone();
                Err(EditBlocked { holder: current })
            }
        }
    }

    /// Read the session currently holding the workarea's edit lock, or
    /// `None` if no lock exists for the workarea or it is unheld. Used by
    /// the Workarea Manager for UI / diagnostics; acquires nothing on the
    /// inner lock.
    pub async fn holder(&self, workarea: &WorkareaId) -> Option<SessionId> {
        let cell = {
            let map = self.map.lock().await;
            map.get(workarea).map(|l| Arc::clone(&l.holder))
        };
        match cell {
            Some(cell) => cell.lock().await.clone(),
            None => None,
        }
    }
}

/// RAII guard returned by [`EditMutexRegistry::acquire`]. Holding it
/// means this session owns the workarea's serial write lock; dropping it
/// releases the lock and clears the holder so a blocked sibling can
/// proceed. The `Drop` impl runs even on panic mid-write, so a panicking
/// write can never leave the lock wedged.
pub struct EditGuard {
    /// The owned inner-mutex guard. Dropped (releasing the lock) when the
    /// `EditGuard` drops. Named with a leading underscore because it is
    /// held purely for its `Drop` side effect.
    _guard: tokio::sync::OwnedMutexGuard<()>,
    /// The holder cell, cleared on drop. Cleared with `try_lock` (a
    /// non-async best-effort) because `Drop` cannot `.await`; the cell is
    /// touched only under the briefly-held outer map lock elsewhere, so a
    /// contended `try_lock` is vanishingly unlikely and a stale holder
    /// string is at worst a cosmetic UI lag, never a correctness issue
    /// (the actual lock is already released via `_guard`).
    holder: Arc<Mutex<Option<SessionId>>>,
}

impl std::fmt::Debug for EditGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EditGuard")
    }
}

impl Drop for EditGuard {
    fn drop(&mut self) {
        if let Ok(mut h) = self.holder.try_lock() {
            *h = None;
        }
        // If the holder cell is momentarily contended we leave the stale
        // name; the inner write lock (`_guard`) still releases here, so
        // the next `acquire` succeeds and overwrites the holder anyway.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wa(id: &str) -> WorkareaId {
        WorkareaId(id.to_string())
    }
    fn sid(id: &str) -> SessionId {
        SessionId(id.to_string())
    }

    #[tokio::test]
    async fn second_acquire_blocks_then_times_out_naming_holder() {
        let reg = EditMutexRegistry::new();
        let w = wa("wa-1");
        let guard_a = reg
            .acquire(&w, &sid("A"), Duration::from_secs(5))
            .await
            .expect("A acquires");
        assert_eq!(reg.holder(&w).await, Some(sid("A")));

        // B blocks then errors, naming A.
        let blocked = reg
            .acquire(&w, &sid("B"), Duration::from_millis(50))
            .await
            .expect_err("B blocks");
        assert_eq!(blocked.holder, Some(sid("A")));
        assert!(blocked.to_string().contains(EDIT_MUTEX_BLOCKED_WIRE_CODE));

        // Releasing A lets B succeed; the holder flips to B.
        drop(guard_a);
        let guard_b = reg
            .acquire(&w, &sid("B"), Duration::from_secs(5))
            .await
            .expect("B acquires after release");
        assert_eq!(reg.holder(&w).await, Some(sid("B")));
        drop(guard_b);
        assert_eq!(reg.holder(&w).await, None);
    }

    #[tokio::test]
    async fn distinct_workareas_do_not_serialize() {
        let reg = EditMutexRegistry::new();
        let g1 = reg
            .acquire(&wa("wa-1"), &sid("A"), Duration::from_secs(5))
            .await
            .expect("A on wa-1");
        // A different workarea's lock is independent — no block.
        let g2 = reg
            .acquire(&wa("wa-2"), &sid("B"), Duration::from_millis(50))
            .await
            .expect("B on wa-2 not blocked by wa-1");
        drop(g1);
        drop(g2);
    }

    #[tokio::test]
    async fn holder_none_for_unknown_or_released_workarea() {
        let reg = EditMutexRegistry::new();
        assert_eq!(reg.holder(&wa("never-touched")).await, None);
    }

    #[test]
    fn write_class_set_is_exactly_the_frozen_tools() {
        for t in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
            assert!(is_write_class(t), "{t} must be write-class");
        }
        for t in ["Read", "Grep", "Bash", "Glob", "TodoWrite", "WebFetch"] {
            assert!(!is_write_class(t), "{t} must NOT be write-class");
        }
    }
}
