//! MetaTrader 5 terminals as registered data sources (markets-research.md
//! appendix E, D847–D849): the shared types of the API, the choice of the
//! symbols a terminal pushes, its presence, the reading of its catalogue,
//! and the Expert Advisor generated for it. Pure code: no I/O, no clock.
//! A terminal never places an order (D534, D123).

use serde::{Deserialize, Serialize};

/// Version of the generated Expert Advisor.
pub const EA_VERSION: &str = "2.00";
/// Prefix of a terminal token, to tell it apart in a log or a leak scan.
pub const TOKEN_PREFIX: &str = "pnex_mt5_";
/// Seconds between two readings of its configuration by the EA.
pub const POLL_SECONDS: u32 = 300;
/// Bar symbols sent to one terminal at most (a broker lists a few thousand).
pub const BAR_SYMBOLS_MAX: usize = 5000;
/// Tick symbols at most: ticks are heavy (Q-MT4).
pub const TICK_SYMBOLS_MAX: usize = 50;
/// Symbols of one announce call at most.
pub const ANNOUNCE_SYMBOLS_MAX: usize = 20_000;
/// Calls of an org's terminals waiting in the inbox at most (1 MiB each at
/// most; the markets sweep empties it every five minutes): anyone who signs
/// up owns an org, the inbox is the platform's database (SEC-84).
pub const INBOX_MAX: i64 = 2_000;
/// Terminals of one org at most.
pub const TERMINALS_MAX: i64 = 20;
/// Longest pattern list of a selection.
pub const PATTERNS_MAX_LEN: usize = 2000;
pub const NAME_MAX: usize = 120;
pub const BACKFILL_DAYS_MAX: u32 = 3650;

/// What the EA's heartbeat says of the terminal (D108-like presence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    /// No call yet: the EA was never started.
    Never,
    /// A call within the last three minutes.
    Online,
    /// The last call is older than three minutes, younger than an hour.
    Late,
    Offline,
}

/// Presence from the age in seconds of the last call.
pub fn presence(age_seconds: Option<i64>) -> Presence {
    match age_seconds {
        None => Presence::Never,
        Some(s) if s <= 180 => Presence::Online,
        Some(s) if s <= 3600 => Presence::Late,
        Some(_) => Presence::Offline,
    }
}

/// A terminal as read: never its token (R4, R16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Mt5Terminal {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub id: String,
    pub name: String,
    /// Broker, server and account number as typed (not secrets).
    pub broker: String,
    pub server: String,
    pub account: String,
    /// Symbols whose 1-minute bars are pushed: `*` for the whole catalogue,
    /// else comma-separated names or prefixes (`XAU*`).
    pub bars: String,
    /// Symbols whose ticks are pushed, same form; empty: none.
    pub ticks: String,
    /// Whether an EA was generated (a token exists).
    pub has_token: bool,
    pub presence: Presence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub last_seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ea_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_build: Option<i64>,
    /// Symbols the terminal announced.
    pub symbols: i64,
    /// Last backfill asked: its days and when.
    pub backfill_days: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub backfill_requested_at: Option<String>,
    pub version: i32,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub created_at: String,
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub updated_at: String,
}

/// A terminal as written. `bars` absent: the whole catalogue (Q-MT3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Mt5TerminalInput {
    pub name: String,
    #[serde(default)]
    pub broker: String,
    #[serde(default)]
    pub server: String,
    #[serde(default)]
    pub account: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks: Option<String>,
    /// On an update, the version read (optimistic concurrency).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i32>,
}

/// Body of a backfill request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Mt5Backfill {
    /// Days of 1-minute bars sent again, 1 to 3650.
    pub days: u32,
}

/// One symbol of a terminal's catalogue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Mt5Symbol {
    pub symbol: String,
    pub description: String,
    pub path: String,
    pub base: String,
    pub profit: String,
    pub digits: i32,
    pub contract_size: f64,
    /// First 1-minute bar the broker keeps (UTC), when announced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "openapi", schema(format = "date-time"))]
    pub first_m1: Option<String>,
    /// Whether the selection sends its bars, its ticks.
    pub bars: bool,
    pub ticks: bool,
}

