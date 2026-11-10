//! Public trading accounts (markets-research.md appendix K, D930): figures on
//! when an account's entries fall, before a move of the instrument or
//! around a release of the calendar. Pure code: no I/O, no clock. Figures
//! only: no intention is ever attributed (D742), a proposal is a lead.

use serde::{Deserialize, Serialize};

/// One trade of an account, as its platform published it (D927). Times in
/// seconds since the epoch, UTC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trade {
    pub trade_key: String,
    pub symbol: String,
    pub buy: bool,
    /// Absent when the owner hides position sizes on the platform.
    pub lots: Option<f64>,
    pub open_at: i64,
    pub close_at: Option<i64>,
    pub open_price: f64,
    pub close_price: Option<f64>,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
    pub profit: Option<f64>,
    pub pips: Option<f64>,
    pub commission: Option<f64>,
    pub swap: Option<f64>,
    /// Gain of the trade in percent of the account, as the platform publishes it.
    pub gain_pct: Option<f64>,
    pub magic: Option<String>,
    pub comment: Option<String>,
}

/// The key of a trade: its ticket, else its opening and its close. Rows
/// identical on all of these (a grid closing a basket) are told apart by
/// their rank in the file: [`parse_trades_csv`] appends `#2`, `#3`…
pub fn trade_key(ticket: Option<&str>, t: &Trade) -> String {
    match ticket.map(str::trim).filter(|t| !t.is_empty()) {
        Some(k) => format!("ticket:{k}"),
        None => format!(
            "open:{}:{}:{}:{}:{}:close:{}:{}",
            t.open_at,
            t.symbol,
            if t.buy { "buy" } else { "sell" },
            t.lots.map_or(String::new(), |l| l.to_string()),
            t.open_price,
            t.close_at.map_or(String::new(), |c| c.to_string()),
            t.close_price.map_or(String::new(), |c| c.to_string()),
        ),
    }
}

/// Columns of the canonical import CSV (D929 a): the export of a platform is
/// turned into this form by the client that sends it.
pub const CSV_COLUMNS: [&str; 17] = [
    "ticket",
    "symbol",
    "side",
    "lots",
    "open_at",
    "close_at",
    "open_price",
    "close_price",
    "sl",
    "tp",
    "profit",
    "pips",
    "commission",
    "swap",
    "gain_pct",
    "magic",
    "comment",
];
/// Columns a row cannot do without.
const CSV_REQUIRED: [&str; 4] = ["symbol", "side", "open_at", "open_price"];

/// Why a CSV is refused: the 1-based line (0 = header) and the column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvError {
    pub line: usize,
    pub column: String,
}

/// Reads the canonical CSV: header with every required column (unknown
/// columns refused), one trade per row, ISO 8601 instants with an offset,
/// `side` `buy` or `sell`. The first faulty cell refuses the whole file.
pub fn parse_trades_csv(text: &str) -> Result<Vec<Trade>, CsvError> {
    let table = crate::ontology::import::parse_csv(text, Some(','));
    let err = |line: usize, column: &str| CsvError {
        line,
        column: column.to_string(),
    };
    let Some(head) = table.first() else {
        return Err(err(0, "header"));
    };
    let head: Vec<String> = head.iter().map(|h| h.trim().to_ascii_lowercase()).collect();
    if let Some(h) = head.iter().find(|h| !CSV_COLUMNS.contains(&h.as_str())) {
        return Err(err(0, h));
    }
    if let Some(c) = CSV_REQUIRED.iter().find(|c| !head.iter().any(|h| h == *c)) {
        return Err(err(0, c));
    }
    let mut out = Vec::new();
    for (i, row) in table.iter().enumerate().skip(1) {
        if row.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let cell = |name: &str| {
            head.iter()
                .position(|h| h == name)
                .and_then(|p| row.get(p))
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
        };
        let num = |name: &str| -> Result<Option<f64>, CsvError> {
            cell(name)
                .map(|c| {
                    c.parse::<f64>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| err(i, name))
                })
                .transpose()
        };
        let instant = |name: &str| -> Result<Option<i64>, CsvError> {
            cell(name)
                .map(|c| {
                    chrono::DateTime::parse_from_rfc3339(c)
                        .map(|d| d.timestamp())
                        .map_err(|_| err(i, name))
                })
                .transpose()
        };
        let need = |v: Option<f64>, name: &str| v.ok_or_else(|| err(i, name));
        let symbol = cell("symbol")
            .filter(|s| s.len() <= 32)
            .ok_or_else(|| err(i, "symbol"))?;
        let buy = match cell("side").map(str::to_ascii_lowercase).as_deref() {
            Some("buy") => true,
            Some("sell") => false,
            _ => return Err(err(i, "side")),
        };
        let lots = num("lots")?;
        let open_at = instant("open_at")?.ok_or_else(|| err(i, "open_at"))?;
        let open_price = need(num("open_price")?, "open_price")?;
        let text = |name: &str| cell(name).map(|c| c.chars().take(500).collect::<String>());
        let ticket = cell("ticket").filter(|t| t.len() <= 64);
        if cell("ticket").is_some() && ticket.is_none() {
            return Err(err(i, "ticket"));
        }
        let mut t = Trade {
            trade_key: String::new(),
            symbol: symbol.to_string(),
            buy,
            lots,
            open_at,
            close_at: instant("close_at")?,
            open_price,
            close_price: num("close_price")?,
            sl: num("sl")?,
            tp: num("tp")?,
            profit: num("profit")?,
            pips: num("pips")?,
            commission: num("commission")?,
            swap: num("swap")?,
            gain_pct: num("gain_pct")?,
            magic: text("magic"),
            comment: text("comment"),
        };
        t.trade_key = trade_key(ticket, &t);
        out.push(t);
    }
    // The n-th row with the same key is another trade: the same export read
    // again gives the same ranks.
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for t in &mut out {
        let n = seen.entry(t.trade_key.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            t.trade_key = format!("{}#{n}", t.trade_key);
        }
    }
    Ok(out)
}

