//! Process-signal handling for the Core daemon.
//!
//! V0.1 surface (Task 11):
//!
//! - Unix: SIGTERM, SIGINT, SIGHUP are observed via `tokio::signal::unix`.
//!   SIGTERM / SIGINT trigger a graceful-shutdown cause; SIGHUP is a
//!   placeholder for future config-reload (`design/01 §3.4`; full reload
//!   is V1.0).
//! - Windows: only `tokio::signal::ctrl_c()` is wired up — there is no
//!   SIGHUP equivalent in V0.1.
//!
//! Per the orchestrator's drift note for Task 11 we use `tokio::signal::*`
//! (already a dependency via the `signal` feature) instead of pulling in
//! `signal-hook` / `signal-hook-tokio`. The result is one fewer crate in
//! the dependency graph for the same observable behaviour.

#[cfg(unix)]
use concerto_error::Error;
use concerto_error::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Why the runtime is shutting down.
///
/// Surfaced on the shutdown event channel so the supervisor (and
/// eventually the audit log) can record the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownCause {
    /// SIGTERM received (the OS service-manager's "please stop" signal).
    Sigterm,
    /// SIGINT received (Ctrl-C from an interactive shell).
    Sigint,
    /// Cancellation was triggered programmatically — e.g. a fatal
    /// startup error or an admin RPC. The token is the source of truth;
    /// the signal listener relays signals into the same token.
    Programmatic,
}

impl ShutdownCause {
    /// Human-readable label for logging.
    pub fn as_str(self) -> &'static str {
        match self {
            ShutdownCause::Sigterm => "sigterm",
            ShutdownCause::Sigint => "sigint",
            ShutdownCause::Programmatic => "programmatic",
        }
    }
}

/// What a SIGHUP currently means: nothing observable. Reload is V1.0.
///
/// Kept as a typed event so call-sites can pattern-match a real enum
/// when the V1.0 reload lands, instead of refactoring control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadEvent {
    /// Operator asked us to reload config. V0.1 logs and does nothing.
    SighupReceived,
}

/// Spawns the signal-listening task.
///
/// The task lives until either (a) one of the registered signals fires
/// (in which case it cancels `shutdown` with the corresponding cause and
/// exits) or (b) `shutdown` is cancelled by someone else (in which case
/// the listener notices and exits).
///
/// `reload_tx` receives `ReloadEvent::SighupReceived` on every SIGHUP.
/// It is dropped (closing the channel) when the listener exits. The
/// channel is bounded to 1 — multiple HUPs in flight collapse into one.
///
/// Returns a [`tokio::task::JoinHandle`] for the spawned listener and
/// the receiver half of the reload channel.
pub fn install(
    shutdown: CancellationToken,
) -> Result<(tokio::task::JoinHandle<()>, mpsc::Receiver<ReloadEvent>)> {
    let (reload_tx, reload_rx) = mpsc::channel(1);
    let handle = spawn_listener(shutdown, reload_tx)?;
    Ok((handle, reload_rx))
}

/// Used internally by `install` and by tests that want to drive the
/// listener with a synthetic cancellation token.
fn spawn_listener(
    shutdown: CancellationToken,
    reload_tx: mpsc::Sender<ReloadEvent>,
) -> Result<tokio::task::JoinHandle<()>> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .map_err(|e| Error::Internal(format!("install SIGTERM handler: {e}")))?;
        let mut sigint = signal(SignalKind::interrupt())
            .map_err(|e| Error::Internal(format!("install SIGINT handler: {e}")))?;
        let mut sighup = signal(SignalKind::hangup())
            .map_err(|e| Error::Internal(format!("install SIGHUP handler: {e}")))?;

        Ok(tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::debug!("signal listener: shutdown token cancelled externally");
                        return;
                    }
                    _ = sigterm.recv() => {
                        tracing::info!(cause = ShutdownCause::Sigterm.as_str(), "shutdown requested");
                        shutdown.cancel();
                        return;
                    }
                    _ = sigint.recv() => {
                        tracing::info!(cause = ShutdownCause::Sigint.as_str(), "shutdown requested");
                        shutdown.cancel();
                        return;
                    }
                    _ = sighup.recv() => {
                        // V0.1 placeholder: hot config reload is V1.0
                        // (see design/01 §3.4 R-4). Surface the event
                        // for any opt-in subscriber; if the channel is
                        // already full (a prior HUP unconsumed) just
                        // drop the new one — coalescing is fine.
                        tracing::info!("SIGHUP received; config reload is V1.0 (no-op in V0.1)");
                        let _ = reload_tx.try_send(ReloadEvent::SighupReceived);
                    }
                }
            }
        }))
    }
    #[cfg(not(unix))]
    {
        let _ = reload_tx; // unused on Windows in V0.1
        Ok(tokio::spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("signal listener: shutdown token cancelled externally");
                }
                res = tokio::signal::ctrl_c() => {
                    match res {
                        Ok(()) => {
                            tracing::info!(
                                cause = ShutdownCause::Sigint.as_str(),
                                "shutdown requested"
                            );
                            shutdown.cancel();
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "ctrl_c handler failed");
                            shutdown.cancel();
                        }
                    }
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn listener_exits_when_token_cancelled_externally() {
        let token = CancellationToken::new();
        let (handle, _reload_rx) = install(token.clone()).expect("install signal listener");

        // Cancel externally; listener should observe and exit.
        token.cancel();

        // Give the listener a moment to wake up.
        let res = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(
            res.is_ok(),
            "signal listener should exit promptly after token cancellation"
        );
    }
}
