//! Public trading accounts (markets-research.md appendix K): their trades are
//! rows of `account_trades` (D927), known at their import unless proven
//! otherwise (D928); an account is an object once imported, added to a case
//! or retained from a detection (Q-TA5); erasing it purges its rows (D932).
//! The detector (D930) counts entries before a move of the instrument or
//! around a release of the calendar: figures, never an intention.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use pnex_core::markets::accounts::{self as acc, Entry, Figures, Thresholds, Trade};
use pnex_core::ontology::api::{LinkInput, ObjectInput};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::services::artifact_store::S3Store;
use crate::services::ontology::{links, objects, packs, Actor, OntologyError};

const PACK: &str = "trading_accounts";
const T_ACCOUNT: &str = "trading_account";
const T_INSTRUMENT: &str = "instrument";
const L_TRADES: &str = "trades";
/// Platforms whose accounts are read (D929); the pack's enum.
pub const PLATFORMS: [&str; 1] = ["myfxbook"];
/// Calendar categories an entry may fall around (D882).
const RELEASES: [&str; 2] = ["economic_release", "central_bank_meeting"];
/// Rows of one import, and of one read.
pub const IMPORT_MAX: usize = 50_000;
/// Detector name of an entry-timing proposal in a case (D930).
pub const ENTRY_TIMING: &str = "entry_timing@1";

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn at(t: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(t, 0).unwrap_or_default()
}

/// `<platform>:<id>`, the platform known and the id plain.
pub fn account_ref_ok(r: &str) -> bool {
    let Some((p, id)) = r.split_once(':') else {
        return false;
    };
    PLATFORMS.contains(&p)
        && (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// What an import wrote.
#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ImportResult {
    /// The `trading_account` object.
    #[schema(format = "uuid")]
    pub account_id: String,
    pub inserted: u64,
    /// Rows already known: they keep their first `known_at`.
    pub kept: u64,
}

/// Writes the trades of an account, known now and backfilled (no proof of
/// live publication, D928); a trade already stored is left as it was. Then
/// the account object and its `trades` links are created or refreshed.
pub async fn import<C: ConnectionTrait>(
    db: &C,
    org: i64,
    actor: &Actor,
    account_ref: &str,
    display_name: Option<&str>,
    currency: Option<&str>,
    trades: &[Trade],
    source: &str,
    now: DateTime<Utc>,
) -> crate::services::ontology::Result<ImportResult> {
    let (platform, _) = account_ref.split_once(':').unwrap_or_default();
    let mut inserted = 0;
    for chunk in trades.chunks(500) {
        let rows: Vec<Value> = chunk
            .iter()
            .map(|t| {
                json!({
                    "k": t.trade_key, "s": t.symbol, "d": if t.buy { "buy" } else { "sell" },
                    "l": t.lots, "o": at(t.open_at), "c": t.close_at.map(at),
                    "op": t.open_price, "cp": t.close_price, "sl": t.sl, "tp": t.tp,
                    "p": t.profit, "pi": t.pips, "co": t.commission, "sw": t.swap, "g": t.gain_pct,
                    "m": t.magic, "cm": t.comment,
                })
            })
            .collect();
        let r = db
            .execute_raw(stmt(
                "INSERT INTO account_trades (org_id, platform, account_ref, trade_key, symbol, side, lots, \
                   open_at, close_at, open_price, close_price, sl, tp, profit, pips, commission, swap, \
                   gain_pct, magic, comment, source, known_at, backfilled) \
                 SELECT $1, $2, $3, r.k, r.s, r.d, r.l, r.o, r.c, r.op, r.cp, r.sl, r.tp, r.p, r.pi, \
                   r.co, r.sw, r.g, r.m, r.cm, $4, $5, TRUE \
                 FROM jsonb_to_recordset($6::jsonb) AS r(k text, s text, d text, l float8, o timestamptz, \
                   c timestamptz, op float8, cp float8, sl float8, tp float8, p float8, pi float8, \
                   co float8, sw float8, g float8, m text, cm text) \
                 ON CONFLICT DO NOTHING",
                vec![
                    org.into(),
                    platform.into(),
                    account_ref.into(),
                    source.into(),
                    now.into(),
                    Value::Array(rows).into(),
                ],
            ))
            .await?;
        inserted += r.rows_affected();
    }
    let account = account_object(db, org, actor, account_ref, display_name, now).await?;
    if let Some(c) = currency {
        set_currency(db, org, actor, account, c).await?;
    }
    refresh_links(db, org, actor, account, account_ref).await?;
    Ok(ImportResult {
        account_id: account.to_string(),
        inserted,
        kept: trades.len() as u64 - inserted,
    })
}

/// The account object, created when missing (Q-TA5); found by its ref.
async fn account_object<C: ConnectionTrait>(
    db: &C,
    org: i64,
    actor: &Actor,
    account_ref: &str,
    display_name: Option<&str>,
    now: DateTime<Utc>,
) -> crate::services::ontology::Result<Uuid> {
    if let Some(o) =
        objects::find_by_key(db, org, T_ACCOUNT, "account_ref", &json!(account_ref)).await?
    {
        return Ok(o.id);
    }
    let (platform, _) = account_ref.split_once(':').unwrap_or_default();
    let mut props = Map::new();
    props.insert("account_ref".into(), json!(account_ref));
    props.insert("platform".into(), json!(platform));
    props.insert("known_at".into(), json!(now.to_rfc3339()));
    let name = display_name.map(str::trim).filter(|n| !n.is_empty());
    if let Some(n) = name {
        props.insert("display_name".into(), json!(n));
    }
    let input = ObjectInput {
        type_key: T_ACCOUNT.into(),
        title: name.unwrap_or(account_ref).chars().take(200).collect(),
        properties: props,
        expected_version: None,
    };
    let v = objects::create(db, org, actor, &input).await?;
    Ok(Uuid::parse_str(&v.id).unwrap_or_default())
}

/// An ISO 4217 code, or `USC` (a cent account).
pub fn currency_ok(c: &str) -> bool {
    c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase())
}

