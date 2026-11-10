//! Prediction markets (markets-research.md appendix J, D918–D925): the
//! platform's public answers decoded, a resolution as dated states, the
//! model features at an instant, the calibration against the market and the
//! early bet detector. Pure code: no I/O.
//!
//! The answer shapes follow the platform's public APIs as known on
//! 2026-11-10; they could not be checked from France (the hosts are blocked
//! there, an org reaches them through its own proxy). Every decoder is
//! tolerant: an unknown or malformed field is skipped, never a panic.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Hosts fixed in the code (D822, R9): an org names no host.
pub const GAMMA_BASE: &str = "https://gamma-api.polymarket.com";
pub const CLOB_BASE: &str = "https://clob.polymarket.com";
pub const DATA_BASE: &str = "https://data-api.polymarket.com";
pub const LICENSE_NOTE: &str = "Public read-only APIs of the platform; PNeX never bets, \
    signs or holds a wallet key (J.1). Terms of use to check before a SaaS use; private to the org";

/// Minutes between two points of the default series and of a followed one.
pub const FIDELITY_HOURLY: u32 = 60;
pub const FIDELITY_FINE: u32 = 1;

/// A condition id: `0x` and 64 hex digits.
pub fn condition_ok(s: &str) -> bool {
    s.len() == 66 && s.starts_with("0x") && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A market slug: lowercase words joined by `-`, 200 characters at most.
pub fn slug_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A wallet address: `0x` and 40 hex digits, kept lowercase.
pub fn wallet_ok(s: &str) -> bool {
    s.len() == 42 && s.starts_with("0x") && s[2..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// The states of a resolution (D920).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionState {
    Proposed,
    Disputed,
    Final,
    Indeterminate,
}

impl ResolutionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Disputed => "disputed",
            Self::Final => "final",
            Self::Indeterminate => "indeterminate",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "proposed" => Self::Proposed,
            "disputed" => Self::Disputed,
            "final" => Self::Final,
            "indeterminate" => Self::Indeterminate,
            _ => return None,
        })
    }
}

/// A market as the metadata API describes it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Market {
    pub condition_id: String,
    pub slug: String,
    pub question: String,
    /// Resolution rules, full text (D918).
    pub rules: String,
    pub resolution_source: String,
    pub end_at: Option<i64>,
    pub outcomes: Vec<String>,
    /// One order book token per outcome, same order.
    pub tokens: Vec<String>,
    pub volume: Option<f64>,
    pub closed: bool,
    /// The resolution state the platform shows now, if any.
    pub state: Option<ResolutionState>,
    /// The outcome priced 1 once resolved.
    pub winner: Option<String>,
}

/// A list field the API sends either as an array or as JSON text.
fn list(v: Option<&Value>) -> Vec<String> {
    let items = match v {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) => serde_json::from_str::<Vec<Value>>(s).unwrap_or_default(),
        _ => Vec::new(),
    };
    items
        .iter()
        .filter_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .collect()
}

fn text(m: &Value, k: &str) -> String {
    m.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
    .filter(|x: &f64| x.is_finite())
}

/// An RFC 3339 or `YYYY-MM-DD` instant as Unix seconds.
pub fn instant(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.timestamp())
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc().timestamp())
        })
}

/// The state shown by the platform: the oracle's status first, else
/// closed with a winner = final.
pub fn state_of(uma_status: &str, closed: bool, winner: bool) -> Option<ResolutionState> {
    match uma_status.to_ascii_lowercase().as_str() {
        "proposed" => Some(ResolutionState::Proposed),
        "disputed" | "challenged" => Some(ResolutionState::Disputed),
        "resolved" | "settled" if winner => Some(ResolutionState::Final),
        "resolved" | "settled" => Some(ResolutionState::Indeterminate),
        _ if closed && winner => Some(ResolutionState::Final),
        _ => None,
    }
}

