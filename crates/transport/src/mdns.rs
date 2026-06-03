//! mDNS LAN discovery for the Core (`design/11 §3.5`, §6.4, §12 R-3, Task 213).
//!
//! The Core (**responder**) advertises a `_concerto._tcp.local` service whose
//! TXT record carries exactly four keys — `endpoint_id` / `core_pubkey` /
//! `version` / `caps` (`design/11 §3.5`, FROZEN). Clients (**browser**) on the
//! same network browse for it, parse the TXT into a [`DiscoveredCore`], and hand
//! the discovered `endpoint_id` to the Task-212 Iroh connect path
//! ([`connect_channel`](crate::endpoint::connect_channel)) — opening Iroh
//! **directly** to the discovered endpoint, never consulting the relay
//! (`ConnectionPath::Lan`).
//!
//! The FROZEN type *declarations* — [`DiscoveredCore`](crate::api::DiscoveredCore),
//! [`MdnsConfig`](crate::api::MdnsConfig),
//! [`MdnsResponder`](crate::api::MdnsResponder),
//! [`MdnsBrowser`](crate::api::MdnsBrowser) — and the service-type / TXT-key
//! consts live in [`crate::api`] (the interface-generator convention); this
//! module holds their method impls and the publish/browse logic.
//!
//! # What is safe to broadcast
//!
//! mDNS is **public on the LAN** (`design/12 §3.6`): the TXT record and the
//! public-key fingerprint are visible to anyone on the segment. So the TXT
//! carries **nothing secret** — only the Iroh endpoint id (already a routing
//! address), the Core Ed25519 *public* key (a fingerprint hint, NOT an auth
//! credential — trust is still established by the QR/cert pairing flow, Task
//! 207), the version, and the coarse caps list. No device certs, no tokens.
//!
//! # Opt-out — and why `disable_remote` is NOT an mDNS switch
//!
//! Publication is suppressible via a dedicated managed/per-network opt-out
//! ([`MdnsConfig::opt_out`]). **`disable_remote = true` does NOT silence mDNS**
//! (`design/11 §6.4`: LAN-only mode "Continues to publish mDNS"): the only thing
//! that silences mDNS is the dedicated opt-out. The two settings are orthogonal
//! — `disable_remote` gates relay registration + remote accept (Task 211/212);
//! the mDNS opt-out gates LAN advertisement.
//!
//! # IPv4 + IPv6 (R-3)
//!
//! `mdns-sd` registers A **and** AAAA records and the browser accepts either —
//! some networks suppress one but not the other (`design/11 §12 R-3`). The
//! responder advertises every host address it is given; the loopback interfaces
//! are enabled by default, which is what makes the Tier-2 double hermetic.
//!
//! # Cross-platform
//!
//! `mdns-sd` is pure-Rust (`if-addrs` + `socket2`) and builds on
//! Windows/Linux/macOS; nothing here is `#[cfg(unix)]`.

use std::net::IpAddr;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::api::{
    DiscoveredCore, MdnsBrowser, MdnsConfig, MdnsResponder, SERVICE_TYPE, TXT_CAPS,
    TXT_CORE_PUBKEY, TXT_ENDPOINT_ID, TXT_VERSION,
};
use crate::error::{Result, TransportError};

/// The local hostname the responder advertises the service under. mDNS host
/// names must end in `.local.`; a fixed, instance-scoped label keeps the
/// registration deterministic (the actual reachability comes from the Iroh
/// `endpoint_id` in the TXT record, not from this name resolving).
fn host_name(instance: &str) -> String {
    // Sanitize to a DNS-label-safe token (the instance name may contain spaces
    // / punctuation from a user-set Core name).
    let label: String = instance
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let label = label.trim_matches('-');
    let label = if label.is_empty() {
        "concerto-core"
    } else {
        label
    };
    format!("{label}.local.")
}

impl MdnsConfig {
    /// Build a publish config from the live transport values. `addrs` is the set
    /// of host IPs to advertise (both IPv4 and IPv6 where available, R-3);
    /// passing an empty set lets `mdns-sd` fill in the host's auto-detected
    /// addresses. `opt_out` suppresses publication entirely (the managed /
    /// per-network mDNS opt-out — independent of `disable_remote`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        instance_name: impl Into<String>,
        endpoint_id: impl Into<String>,
        core_pubkey_b64: impl Into<String>,
        version: impl Into<String>,
        caps: impl Into<String>,
        port: u16,
        addrs: Vec<IpAddr>,
        opt_out: bool,
    ) -> Self {
        Self {
            instance_name: instance_name.into(),
            endpoint_id: endpoint_id.into(),
            core_pubkey_b64: core_pubkey_b64.into(),
            version: version.into(),
            caps: caps.into(),
            port,
            addrs,
            opt_out,
        }
    }
}

