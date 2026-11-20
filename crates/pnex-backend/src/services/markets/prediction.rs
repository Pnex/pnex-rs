//! Prediction markets (markets-research.md §3.15, D918–D925): reads
//! over what the pack's pipelines collect (pipelines.md D946): the
//! markets and their dated resolution states as objects, their prices and
//! trades in the org's tables `prediction_prices` and `prediction_trades`.
//! Reads: the model inputs at an instant (D922), the external model's
//! forecasts against the market (D925) and the early bets of a market (D924).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use pnex_core::markets::prediction::{
    self as pm, EarlyBet, EarlyBetThresholds, Forecast, ForecastScores, Market, MarketFeature,
    ResolutionState, StateAt, Trade,
};
use pnex_core::ontology::api::ObjectInput;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::services::artifact_store::S3Store;
use crate::services::datasets;
use crate::services::ontology::{links, objects, packs, Actor};

pub const PACK: &str = "prediction_markets";
const T_MARKET: &str = "prediction_market";
const T_RESOLUTION: &str = "prediction_resolution";
const L_BEARS_ON: &str = "bears_on";
/// Tables the pack's pipelines write.
const PRICES: &str = "prediction_prices";
const TRADES: &str = "prediction_trades";
/// Most trades one read keeps.
const TRADES_MAX: usize = 200_000;

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

async fn setting(db: &DatabaseConnection, org: i64, key: &str) -> Result<Option<Value>, DbErr> {
    packs::setting_value(db, org, &format!("{PACK}.{key}")).await
}

async fn number_setting(db: &DatabaseConnection, org: i64, key: &str, default: f64) -> f64 {
    setting(db, org, key)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_f64())
        .unwrap_or(default)
}