/// The account's currency, written when it changes: every profit of the
/// account is in it.
async fn set_currency<C: ConnectionTrait>(
    db: &C,
    org: i64,
    actor: &Actor,
    account: Uuid,
    currency: &str,
) -> crate::services::ontology::Result<()> {
    let row = objects::find(db, org, account).await?;
    if row.properties.get("currency").and_then(Value::as_str) == Some(currency) {
        return Ok(());
    }
    let mut props = row.properties.as_object().cloned().unwrap_or_default();
    props.insert("currency".into(), json!(currency));
    let input = ObjectInput {
        type_key: row.type_key.clone(),
        title: row.title.clone(),
        properties: props,
        expected_version: Some(row.version),
    };
    objects::update_in(db, org, actor, account, &input).await?;
    Ok(())
}

/// One `trades` link per instrument the account traded, with its count and
/// period; an instrument without an object gets no link.
async fn refresh_links<C: ConnectionTrait>(
    db: &C,
    org: i64,
    actor: &Actor,
    account: Uuid,
    account_ref: &str,
) -> crate::services::ontology::Result<()> {
    let instruments = db
        .query_all_raw(stmt(
            "SELECT id, properties ->> 'ticker' AS ticker FROM objects \
             WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL AND erased_at IS NULL",
            vec![org.into(), T_INSTRUMENT.into()],
        ))
        .await?;
    let symbols = db
        .query_all_raw(stmt(
            "SELECT symbol, count(*) AS n, \
               to_char(min(open_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS f, \
               to_char(max(close_at) AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS l \
             FROM account_trades WHERE org_id = $1 AND account_ref = $2 GROUP BY symbol",
            vec![org.into(), account_ref.into()],
        ))
        .await?;
    for i in &instruments {
        let (Ok(id), Ok(ticker)) = (
            i.try_get::<Uuid>("", "id"),
            i.try_get::<String>("", "ticker"),
        ) else {
            continue;
        };
        let mut n = 0i64;
        let (mut first, mut last): (Option<String>, Option<String>) = (None, None);
        for s in &symbols {
            let sym: String = s.try_get("", "symbol").unwrap_or_default();
            if !acc::symbol_matches(&sym, &ticker) {
                continue;
            }
            n += s.try_get::<i64>("", "n").unwrap_or(0);
            let f: Option<String> = s.try_get("", "f").ok();
            let l: Option<String> = s.try_get("", "l").ok().flatten();
            first = first.into_iter().chain(f).min();
            last = last.into_iter().chain(l).max();
        }
        if n == 0 {
            continue;
        }
        let attributes = json!({
            "trade_count": n,
            "first_open_at": first.unwrap_or_default(),
            "last_close_at": last.unwrap_or_default(),
        });
        let open = db
            .query_one_raw(stmt(
                "SELECT id FROM resource_edges WHERE org_id = $1 AND relation = $2 \
                   AND source_object_id = $3 AND target_object_id = $4 AND valid_to IS NULL",
                vec![org.into(), L_TRADES.into(), account.into(), id.into()],
            ))
            .await?;
        match open.and_then(|r| r.try_get::<i64>("", "id").ok()) {
            Some(link) => {
                let attrs = attributes.as_object().cloned().unwrap_or_default();
                links::update(db, org, actor, link, &attrs).await?;
            }
            None => {
                let input = LinkInput {
                    link_type: L_TRADES.into(),
                    source_id: account.to_string(),
                    target_id: id.to_string(),
                    attributes: attributes.as_object().cloned().unwrap_or_default(),
                    ..Default::default()
                };
                match links::create(db, org, actor, &input).await {
                    Ok(_) | Err(OntologyError::KeyTaken) => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }
    Ok(())
}

/// A stored trade as the read route gives it.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AccountTrade {
    pub account_ref: String,
    pub trade_key: String,
    pub symbol: String,
    /// `buy` or `sell`.
    pub side: String,
    /// Absent when the owner hides position sizes.
    pub lots: Option<f64>,
    #[schema(format = "date-time")]
    pub open_at: String,
    #[schema(format = "date-time")]
    pub close_at: Option<String>,
    pub open_price: f64,
    pub close_price: Option<f64>,
    pub sl: Option<f64>,
    pub tp: Option<f64>,
    pub profit: Option<f64>,
    pub pips: Option<f64>,
    pub commission: Option<f64>,
    pub swap: Option<f64>,
    /// Gain in percent of the account, as published.
    pub gain_pct: Option<f64>,
    pub magic: Option<String>,
    pub comment: Option<String>,
    /// When PNeX learnt the trade (D928).
    #[schema(format = "date-time")]
    pub known_at: String,
    /// No proof the trade was public before its import.
    pub backfilled: bool,
}

/// Trades known at `as_of` (D928), optionally of one account, by opening.
pub async fn list(
    db: &DatabaseConnection,
    org: i64,
    account: Option<&str>,
    hidden: &[String],
    as_of: DateTime<Utc>,
    limit: u64,
    offset: u64,
) -> Result<(u64, Vec<AccountTrade>), DbErr> {
    let filter = "org_id = $1 AND known_at <= $2 AND ($3::text IS NULL OR account_ref = $3) \
                  AND NOT (account_ref = ANY($4))";
    let values = || -> Vec<sea_orm::Value> {
        vec![
            org.into(),
            as_of.into(),
            account.map(str::to_string).into(),
            hidden.to_vec().into(),
        ]
    };
    let count = db
        .query_one_raw(stmt(
            &format!("SELECT count(*) AS n FROM account_trades WHERE {filter}"),
            values(),
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "n").ok())
        .unwrap_or(0) as u64;
    let mut v = values();
    v.push((limit as i64).into());
    v.push((offset as i64).into());
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT account_ref, trade_key, symbol, side, lots, \
                   to_char(open_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS open_at, \
                   to_char(close_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS close_at, \
                   open_price, close_price, sl, tp, profit, pips, commission, swap, gain_pct, magic, comment, \
                   to_char(known_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS known_at, backfilled \
                 FROM account_trades WHERE {filter} \
                 ORDER BY open_at, account_ref, trade_key LIMIT $5 OFFSET $6"
            ),
            v,
        ))
        .await?;
    let out = rows
        .iter()
        .filter_map(|r| {
            Some(AccountTrade {
                account_ref: r.try_get("", "account_ref").ok()?,
                trade_key: r.try_get("", "trade_key").ok()?,
                symbol: r.try_get("", "symbol").ok()?,
                side: r.try_get("", "side").ok()?,
                lots: r.try_get("", "lots").ok()?,
                open_at: r.try_get("", "open_at").ok()?,
                close_at: r.try_get("", "close_at").ok()?,
                open_price: r.try_get("", "open_price").ok()?,
                close_price: r.try_get("", "close_price").ok()?,
                sl: r.try_get("", "sl").ok()?,
                tp: r.try_get("", "tp").ok()?,
                profit: r.try_get("", "profit").ok()?,
                pips: r.try_get("", "pips").ok()?,
                commission: r.try_get("", "commission").ok()?,
                swap: r.try_get("", "swap").ok()?,
                gain_pct: r.try_get("", "gain_pct").ok()?,
                magic: r.try_get("", "magic").ok()?,
                comment: r.try_get("", "comment").ok()?,
                known_at: r.try_get("", "known_at").ok()?,
                backfilled: r.try_get("", "backfilled").ok()?,
            })
        })
        .collect();
    Ok((count, out))
}

/// The account refs of hidden objects (governance.md D801).
pub async fn refs_of(
    db: &DatabaseConnection,
    org: i64,
    hidden: Option<&std::sync::Arc<std::collections::HashSet<String>>>,
) -> Result<Vec<String>, DbErr> {
    let Some(h) = hidden.filter(|h| !h.is_empty()) else {
        return Ok(Vec::new());
    };
    let ids: Vec<Uuid> = h.iter().filter_map(|i| Uuid::parse_str(i).ok()).collect();
    let rows = db
        .query_all_raw(stmt(
            "SELECT properties ->> 'account_ref' AS r FROM objects \
             WHERE org_id = $1 AND type_key = $2 AND id = ANY($3)",
            vec![org.into(), T_ACCOUNT.into(), ids.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get("", "r").ok())
        .collect())
}

/// Erasing an account purges its trades (D932). Any other object: nothing.
pub async fn before_erase<C: ConnectionTrait>(
    db: &C,
    org: i64,
    row: &crate::models::_entities::objects::Model,
) -> Result<(), DbErr> {
    if row.type_key != T_ACCOUNT {
        return Ok(());
    }
    let Some(r) = row.properties.get("account_ref").and_then(Value::as_str) else {
        return Ok(());
    };
    db.execute_raw(stmt(
        "DELETE FROM account_trades WHERE org_id = $1 AND account_ref = $2",
        vec![org.into(), r.into()],
    ))
    .await?;
    Ok(())
}

async fn setting(db: &DatabaseConnection, org: i64, key: &str) -> Option<Value> {
    packs::setting_value(db, org, &format!("{PACK}.{key}"))
        .await
        .ok()
        .flatten()
}

/// The org's thresholds (Q-TA2).
pub async fn thresholds(db: &DatabaseConnection, org: i64) -> Thresholds {
    let d = Thresholds::default();
    let num = |v: Option<Value>, d: f64| v.and_then(|v| v.as_f64()).unwrap_or(d);
    Thresholds {
        window_minutes: num(
            setting(db, org, "entry_window_minutes").await,
            d.window_minutes as f64,
        ) as i64,
        move_pct: num(setting(db, org, "entry_move_pct").await, d.move_pct),
        release_window_minutes: num(
            setting(db, org, "release_window_minutes").await,
            d.release_window_minutes as f64,
        ) as i64,
        min_count: num(
            setting(db, org, "entry_min_count").await,
            d.min_count as f64,
        ) as usize,
        min_share_pct: num(
            setting(db, org, "entry_min_share_pct").await,
            d.min_share_pct,
        ),
    }
}

/// An entry-timing proposal: the account, the instrument, the figures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryTiming {
    pub account_ref: String,
    pub ticker: String,
    pub from: i64,
    pub to: i64,
    pub figures: Figures,
}

/// Bars are read every five minutes: a detection window of a year stays
/// within one read.
const BAR_STEP_S: i64 = 300;

/// Prices of `ticker` as (known at, close), known by `to`.
async fn prices(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    ticker: &str,
    from: i64,
    to: i64,
) -> Vec<(i64, f64)> {
    use crate::services::markets::decisions as svc;
    let source = setting(db, org, "bars_source")
        .await
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "histdata".into());
    let Ok(Some(table)) = svc::table_of(db, org, &source).await else {
        return Vec::new();
    };
    let step_us = BAR_STEP_S * 1_000_000;
    let q = svc::BarsQuery {
        instrument: ticker,
        source: &source,
        interval_us: step_us,
        from_us: Some(from * 1_000_000 - step_us),
        to_us: Some(to * 1_000_000),
        limit: 120_000,
        as_of_us: to * 1_000_000,
    };
    let scale = 10f64.powi(pnex_core::markets::history::SCALE as i32);
    match svc::bars(db, store, org, table, &q).await {
        Ok(bars) => bars
            .iter()
            .map(|b| (b.ts_us / 1_000_000 + BAR_STEP_S, b.close as f64 / scale))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Instants of the org's releases and central bank decisions in a window.
async fn releases(
    db: &DatabaseConnection,
    org: i64,
    from: i64,
    to: i64,
) -> Result<Vec<i64>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT extract(epoch FROM planned_start)::bigint AS t FROM time_ranges \
             WHERE org_id = $1 AND category = ANY($2) AND planned_start BETWEEN $3 AND $4 ORDER BY 1",
            vec![
                org.into(),
                RELEASES.iter().map(|c| c.to_string()).collect::<Vec<_>>().into(),
                at(from).into(),
                at(to).into(),
            ],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get("", "t").ok())
        .collect())
}