impl MdnsResponder {
    /// Publish the `_concerto._tcp.local` service from this config — UNLESS the
    /// opt-out is set, in which case this is a no-op handle that advertises
    /// nothing (the managed / per-network mDNS opt-out, `design/11 §3.5`). Note
    /// `disable_remote` is **not** consulted here: LAN-only mode still publishes
    /// mDNS (`design/11 §6.4`); the caller passes `opt_out` from the dedicated
    /// setting only.
    ///
    /// Registers A **and** AAAA records (R-3) for every address in the config
    /// (or the host's auto-detected addresses when none are supplied). The
    /// returned handle deregisters cleanly on [`Self::shutdown`] / drop (an mDNS
    /// goodbye packet), so stale records don't linger.
    pub fn publish(config: MdnsConfig) -> Result<Self> {
        if config.opt_out {
            tracing::info!(
                "mdns: publication opted out (managed / per-network) — not advertising _concerto._tcp.local"
            );
            return Ok(Self {
                daemon: None,
                fullname: None,
                config,
            });
        }

        let daemon = ServiceDaemon::new()
            .map_err(|e| TransportError::Mdns(format!("creating mDNS daemon: {e}")))?;

        let info = build_service_info(&config)?;
        let fullname = info.get_fullname().to_string();

        daemon
            .register(info)
            .map_err(|e| TransportError::Mdns(format!("registering mDNS service: {e}")))?;

        tracing::info!(
            service = SERVICE_TYPE,
            instance = %config.instance_name,
            endpoint_id = %config.endpoint_id,
            "mdns: published _concerto._tcp.local (TXT: endpoint_id/core_pubkey/version/caps, IPv4+IPv6)"
        );

        Ok(Self {
            daemon: Some(daemon),
            fullname: Some(fullname),
            config,
        })
    }

    /// Whether this responder is actually advertising (false when the opt-out
    /// suppressed publication).
    pub fn is_publishing(&self) -> bool {
        self.daemon.is_some()
    }

    /// The config this responder was published from — lets the owning transport
    /// decide whether a re-announce is needed (on `version` / `endpoint_id` /
    /// `caps` change, `design/11 §3.5`).
    pub fn config(&self) -> &MdnsConfig {
        &self.config
    }

    /// The full instance name registered (`<instance>._concerto._tcp.local.`),
    /// or `None` when publication was opted out. Lets the browser side filter
    /// its own loopback advertisement in tests.
    pub fn fullname(&self) -> Option<&str> {
        self.fullname.as_deref()
    }

    /// Deregister the service (sends the mDNS goodbye packet) and stop the
    /// daemon. Idempotent — safe to call before drop. After this the responder
    /// advertises nothing.
    pub fn shutdown(&mut self) {
        if let (Some(daemon), Some(fullname)) = (self.daemon.as_ref(), self.fullname.as_ref()) {
            // Best-effort goodbye; a failure here only means a stale record may
            // linger until its TTL expires.
            if let Err(e) = daemon.unregister(fullname) {
                tracing::warn!(%fullname, error = %e, "mdns: unregister failed");
            }
            let _ = daemon.shutdown();
        }
        self.daemon = None;
        self.fullname = None;
    }
}

impl Drop for MdnsResponder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Build the `mdns-sd` [`ServiceInfo`] for the Concerto service from `config`,
/// stamping the exact four-key TXT record (`design/11 §3.5`, FROZEN).
fn build_service_info(config: &MdnsConfig) -> Result<ServiceInfo> {
    let properties = [
        (TXT_ENDPOINT_ID, config.endpoint_id.as_str()),
        (TXT_CORE_PUBKEY, config.core_pubkey_b64.as_str()),
        (TXT_VERSION, config.version.as_str()),
        (TXT_CAPS, config.caps.as_str()),
    ];

    let host = host_name(&config.instance_name);
    // `mdns-sd` takes addresses as an `&[IpAddr]`; an empty slice means "let the
    // library auto-detect and keep them updated" via `enable_addr_auto`.
    let addrs: &[IpAddr] = &config.addrs;

    let mut info = ServiceInfo::new(
        SERVICE_TYPE,
        &config.instance_name,
        &host,
        addrs,
        config.port,
        &properties[..],
    )
    .map_err(|e| TransportError::Mdns(format!("building mDNS service info: {e}")))?;

    if config.addrs.is_empty() {
        // Advertise every host address (IPv4 + IPv6, R-3) and keep the record
        // updated as interfaces come and go.
        info = info.enable_addr_auto();
    }

    Ok(info)
}

