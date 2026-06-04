//! In-memory transport state helpers (`design/11 §4`, Task 212).
//!
//! The FROZEN type *declarations* — [`TransportState`](crate::api::TransportState),
//! [`ActiveSession`](crate::api::ActiveSession),
//! [`ConnectionPath`](crate::api::ConnectionPath),
//! [`NatStats`](crate::api::NatStats), [`DeviceId`](crate::api::DeviceId) — live
//! in [`crate::api`] (the interface-generator convention). This module holds the
//! path-classification logic those types delegate to.
//!
//! The transport stores **no business state in SQLite** (`design/11 §4`); the
//! types above are the in-memory model 216 (NAT telemetry) and 217
//! (`TransportHandle`) read.

use iroh::endpoint::Connection;
use iroh::{TransportAddr, Watcher};

use crate::api::ConnectionPath;

/// Classify the live [`ConnectionPath`] of an Iroh connection from its currently
/// **selected** path (`design/11 §4`, §3.6).
///
/// Reads `Connection::paths()` (a `Watcher<Value = PathInfoList>`) and inspects
/// the selected path: a relay path → [`ConnectionPath::Relayed`]; an IP path →
/// [`ConnectionPath::Lan`] if the remote address is loopback / private /
/// link-local, else [`ConnectionPath::Direct`]. With no selected path yet,
/// defaults to the most conservative remote classification
/// ([`ConnectionPath::Relayed`]) so the `disable_remote` LAN-only gate never
/// *over*-admits; 216's telemetry re-reads this as paths migrate.
///
/// The `Lan`-vs-`Direct` split is a documented heuristic: Iroh exposes "IP path"
/// vs "relay path" but does not itself label an IP path loopback/LAN vs
/// hole-punched-WAN, so 212 uses the address range. Task 213 (mDNS) refines this
/// when a session is known to have come from a LAN-discovered endpoint.
pub fn classify_path(conn: &Connection) -> ConnectionPath {
    let paths = conn.paths().get();
    let chosen = paths
        .iter()
        .find(|p| p.is_selected())
        .or_else(|| paths.iter().next());

    match chosen.map(|p| p.remote_addr()) {
        Some(TransportAddr::Relay(_)) => ConnectionPath::Relayed,
        Some(TransportAddr::Ip(sa)) if is_lan_addr(&sa.ip()) => ConnectionPath::Lan,
        Some(TransportAddr::Ip(_)) => ConnectionPath::Direct,
        _ => ConnectionPath::Relayed,
    }
}

/// Whether `ip` is a loopback / private / link-local address — the heuristic
/// that distinguishes [`ConnectionPath::Lan`] from [`ConnectionPath::Direct`].
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
    use crate::api::NatStats;

    #[test]
    fn lan_addr_heuristic() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        assert!(is_lan_addr(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_lan_addr(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5))));
        assert!(is_lan_addr(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_lan_addr(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
        assert!(!is_lan_addr(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(is_lan_addr(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(!is_lan_addr(&IpAddr::V6(Ipv6Addr::new(
            0x2606, 0x4700, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn nat_stats_record() {
        use crate::api::ClientKind;
        let mut s = NatStats::default();
        s.record(ConnectionPath::Direct, "direct", ClientKind::Mobile);
        s.record(
            ConnectionPath::Direct,
            "direct",
            ClientKind::DesktopSplitHost,
        );
        s.record(
            ConnectionPath::Relayed,
            "relayed",
            ClientKind::DesktopSplitHost,
        );
        s.record(ConnectionPath::Lan, "lan", ClientKind::Mobile);
        assert_eq!(s.direct_today, 2);
        assert_eq!(s.relayed_today, 1);
        assert_eq!(s.lan_today, 1);
        // by-client-kind: mobile got 1 direct + 1 lan; desktop got 1 direct + 1 relayed.
        let mobile = s.by_client_kind[&ClientKind::Mobile];
        assert_eq!(mobile.direct, 1);
        assert_eq!(mobile.lan, 1);
        assert_eq!(mobile.relayed, 0);
        let desktop = s.by_client_kind[&ClientKind::DesktopSplitHost];
        assert_eq!(desktop.direct, 1);
        assert_eq!(desktop.relayed, 1);
        // by-network-class mirrors the path label.
        assert_eq!(s.by_network_class["direct"].direct, 2);
        assert_eq!(s.by_network_class["relayed"].relayed, 1);
        assert_eq!(s.by_network_class["lan"].lan, 1);
    }

    #[test]
    fn connection_path_is_lan() {
        assert!(ConnectionPath::Lan.is_lan());
        assert!(!ConnectionPath::Direct.is_lan());
        assert!(!ConnectionPath::Relayed.is_lan());
    }
}
