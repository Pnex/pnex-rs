//! PNeX browser agent (extension-collector.md D254–D269), compiled to wasm
//! and loaded by the extension's service worker and options page.
//!
//! - options page: [`login`] (PKCE through the server's native sign-in
//!   bridge), [`pair`] (signed by the install key, D266), [`unpair`];
//! - service worker: [`start`] keeps one agent loop alive: signed ticket,
//!   `/ws/extension` Noise link, missions run one platform tab at a time
//!   (D255) at a human pace (D258).
//!
//! The planning rules ([`due_missions`], [`next_run`]) are plain Rust,
//! tested natively; the browser parts only build for wasm32.

use std::collections::{BTreeMap, BTreeSet};

use pnex_core::browser_agent::{self as ba, AssignedMission};
use serde::{Deserialize, Serialize};

/// Shortest gap between two navigations of one platform tab (D258).
pub const NAV_GAP_MS: f64 = 20_000.0;
/// Spread of a mission interval (D258 jitter): ±15 %.
pub const JITTER: f64 = 0.15;
/// The agent pings well under the server watchdog (90 s).
pub const PING_MS: f64 = 20_000.0;
/// Storage key of the agent credentials.
pub const CREDS_KEY: &str = "pnex.creds";

/// What pairing left in the extension storage (D263).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Creds {
    pub server: String,
    pub org_id: i64,
    /// Name of the org at pairing time, shown in the options page.
    #[serde(default)]
    pub org_name: String,
    pub agent_id: i64,
    pub device_id: String,
    pub name: String,
    pub token: String,
    pub encryption_key: String,
}

/// Form/query encoding (RFC 3986 unreserved kept).
pub fn urlencode(input: &str) -> String {
    input
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Server base without trailing slash; `None` unless http(s).
pub fn normalize_server(raw: &str) -> Option<String> {
    let s = raw.trim().trim_end_matches('/');
    (s.starts_with("https://") || s.starts_with("http://")).then(|| s.to_string())
}

/// Redirect target of the sign-in: the server's native bridge.
pub fn native_redirect(server: &str) -> String {
    format!("{server}/api/v1/oauth2/native")
}

/// Sign-in page (Rauthy through the server, PKCE S256).
pub fn sso_url(server: &str, challenge: &str, state: &str) -> String {
    format!(
        "{server}/api/v1/oauth2/sso?code_challenge={}&code_challenge_method=S256&redirect_uri={}&state={}",
        urlencode(challenge),
        urlencode(&native_redirect(server)),
        urlencode(state)
    )
}

/// `wss://…/ws/extension?ticket=` from the server base.
pub fn ws_url(server: &str, ticket: &str) -> String {
    let base = server
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| server.strip_prefix("http://").map(|r| format!("ws://{r}")))
        .unwrap_or_else(|| server.to_string());
    format!("{base}/ws/extension?ticket={}", urlencode(ticket))
}

/// Next run of a mission: its interval (never under the compiled floor)
/// spread by `spread` in [-1, 1].
pub fn next_run(now: f64, interval_secs: i32, spread: f64) -> f64 {
    let secs = f64::from(interval_secs.max(ba::MIN_INTERVAL_SECS));
    now + secs * 1000.0 * (1.0 + JITTER * spread.clamp(-1.0, 1.0))
}

/// Missions to start now: due, one per platform, on a platform neither busy
/// nor navigated within [`NAV_GAP_MS`] (D255, D258). A mission never run is
/// due at once.
pub fn due_missions<'a>(
    missions: &'a [AssignedMission],
    next: &BTreeMap<String, f64>,
    busy: &BTreeSet<String>,
    last_nav: &BTreeMap<String, f64>,
    now: f64,
) -> Vec<&'a AssignedMission> {
    let mut taken = BTreeSet::new();
    missions
        .iter()
        .filter(|m| next.get(&m.id).is_none_or(|t| *t <= now))
        .filter(|m| {
            let platform = ba::platform_of(&m.target).to_string();
            !busy.contains(&platform)
                && last_nav
                    .get(&platform)
                    .is_none_or(|t| now - t >= NAV_GAP_MS)
                && taken.insert(platform)
        })
        .collect()
}