/// Markets of a metadata answer (an array, or `{ "data": [...] }`).
pub fn markets(answer: &Value) -> Vec<Market> {
    let rows = answer
        .as_array()
        .or_else(|| answer.get("data").and_then(Value::as_array));
    rows.into_iter()
        .flatten()
        .filter_map(|m| {
            let condition_id = text(m, "conditionId").to_ascii_lowercase();
            if !condition_ok(&condition_id) {
                return None;
            }
            let outcomes = list(m.get("outcomes"));
            let prices: Vec<f64> = list(m.get("outcomePrices"))
                .iter()
                .filter_map(|p| p.parse().ok())
                .collect();
            let closed = m.get("closed").and_then(Value::as_bool).unwrap_or(false);
            // A price of exactly 1 on a closed market names the winner.
            let winner = match closed {
                true => prices
                    .iter()
                    .position(|p| (*p - 1.0).abs() < 1e-9)
                    .and_then(|i| outcomes.get(i).cloned()),
                false => None,
            };
            Some(Market {
                slug: text(m, "slug"),
                question: text(m, "question"),
                rules: text(m, "description"),
                resolution_source: text(m, "resolutionSource"),
                end_at: instant(&text(m, "endDate")),
                tokens: list(m.get("clobTokenIds")),
                volume: number(m.get("volumeNum")).or_else(|| number(m.get("volume"))),
                state: state_of(&text(m, "umaResolutionStatus"), closed, winner.is_some()),
                outcomes,
                closed,
                winner,
                condition_id,
            })
        })
        .collect()
}

/// Points `(unix seconds, price)` of a price history answer, sorted, the
/// price kept in [0, 1].
pub fn price_history(answer: &Value) -> Vec<(i64, f64)> {
    let mut out: Vec<(i64, f64)> = answer
        .get("history")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let t = p.get("t")?.as_i64()?;
            let v = number(p.get("p"))?;
            (0.0..=1.0).contains(&v).then_some((t, v))
        })
        .collect();
    out.sort_by_key(|(t, _)| *t);
    out.dedup_by_key(|(t, _)| *t);
    out
}

/// A trade of the public trade feed.
#[derive(Debug, Clone, PartialEq)]
pub struct Trade {
    pub wallet: String,
    /// `buy` or `sell`.
    pub side: String,
    pub token: String,
    pub condition_id: String,
    pub outcome: String,
    /// Shares.
    pub size: f64,
    /// Price of a share in USDC, [0, 1].
    pub price: f64,
    pub ts: i64,
    pub tx: String,
}

impl Trade {
    /// Amount of the trade in USDC.
    pub fn usdc(&self) -> f64 {
        self.size * self.price
    }
}

/// Trades of a trade feed answer; a row missing a field is skipped.
pub fn trades(answer: &Value) -> Vec<Trade> {
    answer
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let wallet = text(t, "proxyWallet").to_ascii_lowercase();
            let side = text(t, "side").to_ascii_lowercase();
            let size = number(t.get("size"))?;
            let price = number(t.get("price"))?;
            let ts = t.get("timestamp").and_then(Value::as_i64)?;
            let condition_id = text(t, "conditionId").to_ascii_lowercase();
            (wallet_ok(&wallet)
                && (side == "buy" || side == "sell")
                && size >= 0.0
                && (0.0..=1.0).contains(&price)
                && condition_ok(&condition_id))
            .then(|| Trade {
                wallet,
                side,
                token: text(t, "asset"),
                condition_id,
                outcome: text(t, "outcome"),
                size,
                price,
                // Milliseconds sent by mistake are brought back to seconds.
                ts: if ts > 10_000_000_000 { ts / 1000 } else { ts },
                tx: text(t, "transactionHash"),
            })
        })
        .collect()
}

/// A resolution state as recorded (D920).
#[derive(Debug, Clone, PartialEq)]
pub struct StateAt {
    pub known_at: i64,
    pub state: ResolutionState,
    pub outcome: Option<String>,
}

/// What was known of a resolution at an instant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "outcome")]
pub enum Resolution {
    /// Nothing yet, a proposal or a dispute: never resolved ahead of time.
    Unresolved,
    Final(String),
    Indeterminate,
}

/// The resolution as known at `as_of`: the last state known by then. A
/// proposal or a dispute is unresolved, a final state without an outcome
/// is indeterminate.
pub fn resolution_at(states: &[StateAt], as_of: i64) -> Resolution {
    let last = states
        .iter()
        .filter(|s| s.known_at <= as_of)
        .max_by_key(|s| s.known_at);
    match last {
        Some(StateAt {
            state: ResolutionState::Final,
            outcome: Some(o),
            ..
        }) => Resolution::Final(o.clone()),
        Some(StateAt {
            state: ResolutionState::Final | ResolutionState::Indeterminate,
            ..
        }) => Resolution::Indeterminate,
        _ => Resolution::Unresolved,
    }
}

