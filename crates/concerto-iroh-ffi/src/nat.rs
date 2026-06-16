//! Client-side NAT path classification (Task 509, design/16 §4.6).
//!
//! `natStats` reports the [`ConnectionPath`](concerto_transport::ConnectionPath)
//! of THIS device's own session(s) — a **client-side** classification of the
//! live Iroh connection, NOT a Core RPC (the Core's `Runtime.GetNatStats` is a
//! different, server-aggregate surface). The classification mirrors
//! `concerto_transport::classify_path` exactly: a relay path → `Relayed`; an IP
//! path → `Lan` when the remote address is loopback / private / link-local, else
//! `Direct`; no selected path → the conservative `Relayed`.
//!
//! The live-connection classification reuses `concerto_transport::classify_path`
//! directly at session-open time (see `lib.rs`). The pure helper below mirrors
//! that mapping for the **address-range** half so the mapping is unit-testable
//! without a live QUIC connection.

use concerto_transport::ConnectionPath;

use crate::NatPath;

/// Map the transport's `ConnectionPath` onto the FFI-exported [`NatPath`].
pub fn to_ffi(path: ConnectionPath) -> NatPath {
    match path {
        ConnectionPath::Direct => NatPath::Direct,
        ConnectionPath::Relayed => NatPath::Relayed,
        ConnectionPath::Lan => NatPath::Lan,
    }
}

/// Classify an **IP** remote address the same way `concerto_transport`'s
/// `is_lan_addr` heuristic splits `Lan` from `Direct`: loopback / private /
/// link-local → `Lan`, otherwise `Direct`. (A relay path is classified
/// separately — there is no IP to inspect — and always maps to `Relayed`.)
///
/// This is the unit-testable mirror of the transport's address-range logic; the
/// live path is taken from `concerto_transport::classify_path(&connection)` at
/// open time (see `lib.rs::classify_initial_path`), so this mirror is only
/// compiled for the classification unit test.
#[cfg(test)]
pub fn classify_ip(ip: &std::net::IpAddr) -> ConnectionPath {
    if is_lan_addr(ip) {
        ConnectionPath::Lan
    } else {
        ConnectionPath::Direct
    }
}

/// Whether `ip` is loopback / private / link-local — identical to
/// `concerto_transport::state::is_lan_addr` (kept private there). Mirrored here
/// so the classification is testable without a live `Connection`.
#[cfg(test)]
fn is_lan_addr(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        std::net::IpAddr::V6(v6) => {
            // Loopback (::1), link-local (fe80::/10), or unique-local (fc00::/7).
            v6.is_loopback()
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    /// The classify_path mapping the task names: loopback → Lan, a public v4 →
    /// Direct, and a relay path → Relayed (relay maps unconditionally, no IP).
    #[test]
    fn classify_path_mapping_matches_transport_semantics() {
        // loopback → Lan
        assert_eq!(
            classify_ip(&IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ConnectionPath::Lan
        );
        assert_eq!(
            to_ffi(classify_ip(&IpAddr::V4(Ipv4Addr::LOCALHOST))),
            NatPath::Lan
        );

        // private + link-local v4 → Lan
        assert_eq!(
            classify_ip(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5))),
            ConnectionPath::Lan
        );
        assert_eq!(
            classify_ip(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))),
            ConnectionPath::Lan
        );

        // a public v4 → Direct
        assert_eq!(
            classify_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))),
            ConnectionPath::Direct
        );
        assert_eq!(
            to_ffi(classify_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)))),
            NatPath::Direct
        );

        // v6 loopback → Lan, public v6 → Direct
        assert_eq!(
            classify_ip(&IpAddr::V6(Ipv6Addr::LOCALHOST)),
            ConnectionPath::Lan
        );
        assert_eq!(
            classify_ip(&IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0, 0, 0, 0, 0, 1))),
            ConnectionPath::Direct
        );

        // relay → Relayed (the enum mapping; the live path comes from
        // classify_path returning ConnectionPath::Relayed for a relay address).
        assert_eq!(to_ffi(ConnectionPath::Relayed), NatPath::Relayed);
    }
}
