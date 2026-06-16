//! The Rust-side session registry (Task 509, design/16 §3.2 — "represent
//! SessionHandle as an opaque numeric id backed by a Rust-side registry").
//!
//! `openSession` returns an opaque `u64` handle id; the live session state
//! (the client Iroh endpoint, the tonic [`Channel`], the signed device cert
//! metadata, the per-subscription cancel handles) lives here behind a
//! `Mutex<HashMap>`. This is the simplest representation that crosses the uniffi
//! boundary cleanly — the foreign side only ever holds a `u64`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use iroh::Endpoint;
use tokio::sync::oneshot;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::transport::Channel;

/// One live session: the bound client endpoint, the Noise-IK-wrapped tonic
/// channel, the device-cert metadata value attached to every call, and the
/// classified connection path for `natStats`.
pub struct Session {
    /// This device's local client Iroh endpoint. NOTE: the live API connection
    /// the RPCs actually ride is dialed and owned by the `IrohConnector` INSIDE
    /// `channel` (see `connect_channel`), NOT by this endpoint — dropping this
    /// endpoint closes this device's local Iroh endpoint/socket, while dropping
    /// `channel` is what tears down RPC traffic. Both are dropped together on
    /// `closeSession` / registry removal. Held purely for that teardown — never
    /// read after construction, hence the allow.
    #[allow(dead_code)]
    pub endpoint: Endpoint,
    /// The tonic channel over Iroh + Noise IK (built by `connect_channel`). Its
    /// `IrohConnector` owns the live API connection the RPCs ride; dropping the
    /// channel tears down that RPC traffic.
    pub channel: Channel,
    /// STANDARD-base64 of the on-wire signed device cert, pre-parsed as an ASCII
    /// metadata value, attached under `concerto-device-cert` on every RPC.
    pub cert_value: MetadataValue<Ascii>,
    /// This session's classified [`ConnectionPath`](concerto_transport::ConnectionPath),
    /// computed at open time from the live Iroh connection (client-side
    /// classification; NOT a Core RPC). `natStats` reports this.
    pub path: concerto_transport::ConnectionPath,
    /// Live server-streaming subscriptions on this session, keyed by id; cancel
    /// fires the oneshot, which the stream pump selects on to drop its task.
    pub subscriptions: HashMap<u64, oneshot::Sender<()>>,
}

/// The process-global registry. One per loaded library; the FFI surface looks
/// sessions up by their `u64` handle.
pub struct Registry {
    next_id: AtomicU64,
    sessions: Mutex<HashMap<u64, Session>>,
}

impl Registry {
    pub fn new() -> Self {
        Self {
            // Start at 1 so 0 can be a sentinel/never-issued id.
            next_id: AtomicU64::new(1),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Insert a freshly-opened session and return its handle id.
    pub fn insert(&self, session: Session) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.sessions
            .lock()
            .expect("registry poisoned")
            .insert(id, session);
        id
    }

    /// Run `f` against a live session under the lock. `None` if the handle is
    /// unknown (already closed / never issued).
    pub fn with_session<R>(&self, handle: u64, f: impl FnOnce(&mut Session) -> R) -> Option<R> {
        let mut guard = self.sessions.lock().expect("registry poisoned");
        guard.get_mut(&handle).map(f)
    }

    /// Clone the cert metadata value + the channel for a handle (so the RPC can
    /// run WITHOUT holding the registry lock across an `.await`). `None` if the
    /// handle is unknown.
    pub fn channel_and_cert(&self, handle: u64) -> Option<(Channel, MetadataValue<Ascii>)> {
        self.with_session(handle, |s| (s.channel.clone(), s.cert_value.clone()))
    }

    /// Remove + return a session (the `closeSession` path). The caller drops it:
    /// dropping `channel` tears down the RPC connection (owned by its
    /// `IrohConnector`) and dropping `endpoint` closes this device's local Iroh
    /// endpoint/socket; any in-flight subscription oneshots fire on drop.
    pub fn remove(&self, handle: u64) -> Option<Session> {
        self.sessions
            .lock()
            .expect("registry poisoned")
            .remove(&handle)
    }

    /// All live session paths (for an aggregate `natStats` view).
    pub fn all_paths(&self) -> Vec<concerto_transport::ConnectionPath> {
        self.sessions
            .lock()
            .expect("registry poisoned")
            .values()
            .map(|s| s.path)
            .collect()
    }

    /// Number of live subscriptions held on a session (`None` if the handle is
    /// unknown). Introspection for the registry-leak regression: after a stream
    /// reaches EOS / errors / is cancelled, its `subscriptions` entry must be
    /// gone (the spawned task removes its own id on EVERY exit path), so a
    /// drained session reports 0.
    pub fn subscription_count(&self, handle: u64) -> Option<usize> {
        self.with_session(handle, |s| s.subscriptions.len())
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.sessions.lock().expect("registry poisoned").len()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A registry test that does not need a live Iroh session: we exercise the
    // id allocation + lookup + removal contract with a hand-built Session is not
    // possible (Session holds a real Endpoint/Channel), so we test the id
    // bookkeeping via a parallel minimal registry of the SAME shape. The
    // open/lookup/close lifecycle over a REAL session is covered by the loopback
    // integration test. Here we prove the id monotonicity + sentinel invariant
    // on the AtomicU64 directly, which is the load-bearing registry logic.
    #[test]
    fn ids_are_monotonic_and_skip_zero() {
        let r = Registry::new();
        // We cannot insert a real Session in a unit test, but we can prove the
        // id generator never returns 0 and strictly increases.
        let a = r.next_id.fetch_add(1, Ordering::Relaxed);
        let b = r.next_id.fetch_add(1, Ordering::Relaxed);
        assert_eq!(a, 1, "first issued id is 1 (0 reserved as sentinel)");
        assert!(b > a, "ids strictly increase");
        assert_eq!(r.len(), 0, "no sessions inserted");
        assert!(r.remove(999).is_none(), "unknown handle removes to None");
        assert!(
            r.channel_and_cert(999).is_none(),
            "unknown handle lookup is None"
        );
        assert!(r.all_paths().is_empty());
    }
}
