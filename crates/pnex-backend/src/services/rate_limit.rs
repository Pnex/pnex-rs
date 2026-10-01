//! Cross-pod rate limiting of unauthenticated / sensitive routes.
//!
//! - **Counters in Valkey** (fixed window, one atomic Lua `INCR` + `PEXPIRE`
//!   per request) so N pods behind a load balancer share one budget per
//!   client; **local in-memory fallback** when Valkey is not configured or
//!   unreachable (dev single-node, Valkey outage — degraded, never open).
//! - **Client identity** = socket peer address. `X-Forwarded-For` /
//!   `X-Real-IP` are only honoured when the peer itself is a trusted proxy
//!   (`PNEX_TRUSTED_PROXIES` / `settings.rate_limit.trusted_proxies`,
//!   default: loopback + private ranges, i.e. the ingress / docker / k8s
//!   network). IPv6 clients are bucketed per /64.
//! - Denials answer **429** with a registered error code and `Retry-After`.
//!
//! Rules are matched on the request path, first match wins (see [`RULES`]).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use loco_rs::config::Config;
use redis::aio::ConnectionManager;

/// One protected route family.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Stable bucket name (part of the Valkey key).
    pub name: &'static str,
    /// Path prefix (or exact path when `exact`).
    pub path: &'static str,
    pub exact: bool,
    /// Requests allowed per client and window.
    pub limit: u32,
    pub window: Duration,
    /// Error code of the 429 body (registered in `pnex_core::err_codes`).
    pub code: &'static str,
    pub message: &'static str,
}

const MINUTE: Duration = Duration::from_secs(60);

/// Protected routes, first match wins. Budgets are per client address:
/// generous enough for a whole site behind one NAT, tight enough to make
/// credential / token guessing pointless.
pub const RULES: &[Rule] = &[
    // Native login bridge: the app polls every ~1-2 s for up to 5 min.
    Rule {
        name: "oauth2-native-poll",
        path: "/api/v1/oauth2/native/",
        exact: false,
        limit: 240,
        window: MINUTE,
        code: pnex_core::err_codes::RATE_LIMITED,
        message: "Too many requests, retry later.",
    },
    // Login / token refresh / SSO redirects (credential stuffing).
    Rule {
        name: "oauth2",
        path: "/api/v1/oauth2/",
        exact: false,
        limit: 60,
        window: MINUTE,
        code: pnex_core::err_codes::RATE_LIMITED,
        message: "Too many requests, retry later.",
    },
    // Edge agent enrollment (code guessing) — historical budget and code.
    Rule {
        name: "agent-enroll",
        path: "/api/v1/agent/enroll",
        exact: true,
        limit: 10,
        window: MINUTE,
        code: pnex_core::err_codes::AGENT_ENROLL_RATE_LIMITED,
        message: "Too many enrollment attempts, retry in a minute.",
    },
    // Device-token authenticated sockets: provisioning (Announce),
    // telemetry ingest, camera uplink (device token guessing).
    Rule {
        name: "ws-device",
        path: "/ws/device",
        exact: true,
        limit: 300,
        window: MINUTE,
        code: pnex_core::err_codes::RATE_LIMITED,
        message: "Too many requests, retry later.",
    },
    Rule {
        name: "ws-ingest",
        path: "/ws/sensor/ingest",
        exact: true,
        limit: 300,
        window: MINUTE,
        code: pnex_core::err_codes::RATE_LIMITED,
        message: "Too many requests, retry later.",
    },
    Rule {
        name: "ws-camera",
        path: "/ws/camera",
        exact: false,
        limit: 300,
        window: MINUTE,
        code: pnex_core::err_codes::RATE_LIMITED,
        message: "Too many requests, retry later.",
    },
    // Public share links (token guessing); assets of a tour count too.
    Rule {
        name: "public-tours",
        path: "/api/v1/public/tours/",
        exact: false,
        limit: 600,
        window: MINUTE,
        code: pnex_core::err_codes::RATE_LIMITED,
        message: "Too many requests, retry later.",
    },
];

/// First rule matching `path`.
pub fn rule_for(path: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| {
        if r.exact {
            path == r.path || path.strip_suffix('/') == Some(r.path)
        } else {
            path.starts_with(r.path)
        }
    })
}

// ───────────────────────────── Trusted proxies ─────────────────────────────

/// CIDR list of reverse proxies allowed to set forwarding headers.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies(Vec<(IpAddr, u8)>);