/// Inputs of the decision model for one market at an instant (D922).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Features {
    /// Last price known at the instant.
    pub probability: f64,
    /// Instant of that price (RFC 3339).
    #[cfg_attr(feature = "openapi", schema(format = DateTime))]
    pub price_ts: String,
    /// In probability points (−1 to 1); `None` without a point that far back.
    pub change_1h: Option<f64>,
    pub change_24h: Option<f64>,
    pub days_to_end: Option<f64>,
}

fn rfc3339(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Price at `t`: the last point at or before it.
fn price_at(points: &[(i64, f64)], t: i64) -> Option<(i64, f64)> {
    let i = points.partition_point(|(ts, _)| *ts <= t);
    i.checked_sub(1).map(|i| points[i])
}

/// Features at `as_of` from sorted points; `None` without a known price.
/// A point older than 2 days is no level: the market is silent.
pub fn features(points: &[(i64, f64)], as_of: i64, end_at: Option<i64>) -> Option<Features> {
    let (ts, p) = price_at(points, as_of)?;
    if as_of - ts > 2 * 86_400 {
        return None;
    }
    let back = |s: i64| {
        price_at(points, as_of - s)
            .filter(|(t, _)| as_of - s - t <= s)
            .map(|(_, q)| p - q)
    };
    Some(Features {
        probability: p,
        price_ts: rfc3339(ts),
        change_1h: back(3_600),
        change_24h: back(86_400),
        days_to_end: end_at.map(|e| (e - as_of) as f64 / 86_400.0),
    })
}

/// Brier score of `(probability, happened)` pairs: 0 perfect, 1 worst.
pub fn brier(pairs: &[(f64, bool)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    let sum: f64 = pairs
        .iter()
        .map(|(p, y)| (p - f64::from(u8::from(*y))).powi(2))
        .sum();
    Some(sum / pairs.len() as f64)
}

/// A tenth of the probability axis (D925).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CalibrationBin {
    pub from: f64,
    pub to: f64,
    pub count: u64,
    pub mean_probability: f64,
    /// Share of the pairs of this tenth whose outcome happened.
    pub observed: f64,
}

/// Calibration curve by tenth; empty tenths are left out.
pub fn calibration(pairs: &[(f64, bool)]) -> Vec<CalibrationBin> {
    let mut bins = [(0u64, 0f64, 0f64); 10];
    for (p, y) in pairs {
        let i = ((p * 10.0).floor() as usize).min(9);
        bins[i].0 += 1;
        bins[i].1 += p;
        bins[i].2 += f64::from(u8::from(*y));
    }
    bins.iter()
        .enumerate()
        .filter(|(_, b)| b.0 > 0)
        .map(|(i, (n, sp, sy))| CalibrationBin {
            from: i as f64 / 10.0,
            to: (i + 1) as f64 / 10.0,
            count: *n,
            mean_probability: sp / *n as f64,
            observed: sy / *n as f64,
        })
        .collect()
}

/// A model's probability for an outcome of a market, posted with a
/// decision and scored against the market once resolved (D925).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Forecast {
    /// Condition id of the market (`0x` and 64 hex digits).
    pub market: String,
    /// Outcome as the market names it (`Yes`, `No`…).
    pub outcome: String,
    /// In [0, 1].
    pub probability: f64,
}

pub const FORECASTS_MAX: usize = 50;
pub const OUTCOME_MAX: usize = 64;

/// The first defect of a forecast list: (field, token).
pub fn forecast_defect(f: &[Forecast]) -> Option<(String, String)> {
    if f.len() > FORECASTS_MAX {
        return Some(("forecasts".into(), format!("max_items:{FORECASTS_MAX}")));
    }
    f.iter().enumerate().find_map(|(i, x)| {
        let field = |n: &str| format!("forecasts.{i}.{n}");
        if !condition_ok(&x.market.to_ascii_lowercase()) {
            Some((field("market"), "invalid".into()))
        } else if x.outcome.trim().is_empty() || x.outcome.chars().count() > OUTCOME_MAX {
            Some((field("outcome"), "invalid".into()))
        } else if !(0.0..=1.0).contains(&x.probability) {
            Some((field("probability"), "range:0..1".into()))
        } else {
            None
        }
    })
}

/// A market a member says bears on the instrument, open at the instant,
/// with its inputs for the model (D922).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MarketFeature {
    #[cfg_attr(feature = "openapi", schema(format = "uuid"))]
    pub market_id: String,
    pub condition_id: String,
    pub question: String,
    /// The outcome whose probability is given (the market's first one).
    pub outcome: String,
    /// `up`, `down` or `unknown`, as the member said (D921).
    pub direction: String,
    pub features: Features,
}