pub mod adapters;

#[cfg(target_arch = "wasm32")]
mod host;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(test)]
mod tests {
    use super::*;
    use ba::{CollectValueParams, MissionSpec};

    fn mission(id: &str, url: &str) -> AssignedMission {
        let spec = MissionSpec::CollectValue(CollectValueParams {
            url: url.into(),
            selector: ".v".into(),
            attr: "text".into(),
            metric: "v".into(),
        });
        let (spec, target) = ba::check_spec(&spec).unwrap();
        AssignedMission {
            id: id.into(),
            spec,
            target,
            interval_secs: 60,
            scroll_depth: 0,
            foreground: false,
        }
    }

    #[test]
    fn one_mission_per_platform_and_human_pace() {
        let ms = [
            mission("a", "https://x.org/1"),
            mission("b", "https://x.org/2"),
            mission("c", "https://y.org/1"),
        ];
        let none = BTreeMap::new();
        let due = due_missions(&ms, &none, &BTreeSet::new(), &none, 1_000_000.0);
        let ids: Vec<&str> = due.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"], "never two tabs on x.org");

        let busy = BTreeSet::from(["web:x.org".to_string()]);
        let due = due_missions(&ms, &none, &busy, &none, 1_000_000.0);
        assert_eq!(due.len(), 1);

        let nav = BTreeMap::from([("web:y.org".to_string(), 1_000_000.0 - 5_000.0)]);
        let due = due_missions(&ms, &none, &BTreeSet::new(), &nav, 1_000_000.0);
        assert_eq!(due.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn interval_floor_and_jitter() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(
            close(next_run(0.0, 10, 0.0), 60_000.0),
            "floor compiled in the agent"
        );
        assert!(close(next_run(0.0, 100, 1.0), 115_000.0));
        assert!(close(next_run(0.0, 100, -5.0), 85_000.0));
    }

    #[test]
    fn locales_have_the_same_keys() {
        let keys = |lang: &str| -> BTreeSet<String> {
            let path = format!(
                "{}/extension/_locales/{lang}/messages.json",
                env!("CARGO_MANIFEST_DIR")
            );
            let json: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            json.as_object().unwrap().keys().cloned().collect()
        };
        assert_eq!(keys("en"), keys("fr"));
    }

    #[test]
    fn urls() {
        assert_eq!(
            normalize_server(" https://p.io/ ").as_deref(),
            Some("https://p.io")
        );
        assert!(normalize_server("ftp://p.io").is_none());
        assert_eq!(
            ws_url("https://p.io", "a+b"),
            "wss://p.io/ws/extension?ticket=a%2Bb"
        );
        assert_eq!(
            ws_url("http://localhost:5150", "t"),
            "ws://localhost:5150/ws/extension?ticket=t"
        );
        assert!(sso_url("https://p.io", "c", "s")
            .contains("redirect_uri=https%3A%2F%2Fp.io%2Fapi%2Fv1%2Foauth2%2Fnative"));
    }

    /// Every followed platform gets the response hook: without it the
    /// agent reads no post there (TikTok shipped without it).
    #[test]
    fn hook_runs_on_every_followed_platform() {
        for manifest in [
            include_str!("../extension/manifest.json"),
            include_str!("../extension/manifest.firefox.json"),
        ] {
            let m: serde_json::Value = serde_json::from_str(manifest).unwrap();
            for platform in [
                ba::SocialPlatform::X,
                ba::SocialPlatform::YouTube,
                ba::SocialPlatform::TikTok,
            ] {
                let page = &platform.list_urls("a")[0];
                let host = page.split('/').nth(2).unwrap();
                for script in m["content_scripts"].as_array().unwrap() {
                    let matches = script["matches"].to_string();
                    assert!(
                        matches.contains(&format!("https://{host}/*")),
                        "{host}: {script}"
                    );
                }
                assert!(
                    include_str!("../extension/hook.js").contains(&format!("\"{host}\"")),
                    "{host} missing from hook.js"
                );
            }
        }
    }
}