impl DiscoveredCore {
    /// Parse a resolved Concerto service's TXT record into the discovered-Core
    /// descriptor. Requires all four FROZEN keys (`design/11 §3.5`); a missing
    /// key means the advertiser is not a Concerto Core (or speaks an
    /// incompatible schema) and is skipped by the browser. `addresses` is the
    /// resolved IP set (IPv4 + IPv6).
    pub(crate) fn from_txt(
        fullname: &str,
        addresses: Vec<IpAddr>,
        get: impl Fn(&str) -> Option<String>,
    ) -> Option<Self> {
        let endpoint_id = get(TXT_ENDPOINT_ID)?;
        let core_pubkey_b64 = get(TXT_CORE_PUBKEY)?;
        let version = get(TXT_VERSION)?;
        // `caps` may legitimately be empty (a Core with no optional features),
        // but the key must be present.
        let caps = get(TXT_CAPS)?;
        Some(Self {
            instance_name: instance_label(fullname),
            endpoint_id,
            core_pubkey_b64,
            version,
            caps,
            addresses,
        })
    }

    /// The advertised features as a vec (the comma-separated `caps` TXT value,
    /// empties dropped). A coarse hint a client uses to decide whether to bother
    /// connecting (`design/11 §3.5`).
    pub fn caps_list(&self) -> Vec<String> {
        self.caps
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }
}

/// The instance label from a resolved fullname
/// (`<instance>._concerto._tcp.local.` → `<instance>`).
fn instance_label(fullname: &str) -> String {
    fullname
        .strip_suffix(&format!(".{SERVICE_TYPE}"))
        .unwrap_or(fullname)
        .to_string()
}

impl MdnsBrowser {
    /// Start browsing for `_concerto._tcp.local` on the LAN. Spawns the
    /// `mdns-sd` daemon thread and a background task that turns resolved
    /// services into [`DiscoveredCore`] descriptors on an internal channel,
    /// drained by [`Self::recv`]. `exclude_fullname` (typically the local
    /// responder's own instance, used by the Tier-2 double) is filtered out so a
    /// browser does not rediscover its own host's advertisement.
    pub fn start(exclude_fullname: Option<String>) -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| TransportError::Mdns(format!("creating mDNS browse daemon: {e}")))?;
        let receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| TransportError::Mdns(format!("browsing {SERVICE_TYPE}: {e}")))?;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        // The `mdns-sd` receiver is a blocking `flume::Receiver`; bridge it onto
        // the async channel on a blocking task so `recv()` is fully async and
        // bounded by the caller's own timeout.
        tokio::task::spawn_blocking(move || {
            while let Ok(event) = receiver.recv() {
                if let ServiceEvent::ServiceResolved(resolved) = event {
                    if let Some(excl) = &exclude_fullname {
                        if resolved.get_fullname() == excl {
                            continue;
                        }
                    }
                    let addresses: Vec<IpAddr> = resolved
                        .get_addresses()
                        .iter()
                        .map(|s| s.to_ip_addr())
                        .collect();
                    let discovered =
                        DiscoveredCore::from_txt(resolved.get_fullname(), addresses, |k| {
                            resolved.get_property_val_str(k).map(|v| v.to_string())
                        });
                    if let Some(core) = discovered {
                        // Receiver dropped → stop bridging.
                        if tx.send(core).is_err() {
                            break;
                        }
                    } else {
                        tracing::debug!(
                            fullname = resolved.get_fullname(),
                            "mdns: resolved a _concerto._tcp.local advertiser missing a required TXT key; skipping"
                        );
                    }
                }
            }
        });

        Ok(Self {
            daemon: Some(daemon),
            rx,
        })
    }

    /// Await the next discovered Core (LAN-preferred path the client then opens
    /// Iroh to via the Task-212 connect path). `None` once the daemon thread has
    /// stopped. Callers bound their wait with a timeout (mDNS discovery is
    /// best-effort; a quiet LAN never resolves).
    pub async fn recv(&mut self) -> Option<DiscoveredCore> {
        self.rx.recv().await
    }

    /// Stop browsing and shut the daemon down. Idempotent.
    pub fn shutdown(&mut self) {
        if let Some(daemon) = self.daemon.take() {
            let _ = daemon.stop_browse(SERVICE_TYPE);
            let _ = daemon.shutdown();
        }
    }
}