/// Accounts whose entries on an instrument of the case, opened in
/// `[from, to]` and known by `to`, fall before moves or around releases.
pub async fn case_detections(
    db: &DatabaseConnection,
    store: Option<&S3Store>,
    org: i64,
    members: &HashMap<Uuid, crate::models::_entities::objects::Model>,
    from: i64,
    to: i64,
) -> Result<Vec<(Uuid, EntryTiming)>, DbErr> {
    let instruments: Vec<(Uuid, String)> = members
        .iter()
        .filter(|(_, o)| o.type_key == T_INSTRUMENT)
        .filter_map(|(id, o)| Some((*id, o.properties.get("ticker")?.as_str()?.to_string())))
        .collect();
    if instruments.is_empty() {
        return Ok(Vec::new());
    }
    let th = thresholds(db, org).await;
    let rel = releases(
        db,
        org,
        from - th.release_window_minutes * 60,
        to + th.release_window_minutes * 60,
    )
    .await?;
    let mut out = Vec::new();
    for (id, ticker) in instruments {
        let rows = db
            .query_all_raw(stmt(
                "SELECT account_ref, symbol, side, extract(epoch FROM open_at)::bigint AS t, \
                   COALESCE(profit, 0) AS p FROM account_trades \
                 WHERE org_id = $1 AND open_at BETWEEN $2 AND $3 AND known_at <= $4 \
                   AND upper(symbol) LIKE $5 ORDER BY open_at LIMIT 200000",
                vec![
                    org.into(),
                    at(from).into(),
                    at(to).into(),
                    Utc::now().into(),
                    format!("{}%", ticker.to_ascii_uppercase().replace(['%', '_'], "")).into(),
                ],
            ))
            .await?;
        let mut by_account: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        for r in &rows {
            let (Ok(a), Ok(sym), Ok(side), Ok(t), Ok(p)) = (
                r.try_get::<String>("", "account_ref"),
                r.try_get::<String>("", "symbol"),
                r.try_get::<String>("", "side"),
                r.try_get::<i64>("", "t"),
                r.try_get::<f64>("", "p"),
            ) else {
                continue;
            };
            if !acc::symbol_matches(&sym, &ticker) {
                continue;
            }
            by_account.entry(a).or_default().push(Entry {
                open_at: t,
                buy: side == "buy",
                profit: p,
            });
        }
        if by_account.is_empty() {
            continue;
        }
        let px = match store {
            Some(s) => prices(db, s, org, &ticker, from, to + th.window_minutes * 60).await,
            None => Vec::new(),
        };
        for (account_ref, entries) in by_account {
            let figures = acc::entry_figures(&entries, &px, &rel, &th);
            if acc::proposes(&figures, &th) {
                out.push((
                    id,
                    EntryTiming {
                        account_ref,
                        ticker: ticker.clone(),
                        from: entries.first().map_or(from, |e| e.open_at),
                        to: entries.last().map_or(to, |e| e.open_at),
                        figures,
                    },
                ));
            }
        }
    }
    Ok(out)
}