/// Default trusted proxies: loopback + private networks (ingress, docker
/// bridge, k8s pod network). Override with `PNEX_TRUSTED_PROXIES`.
pub const DEFAULT_TRUSTED_PROXIES: &str =
    "127.0.0.0/8,::1/128,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,fc00::/7";

impl TrustedProxies {
    /// Parses a comma/space separated CIDR list (bare IPs = host routes).
    /// `none` (or an empty list) trusts nobody. Invalid entries are skipped
    /// with a warning.
    pub fn parse(raw: &str) -> Self {
        let mut out = Vec::new();
        for item in raw
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            if item.eq_ignore_ascii_case("none") {
                return Self(Vec::new());
            }
            let (addr, bits) = match item.split_once('/') {
                Some((a, b)) => (a, b.parse::<u8>().ok()),
                None => (item, None),
            };
            let Ok(ip) = addr.parse::<IpAddr>() else {
                tracing::warn!(entry = item, "trusted proxies: invalid entry ignored");
                continue;
            };
            let max = if ip.is_ipv4() { 32 } else { 128 };
            let bits = bits.unwrap_or(max);
            if bits > max {
                tracing::warn!(entry = item, "trusted proxies: invalid prefix ignored");
                continue;
            }
            out.push((ip, bits));
        }
        Self(out)
    }

    /// `PNEX_TRUSTED_PROXIES` > `settings.rate_limit.trusted_proxies`
    /// (string or list) > [`DEFAULT_TRUSTED_PROXIES`].
    pub fn from_config(config: &Config) -> Self {
        if let Ok(raw) = std::env::var("PNEX_TRUSTED_PROXIES") {
            return Self::parse(&raw);
        }
        let from_yaml = config
            .settings
            .as_ref()
            .and_then(|s| s.get("rate_limit"))
            .and_then(|r| r.get("trusted_proxies"))
            .map(|v| match v {
                serde_json::Value::Array(items) => items
                    .iter()
                    .filter_map(|i| i.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                other => other.as_str().unwrap_or_default().to_string(),
            });
        Self::parse(from_yaml.as_deref().unwrap_or(DEFAULT_TRUSTED_PROXIES))
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = canonical(ip);
        self.0.iter().any(|(net, bits)| in_cidr(ip, *net, *bits))
    }
}

/// IPv4-mapped IPv6 (`::ffff:a.b.c.d`) → plain IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

fn in_cidr(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(n)) => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(a) & mask == u32::from(n) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(n)) => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

/// Resolves the client address: the socket peer, unless the peer is a
/// trusted proxy — then the right-most untrusted `X-Forwarded-For` hop
/// (falling back to `X-Real-IP`). `None` = unknown (no connect info).
pub fn client_ip(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &TrustedProxies,
) -> Option<IpAddr> {
    let peer = canonical(peer?);
    if !trusted.contains(peer) {
        return Some(peer);
    }
    let forwarded: Vec<IpAddr> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|s| s.trim().parse::<IpAddr>().ok())
        .map(canonical)
        .collect();
    if let Some(ip) = forwarded.iter().rev().find(|ip| !trusted.contains(**ip)) {
        return Some(*ip);
    }
    if let Some(first) = forwarded.first() {
        // Every hop is internal (request from inside the cluster).
        return Some(*first);
    }
    headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<IpAddr>().ok())
        .map(canonical)
        .or(Some(peer))
}

/// Bucket identity of a client: full IPv4, IPv6 /64 (one subscriber).
pub fn client_key(ip: Option<IpAddr>) -> String {
    match ip {
        Some(IpAddr::V4(v4)) => v4.to_string(),
        Some(IpAddr::V6(v6)) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        None => "unknown".into(),
    }
}

// ───────────────────────────── Limiter ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny { retry_after: Duration },
}

/// Atomic fixed window: increments, arms the expiry on the first hit (or
/// if it was lost), returns `{count, pttl}`.
const WINDOW_SCRIPT: &str = r"
local c = redis.call('INCR', KEYS[1])
local ttl = redis.call('PTTL', KEYS[1])
if c == 1 or ttl < 0 then
  redis.call('PEXPIRE', KEYS[1], ARGV[1])
  ttl = tonumber(ARGV[1])
end
return {c, ttl}
";

/// Bound of one Valkey round-trip on the request path.
const VALKEY_TIMEOUT: Duration = Duration::from_millis(250);
/// Local fallback map bound (attacker-controlled keys).
const LOCAL_MAX: usize = 50_000;