impl Drop for MdnsBrowser {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn host_name_sanitizes() {
        assert_eq!(host_name("My Core!"), "My-Core.local.");
        assert_eq!(host_name(""), "concerto-core.local.");
        assert_eq!(host_name("---"), "concerto-core.local.");
        assert_eq!(host_name("box-1"), "box-1.local.");
    }

    #[test]
    fn instance_label_strips_service_suffix() {
        assert_eq!(
            instance_label(&format!("alice.{SERVICE_TYPE}")),
            "alice".to_string()
        );
        // A bare name with no suffix is returned as-is.
        assert_eq!(instance_label("alice"), "alice".to_string());
    }

    #[test]
    fn from_txt_requires_all_four_keys() {
        use std::collections::BTreeMap;
        let full: BTreeMap<&str, &str> = [
            (TXT_ENDPOINT_ID, "ep123"),
            (TXT_CORE_PUBKEY, "cGs="),
            (TXT_VERSION, "1.2.3"),
            (TXT_CAPS, "files,streams"),
        ]
        .into_iter()
        .collect();

        let parsed = DiscoveredCore::from_txt(&format!("alice.{SERVICE_TYPE}"), vec![], |k| {
            full.get(k).map(|v| v.to_string())
        })
        .expect("all four keys present");
        assert_eq!(parsed.endpoint_id, "ep123");
        assert_eq!(parsed.core_pubkey_b64, "cGs=");
        assert_eq!(parsed.version, "1.2.3");
        assert_eq!(parsed.caps_list(), vec!["files", "streams"]);

        // Dropping any one required key fails the parse.
        for missing in [TXT_ENDPOINT_ID, TXT_CORE_PUBKEY, TXT_VERSION, TXT_CAPS] {
            let mut partial = full.clone();
            partial.remove(missing);
            assert!(
                DiscoveredCore::from_txt(SERVICE_TYPE, vec![], |k| partial
                    .get(k)
                    .map(|v| v.to_string()))
                .is_none(),
                "parse should fail when '{missing}' is absent"
            );
        }
    }

    #[test]
    fn empty_caps_is_valid_but_present() {
        use std::collections::BTreeMap;
        let m: BTreeMap<&str, &str> = [
            (TXT_ENDPOINT_ID, "ep"),
            (TXT_CORE_PUBKEY, "pk"),
            (TXT_VERSION, "0.0.1"),
            (TXT_CAPS, ""),
        ]
        .into_iter()
        .collect();
        let parsed =
            DiscoveredCore::from_txt(SERVICE_TYPE, vec![], |k| m.get(k).map(|v| v.to_string()))
                .expect("empty caps is still a present key");
        assert!(parsed.caps_list().is_empty());
    }

    #[test]
    fn opt_out_responder_publishes_nothing() {
        let cfg = MdnsConfig::new(
            "test-core",
            "endpoint-abc",
            "cHVia2V5",
            "1.0.0",
            "files",
            4711,
            vec![],
            /* opt_out = */ true,
        );
        let responder = MdnsResponder::publish(cfg).expect("opt-out publish is a no-op handle");
        assert!(!responder.is_publishing());
        assert!(responder.fullname().is_none());
    }

    #[test]
    fn service_type_and_txt_keys_are_frozen() {
        // The FROZEN wire contract (`design/11 §3.5`): mobile (511) / web (521)
        // browse for exactly these strings.
        assert_eq!(SERVICE_TYPE, "_concerto._tcp.local.");
        assert_eq!(TXT_ENDPOINT_ID, "endpoint_id");
        assert_eq!(TXT_CORE_PUBKEY, "core_pubkey");
        assert_eq!(TXT_VERSION, "version");
        assert_eq!(TXT_CAPS, "caps");
        // No accidental duplicate / typo'd keys.
        let keys: BTreeSet<&str> = [TXT_ENDPOINT_ID, TXT_CORE_PUBKEY, TXT_VERSION, TXT_CAPS]
            .into_iter()
            .collect();
        assert_eq!(keys.len(), 4);
    }
}