/// What a retained proposal writes: the account object (found by its ref,
/// else created) and its `trades` links; the note gives the figures and
/// attributes no intention. Returns the account's id and the note.
pub async fn retain_entry_timing<C: ConnectionTrait>(
    db: &C,
    org: i64,
    actor: &Actor,
    payload: &Value,
) -> crate::services::ontology::Result<(Uuid, String)> {
    use crate::services::ontology::invalid;
    let p: EntryTiming = serde_json::from_value(payload.clone())
        .map_err(|_| invalid("values.payload", pnex_core::err_codes::FIELD_INVALID))?;
    if !account_ref_ok(&p.account_ref) {
        return Err(invalid(
            "values.payload.account_ref",
            pnex_core::err_codes::FIELD_INVALID,
        ));
    }
    let account = account_object(db, org, actor, &p.account_ref, None, Utc::now()).await?;
    refresh_links(db, org, actor, account, &p.account_ref).await?;
    let f = &p.figures;
    let note = format!(
        "{ENTRY_TIMING}: {} entries on {} between {} and {}; {} followed by a move at or above the \
         threshold ({} the entry's way, profit {:.2}), {} around a release (profit {:.2}). \
         Figures of trades the account published: a proposal, not an accusation; no intention is attributed.",
        f.entries,
        p.ticker,
        at(p.from).to_rfc3339(),
        at(p.to).to_rfc3339(),
        f.before_move,
        f.before_move_with_side,
        f.before_move_profit,
        f.around_release,
        f.around_release_profit,
    );
    Ok((account, note))
}