pub struct RateLimiter {
    conn: Option<ConnectionManager>,
    script: redis::Script,
    local: Mutex<HashMap<String, (Instant, u32)>>,
    last_warn: AtomicI64,
}

impl RateLimiter {
    /// `conn = None` → local in-memory counters only (per pod).
    pub fn new(conn: Option<ConnectionManager>) -> Self {
        Self {
            conn,
            script: redis::Script::new(WINDOW_SCRIPT),
            local: Mutex::new(HashMap::new()),
            last_warn: AtomicI64::new(0),
        }
    }

    /// Counts one hit of `bucket` and decides.
    pub async fn hit(&self, bucket: &str, limit: u32, window: Duration) -> Decision {
        if let Some(conn) = &self.conn {
            match self.hit_valkey(conn.clone(), bucket, window).await {
                Ok((count, ttl_ms)) => return decide(count, limit, ttl_ms, window),
                Err(e) => self.warn_fallback(&e),
            }
        }
        self.hit_local(bucket, limit, window)
    }

    async fn hit_valkey(
        &self,
        mut conn: ConnectionManager,
        bucket: &str,
        window: Duration,
    ) -> Result<(u64, i64), String> {
        let key = format!("pnex:rl:{bucket}");
        let mut invocation = self.script.key(key);
        invocation.arg(window.as_millis() as u64);
        let fut = invocation.invoke_async::<(u64, i64)>(&mut conn);
        match tokio::time::timeout(VALKEY_TIMEOUT, fut).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("timeout".into()),
        }
    }

    fn hit_local(&self, bucket: &str, limit: u32, window: Duration) -> Decision {
        let now = Instant::now();
        let mut map = self.local.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= LOCAL_MAX {
            map.retain(|_, (start, _)| now.duration_since(*start) < window);
            if map.len() >= LOCAL_MAX {
                map.clear();
            }
        }
        let entry = map.entry(bucket.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        entry.1 = entry.1.saturating_add(1);
        let left = window.saturating_sub(now.duration_since(entry.0));
        decide(u64::from(entry.1), limit, left.as_millis() as i64, window)
    }

    fn warn_fallback(&self, err: &str) {
        let now = chrono::Utc::now().timestamp();
        let last = self.last_warn.load(Ordering::Relaxed);
        if now - last >= 60
            && self
                .last_warn
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            tracing::warn!(error = %err, "rate limit: valkey unavailable, local counters used");
        }
    }
}

fn decide(count: u64, limit: u32, ttl_ms: i64, window: Duration) -> Decision {
    if count <= u64::from(limit) {
        return Decision::Allow;
    }
    let ms = if ttl_ms > 0 {
        ttl_ms as u64
    } else {
        window.as_millis() as u64
    };
    Decision::Deny {
        retry_after: Duration::from_millis(ms),
    }
}

// ───────────────────────────── Axum layer ─────────────────────────────

pub struct RateLimitState {
    pub enabled: bool,
    pub limiter: RateLimiter,
    pub trusted: TrustedProxies,
}

impl RateLimitState {
    /// Reads the config: `PNEX_RATE_LIMIT=off` (or
    /// `settings.rate_limit.enabled: false`) disables the layer.
    pub async fn from_config(config: &Config) -> Self {
        let yaml_enabled = config
            .settings
            .as_ref()
            .and_then(|s| s.get("rate_limit"))
            .and_then(|r| r.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let enabled = match std::env::var("PNEX_RATE_LIMIT") {
            Ok(v) => !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "off" | "false" | "0"
            ),
            Err(_) => yaml_enabled,
        };
        let conn = crate::services::shared_valkey::conn(config).await;
        if conn.is_none() {
            tracing::info!("rate limit: valkey not configured — per-pod counters");
        }
        Self {
            enabled,
            limiter: RateLimiter::new(conn),
            trusted: TrustedProxies::from_config(config),
        }
    }
}

/// 429 with the rule's error code and `Retry-After` (whole seconds, ≥ 1).
pub fn too_many(rule: &Rule, retry_after: Duration) -> Response {
    let mut resp = loco_rs::Error::CustomError(
        StatusCode::TOO_MANY_REQUESTS,
        loco_rs::controller::ErrorDetail::new(rule.code, rule.message.to_string()),
    )
    .into_response();
    let secs = retry_after.as_millis().div_ceil(1000).max(1);
    if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
        resp.headers_mut()
            .insert(axum::http::header::RETRY_AFTER, v);
    }
    resp
}