/// The forecasts of a model against the market on the same markets (D925).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ForecastScores {
    pub model: String,
    pub model_version: String,
    pub forecasts: u64,
    /// Forecasts on a market resolved final with a market price known at
    /// the decision's instant: the pairs both scores read.
    pub scored: u64,
    pub brier_model: Option<f64>,
    pub brier_market: Option<f64>,
    pub calibration_model: Vec<CalibrationBin>,
    pub calibration_market: Vec<CalibrationBin>,
}

/// Thresholds of the early bet detector (D924), set by the org.
#[derive(Debug, Clone, PartialEq)]
pub struct EarlyBetThresholds {
    /// The wallet was first seen less than this many days before.
    pub wallet_days: f64,
    /// Share of the wallet's volume on this market, in %.
    pub share_pct: f64,
    /// Stake on the market inside the window, in USDC.
    pub min_usdc: f64,
    /// Window before the trigger, in hours.
    pub window_hours: f64,
    /// A probability move of at least this many points within an hour.
    pub jump_points: f64,
}

impl Default for EarlyBetThresholds {
    fn default() -> Self {
        Self {
            wallet_days: 7.0,
            share_pct: 80.0,
            min_usdc: 10_000.0,
            window_hours: 48.0,
            jump_points: 15.0,
        }
    }
}

/// Instants where the price moved by at least `points` within an hour.
pub fn jumps(points: &[(i64, f64)], jump_points: f64) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    for (i, (t, p)) in points.iter().enumerate() {
        let moved = points[..i]
            .iter()
            .rev()
            .take_while(|(s, _)| t - s <= 3_600)
            .any(|(_, q)| (p - q).abs() * 100.0 >= jump_points);
        // One jump per hour: the same move is not counted at each point.
        if moved && out.last().is_none_or(|last| t - last > 3_600) {
            out.push(*t);
        }
    }
    out
}

/// A wallet whose bet came before a trigger (D924): the figures that
/// raised it, never an intention.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EarlyBet {
    pub wallet: String,
    pub condition_id: String,
    /// Bought on this market inside the window, USDC.
    pub stake_usdc: f64,
    /// Share of the wallet's whole volume on this market, %.
    pub share_pct: f64,
    /// First trade of the wallet in the collected feed.
    pub first_seen: i64,
    pub first_bet: i64,
    pub last_bet: i64,
    pub trigger_at: i64,
    /// `jump` or `resolution`.
    pub trigger: String,
}

