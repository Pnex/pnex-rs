//! Outbound requests to user-chosen hosts (R8, SEC-W3 / SEC-14): the
//! http-fetch node, notification webhooks and LLM providers resolve their
//! host through [`GuardedResolver`] and check IP literals and redirects
//! with [`check_url`].
//!
//! Policy (`PNEX_EGRESS`, read once per process):
//! - `lan` (default, self-hosted): loopback, link-local (cloud metadata
//!   169.254.169.254 included), unspecified, multicast and broadcast
//!   addresses are refused, as are single-label host names (Docker service
//!   names: `postgres`, `valkey`…). Private ranges stay reachable: a LAN
//!   device or a local LLM (Ollama) is a central self-hosted use.
//! - `public` (shared / SaaS hosting): private ranges (RFC 1918, CGNAT,
//!   IPv6 ULA) are refused too.
//! - `open`: no filtering (development only).
//!
//! `PNEX_EGRESS_ALLOW_HOSTS` (comma separated) lifts the name rule for
//! chosen internal names (`ollama`, `homeassistant`); the addresses they
//! resolve to are still checked.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;

/// Egress policy of the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressPolicy {
    Lan,
    Public,
    Open,
}

impl EgressPolicy {
    /// Parses the `PNEX_EGRESS` value; unknown values fall back to `Lan`.
    pub fn parse(v: &str) -> Self {
        match v.trim().to_ascii_lowercase().as_str() {
            "public" => Self::Public,
            "open" => Self::Open,
            _ => Self::Lan,
        }
    }

    /// Wire value (`PNEX_EGRESS`), passed on to the flow runtime.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lan => "lan",
            Self::Public => "public",
            Self::Open => "open",
        }
    }
}

static POLICY: OnceLock<EgressPolicy> = OnceLock::new();

/// Sets the policy of this process (backend: from its configuration). The
/// first value wins; without a call, `PNEX_EGRESS` is read.
pub fn init(policy: EgressPolicy) {
    let _ = POLICY.set(policy);
}

static ALLOW_HOSTS: OnceLock<Vec<String>> = OnceLock::new();

fn parse_hosts(v: &str) -> Vec<String> {
    v.split(',')
        .map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect()
}

fn allowed_hosts() -> &'static [String] {
    ALLOW_HOSTS
        .get_or_init(|| parse_hosts(&std::env::var("PNEX_EGRESS_ALLOW_HOSTS").unwrap_or_default()))
}

/// Policy of this process.
pub fn policy() -> EgressPolicy {
    *POLICY.get_or_init(|| EgressPolicy::parse(&std::env::var("PNEX_EGRESS").unwrap_or_default()))
}

/// Whether an outbound connection to `ip` is allowed under `policy`.
pub fn ip_allowed(ip: IpAddr, policy: EgressPolicy) -> bool {
    if policy == EgressPolicy::Open {
        return true;
    }
    let ip = match ip {
        // ::ffff:a.b.c.d is judged as a.b.c.d.
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => v4_allowed(v4, policy),
        IpAddr::V6(v6) => v6_allowed(v6, policy),
    }
}

fn v4_allowed(ip: Ipv4Addr, policy: EgressPolicy) -> bool {
    if ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.octets()[0] == 0
    {
        return false;
    }
    let o = ip.octets();
    let private = ip.is_private() || (o[0] == 100 && (o[1] & 0xc0) == 64);
    !(policy == EgressPolicy::Public && private)
}

fn v6_allowed(ip: Ipv6Addr, policy: EgressPolicy) -> bool {
    let seg0 = ip.segments()[0];
    let link_local = (seg0 & 0xffc0) == 0xfe80;
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || link_local {
        return false;
    }
    let unique_local = (seg0 & 0xfe00) == 0xfc00;
    !(policy == EgressPolicy::Public && unique_local)
}

/// Whether a host name may be resolved at all: single-label names
/// (internal service names) and `localhost` are refused unless `Open`.
pub fn host_allowed(host: &str, policy: EgressPolicy) -> bool {
    if policy == EgressPolicy::Open {
        return true;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.parse::<IpAddr>().is_ok() || allowed_hosts().contains(&host) {
        return true;
    }
    !host.is_empty()
        && host.contains('.')
        && host != "localhost"
        && !host.ends_with(".localhost")
        && !host.ends_with(".internal")
}

/// Refusal reason of a URL (`None` = allowed): its host name, and its IP
/// when the host is a literal (the resolver is not called for those).
pub fn check_host(host: &str, policy: EgressPolicy) -> Option<&'static str> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return (!ip_allowed(ip, policy)).then_some("egress_address_refused");
    }
    (!host_allowed(bare, policy)).then_some("egress_host_refused")
}

