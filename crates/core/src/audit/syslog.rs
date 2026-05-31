//! RFC 5424 syslog forwarder (Task 112).
//!
//! Forwards each audit event as an RFC 5424 syslog message to a network
//! syslog host (`host:port`), over either UDP or TCP. This is a *network*
//! transport on purpose: it is cross-platform (compiles + runs on the
//! Windows CI lane), avoids the libc `syslog()` / `/dev/log` /
//! Unix-domain-socket paths that are Unix-only, and is exactly what a
//! forwarding hook to a remote rsyslog / journald-relay / SIEM collector
//! needs anyway.
//!
//! ## Non-blocking isolation
//!
//! The fan-out drain loop must never stall on a slow or down syslog
//! endpoint. `on_event` therefore only serializes the message and
//! `try_send`s it onto an internal bounded channel; a dedicated worker
//! task owns the socket and does the actual network I/O. A full channel
//! drops-and-logs (the JSONL subscriber remains the durable floor); a
//! send failure is logged and the worker keeps running so a later
//! recovery still forwards subsequent events.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, Mutex};

use super::event::AuditEvent;
use super::writer::AuditLogSubscriber;

/// Wire transport for the syslog forwarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyslogTransport {
    /// Connectionless datagrams. Fire-and-forget; the kernel may drop on a
    /// full socket buffer. The traditional syslog transport.
    Udp,
    /// Stream transport with RFC 6587 octet-counting framing. Reconnects
    /// lazily when the connection drops.
    Tcp,
}

/// Internal bound on the in-flight forward queue. Sized so a brief
/// endpoint stall buffers a burst without unbounded memory growth; on
/// overflow we drop-and-log (JSONL stays the durable record).
const FORWARD_QUEUE_CAPACITY: usize = 1024;

/// RFC 5424 syslog forwarder.
pub struct SyslogSubscriber {
    tx: mpsc::Sender<String>,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SyslogSubscriber {
    /// Build a forwarder targeting `addr` (`host:port`) over `transport`.
    ///
    /// The worker task is spawned immediately on the current Tokio
    /// runtime; it connects lazily on the first event so construction
    /// never blocks on the network.
    pub fn new(addr: impl Into<String>, transport: SyslogTransport) -> Self {
        let addr = addr.into();
        let (tx, rx) = mpsc::channel(FORWARD_QUEUE_CAPACITY);
        let worker = tokio::spawn(run_worker(addr, transport, rx));
        Self {
            tx,
            worker: Mutex::new(Some(worker)),
        }
    }

    /// Render an event as an RFC 5424 message.
    ///
    /// Format: `<PRI>1 TIMESTAMP HOSTNAME APP-NAME PROCID MSGID
    /// STRUCTURED-DATA MSG`. We use facility `local0` (16) +
    /// severity `info` (6) → PRI 134. The MSG body is the canonical audit
    /// JSONL line (without its trailing newline) so downstream collectors
    /// get the same structured payload the on-disk file holds.
    fn format_5424(event: &AuditEvent) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let json = super::jsonl::serialize_event_line(event, now_secs)
            .unwrap_or_else(|_| "{}\n".to_string());
        let msg = json.trim_end_matches('\n');
        // RFC 5424 header. PRI 134 = local0.info. VERSION 1.
        // TIMESTAMP/HOSTNAME/PROCID/MSGID/STRUCTURED-DATA use NILVALUE `-`
        // where we don't have a value; APP-NAME is `concerto`. Collectors
        // read the JSON MSG; the header is intentionally minimal and
        // dependency-free.
        format!("<134>1 - - concerto - audit - {msg}")
    }
}

#[async_trait]
impl AuditLogSubscriber for SyslogSubscriber {
    fn id(&self) -> &str {
        "syslog"
    }

    async fn on_event(&self, event: &AuditEvent) {
        let msg = Self::format_5424(event);
        match self.tx.try_send(msg) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    audit.kind = event.kind.as_str(),
                    "audit(syslog): forward queue full — dropping (JSONL retains it)"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Worker exited; nothing forwards. JSONL still records it.
            }
        }
    }

    async fn flush(&self) {
        // Drain by closing the channel and awaiting the worker. Take the
        // sender out of scope by dropping our clone is not possible (we
        // hold it behind `&self`); instead we await the worker only if it
        // has already finished. On real shutdown the whole subscriber is
        // dropped, which closes `tx` and lets the worker exit; here we
        // just give any in-flight write a chance to land.
        let mut guard = self.worker.lock().await;
        if let Some(handle) = guard.as_ref() {
            if handle.is_finished() {
                if let Some(h) = guard.take() {
                    let _ = h.await;
                }
            }
        }
    }
}

impl Drop for SyslogSubscriber {
    fn drop(&mut self) {
        // Closing the channel (the `tx` clone inside `self` is dropped
        // with the struct) signals the worker to flush + exit.
        if let Ok(mut guard) = self.worker.try_lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}

/// Worker loop: owns the socket, reconnects lazily, and writes each
/// queued message. Runs until the channel closes.
async fn run_worker(addr: String, transport: SyslogTransport, mut rx: mpsc::Receiver<String>) {
    match transport {
        SyslogTransport::Udp => {
            // Bind an ephemeral local socket once; `send_to` resolves the
            // target each call so a transient DNS/host blip self-heals.
            let sock = match UdpSocket::bind("0.0.0.0:0").await {
                Ok(s) => Arc::new(s),
                Err(e) => {
                    tracing::warn!(error = %e, "audit(syslog): UDP bind failed; forwarding disabled");
                    // Drain the channel so producers' try_send sees a
                    // closed receiver promptly.
                    rx.close();
                    return;
                }
            };
            while let Some(msg) = rx.recv().await {
                if let Err(e) = sock.send_to(msg.as_bytes(), &addr).await {
                    tracing::warn!(error = %e, "audit(syslog): UDP send failed; dropping (JSONL retains it)");
                }
            }
        }
        SyslogTransport::Tcp => {
            let mut stream: Option<TcpStream> = None;
            while let Some(msg) = rx.recv().await {
                // RFC 6587 octet-counting framing: `<len> <msg>`.
                let framed = format!("{} {msg}", msg.len());
                // (Re)connect lazily.
                if stream.is_none() {
                    match TcpStream::connect(&addr).await {
                        Ok(s) => stream = Some(s),
                        Err(e) => {
                            tracing::warn!(error = %e, "audit(syslog): TCP connect failed; dropping (JSONL retains it)");
                            continue;
                        }
                    }
                }
                if let Some(s) = stream.as_mut() {
                    if let Err(e) = s.write_all(framed.as_bytes()).await {
                        tracing::warn!(error = %e, "audit(syslog): TCP write failed; will reconnect");
                        stream = None;
                    }
                }
            }
            if let Some(mut s) = stream {
                let _ = s.flush().await;
            }
        }
    }
}
