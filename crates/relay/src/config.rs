//! Twelve-Factor env-var config parsing (`design/11 §6.3`, Task 214).
//!
//! Env only — no config file, no flags beyond `--help` / `--version`. Parse
//! once at startup, validate, fail fast with a precise message naming the bad
//! var. The FROZEN config struct ([`RelayConfig`](crate::api::RelayConfig)) is
//! declared in [`crate::api`]; this module holds the parse/validate logic.

use std::net::SocketAddr;
use std::str::FromStr;

use crate::api::{
    RelayConfig, DEFAULT_MAX_ROUTES, DEFAULT_PROMETHEUS_LISTEN_ADDR, DEFAULT_RELAY_LISTEN_ADDR,
};
use crate::error::{RelayError, Result};

/// The names of the FROZEN env vars (`design/11 §6.3`). Exposed for `--help`
/// text and for the error messages.
pub const ENV_RELAY_LISTEN_ADDR: &str = "RELAY_LISTEN_ADDR";
pub const ENV_WSS_LISTEN_ADDR: &str = "WSS_LISTEN_ADDR";
pub const ENV_WEBHOOK_LISTEN_ADDR: &str = "WEBHOOK_LISTEN_ADDR";
pub const ENV_MAX_ROUTES: &str = "MAX_ROUTES";
pub const ENV_BANDWIDTH_CAP_PER_ENDPOINT: &str = "BANDWIDTH_CAP_PER_ENDPOINT";
pub const ENV_PROMETHEUS_LISTEN_ADDR: &str = "PROMETHEUS_LISTEN_ADDR";

impl RelayConfig {
    /// Parse + validate the relay config from the process environment
    /// (`design/11 §6.3`). Fails fast with a precise message naming the bad var.
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Parse + validate from an arbitrary `key -> value` lookup. The env path
    /// ([`Self::from_env`]) delegates here; tests drive it with a fixed map so
    /// they never touch process-global `std::env` (no cross-test interference).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let relay_listen_addr = parse_socket_addr(
            ENV_RELAY_LISTEN_ADDR,
            lookup(ENV_RELAY_LISTEN_ADDR),
            DEFAULT_RELAY_LISTEN_ADDR,
        )?;

        // Reserved for Task 215: parse + validate so the env surface is frozen
        // and a malformed value fails fast, but the WSS bridge is built in 215.
        let wss_listen_addr = match lookup(ENV_WSS_LISTEN_ADDR) {
            Some(raw) if !raw.trim().is_empty() => {
                Some(parse_socket_addr_required(ENV_WSS_LISTEN_ADDR, raw.trim())?)
            }
            _ => None,
        };

        // Task 315: the inbound-webhook route (`design/11 §3.4.1`), a sibling of
        // the WSS listener. Additive + opt-in; a malformed value fails fast.
        let webhook_listen_addr = match lookup(ENV_WEBHOOK_LISTEN_ADDR) {
            Some(raw) if !raw.trim().is_empty() => Some(parse_socket_addr_required(
                ENV_WEBHOOK_LISTEN_ADDR,
                raw.trim(),
            )?),
            _ => None,
        };

        let max_routes = match lookup(ENV_MAX_ROUTES) {
            Some(raw) if !raw.trim().is_empty() => {
                let n = usize::from_str(raw.trim()).map_err(|e| {
                    RelayError::Config(format!(
                        "{ENV_MAX_ROUTES}='{raw}' is not a valid unsigned integer: {e}"
                    ))
                })?;
                if n == 0 {
                    return Err(RelayError::Config(format!(
                        "{ENV_MAX_ROUTES}=0 is invalid: the routing table must allow at least one route"
                    )));
                }
                n
            }
            _ => DEFAULT_MAX_ROUTES,
        };

        let bandwidth_cap_per_endpoint = match lookup(ENV_BANDWIDTH_CAP_PER_ENDPOINT) {
            Some(raw) if !raw.trim().is_empty() => {
                let n = u64::from_str(raw.trim()).map_err(|e| {
                    RelayError::Config(format!(
                        "{ENV_BANDWIDTH_CAP_PER_ENDPOINT}='{raw}' is not a valid unsigned integer (bytes): {e}"
                    ))
                })?;
                if n == 0 {
                    return Err(RelayError::Config(format!(
                        "{ENV_BANDWIDTH_CAP_PER_ENDPOINT}=0 is invalid: a zero cap forwards nothing; unset it for unlimited"
                    )));
                }
                Some(n)
            }
            // Unset → unlimited (`design/11 §3.9` default is configurable).
            _ => None,
        };

        let prometheus_listen_addr = parse_socket_addr(
            ENV_PROMETHEUS_LISTEN_ADDR,
            lookup(ENV_PROMETHEUS_LISTEN_ADDR),
            DEFAULT_PROMETHEUS_LISTEN_ADDR,
        )?;

        Ok(Self {
            relay_listen_addr,
            wss_listen_addr,
            webhook_listen_addr,
            max_routes,
            bandwidth_cap_per_endpoint,
            prometheus_listen_addr,
        })
    }
}

/// Parse a socket address from an optional env value, falling back to `default`
/// when unset/blank. Names the var in any error.
fn parse_socket_addr(var: &str, value: Option<String>, default: &str) -> Result<SocketAddr> {
    match value {
        Some(raw) if !raw.trim().is_empty() => parse_socket_addr_required(var, raw.trim()),
        // Unset/blank → the documented default (itself always valid).
        _ => SocketAddr::from_str(default).map_err(|e| {
            RelayError::Config(format!(
                "internal: default {var}='{default}' is invalid: {e}"
            ))
        }),
    }
}