/// Keeps the allowed addresses of a resolution.
pub fn filter_addrs(
    addrs: impl IntoIterator<Item = SocketAddr>,
    policy: EgressPolicy,
) -> Vec<SocketAddr> {
    addrs
        .into_iter()
        .filter(|a| ip_allowed(a.ip(), policy))
        .collect()
}

#[cfg(feature = "egress")]
mod client {
    use super::*;

    /// reqwest resolver applying the process policy: a name whose every
    /// address is refused fails the request before any connection.
    #[derive(Debug, Clone, Copy, Default)]
    pub struct GuardedResolver;

    impl reqwest::dns::Resolve for GuardedResolver {
        fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            let host = name.as_str().to_string();
            Box::pin(async move {
                let policy = policy();
                if !host_allowed(&host, policy) {
                    return Err(refused(format!("egress refused for host {host}")));
                }
                let found = tokio::net::lookup_host((host.as_str(), 0)).await?;
                let allowed = filter_addrs(found, policy);
                if allowed.is_empty() {
                    return Err(refused(format!(
                        "egress refused: {host} resolves to internal addresses only"
                    )));
                }
                Ok(Box::new(allowed.into_iter()) as reqwest::dns::Addrs)
            })
        }
    }

    fn refused(msg: String) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            msg,
        ))
    }

    /// Refusal reason of `url` under the process policy (`None` = allowed).
    pub fn check_url(url: &reqwest::Url) -> Option<&'static str> {
        match url.host_str() {
            Some(host) => check_host(host, policy()),
            None => Some("egress_host_refused"),
        }
    }

    /// Redirect policy: at most `max` hops, each re-checked (R8).
    pub fn redirect_policy(max: usize) -> reqwest::redirect::Policy {
        reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= max {
                attempt.error("too many redirects")
            } else if let Some(code) = check_url(attempt.url()) {
                attempt.error(code)
            } else {
                attempt.follow()
            }
        })
    }

    /// Client builder with the guarded resolver installed.
    pub fn guarded(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        builder.dns_resolver(std::sync::Arc::new(GuardedResolver))
    }
}

#[cfg(feature = "egress")]
pub use client::{check_url, guarded, redirect_policy, GuardedResolver};

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn lan_refuses_loopback_link_local_and_metadata() {
        let p = EgressPolicy::Lan;
        for bad in [
            "127.0.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!ip_allowed(ip(bad), p), "{bad} must be refused");
        }
        for ok in ["192.168.1.20", "10.0.0.5", "fd00::1", "93.184.216.34"] {
            assert!(ip_allowed(ip(ok), p), "{ok} must stay reachable on a LAN");
        }
    }

    #[test]
    fn public_refuses_private_ranges_too() {
        let p = EgressPolicy::Public;
        for bad in [
            "192.168.1.20",
            "10.0.0.5",
            "172.16.3.4",
            "100.64.0.1",
            "fd00::1",
        ] {
            assert!(!ip_allowed(ip(bad), p), "{bad} must be refused");
        }
        assert!(ip_allowed(ip("93.184.216.34"), p));
        assert!(ip_allowed(ip("2606:4700::1111"), p));
    }

    #[test]
    fn open_allows_everything() {
        assert!(ip_allowed(ip("127.0.0.1"), EgressPolicy::Open));
        assert!(host_allowed("valkey", EgressPolicy::Open));
    }

    #[test]
    fn internal_service_names_are_refused() {
        let p = EgressPolicy::Lan;
        for bad in [
            "valkey",
            "postgres",
            "localhost",
            "api.localhost",
            "metadata.google.internal",
        ] {
            assert!(!host_allowed(bad, p), "{bad} must be refused");
        }
        for ok in ["api.open-meteo.com", "shelly.local", "192.168.1.20"] {
            assert!(host_allowed(ok, p), "{ok} must be allowed");
        }
    }

    #[test]
    fn host_list_parsing() {
        assert_eq!(
            parse_hosts(" Ollama, ,homeassistant. "),
            vec!["ollama", "homeassistant"]
        );
    }

    #[test]
    fn check_host_covers_literals_and_names() {
        let p = EgressPolicy::Lan;
        assert_eq!(check_host("127.0.0.1", p), Some("egress_address_refused"));
        assert_eq!(check_host("[::1]", p), Some("egress_address_refused"));
        assert_eq!(check_host("valkey", p), Some("egress_host_refused"));
        assert_eq!(check_host("example.com", p), None);
        assert_eq!(EgressPolicy::parse("PUBLIC"), EgressPolicy::Public);
        assert_eq!(EgressPolicy::parse("whatever"), EgressPolicy::Lan);
    }
}