/// Axum middleware (`from_fn_with_state`): counts and enforces [`RULES`].
pub async fn middleware(
    State(state): State<Arc<RateLimitState>>,
    req: Request,
    next: Next,
) -> Response {
    if !state.enabled {
        return next.run(req).await;
    }
    let Some(rule) = rule_for(req.uri().path()) else {
        return next.run(req).await;
    };
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    let client = client_key(client_ip(peer, req.headers(), &state.trusted));
    let bucket = format!("{}:{client}", rule.name);
    match state.limiter.hit(&bucket, rule.limit, rule.window).await {
        Decision::Allow => next.run(req).await,
        Decision::Deny { retry_after } => {
            tracing::info!(rule = rule.name, %client, "rate limited");
            too_many(rule, retry_after)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn xff(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_str(v).unwrap());
        h
    }

    #[test]
    fn rules_match_expected_paths() {
        assert_eq!(rule_for("/api/v1/oauth2/token").unwrap().name, "oauth2");
        assert_eq!(
            rule_for("/api/v1/oauth2/native/abc").unwrap().name,
            "oauth2-native-poll"
        );
        assert_eq!(rule_for("/api/v1/oauth2/native").unwrap().name, "oauth2");
        assert_eq!(
            rule_for("/api/v1/agent/enroll").unwrap().name,
            "agent-enroll"
        );
        assert_eq!(rule_for("/ws/camera/live").unwrap().name, "ws-camera");
        assert!(rule_for("/api/v1/agent/enrollments").is_none());
        assert!(rule_for("/api/v1/devices").is_none());
        assert!(rule_for("/ws/notify").is_none());
    }

    #[test]
    fn forwarded_headers_only_trusted_from_proxies() {
        let trusted = TrustedProxies::parse(DEFAULT_TRUSTED_PROXIES);
        // Public peer: headers ignored (spoofing attempt).
        assert_eq!(
            client_ip(Some(ip("203.0.113.9")), &xff("1.2.3.4"), &trusted),
            Some(ip("203.0.113.9"))
        );
        // Trusted ingress: right-most untrusted hop wins (client-injected
        // left entries are ignored).
        assert_eq!(
            client_ip(
                Some(ip("10.0.0.5")),
                &xff("6.6.6.6, 198.51.100.7, 10.0.0.9"),
                &trusted
            ),
            Some(ip("198.51.100.7"))
        );
        // X-Real-IP fallback behind a trusted proxy.
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", HeaderValue::from_static("198.51.100.8"));
        assert_eq!(
            client_ip(Some(ip("127.0.0.1")), &h, &trusted),
            Some(ip("198.51.100.8"))
        );
        // Mapped IPv4 peer is canonicalised.
        assert_eq!(
            client_ip(Some(ip("::ffff:203.0.113.1")), &HeaderMap::new(), &trusted),
            Some(ip("203.0.113.1"))
        );
        // `none` trusts nobody, even loopback.
        let nobody = TrustedProxies::parse("none");
        assert_eq!(
            client_ip(Some(ip("127.0.0.1")), &xff("1.2.3.4"), &nobody),
            Some(ip("127.0.0.1"))
        );
        assert_eq!(client_ip(None, &xff("1.2.3.4"), &trusted), None);
    }

    #[test]
    fn ipv6_clients_bucket_per_64() {
        assert_eq!(
            client_key(Some(ip("2001:db8:1:2:aaaa::1"))),
            client_key(Some(ip("2001:db8:1:2:bbbb::2")))
        );
        assert_eq!(client_key(Some(ip("192.0.2.1"))), "192.0.2.1");
    }

    #[tokio::test]
    async fn local_fallback_enforces_the_window() {
        let rl = RateLimiter::new(None);
        for _ in 0..3 {
            assert_eq!(rl.hit("t:a", 3, MINUTE).await, Decision::Allow);
        }
        assert!(matches!(
            rl.hit("t:a", 3, MINUTE).await,
            Decision::Deny { retry_after } if retry_after <= MINUTE && retry_after > Duration::ZERO
        ));
        // Other client unaffected.
        assert_eq!(rl.hit("t:b", 3, MINUTE).await, Decision::Allow);
        // Window expiry resets the budget.
        let short = Duration::from_millis(30);
        assert_eq!(rl.hit("t:c", 1, short).await, Decision::Allow);
        assert!(matches!(
            rl.hit("t:c", 1, short).await,
            Decision::Deny { .. }
        ));
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(rl.hit("t:c", 1, short).await, Decision::Allow);
    }
}