/// Early bets on one market. `market` = its trades, `all` = every
/// collected trade of the wallets involved (their volume and their first
/// trade), `triggers` = (instant, `jump` | `resolution`).
pub fn early_bets(
    market: &[Trade],
    all: &[Trade],
    triggers: &[(i64, &str)],
    th: &EarlyBetThresholds,
) -> Vec<EarlyBet> {
    let window = (th.window_hours * 3_600.0) as i64;
    let young = (th.wallet_days * 86_400.0) as i64;
    let mut out = Vec::new();
    let mut wallets: Vec<&str> = market.iter().map(|t| t.wallet.as_str()).collect();
    wallets.sort_unstable();
    wallets.dedup();
    for w in wallets {
        let theirs: Vec<&Trade> = all.iter().filter(|t| t.wallet == w).collect();
        let Some(first_seen) = theirs.iter().map(|t| t.ts).min() else {
            continue;
        };
        let total: f64 = theirs.iter().map(|t| t.usdc()).sum();
        let here: f64 = market
            .iter()
            .filter(|t| t.wallet == w)
            .map(Trade::usdc)
            .sum();
        let share = if total > 0.0 {
            here / total * 100.0
        } else {
            0.0
        };
        if share < th.share_pct {
            continue;
        }
        for (at, kind) in triggers {
            let bets: Vec<&Trade> = market
                .iter()
                .filter(|t| t.wallet == w && t.side == "buy" && t.ts < *at && at - t.ts <= window)
                .collect();
            let stake: f64 = bets.iter().map(|t| t.usdc()).sum();
            let (Some(first_bet), Some(last_bet)) = (
                bets.iter().map(|t| t.ts).min(),
                bets.iter().map(|t| t.ts).max(),
            ) else {
                continue;
            };
            if stake < th.min_usdc || at - first_seen > young {
                continue;
            }
            out.push(EarlyBet {
                wallet: w.to_string(),
                condition_id: bets[0].condition_id.clone(),
                stake_usdc: (stake * 100.0).round() / 100.0,
                share_pct: (share * 10.0).round() / 10.0,
                first_seen,
                first_bet,
                last_bet,
                trigger_at: *at,
                trigger: (*kind).to_string(),
            });
            // One proposal per wallet and market: the first trigger it preceded.
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const C: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
    const W: &str = "0x2222222222222222222222222222222222222222";
    const V: &str = "0x3333333333333333333333333333333333333333";

    #[test]
    fn markets_are_decoded_whatever_the_list_form() {
        let a = json!([{
            "conditionId": C.to_uppercase().replace("0X", "0x"), "slug": "fed-cut-december",
            "question": "Fed cut in December?", "description": "Resolves Yes if…",
            "resolutionSource": "https://www.federalreserve.gov", "endDate": "2026-12-10T00:00:00Z",
            "outcomes": "[\"Yes\", \"No\"]", "outcomePrices": "[\"1\", \"0\"]",
            "clobTokenIds": "[\"123\", \"456\"]", "volumeNum": 1250000.5, "closed": true,
            "umaResolutionStatus": "resolved"
        }, { "conditionId": "nope" }]);
        let m = markets(&a);
        assert_eq!(m.len(), 1);
        let m = &m[0];
        assert_eq!(m.condition_id, C);
        assert_eq!(m.outcomes, vec!["Yes", "No"]);
        assert_eq!(m.tokens, vec!["123", "456"]);
        assert_eq!(m.winner.as_deref(), Some("Yes"));
        assert_eq!(m.state, Some(ResolutionState::Final));
        assert_eq!(m.end_at, instant("2026-12-10T00:00:00Z"));
        let open = markets(
            &json!({ "data": [{ "conditionId": C, "outcomes": ["Yes", "No"],
            "outcomePrices": ["0.4", "0.6"], "closed": false, "umaResolutionStatus": "disputed" }] }),
        );
        assert_eq!(open[0].winner, None);
        assert_eq!(open[0].state, Some(ResolutionState::Disputed));
    }

    #[test]
    fn prices_and_trades_skip_bad_rows() {
        let h = price_history(&json!({ "history": [
            { "t": 20, "p": 0.6 }, { "t": 10, "p": "0.5" }, { "t": 30, "p": 1.7 }, { "t": 20, "p": 0.9 }
        ] }));
        assert_eq!(h, vec![(10, 0.5), (20, 0.6)]);
        let t = trades(&json!([
            { "proxyWallet": W.to_uppercase().replace("0X", "0x"), "side": "BUY", "asset": "123",
              "conditionId": C, "outcome": "Yes", "size": 1000, "price": 0.2, "timestamp": 1_700_000_000_000i64,
              "transactionHash": "0xabc" },
            { "proxyWallet": "bad", "side": "BUY", "conditionId": C, "size": 1, "price": 0.2, "timestamp": 1 }
        ]));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].wallet, W);
        assert_eq!(t[0].ts, 1_700_000_000);
        assert!((t[0].usdc() - 200.0).abs() < 1e-9);
    }

    /// The Strategy case: sale on May 26–31, known on June 1, final « No ».
    /// Before the oracle settles, a read never resolves it ahead of time.
    #[test]
    fn strategy_case_is_unresolved_until_known() {
        let day = |d: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 6, d)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        };
        let states = vec![
            StateAt {
                known_at: day(1),
                state: ResolutionState::Proposed,
                outcome: Some("Yes".into()),
            },
            StateAt {
                known_at: day(2),
                state: ResolutionState::Disputed,
                outcome: None,
            },
            StateAt {
                known_at: day(5),
                state: ResolutionState::Final,
                outcome: Some("No".into()),
            },
        ];
        assert_eq!(resolution_at(&states, day(1) - 1), Resolution::Unresolved);
        assert_eq!(
            resolution_at(&states, day(1)),
            Resolution::Unresolved,
            "a proposal is no resolution"
        );
        assert_eq!(
            resolution_at(&states, day(3)),
            Resolution::Unresolved,
            "in dispute"
        );
        assert_eq!(
            resolution_at(&states, day(5)),
            Resolution::Final("No".into())
        );
    }

    /// The Zelensky case: credible sources disagree; disputed twice, then
    /// settled without a clean outcome.
    #[test]
    fn zelensky_case_stays_disputed_then_indeterminate() {
        let states = vec![
            StateAt {
                known_at: 100,
                state: ResolutionState::Proposed,
                outcome: Some("Yes".into()),
            },
            StateAt {
                known_at: 200,
                state: ResolutionState::Disputed,
                outcome: None,
            },
            StateAt {
                known_at: 300,
                state: ResolutionState::Proposed,
                outcome: Some("No".into()),
            },
            StateAt {
                known_at: 400,
                state: ResolutionState::Disputed,
                outcome: None,
            },
            StateAt {
                known_at: 500,
                state: ResolutionState::Indeterminate,
                outcome: None,
            },
        ];
        for t in [150, 250, 350, 450] {
            assert_eq!(resolution_at(&states, t), Resolution::Unresolved, "{t}");
        }
        assert_eq!(resolution_at(&states, 500), Resolution::Indeterminate);
    }

    #[test]
    fn features_read_only_the_past() {
        let pts = vec![
            (0, 0.2),
            (3_600, 0.3),
            (86_400, 0.5),
            (90_000, 0.55),
            (200_000, 0.9),
        ];
        let f = features(&pts, 90_000, Some(90_000 + 2 * 86_400)).unwrap();
        assert!((f.probability - 0.55).abs() < 1e-9);
        assert!((f.change_1h.unwrap() - 0.05).abs() < 1e-9);
        assert!((f.change_24h.unwrap() - 0.25).abs() < 1e-9);
        assert!((f.days_to_end.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(features(&pts, -1, None), None, "before the first point");
        assert_eq!(
            features(&pts, 200_000 + 3 * 86_400, None),
            None,
            "silent market"
        );
    }

    #[test]
    fn brier_and_calibration() {
        let pairs = [(0.9, true), (0.8, true), (0.15, false), (0.95, false)];
        let b = brier(&pairs).unwrap();
        assert!((b - (0.01 + 0.04 + 0.0225 + 0.9025) / 4.0).abs() < 1e-9);
        let c = calibration(&pairs);
        assert_eq!(c.iter().map(|b| b.count).sum::<u64>(), 4);
        let top = c.iter().find(|b| b.from == 0.9).unwrap();
        assert_eq!(top.count, 2);
        assert!((top.observed - 0.5).abs() < 1e-9);
        assert_eq!(brier(&[]), None);
    }

    fn trade(w: &str, side: &str, usdc: f64, ts: i64, cond: &str) -> Trade {
        Trade {
            wallet: w.into(),
            side: side.into(),
            token: "1".into(),
            condition_id: cond.into(),
            outcome: "Yes".into(),
            size: usdc / 0.5,
            price: 0.5,
            ts,
            tx: format!("0x{ts}"),
        }
    }

    #[test]
    fn early_bets_need_every_threshold() {
        let other = "0x4444444444444444444444444444444444444444444444444444444444444444";
        let h = 3_600;
        let at = 100 * h;
        let market = vec![
            // A fresh wallet, almost all of its volume here, 12k before the jump.
            trade(W, "buy", 12_000.0, at - 10 * h, C),
            // An old wallet doing the same: not young.
            trade(V, "buy", 50_000.0, at - 5 * h, C),
        ];
        let mut all = market.clone();
        all.push(trade(W, "buy", 1_000.0, at - 20 * h, other));
        all.push(trade(V, "buy", 1_000.0, at - 60 * 24 * h, other));
        let th = EarlyBetThresholds::default();
        let pts = vec![(at - 2 * h, 0.3), (at - 1800, 0.31), (at, 0.5)];
        let triggers: Vec<(i64, &str)> = jumps(&pts, th.jump_points)
            .into_iter()
            .map(|t| (t, "jump"))
            .collect();
        assert_eq!(triggers, vec![(at, "jump")]);
        let found = early_bets(&market, &all, &triggers, &th);
        assert_eq!(found.len(), 1, "{found:?}");
        let b = &found[0];
        assert_eq!(b.wallet, W);
        assert!((b.stake_usdc - 12_000.0).abs() < 1e-6);
        assert!(b.share_pct > 90.0);
        // Below the stake, or outside the window: nothing.
        let small = EarlyBetThresholds {
            min_usdc: 20_000.0,
            ..th.clone()
        };
        assert!(early_bets(&market, &all, &triggers, &small).is_empty());
        let short = EarlyBetThresholds {
            window_hours: 5.0,
            ..th
        };
        assert!(early_bets(&market, &all, &triggers, &short).is_empty());
    }
}