fn rfc3339(t: i64) -> String {
    DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn split(p: &Value, k: &str) -> Vec<String> {
    p.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn market_of(p: &Value) -> Market {
    let text = |k: &str| {
        p.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Market {
        condition_id: text("condition_id"),
        slug: text("slug"),
        outcomes: split(p, "outcomes"),
        tokens: split(p, "tokens"),
        end_at: pm::instant(&text("end_at")),
        ..Default::default()
    }
}

/// Every recorded resolution state of a market.
async fn states_of<C: ConnectionTrait>(
    db: &C,
    org: i64,
    condition_id: &str,
) -> Result<Vec<StateAt>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT properties FROM objects WHERE org_id = $1 AND type_key = $2 \
               AND valid_to IS NULL AND properties ->> 'condition_id' = $3",
            vec![org.into(), T_RESOLUTION.into(), condition_id.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<Value>("", "properties").ok())
        .filter_map(|p| {
            Some(StateAt {
                known_at: pm::instant(p.get("known_at")?.as_str()?)?,
                state: ResolutionState::parse(p.get("state")?.as_str()?)?,
                outcome: p.get("outcome").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect())
}

fn storage(e: impl std::fmt::Display) -> DbErr {
    DbErr::Custom(format!("prediction table not read: {e}"))
}

/// Rows of an org table whose `ts` falls in `[from, to]` (seconds), as
/// records; none when the table was never written.
async fn rows_between(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    table: &str,
    columns: &[&str],
    from: i64,
    to: i64,
) -> Result<Vec<Map<String, Value>>, DbErr> {
    let Some(r) = db
        .query_one_raw(stmt(
            "SELECT id FROM datasets WHERE org_id = $1 AND name = $2",
            vec![org.into(), table.into()],
        ))
        .await?
    else {
        return Ok(Vec::new());
    };
    let id: Uuid = r.try_get("", "id")?;
    let files = datasets::view_files(db, org, id, None)
        .await
        .map_err(|e| storage(format!("{e:?}")))?;
    let (from_us, to_us) = (from.saturating_mul(1_000_000), to.saturating_mul(1_000_000));
    let mut out = Vec::new();
    for f in files {
        let batches = datasets::parquet::read_time_range(
            store, &f.key, f.bytes, columns, "ts", from_us, to_us,
        )
        .await
        .map_err(storage)?;
        for b in batches {
            for v in datasets::parquet::records(&b).map_err(storage)? {
                if let Value::Object(m) = v {
                    out.push(m);
                }
            }
        }
    }
    Ok(out)
}

/// Seconds of a `ts` read back from a table (`2026-11-16T00:00:00`, UTC).
fn seconds(v: Option<&Value>) -> Option<i64> {
    let s = v?.as_str()?.trim_end_matches('Z');
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|t| t.and_utc().timestamp())
}

fn float(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Price points of a token in `[from, as_of]`, sorted, the finest reading
/// of an instant kept. A price is public at its instant (markets-research.md §3.12).
async fn points_of(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    token: &str,
    from: i64,
    as_of: i64,
) -> Result<Vec<(i64, f64)>, DbErr> {
    let cols = ["token", "fidelity", "ts", "price"];
    let mut points: Vec<(i64, i64, f64)> = rows_between(db, store, org, PRICES, &cols, from, as_of)
        .await?
        .iter()
        .filter(|r| r.get("token").and_then(Value::as_str) == Some(token))
        .filter_map(|r| {
            let t = seconds(r.get("ts"))?;
            let fidelity = float(r.get("fidelity")).unwrap_or(60.0) as i64;
            Some((t, fidelity, float(r.get("price"))?))
        })
        .filter(|(t, _, p)| (from..=as_of).contains(t) && (0.0..=1.0).contains(p))
        .collect();
    points.sort_by_key(|(t, f, _)| (*t, *f));
    points.dedup_by_key(|(t, _, _)| *t);
    Ok(points.into_iter().map(|(t, _, p)| (t, p)).collect())
}

/// Trades in `[from, to]` the filter keeps.
async fn trades_where(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    from: i64,
    to: i64,
    keep: impl Fn(&Trade) -> bool,
) -> Result<Vec<Trade>, DbErr> {
    let cols = [
        "condition_id",
        "token",
        "outcome",
        "wallet",
        "side",
        "size",
        "price",
        "ts",
        "tx",
    ];
    let text = |r: &Map<String, Value>, k: &str| {
        r.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in rows_between(db, store, org, TRADES, &cols, from, to).await? {
        let (Some(ts), Some(size), Some(price)) = (
            seconds(r.get("ts")),
            float(r.get("size")),
            float(r.get("price")),
        ) else {
            continue;
        };
        let t = Trade {
            condition_id: text(&r, "condition_id"),
            token: text(&r, "token"),
            outcome: text(&r, "outcome"),
            wallet: text(&r, "wallet"),
            side: text(&r, "side"),
            size,
            price,
            ts,
            tx: text(&r, "tx"),
        };
        let key = (
            t.tx.clone(),
            t.wallet.clone(),
            t.token.clone(),
            t.side.clone(),
            t.ts,
        );
        if (from..=to).contains(&ts) && keep(&t) && seen.insert(key) {
            out.push(t);
            if out.len() >= TRADES_MAX {
                break;
            }
        }
    }
    Ok(out)
}

/// Live instrument objects of a ticker.
pub async fn instruments(
    db: &DatabaseConnection,
    org: i64,
    ticker: &str,
) -> Result<Vec<Uuid>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT id FROM objects WHERE org_id = $1 AND type_key = 'instrument' \
               AND valid_to IS NULL AND properties ->> 'ticker' = $2",
            vec![org.into(), ticker.into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get("", "id").ok())
        .collect())
}

/// The markets a member said bear on these objects (instruments), as known
/// at `as_of` (D913), unresolved at `as_of` (D920), with their inputs
/// (D922). A market not yet read at `as_of` is absent.
pub async fn features(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    targets: &[Uuid],
    as_of: DateTime<Utc>,
) -> Result<Vec<MarketFeature>, DbErr> {
    use crate::models::_entities::resource_edges;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    let at = as_of.fixed_offset();
    let edges = resource_edges::Entity::find()
        .filter(resource_edges::Column::OrgId.eq(org))
        .filter(resource_edges::Column::Relation.eq(L_BEARS_ON))
        .filter(resource_edges::Column::TargetObjectId.is_in(targets.to_vec()))
        .filter(links::known_at(at, at))
        .all(db)
        .await?;
    let (edges, _) = links::as_known(db, edges, at).await?;
    let t = as_of.timestamp();
    let mut out = Vec::new();
    for e in edges {
        let Some(id) = e.source_object_id else {
            continue;
        };
        // The market as known then: absent when read later (D915).
        let Ok(o) = objects::get_as_of(db, org, id, at).await else {
            continue;
        };
        let props = Value::Object(o.properties.clone());
        let m = market_of(&props);
        let states = states_of(db, org, &m.condition_id).await?;
        if pm::resolution_at(&states, t) != pm::Resolution::Unresolved {
            continue;
        }
        let (Some(token), Some(outcome)) = (m.tokens.first(), m.outcomes.first()) else {
            continue;
        };
        let points = points_of(db, store, org, token, t - 3 * 86_400, t).await?;
        let Some(f) = pm::features(&points, t, m.end_at) else {
            continue;
        };
        out.push(MarketFeature {
            market_id: id.to_string(),
            condition_id: m.condition_id,
            question: o.title.clone(),
            outcome: outcome.clone(),
            direction: e
                .attributes
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            features: f,
        });
    }
    out.sort_by(|a, b| a.condition_id.cmp(&b.condition_id));
    Ok(out)
}

/// The forecasts of each model against the market price at the decision's
/// instant, on the markets resolved final as known now (D925).
pub async fn forecast_scores(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    model: Option<&str>,
    model_version: Option<&str>,
) -> Result<Vec<ForecastScores>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT model, model_version, extract(epoch FROM as_of)::bigint AS t, forecasts \
             FROM market_decisions WHERE org_id = $1 AND forecasts IS NOT NULL \
               AND ($2::text IS NULL OR model = $2) AND ($3::text IS NULL OR model_version = $3) \
             ORDER BY model, model_version, as_of LIMIT 20000",
            vec![
                org.into(),
                model.map(str::to_string).into(),
                model_version.map(str::to_string).into(),
            ],
        ))
        .await?;
    // Market → (id, outcomes, tokens, final outcome now).
    let mut markets: HashMap<String, Option<(Vec<String>, Vec<String>, Option<String>)>> =
        HashMap::new();
    let mut groups: Vec<(String, String, u64, Vec<(f64, bool)>, Vec<(f64, bool)>)> = Vec::new();
    for r in &rows {
        let (Ok(model), Ok(version), Ok(t)) = (
            r.try_get::<String>("", "model"),
            r.try_get::<String>("", "model_version"),
            r.try_get::<i64>("", "t"),
        ) else {
            continue;
        };
        let forecasts: Vec<Forecast> = r
            .try_get::<Value>("", "forecasts")
            .ok()
            .and_then(|f| serde_json::from_value(f).ok())
            .unwrap_or_default();
        if groups.last().is_none_or(|g| g.0 != model || g.1 != version) {
            groups.push((model, version, 0, Vec::new(), Vec::new()));
        }
        let Some(g) = groups.last_mut() else { continue };
        for f in forecasts {
            g.2 += 1;
            let cid = f.market.to_ascii_lowercase();
            if !markets.contains_key(&cid) {
                let found = objects::find_by_key(db, org, T_MARKET, "condition_id", &json!(cid))
                    .await
                    .ok()
                    .flatten();
                let entry = match found {
                    Some(o) => {
                        let m = market_of(&o.properties);
                        let winner =
                            match pm::resolution_at(&states_of(db, org, &cid).await?, i64::MAX) {
                                pm::Resolution::Final(w) => Some(w),
                                _ => None,
                            };
                        Some((m.outcomes, m.tokens, winner))
                    }
                    None => None,
                };
                markets.insert(cid.clone(), entry);
            }
            let Some(Some((outcomes, tokens, Some(winner)))) = markets.get(&cid) else {
                continue;
            };
            let Some(token) = outcomes
                .iter()
                .position(|o| o.eq_ignore_ascii_case(&f.outcome))
                .and_then(|i| tokens.get(i))
            else {
                continue;
            };
            let points = points_of(db, store, org, token, t - 2 * 86_400, t).await?;
            let Some(market_p) = pm::features(&points, t, None).map(|x| x.probability) else {
                continue;
            };
            let happened = winner.eq_ignore_ascii_case(&f.outcome);
            g.3.push((f.probability, happened));
            g.4.push((market_p, happened));
        }
    }
    Ok(groups
        .into_iter()
        .take(100)
        .map(|(model, model_version, n, mine, market)| ForecastScores {
            model,
            model_version,
            forecasts: n,
            scored: mine.len() as u64,
            brier_model: pm::brier(&mine),
            brier_market: pm::brier(&market),
            calibration_model: pm::calibration(&mine),
            calibration_market: pm::calibration(&market),
        })
        .collect())
}

/// The org's early bet thresholds (D924).
pub async fn thresholds(db: &DatabaseConnection, org: i64) -> EarlyBetThresholds {
    let d = EarlyBetThresholds::default();
    EarlyBetThresholds {
        wallet_days: number_setting(db, org, "early_bet_wallet_days", d.wallet_days).await,
        share_pct: number_setting(db, org, "early_bet_share_pct", d.share_pct).await,
        min_usdc: number_setting(db, org, "early_bet_min_usdc", d.min_usdc).await,
        window_hours: number_setting(db, org, "early_bet_window_hours", d.window_hours).await,
        jump_points: number_setting(db, org, "early_bet_jump_points", d.jump_points).await,
    }
}

/// Early bets on a market whose trigger (a jump of its first outcome, or
/// its final resolution) falls in `[from, to]` (D924).
pub async fn early_bets(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    properties: &Value,
    from: i64,
    to: i64,
) -> Result<Vec<EarlyBet>, DbErr> {
    let m = market_of(properties);
    if !pm::condition_ok(&m.condition_id) {
        return Ok(Vec::new());
    }
    let th = thresholds(db, org).await;
    let mut triggers: Vec<(i64, &str)> = Vec::new();
    if let Some(token) = m.tokens.first() {
        let points = points_of(db, store, org, token, from - 3_600, to).await?;
        triggers.extend(
            pm::jumps(&points, th.jump_points)
                .into_iter()
                .filter(|t| *t >= from)
                .map(|t| (t, "jump")),
        );
    }
    triggers.extend(
        states_of(db, org, &m.condition_id)
            .await?
            .iter()
            .filter(|s| s.state == ResolutionState::Final && (from..=to).contains(&s.known_at))
            .map(|s| (s.known_at, "resolution")),
    );
    triggers.sort_unstable();
    if triggers.is_empty() {
        return Ok(Vec::new());
    }
    let window = (th.window_hours * 3_600.0) as i64;
    let here = trades_where(db, store, org, from - window, to, |t| {
        t.condition_id == m.condition_id
    })
    .await?;
    let wallets: HashSet<String> = here.iter().map(|t| t.wallet.clone()).collect();
    // Every collected trade of these wallets: their age and their share.
    let all = trades_where(db, store, org, 0, i64::MAX / 1_000_000, |t| {
        wallets.contains(&t.wallet)
    })
    .await?;
    Ok(pm::early_bets(&here, &all, &triggers, &th))
}

/// Detector name of an early bet in a case (D924).
pub const EARLY_BET: &str = "early_bet@1";

/// An early bet found on a market member of a case: (market, bet).
pub async fn case_detections(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    members: &HashMap<Uuid, crate::models::_entities::objects::Model>,
    from: i64,
    to: i64,
) -> Result<Vec<(Uuid, EarlyBet)>, DbErr> {
    let mut out = Vec::new();
    for (id, o) in members.iter().filter(|(_, o)| o.type_key == T_MARKET) {
        for b in early_bets(db, store, org, &o.properties, from, to).await? {
            out.push((*id, b));
        }
    }
    Ok(out)
}

/// What a retained early bet writes (D923, D924): the wallet becomes an
/// object (found again by its address), linked to the market it bet on;
/// the note gives the figures that raised it and attributes no intention.
/// Returns the wallet's id and the note.
pub async fn retain_early_bet<C: ConnectionTrait>(
    db: &C,
    org: i64,
    actor: &Actor,
    market: Uuid,
    payload: &Value,
) -> crate::services::ontology::Result<(Uuid, String)> {
    use crate::services::ontology::{invalid, OntologyError};
    let wallet = payload
        .get("wallet")
        .and_then(Value::as_str)
        .filter(|w| pm::wallet_ok(w))
        .ok_or_else(|| invalid("values.payload.wallet", pnex_core::err_codes::FIELD_INVALID))?
        .to_string();
    let num = |k: &str| payload.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let at = |k: &str| rfc3339(payload.get(k).and_then(Value::as_i64).unwrap_or(0));
    let id = match objects::find_by_key(db, org, "wallet", "address", &json!(wallet)).await? {
        Some(o) => o.id,
        None => {
            let mut props = Map::new();
            props.insert("address".into(), json!(wallet));
            props.insert("first_seen_at".into(), json!(at("first_seen")));
            let input = ObjectInput {
                type_key: "wallet".into(),
                title: format!("{}…{}", &wallet[..8], &wallet[38..]),
                properties: props,
                expected_version: None,
            };
            let v = objects::create(db, org, actor, &input).await?;
            Uuid::parse_str(&v.id).unwrap_or_default()
        }
    };
    let link = pnex_core::ontology::api::LinkInput {
        link_type: "traded".into(),
        source_id: id.to_string(),
        target_id: market.to_string(),
        attributes: json!({ "stake_usdc": num("stake_usdc") })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        ..Default::default()
    };
    match links::create(db, org, actor, &link).await {
        Ok(_) | Err(OntologyError::KeyTaken) => {}
        Err(e) => return Err(e),
    }
    let note = format!(
        "{EARLY_BET}: wallet first seen {}, {:.1} % of its collected volume on this market, \
         {:.0} USDC bought between {} and {}, before a {} at {}. \
         Figures of public trades: a proposal, not an accusation; no intention is attributed.",
        at("first_seen"),
        num("share_pct"),
        num("stake_usdc"),
        at("first_bet"),
        at("last_bet"),
        payload
            .get("trigger")
            .and_then(Value::as_str)
            .unwrap_or("trigger"),
        at("trigger_at"),
    );
    Ok((id, note))
}