/// What the EA reads: the symbols to push and the backfill to do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Mt5Config {
    pub bars: Vec<String>,
    pub ticks: Vec<String>,
    pub poll_seconds: u32,
    /// Changes each time a backfill is asked; 0: none.
    pub backfill_id: i64,
    /// UTC seconds the backfill starts from; 0: none.
    pub backfill_from: i64,
}

/// Whether `symbol` matches a selection: `*`, names or prefixes (`XAU*`),
/// comma separated, case-insensitive.
pub fn selected(patterns: &str, symbol: &str) -> bool {
    patterns.split(',').map(str::trim).any(|p| {
        if p.is_empty() {
            return false;
        }
        match p.strip_suffix('*') {
            Some(prefix) => symbol
                .to_ascii_uppercase()
                .starts_with(&prefix.to_ascii_uppercase()),
            None => p.eq_ignore_ascii_case(symbol),
        }
    })
}

/// A selection as stored: trimmed patterns of safe characters, or a token.
pub fn clean_patterns(raw: &str) -> Result<String, &'static str> {
    let raw = raw.trim();
    if raw.len() > PATTERNS_MAX_LEN {
        return Err("too_long");
    }
    let parts: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    for p in &parts {
        let name = p.strip_suffix('*').unwrap_or(p);
        if !(name.is_empty() && *p == "*") && !crate::markets::history::mt5_symbol_ok(name) {
            return Err("invalid");
        }
    }
    Ok(parts.join(", "))
}

/// The configuration of a terminal from its catalogue and its selection.
pub fn config(
    catalogue: &[String],
    bars: &str,
    ticks: &str,
    backfill: Option<(i64, i64)>,
) -> Mt5Config {
    let pick = |patterns: &str, max: usize| -> Vec<String> {
        catalogue
            .iter()
            .filter(|s| selected(patterns, s))
            .take(max)
            .cloned()
            .collect()
    };
    let (backfill_id, backfill_from) = backfill.unwrap_or((0, 0));
    Mt5Config {
        bars: pick(bars, BAR_SYMBOLS_MAX),
        ticks: pick(ticks, TICK_SYMBOLS_MAX),
        poll_seconds: POLL_SECONDS,
        backfill_id,
        backfill_from,
    }
}

/// One symbol of an announce call, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Announced {
    pub symbol: String,
    pub description: String,
    pub path: String,
    pub base: String,
    pub profit: String,
    pub digits: i32,
    pub contract_size: f64,
    /// UTC seconds of the first 1-minute bar; `None` when unknown.
    pub first_m1: Option<i64>,
}

/// Reads an announce call: `{"v": 1, "symbols": [{"symbol", "description",
/// "path", "base", "profit", "digits", "contract_size", "first_m1"}]}`. A
/// symbol of unsafe characters is left out; the error is a machine token.
pub fn announced(body: &serde_json::Value) -> Result<Vec<Announced>, &'static str> {
    use serde_json::Value;
    if body.get("v").and_then(Value::as_i64) != Some(1) {
        return Err("version");
    }
    let list = body
        .get("symbols")
        .and_then(Value::as_array)
        .ok_or("shape")?;
    if list.len() > ANNOUNCE_SYMBOLS_MAX {
        return Err("too_many");
    }
    let text = |r: &Value, k: &str, max: usize| -> String {
        r.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(max)
            .collect()
    };
    Ok(list
        .iter()
        .filter_map(|r| {
            let symbol = r.get("symbol").and_then(Value::as_str)?;
            if !crate::markets::history::mt5_symbol_ok(symbol) {
                return None;
            }
            Some(Announced {
                symbol: symbol.to_string(),
                description: text(r, "description", 200),
                path: text(r, "path", 200),
                base: text(r, "base", 16),
                profit: text(r, "profit", 16),
                digits: r
                    .get("digits")
                    .and_then(Value::as_i64)
                    .filter(|d| (0..=12).contains(d))
                    .unwrap_or(0) as i32,
                contract_size: r
                    .get("contract_size")
                    .and_then(Value::as_f64)
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .unwrap_or(0.0),
                first_m1: r.get("first_m1").and_then(Value::as_i64).filter(|t| *t > 0),
            })
        })
        .collect())
}

