//! Per-connection transport-kind tagging seam (Task 201).
//!
//! # The contract
//!
//! [`ConnTransport`] is a request-extension carrier for the
//! [`TransportKind`] a gRPC request **physically arrived on**. The seam
//! has exactly one rule, and every present and future transport listener
//! obeys it:
//!
//! > **Every transport listener tags each request it accepts with a
//! > [`ConnTransport`] before the request reaches a handler. Handlers
//! > never infer transport from socket internals — they only read the
//! > tag.**
//!
//! This replaces the V0.1 hardcode in
//! [`crate::handlers::runtime::RuntimeHandler`], where
//! `GetServerCapabilities` always reported
//! [`TransportKind::Uds`]. With the tag in place, the handler reports the
//! **live** connection's kind and the rest of Phase 2's clients (and
//! `design/15 §3.11`'s remote-mode affordance suppression) can key off
//! `ServerCapabilities.transport_kind`.
//!
//! ## Who tags what
//!
//! - **UDS listener** — tags [`TransportKind::Uds`]. Wired in
//!   [`crate::api_server`] **now** (this task), via a tonic interceptor
//!   layer applied to the whole server. On Windows the co-located
//!   transport is a **named pipe**; it maps to [`TransportKind::Uds`]
//!   too ("co-located, peer-attested" — same trust model, no new enum
//!   variant).
//! - **Iroh listener** — will tag [`TransportKind::Iroh`] in **Task 212**
//!   by inserting `ConnTransport(TransportKind::Iroh)` in its own
//!   listener setup. It must **not** touch the handler.
//! - **WSS bridge** — will tag [`TransportKind::WssBridge`] in **Task
//!   204** the same way.
//!
//! ## Default when absent
//!
//! When a request carries no [`ConnTransport`] (direct in-process handler
//! construction in tests, or any not-yet-tagged path), the handler
//! defaults to [`TransportKind::Uds`] for back-compat. The default lives
//! in the handler, not here, so the carrier stays a dumb value type.

use concerto_proto::v1::TransportKind;

/// Request-extension carrier for the inbound connection's
/// [`TransportKind`].
///
/// Inserted into every request's extensions by the transport listener
/// that accepted the connection (see the [module docs](self) for the
/// all-listeners-tag contract). Read by
/// [`crate::handlers::runtime::RuntimeHandler::get_server_capabilities`]
/// to report the live transport kind in `ServerCapabilities`.
///
/// **FROZEN (Task 201):** the type's name and that it carries a single
/// [`TransportKind`] are the seam every listener writes and the handler
/// reads. Field order / extra fields are a revision task.
///
/// Deliberately holds only a plain `TransportKind` (no `std::os::unix`
/// types) so the carrier compiles on the Windows CI lane; UDS-specific
/// listener glue is gated under `#[cfg(unix)]` in [`crate::api_server`],
/// never here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnTransport(pub TransportKind);

impl ConnTransport {
    /// The tagged transport kind.
    pub fn kind(self) -> TransportKind {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_the_tagged_kind() {
        assert_eq!(
            ConnTransport(TransportKind::Iroh).kind(),
            TransportKind::Iroh
        );
        assert_eq!(ConnTransport(TransportKind::Uds).kind(), TransportKind::Uds);
    }
}
