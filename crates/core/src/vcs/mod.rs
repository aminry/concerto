//! VCS Provider Integration (Task 45, `design/13`; the crate moved to
//! `crates/vcs` by Task 313).
//!
//! Task 313 extracted the whole VCS surface into the dedicated `concerto-vcs`
//! crate (the `VcsProvider` trait + octocrab `GitHubProvider` + the `gh`-CLI
//! fallback + the per-call `choose_backend` dispatch + the `fetch_issue` URL
//! router + the wiremock `testkit` harness). This module is now a thin
//! **wiring shim**: it re-exports the handle types the Core's `boot.rs` and the
//! `Vcs` gRPC handler already use ([`VcsConfig`], [`VcsHandle`], [`gh_cli`]) so
//! those call sites compile unchanged, and it keeps the supervised
//! [`VcsProviderActor`] here — the actor needs the Core's
//! [`crate::supervisor::Actor`] trait, which `concerto-vcs` (a leaf crate that
//! must not depend on the Core) cannot implement.
//!
//! The V0.1 `Vcs` gRPC service behavior is unchanged: [`VcsHandle`] still shells
//! out to `gh` for its Task-45 method set; the new octocrab/trait/dispatch
//! machinery is the *internal* surface the later VCS tasks build on.

use std::sync::Arc;

use async_trait::async_trait;
use concerto_error::Result;
use concerto_persist::Persistence;

// Re-export the moved surface so `crate::vcs::{gh_cli, VcsConfig, VcsHandle}`
// keeps resolving for `boot.rs` + `handlers/vcs.rs`.
pub use concerto_vcs::{gh_cli, VcsConfig, VcsHandle};

use crate::supervisor::{Actor, ActorContext};

/// Supervised actor that owns the [`VcsHandle`] (Task 45). `run` parks on
/// shutdown; the supervisor's factory clones the handle on each restart so the
/// cached `gh` path survives a wrapper panic.
///
/// Stays in `concerto-core` (not `concerto-vcs`) because it implements the
/// Core's [`Actor`] trait — the `concerto-vcs` leaf crate must not depend on the
/// Core. It is a thin wrapper around the moved [`VcsHandle`].
pub struct VcsProviderActor {
    handle: VcsHandle,
}

impl VcsProviderActor {
    /// Build a fresh actor with a new handle.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            handle: VcsHandle::new(persistence),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> VcsHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for VcsProviderActor {
    const NAME: &'static str = "vcs-provider";
    type Config = VcsConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        tracing::info!("VCS provider ready (gh CLI backend)");
        ctx.shutdown.cancelled().await;
        tracing::debug!("VCS provider actor shutting down");
        Ok(())
    }
}