/// Parse a required socket address, naming the var in any error.
fn parse_socket_addr_required(var: &str, raw: &str) -> Result<SocketAddr> {
    SocketAddr::from_str(raw).map_err(|e| {
        RelayError::Config(format!(
            "{var}='{raw}' is not a valid socket address (expected host:port, e.g. 0.0.0.0:80): {e}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{
        DEFAULT_MAX_ROUTES, DEFAULT_PROMETHEUS_LISTEN_ADDR, DEFAULT_RELAY_LISTEN_ADDR,
    };
    use std::collections::HashMap;

    /// Build a lookup from a fixed map — never touches process-global `std::env`.
    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let map: HashMap<&str, &str> = pairs.iter().copied().collect();
        move |k| map.get(k).map(|v| v.to_string())
    }

    #[test]
    fn empty_env_uses_documented_defaults() {
        let c = RelayConfig::from_lookup(lookup(&[])).expect("defaults parse");
        assert_eq!(
            c.relay_listen_addr,
            DEFAULT_RELAY_LISTEN_ADDR.parse().unwrap()
        );
        assert_eq!(
            c.prometheus_listen_addr,
            DEFAULT_PROMETHEUS_LISTEN_ADDR.parse().unwrap()
        );
        assert_eq!(c.max_routes, DEFAULT_MAX_ROUTES);
        assert_eq!(c.bandwidth_cap_per_endpoint, None);
        assert_eq!(c.wss_listen_addr, None, "WSS reserved, unset by default");
        assert_eq!(
            c.webhook_listen_addr, None,
            "webhook route opt-in, unset by default"
        );
    }

    #[test]
    fn all_vars_parse() {
        let c = RelayConfig::from_lookup(lookup(&[
            (ENV_RELAY_LISTEN_ADDR, "127.0.0.1:8080"),
            (ENV_WSS_LISTEN_ADDR, "127.0.0.1:8443"),
            (ENV_WEBHOOK_LISTEN_ADDR, "127.0.0.1:8444"),
            (ENV_MAX_ROUTES, "10000"),
            (ENV_BANDWIDTH_CAP_PER_ENDPOINT, "53687091200"),
            (ENV_PROMETHEUS_LISTEN_ADDR, "127.0.0.1:9091"),
        ]))
        .expect("full config parses");
        assert_eq!(c.relay_listen_addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(c.wss_listen_addr, Some("127.0.0.1:8443".parse().unwrap()));
        assert_eq!(
            c.webhook_listen_addr,
            Some("127.0.0.1:8444".parse().unwrap())
        );
        assert_eq!(c.max_routes, 10_000);
        assert_eq!(c.bandwidth_cap_per_endpoint, Some(53_687_091_200));
        assert_eq!(c.prometheus_listen_addr, "127.0.0.1:9091".parse().unwrap());
    }

    #[test]
    fn malformed_relay_addr_fails_fast_naming_the_var() {
        let err = RelayConfig::from_lookup(lookup(&[(ENV_RELAY_LISTEN_ADDR, "not-an-addr")]))
            .expect_err("malformed addr must fail");
        let msg = err.to_string();
        assert!(
            msg.contains(ENV_RELAY_LISTEN_ADDR),
            "names the bad var: {msg}"
        );
    }

    #[test]
    fn malformed_wss_addr_fails_fast() {
        let err = RelayConfig::from_lookup(lookup(&[(ENV_WSS_LISTEN_ADDR, "1.2.3.4")]))
            .expect_err("port-less WSS addr must fail");
        assert!(err.to_string().contains(ENV_WSS_LISTEN_ADDR));
    }

    #[test]
    fn malformed_webhook_addr_fails_fast() {
        let err = RelayConfig::from_lookup(lookup(&[(ENV_WEBHOOK_LISTEN_ADDR, "1.2.3.4")]))
            .expect_err("port-less webhook addr must fail");
        assert!(err.to_string().contains(ENV_WEBHOOK_LISTEN_ADDR));
    }

    #[test]
    fn zero_max_routes_rejected() {
        let err = RelayConfig::from_lookup(lookup(&[(ENV_MAX_ROUTES, "0")]))
            .expect_err("MAX_ROUTES=0 must fail");
        assert!(err.to_string().contains(ENV_MAX_ROUTES));
    }

    #[test]
    fn non_numeric_max_routes_rejected() {
        let err = RelayConfig::from_lookup(lookup(&[(ENV_MAX_ROUTES, "lots")]))
            .expect_err("non-numeric MAX_ROUTES must fail");
        assert!(err.to_string().contains(ENV_MAX_ROUTES));
    }

    #[test]
    fn zero_bandwidth_cap_rejected() {
        let err = RelayConfig::from_lookup(lookup(&[(ENV_BANDWIDTH_CAP_PER_ENDPOINT, "0")]))
            .expect_err("a zero cap forwards nothing");
        assert!(err.to_string().contains(ENV_BANDWIDTH_CAP_PER_ENDPOINT));
    }

    #[test]
    fn blank_values_fall_back_to_defaults() {
        let c = RelayConfig::from_lookup(lookup(&[
            (ENV_RELAY_LISTEN_ADDR, "   "),
            (ENV_MAX_ROUTES, ""),
            (ENV_WSS_LISTEN_ADDR, ""),
        ]))
        .expect("blank values are treated as unset");
        assert_eq!(
            c.relay_listen_addr,
            DEFAULT_RELAY_LISTEN_ADDR.parse().unwrap()
        );
        assert_eq!(c.max_routes, DEFAULT_MAX_ROUTES);
        assert_eq!(c.wss_listen_addr, None);
    }
}