/// A platform symbol read as an instrument ticker: `XAUUSD#`, `xauusd.m`
/// and `XAUUSDm` all trade `XAUUSD`.
pub fn symbol_matches(symbol: &str, ticker: &str) -> bool {
    let s = symbol.trim().to_ascii_uppercase();
    !ticker.is_empty() && s.starts_with(&ticker.to_ascii_uppercase())
}

/// One entry of an account: when it opened (seconds since the epoch, UTC),
/// its side and its profit as the platform published it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    pub open_at: i64,
    pub buy: bool,
    pub profit: f64,
}

/// Thresholds of the detector, settings of the pack (Q-TA2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// The move is measured over this many minutes after the entry.
    pub window_minutes: i64,
    /// Move at least this large, in percent, either way.
    pub move_pct: f64,
    /// An entry within ± this many minutes of a release is "around" it.
    pub release_window_minutes: i64,
    pub min_count: usize,
    pub min_share_pct: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            window_minutes: 60,
            move_pct: 1.0,
            release_window_minutes: 30,
            min_count: 10,
            min_share_pct: 30.0,
        }
    }
}

/// What the detector saw on one account. Counts and sums, nothing else.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Figures {
    pub entries: usize,
    /// Entries followed by a move ≥ the threshold within the window.
    pub before_move: usize,
    /// Among them, the move went the entry's way.
    pub before_move_with_side: usize,
    pub before_move_profit: f64,
    /// Entries within the release window of a release.
    pub around_release: usize,
    pub around_release_profit: f64,
}

/// The last known price at `t`: `prices` are (instant the price is known,
/// price), sorted by instant.
fn price_at(prices: &[(i64, f64)], t: i64) -> Option<f64> {
    let i = prices.partition_point(|(at, _)| *at <= t);
    (i > 0).then(|| prices[i - 1].1)
}

/// Figures of an account's entries. `prices`: (known at, price) sorted;
/// `releases`: instants sorted. An entry whose move cannot be measured
/// (no price at the entry or at the end of the window) counts as an entry
/// only.
pub fn entry_figures(
    entries: &[Entry],
    prices: &[(i64, f64)],
    releases: &[i64],
    th: &Thresholds,
) -> Figures {
    let mut f = Figures {
        entries: entries.len(),
        ..Default::default()
    };
    let around = th.release_window_minutes * 60;
    for e in entries {
        let end = e.open_at + th.window_minutes * 60;
        // The window must have elapsed in the prices we hold.
        let measured = prices.last().is_some_and(|(at, _)| *at >= end);
        if let (true, Some(p0), Some(p1)) =
            (measured, price_at(prices, e.open_at), price_at(prices, end))
        {
            if p0 > 0.0 {
                let r = (p1 / p0 - 1.0) * 100.0;
                if r.abs() >= th.move_pct {
                    f.before_move += 1;
                    f.before_move_profit += e.profit;
                    if (r > 0.0) == e.buy {
                        f.before_move_with_side += 1;
                    }
                }
            }
        }
        let i = releases.partition_point(|r| *r < e.open_at - around);
        if releases.get(i).is_some_and(|r| *r <= e.open_at + around) {
            f.around_release += 1;
            f.around_release_profit += e.profit;
        }
    }
    f
}