/// Where the EA calls: an absolute `http(s)` origin, without its path.
pub fn base_ok(base: &str) -> bool {
    let b = base.trim();
    (b.starts_with("https://") || b.starts_with("http://"))
        && !b.contains(['"', '\\', '\n', '\r', ' '])
        && b.len() <= 200
}

/// The Expert Advisor of one terminal: its PneX address and its token
/// written in. The token is shown only here, at the time it is made.
pub fn render_ea(base: &str, token: &str, terminal: &str) -> Option<String> {
    if !base_ok(base)
        || !token.starts_with(TOKEN_PREFIX)
        || !token[TOKEN_PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    // The name lands in a comment: no line break, no comment end.
    let name: String = terminal
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_MAX)
        .collect::<String>()
        .replace("*/", "* /");
    Some(
        include_str!("mt5_ea.mq5")
            .replace("{{BASE}}", base.trim().trim_end_matches('/'))
            .replace("{{TOKEN}}", token)
            .replace("{{TERMINAL}}", &name)
            .replace("{{EA_VERSION}}", EA_VERSION),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_selection_takes_names_prefixes_or_all() {
        assert!(selected("*", "EURUSD"));
        assert!(selected("xau*, EURUSD", "XAUUSD"));
        assert!(selected("XAU*", "XAUEUR.m"));
        assert!(!selected("XAU*", "EURUSD"));
        assert!(!selected("", "EURUSD"), "empty: none");
        assert_eq!(clean_patterns(" XAU* ,, eurusd ").unwrap(), "XAU*, eurusd");
        assert_eq!(clean_patterns("*").unwrap(), "*");
        assert!(clean_patterns("EUR USD").is_err());
        assert!(clean_patterns("\"x\"").is_err());
    }

    #[test]
    fn the_config_resolves_the_catalogue() {
        let cat: Vec<String> = ["XAUUSD", "EURUSD", "US500.cash"]
            .map(String::from)
            .to_vec();
        let c = config(&cat, "*", "XAUUSD", Some((7, 1_700_000_000)));
        assert_eq!(c.bars.len(), 3, "the whole catalogue by default");
        assert_eq!(c.ticks, ["XAUUSD"]);
        assert_eq!((c.backfill_id, c.backfill_from), (7, 1_700_000_000));
        assert!(
            config(&[], "*", "", None).bars.is_empty(),
            "nothing announced yet"
        );
    }

    #[test]
    fn presence_follows_the_last_call() {
        assert_eq!(presence(None), Presence::Never);
        assert_eq!(presence(Some(30)), Presence::Online);
        assert_eq!(presence(Some(600)), Presence::Late);
        assert_eq!(presence(Some(7200)), Presence::Offline);
    }

    #[test]
    fn an_announce_keeps_safe_symbols() {
        let body = json!({"v": 1, "symbols": [
            {"symbol": "XAUUSD", "description": "Gold vs US Dollar", "path": "Metals\\XAUUSD",
             "base": "XAU", "profit": "USD", "digits": 2, "contract_size": 100.0, "first_m1": 1104537600},
            {"symbol": "BAD SYMBOL"},
            {"symbol": "EURUSD", "digits": 99, "first_m1": 0}
        ]});
        let a = announced(&body).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].first_m1, Some(1104537600));
        assert_eq!(a[1].digits, 0, "an absurd digit count is dropped");
        assert_eq!(a[1].first_m1, None);
        assert_eq!(announced(&json!({"v": 2})), Err("version"));
    }

    #[test]
    fn the_ea_carries_its_address_and_token_only() {
        let token = format!("{TOKEN_PREFIX}{}", "ab".repeat(32));
        let ea = render_ea("https://pnex.example/", &token, "Demo */ gold\n").unwrap();
        assert!(ea.contains("#define PNEX_BASE    \"https://pnex.example\""));
        assert!(ea.contains(&format!("#define PNEX_TOKEN   \"{token}\"")));
        assert!(ea.contains("\"Demo * / gold\""), "{}", &ea[..400]);
        assert!(!ea.contains("{{"), "every placeholder filled");
        assert!(!ea.contains("OrderSend"), "never an order");
        assert!(render_ea("javascript:x", &token, "t").is_none());
        assert!(render_ea("https://a\"b", &token, "t").is_none());
        assert!(render_ea("https://a", "pnex_mt5_\"", "t").is_none());
    }
}