/// An account is proposed when enough of its entries fall before a move or
/// around a release, in count and in share.
pub fn proposes(f: &Figures, th: &Thresholds) -> bool {
    if f.entries == 0 {
        return false;
    }
    let enough =
        |n: usize| n >= th.min_count && n as f64 * 100.0 / f.entries as f64 >= th.min_share_pct;
    enough(f.before_move) || enough(f.around_release)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;

    fn entry(open_at: i64, buy: bool) -> Entry {
        Entry {
            open_at,
            buy,
            profit: 10.0,
        }
    }

    #[test]
    fn entries_before_a_move_are_counted_with_their_side() {
        // 100 until 1 h, then 102 (+2 %), then flat.
        let prices = [(0, 100.0), (H, 102.0), (3 * H, 102.0)];
        let th = Thresholds::default();
        let f = entry_figures(
            &[entry(0, true), entry(0, false), entry(2 * H, true)],
            &prices,
            &[],
            &th,
        );
        assert_eq!(f.entries, 3);
        assert_eq!(f.before_move, 2);
        assert_eq!(f.before_move_with_side, 1);
        assert_eq!(f.before_move_profit, 20.0);
        assert_eq!(f.around_release, 0);
    }

    #[test]
    fn a_window_not_yet_elapsed_is_not_a_move() {
        let prices = [(0, 100.0), (H / 2, 103.0)];
        let f = entry_figures(&[entry(0, true)], &prices, &[], &Thresholds::default());
        assert_eq!(f.before_move, 0);
        assert_eq!(f.entries, 1);
    }

    #[test]
    fn entries_around_a_release_count_on_both_sides() {
        let releases = [10 * H];
        let th = Thresholds::default();
        let es = [
            entry(10 * H - 29 * 60, true),
            entry(10 * H + 30 * 60, false),
            entry(10 * H + 31 * 60, false),
        ];
        let f = entry_figures(&es, &[], &releases, &th);
        assert_eq!(f.around_release, 2);
    }

    #[test]
    fn the_canonical_csv_reads_trades_and_refuses_the_first_bad_cell() {
        let csv = "ticket,symbol,side,lots,open_at,close_at,open_price,close_price,profit,comment\n\
                   101,XAUUSD,sell,0.01,2024-03-12T12:25:00Z,2024-03-12T15:00:00Z,2160.5,2150.1,10.4,grid 1\n\
                   ,XAUUSD,BUY,0.02,2024-03-12T13:00:00+01:00,,2161,,,\n";
        let t = parse_trades_csv(csv).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].trade_key, "ticket:101");
        assert!(!t[0].buy && t[1].buy);
        assert_eq!(t[0].close_price, Some(2150.1));
        assert_eq!(t[1].open_at, 1_710_244_800);
        assert!(t[1].trade_key.starts_with("open:1710244800:XAUUSD:buy"));
        // A basket of identical positions: three trades, three keys.
        let grid = "symbol,side,lots,open_at,open_price,close_at,close_price,gain_pct\n\
                    XAUUSD,sell,0.01,2024-03-12T12:25:00Z,2160,2024-03-12T14:00:00Z,2150,0.12\n\
                    XAUUSD,sell,0.01,2024-03-12T12:25:00Z,2160,2024-03-12T14:00:00Z,2150,0.12\n\
                    XAUUSD,sell,0.01,2024-03-12T12:25:00Z,2160,2024-03-12T15:00:00Z,2155,0.05\n";
        let g = parse_trades_csv(grid).unwrap();
        let keys: std::collections::HashSet<_> = g.iter().map(|t| t.trade_key.clone()).collect();
        assert_eq!(keys.len(), 3);
        assert!(g[1].trade_key.ends_with("#2"));
        assert_eq!(g[0].gain_pct, Some(0.12));
        // Sizes hidden by the owner: no lots column, or empty cells.
        let hidden = "symbol,side,open_at,open_price,close_at,close_price\n\
                      XAUUSD,buy,2024-03-12T12:25:00Z,2160,2024-03-12T14:00:00Z,2170\n";
        let h = parse_trades_csv(hidden).unwrap();
        assert_eq!(h[0].lots, None);
        assert!(
            h[0].trade_key
                .starts_with("open:1710246300:XAUUSD:buy::2160:"),
            "{}",
            h[0].trade_key
        );
        assert_eq!(
            parse_trades_csv(
                "symbol,side,lots,open_at,open_price\nX,buy,,2024-01-01T00:00:00Z,1\n"
            )
            .unwrap()[0]
                .lots,
            None
        );
        assert_eq!(t[1].close_at, None);
        let bad = |csv: &str| parse_trades_csv(csv).unwrap_err();
        assert_eq!(
            bad("symbol,side,lots,open_at\nX,buy,1,2024-01-01T00:00:00Z").column,
            "open_price"
        );
        assert_eq!(
            bad("symbol,side,lots,open_at,open_price,owner\n").column,
            "owner"
        );
        let e = bad("symbol,side,lots,open_at,open_price\nX,buy,1,2024-01-01,1\n");
        assert_eq!(
            (e.line, e.column.as_str()),
            (1, "open_at"),
            "an instant needs its offset"
        );
        assert_eq!(
            bad("symbol,side,lots,open_at,open_price\nX,long,1,2024-01-01T00:00:00Z,1\n").column,
            "side"
        );
        assert_eq!(
            bad("symbol,side,lots,open_at,open_price\nX,buy,NaN,2024-01-01T00:00:00Z,1\n").column,
            "lots"
        );
    }

    #[test]
    fn a_proposal_needs_count_and_share() {
        let th = Thresholds::default();
        let f = |entries, before_move| Figures {
            entries,
            before_move,
            ..Default::default()
        };
        assert!(proposes(&f(30, 10), &th));
        assert!(!proposes(&f(40, 10), &th), "25 % < 30 %");
        assert!(!proposes(&f(9, 9), &th), "9 < 10 entries");
        assert!(!proposes(&f(0, 0), &th));
    }
}
