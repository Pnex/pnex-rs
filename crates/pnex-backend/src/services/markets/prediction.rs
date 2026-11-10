//! Prediction markets (markets-research.md appendix J, D918–D925): the
//! markets an org follows read from the platform's public APIs, through
//! the org's proxy when it set one (the hosts are blocked in France); their
//! resolution kept as dated states; their probabilities and trades as rows.
//! Reads: the model inputs at an instant (D922), the model's forecasts
//! against the market (D925) and the early bets of a market (D924).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use loco_rs::app::AppContext;
use pnex_core::markets::prediction::{
    self as pm, EarlyBet, EarlyBetThresholds, Forecast, ForecastScores, Market, MarketFeature,
    ResolutionState, StateAt, Trade,
};
use pnex_core::ontology::api::ObjectInput;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::models::_entities::sea_orm_active_enums::OrgMemberRole;
use crate::services::ontology::{links, objects, packs, Actor};

pub const PACK: &str = "prediction_markets";
const T_MARKET: &str = "prediction_market";
const T_RESOLUTION: &str = "prediction_resolution";
const L_BEARS_ON: &str = "bears_on";
/// Vault secret holding the org's http(s) proxy URL.
pub const PROXY_SECRET: &str = "polymarket_proxy";
const MARKETS_MAX: usize = 200;
const REQUESTS_PER_PASS: usize = 60;
const BODY_MAX: usize = 20 * 1024 * 1024;
const PAUSE: Duration = Duration::from_millis(200);
const REFRESH_EVERY_S: i64 = 3_600;
const TRADES_PAGE: usize = 500;
const TRADES_OFFSET_MAX: usize = 5_000;

/// The platform's hosts (mocks in tests).
#[derive(Debug, Clone)]
pub struct Hosts {
    pub gamma: String,
    pub clob: String,
    pub data: String,
}

impl Default for Hosts {
    fn default() -> Self {
        Self {
            gamma: pm::GAMMA_BASE.into(),
            clob: pm::CLOB_BASE.into(),
            data: pm::DATA_BASE.into(),
        }
    }
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn builder() -> reqwest::ClientBuilder {
    pnex_core::egress::guarded(reqwest::Client::builder())
        .redirect(pnex_core::egress::redirect_policy(3))
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("PNEX-Markets/", env!("CARGO_PKG_VERSION")))
}

fn direct() -> reqwest::Client {
    static HTTP: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    HTTP.get_or_init(|| builder().build().expect("reqwest client"))
        .clone()
}

/// The org's client: through its proxy when the vault holds one (an
/// http(s) URL whose host passes the egress guard, R8), else direct. A
/// proxy that cannot be read stops the org's pass: never a silent fallback
/// to a direct call the org did not choose.
async fn client_of(ctx: &AppContext, org: i64) -> Result<reqwest::Client, &'static str> {
    use crate::services::secrets::{store, Keyring};
    let row = store::find_by_name(&ctx.db, Some(org), PROXY_SECRET)
        .await
        .map_err(|_| "proxy_unreadable")?;
    let Some(row) = row else {
        return Ok(direct());
    };
    let ring = Keyring::from_config(&ctx.config).map_err(|_| "proxy_unreadable")?;
    let raw = store::reveal(&ctx.db, &ring, Some(org), row.id)
        .await
        .map_err(|_| "proxy_unreadable")?;
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| "proxy_invalid")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("proxy_invalid");
    }
    if let Some(reason) = pnex_core::egress::check_url(&url) {
        if !super::test_host_allowed(url.as_str()) {
            return Err(reason);
        }
    }
    let proxy = reqwest::Proxy::all(url).map_err(|_| "proxy_invalid")?;
    builder().proxy(proxy).build().map_err(|_| "proxy_invalid")
}

/// A JSON answer, `None` when the host did not answer well.
async fn get_json(client: &reqwest::Client, url: &str) -> Option<Value> {
    let res = client.get(url).send().await.ok()?;
    if !res.status().is_success() {
        tracing::info!(
            status = res.status().as_u16(),
            "prediction market source answered"
        );
        return None;
    }
    let body = res.bytes().await.ok()?;
    if body.len() > BODY_MAX {
        return None;
    }
    serde_json::from_slice(&body).ok()
}

async fn setting(db: &DatabaseConnection, org: i64, key: &str) -> Result<Option<Value>, DbErr> {
    packs::setting_value(db, org, &format!("{PACK}.{key}")).await
}

async fn text_setting(db: &DatabaseConnection, org: i64, key: &str) -> Result<String, DbErr> {
    Ok(setting(db, org, key)
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default())
}

async fn number_setting(db: &DatabaseConnection, org: i64, key: &str, default: f64) -> f64 {
    setting(db, org, key)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_f64())
        .unwrap_or(default)
}

/// Market ids of a setting: condition ids (lowercase) and slugs, checked.
fn ids_of(raw: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    raw.split(',')
        .map(|s| s.trim().to_string())
        .map(|s| match s.starts_with("0x") || s.starts_with("0X") {
            true => s.to_ascii_lowercase(),
            false => s,
        })
        .filter(|s| (pm::condition_ok(s) || pm::slug_ok(s)) && seen.insert(s.clone()))
        .take(MARKETS_MAX)
        .collect()
}

async fn cursor(db: &DatabaseConnection, org: i64, key: &str) -> Result<Option<i64>, DbErr> {
    Ok(db
        .query_one_raw(stmt(
            "SELECT last_ts FROM prediction_cursors WHERE org_id = $1 AND key = $2",
            vec![org.into(), key.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<i64>("", "last_ts").ok()))
}

async fn set_cursor(db: &DatabaseConnection, org: i64, key: &str, ts: i64) -> Result<(), DbErr> {
    db.execute_raw(stmt(
        "INSERT INTO prediction_cursors (org_id, key, last_ts) VALUES ($1, $2, $3) \
         ON CONFLICT (org_id, key) DO UPDATE SET last_ts = EXCLUDED.last_ts, updated_at = now()",
        vec![org.into(), key.into(), ts.into()],
    ))
    .await?;
    Ok(())
}

fn actor(org: i64) -> Actor {
    Actor {
        user_id: None,
        role: OrgMemberRole::Member,
        source_ref: format!("import:{PACK}:{org}"),
    }
}

fn rfc3339(t: i64) -> String {
    DateTime::from_timestamp(t, 0)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// One pass over the orgs that installed the pack and follow markets.
pub async fn sweep(ctx: &AppContext, hosts: &Hosts, now: DateTime<Utc>) -> Result<(), DbErr> {
    let orgs: Vec<i64> = ctx
        .db
        .query_all_raw(stmt(
            "SELECT org_id FROM packs WHERE key = $1 \
               AND coalesce(settings ->> 'market_ids', '') <> '' ORDER BY org_id",
            vec![PACK.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get("", "org_id").ok())
        .collect();
    for org in orgs {
        let client = match client_of(ctx, org).await {
            Ok(c) => c,
            Err(code) => {
                tracing::warn!(org, code, "prediction markets pass skipped");
                continue;
            }
        };
        if let Err(e) = pass(&ctx.db, &client, hosts, org, now.timestamp()).await {
            tracing::warn!(org, "prediction markets pass stopped: {e}");
        }
    }
    Ok(())
}

/// One org: markets and resolutions (hourly), prices and trades.
pub async fn pass(
    db: &DatabaseConnection,
    client: &reqwest::Client,
    hosts: &Hosts,
    org: i64,
    now: i64,
) -> Result<(), DbErr> {
    let ids = ids_of(&text_setting(db, org, "market_ids").await?);
    if ids.is_empty() {
        return Ok(());
    }
    let mut budget = REQUESTS_PER_PASS;
    let refreshed = cursor(db, org, "m:refresh").await?;
    if refreshed.is_none_or(|t| now - t >= REFRESH_EVERY_S) {
        for id in &ids {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let q = match pm::condition_ok(id) {
                true => format!("condition_ids={id}"),
                false => format!("slug={id}"),
            };
            let Some(answer) = get_json(client, &format!("{}/markets?{q}", hosts.gamma)).await
            else {
                continue;
            };
            for m in pm::markets(&answer) {
                if let Err(e) = record_market(db, org, &m, now).await {
                    tracing::warn!(org, "prediction market not recorded: {e:?}");
                }
            }
            tokio::time::sleep(PAUSE).await;
        }
        set_cursor(db, org, "m:refresh", now).await?;
    }
    let followed = followed(db, org, &ids).await?;
    let fine: HashSet<String> = ids_of(&text_setting(db, org, "fine_markets").await?)
        .into_iter()
        .collect();
    let days = number_setting(db, org, "history_days", 90.0)
        .await
        .clamp(1.0, 730.0);
    let first = now - (days * 86_400.0) as i64;
    for m in &followed {
        let mut fidelities = vec![pm::FIDELITY_HOURLY];
        if fine.contains(&m.condition_id) || fine.contains(&m.slug) {
            fidelities.push(pm::FIDELITY_FINE);
        }
        for (token, outcome) in m.tokens.iter().zip(&m.outcomes) {
            for f in &fidelities {
                // A fine series starts a week back at most.
                let start = match f {
                    1 => first.max(now - 7 * 86_400),
                    _ => first,
                };
                prices(
                    db,
                    client,
                    hosts,
                    org,
                    m,
                    token,
                    outcome,
                    *f,
                    start,
                    now,
                    &mut budget,
                )
                .await?;
            }
        }
        trades(db, client, hosts, org, m, first, &mut budget).await?;
    }
    Ok(())
}

/// The followed markets as recorded, from the setting's ids.
async fn followed(db: &DatabaseConnection, org: i64, ids: &[String]) -> Result<Vec<Market>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT properties FROM objects WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL \
               AND (properties ->> 'condition_id' = ANY($3) OR properties ->> 'slug' = ANY($3))",
            vec![org.into(), T_MARKET.into(), ids.to_vec().into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<Value>("", "properties").ok())
        .map(|p| market_of(&p))
        .collect())
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

/// Creates or updates the market's object; a new resolution state becomes
/// a dated `prediction_resolution` (D920).
async fn record_market(
    db: &DatabaseConnection,
    org: i64,
    m: &Market,
    now: i64,
) -> Result<(), crate::services::ontology::OntologyError> {
    let existing =
        objects::find_by_key(db, org, T_MARKET, "condition_id", &json!(m.condition_id)).await?;
    let status = match (m.state, m.closed) {
        (Some(ResolutionState::Final | ResolutionState::Indeterminate), _) => "resolved",
        (_, true) => "closed",
        _ => "open",
    };
    let known_at = existing
        .as_ref()
        .and_then(|o| o.properties.get("known_at").cloned())
        .unwrap_or_else(|| json!(rfc3339(now)));
    let mut props = Map::new();
    props.insert("condition_id".into(), json!(m.condition_id));
    props.insert("slug".into(), json!(clip(&m.slug, 200)));
    props.insert("platform".into(), json!("polymarket"));
    props.insert("rules".into(), json!(clip(&m.rules, 8000)));
    props.insert(
        "resolution_source".into(),
        json!(clip(&m.resolution_source, 1000)),
    );
    if let Some(e) = m.end_at {
        props.insert("end_at".into(), json!(rfc3339(e)));
    }
    props.insert("outcomes".into(), json!(clip(&m.outcomes.join(","), 500)));
    props.insert("tokens".into(), json!(clip(&m.tokens.join(","), 2000)));
    if let Some(v) = m.volume.filter(|v| *v >= 0.0) {
        props.insert("volume".into(), json!(v));
    }
    props.insert("status".into(), json!(status));
    props.insert("known_at".into(), known_at);
    let title = clip(
        if m.question.is_empty() {
            &m.condition_id
        } else {
            &m.question
        },
        pnex_core::ontology::api::TITLE_MAX,
    );
    let a = actor(org);
    let id = match existing {
        Some(o) if o.properties.as_object() == Some(&props) && o.title == title => o.id,
        Some(o) => {
            let input = ObjectInput {
                type_key: T_MARKET.into(),
                title,
                properties: props,
                expected_version: Some(o.version),
            };
            objects::update(db, org, &a, o.id, &input).await?;
            o.id
        }
        None => {
            let input = ObjectInput {
                type_key: T_MARKET.into(),
                title,
                properties: props,
                expected_version: None,
            };
            let v = objects::create(db, org, &a, &input).await?;
            Uuid::parse_str(&v.id).unwrap_or_default()
        }
    };
    let Some(state) = m.state else {
        return Ok(());
    };
    let last = states_of(db, org, id)
        .await?
        .into_iter()
        .max_by_key(|s| s.known_at);
    let outcome = match state {
        ResolutionState::Final | ResolutionState::Proposed => m.winner.clone(),
        _ => None,
    };
    if last.is_some_and(|l| l.state == state && l.outcome == outcome) {
        return Ok(());
    }
    let mut props = Map::new();
    props.insert(
        "ref".into(),
        json!(format!("{}:{}:{now}", m.condition_id, state.as_str())),
    );
    props.insert("market".into(), json!(id.to_string()));
    props.insert("state".into(), json!(state.as_str()));
    if let Some(o) = &outcome {
        props.insert("outcome".into(), json!(clip(o, 200)));
    }
    props.insert("known_at".into(), json!(rfc3339(now)));
    let input = ObjectInput {
        type_key: T_RESOLUTION.into(),
        title: clip(&format!("{}: {}", state.as_str(), m.question), 200),
        properties: props,
        expected_version: None,
    };
    objects::create(db, org, &a, &input).await?;
    Ok(())
}

/// Every recorded resolution state of a market.
async fn states_of<C: ConnectionTrait>(
    db: &C,
    org: i64,
    market: Uuid,
) -> Result<Vec<StateAt>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT properties FROM objects WHERE org_id = $1 AND type_key = $2 \
               AND properties ->> 'market' = $3",
            vec![org.into(), T_RESOLUTION.into(), market.to_string().into()],
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

#[allow(clippy::too_many_arguments)]
async fn prices(
    db: &DatabaseConnection,
    client: &reqwest::Client,
    hosts: &Hosts,
    org: i64,
    m: &Market,
    token: &str,
    outcome: &str,
    fidelity: u32,
    first: i64,
    now: i64,
    budget: &mut usize,
) -> Result<(), DbErr> {
    if !token.bytes().all(|b| b.is_ascii_digit()) || token.is_empty() {
        return Ok(());
    }
    let key = format!("p:{token}:{fidelity}");
    let span = match fidelity {
        1 => 86_400,
        _ => 7 * 86_400,
    };
    let mut start = cursor(db, org, &key).await?.map_or(first, |t| t + 1);
    while start < now && *budget > 0 {
        *budget -= 1;
        let end = (start + span).min(now);
        let url = format!(
            "{}/prices-history?market={token}&startTs={start}&endTs={end}&fidelity={fidelity}",
            hosts.clob
        );
        let Some(answer) = get_json(client, &url).await else {
            return Ok(());
        };
        let points = pm::price_history(&answer);
        insert_prices(db, org, &m.condition_id, token, outcome, fidelity, &points).await?;
        // A window with no point is passed only once it is an hour old: a
        // late point of the last hour is asked again.
        let next = match points.last() {
            Some((t, _)) => *t,
            None if now - end > 3_600 => end,
            None => break,
        };
        set_cursor(db, org, &key, next).await?;
        if end == now {
            break;
        }
        start = next + 1;
        tokio::time::sleep(PAUSE).await;
    }
    Ok(())
}

/// Price rows; a price is known at its instant (public live data, as the
/// bars, H.8).
pub async fn insert_prices<C: ConnectionTrait>(
    db: &C,
    org: i64,
    condition_id: &str,
    token: &str,
    outcome: &str,
    fidelity: u32,
    points: &[(i64, f64)],
) -> Result<(), DbErr> {
    for chunk in points.chunks(500) {
        let mut sql = String::from(
            "INSERT INTO prediction_prices (org_id, condition_id, token, outcome, fidelity, ts, price, known_at) VALUES ",
        );
        let mut values: Vec<sea_orm::Value> = vec![
            org.into(),
            condition_id.into(),
            token.into(),
            outcome.into(),
            (fidelity as i32).into(),
        ];
        for (i, (t, p)) in chunk.iter().enumerate() {
            let at = DateTime::from_timestamp(*t, 0).unwrap_or_default();
            values.push(at.into());
            values.push((*p).into());
            let (a, b) = (6 + 2 * i, 7 + 2 * i);
            sql.push_str(&format!(
                "{}($1, $2, $3, $4, $5, ${a}, ${b}, ${a})",
                if i == 0 { "" } else { ", " }
            ));
        }
        sql.push_str(" ON CONFLICT DO NOTHING");
        db.execute_raw(stmt(&sql, values)).await?;
    }
    Ok(())
}

async fn trades(
    db: &DatabaseConnection,
    client: &reqwest::Client,
    hosts: &Hosts,
    org: i64,
    m: &Market,
    first: i64,
    budget: &mut usize,
) -> Result<(), DbErr> {
    let key = format!("t:{}", m.condition_id);
    let since = cursor(db, org, &key).await?.unwrap_or(first);
    let mut newest = since;
    let mut offset = 0;
    // Newest first: read back until the last trade already kept.
    while offset <= TRADES_OFFSET_MAX && *budget > 0 {
        *budget -= 1;
        let url = format!(
            "{}/trades?market={}&limit={TRADES_PAGE}&offset={offset}",
            hosts.data, m.condition_id
        );
        let Some(answer) = get_json(client, &url).await else {
            return Ok(());
        };
        let page = pm::trades(&answer);
        let fresh: Vec<Trade> = page.iter().filter(|t| t.ts > since).cloned().collect();
        newest = fresh
            .iter()
            .map(|t| t.ts)
            .max()
            .unwrap_or(newest)
            .max(newest);
        insert_trades(db, org, &fresh).await?;
        if page.len() < TRADES_PAGE || fresh.len() < page.len() {
            break;
        }
        offset += TRADES_PAGE;
        tokio::time::sleep(PAUSE).await;
    }
    if newest > since {
        set_cursor(db, org, &key, newest).await?;
    }
    Ok(())
}

pub async fn insert_trades<C: ConnectionTrait>(
    db: &C,
    org: i64,
    rows: &[Trade],
) -> Result<(), DbErr> {
    for chunk in rows.chunks(300) {
        let mut sql = String::from(
            "INSERT INTO prediction_trades (org_id, condition_id, token, outcome, wallet, side, size, price, ts, tx) VALUES ",
        );
        let mut values: Vec<sea_orm::Value> = vec![org.into()];
        for (i, t) in chunk.iter().enumerate() {
            let at = DateTime::from_timestamp(t.ts, 0).unwrap_or_default();
            let base = values.len();
            values.extend([
                t.condition_id.clone().into(),
                t.token.clone().into(),
                t.outcome.clone().into(),
                t.wallet.clone().into(),
                t.side.clone().into(),
                t.size.into(),
                t.price.into(),
                at.into(),
                t.tx.clone().into(),
            ]);
            let p: Vec<String> = (1..=9).map(|k| format!("${}", base + k)).collect();
            sql.push_str(&format!(
                "{}($1, {})",
                if i == 0 { "" } else { ", " },
                p.join(", ")
            ));
        }
        sql.push_str(" ON CONFLICT DO NOTHING");
        db.execute_raw(stmt(&sql, values)).await?;
    }
    Ok(())
}

/// Price points of a token known at `as_of`, sorted.
async fn points_of<C: ConnectionTrait>(
    db: &C,
    org: i64,
    token: &str,
    from: i64,
    as_of: i64,
) -> Result<Vec<(i64, f64)>, DbErr> {
    let at = |t: i64| DateTime::from_timestamp(t, 0).unwrap_or_default();
    let rows = db
        .query_all_raw(stmt(
            "SELECT DISTINCT ON (ts) extract(epoch FROM ts)::bigint AS t, price FROM prediction_prices \
             WHERE org_id = $1 AND token = $2 AND ts >= $3 AND ts <= $4 AND known_at <= $4 \
             ORDER BY ts, fidelity",
            vec![org.into(), token.into(), at(from).into(), at(as_of).into()],
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| Some((r.try_get("", "t").ok()?, r.try_get("", "price").ok()?)))
        .collect())
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
        if pm::resolution_at(&states_of(db, org, id).await?, t) != pm::Resolution::Unresolved {
            continue;
        }
        let (Some(token), Some(outcome)) = (m.tokens.first(), m.outcomes.first()) else {
            continue;
        };
        let points = points_of(db, org, token, t - 3 * 86_400, t).await?;
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
                            match pm::resolution_at(&states_of(db, org, o.id).await?, i64::MAX) {
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
            let points = points_of(db, org, token, t - 2 * 86_400, t).await?;
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

async fn trades_where<C: ConnectionTrait>(
    db: &C,
    filter: &str,
    values: Vec<sea_orm::Value>,
) -> Result<Vec<Trade>, DbErr> {
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT condition_id, token, outcome, wallet, side, size, price, \
                   extract(epoch FROM ts)::bigint AS t, tx FROM prediction_trades WHERE {filter} LIMIT 200000"
            ),
            values,
        ))
        .await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(Trade {
                condition_id: r.try_get("", "condition_id").ok()?,
                token: r.try_get("", "token").ok()?,
                outcome: r.try_get("", "outcome").ok()?,
                wallet: r.try_get("", "wallet").ok()?,
                side: r.try_get("", "side").ok()?,
                size: r.try_get("", "size").ok()?,
                price: r.try_get("", "price").ok()?,
                ts: r.try_get("", "t").ok()?,
                tx: r.try_get("", "tx").ok()?,
            })
        })
        .collect())
}

/// Early bets on a market whose trigger (a jump of its first outcome, or
/// its final resolution) falls in `[from, to]` (D924).
pub async fn early_bets(
    db: &DatabaseConnection,
    org: i64,
    market: Uuid,
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
        let points = points_of(db, org, token, from - 3_600, to).await?;
        triggers.extend(
            pm::jumps(&points, th.jump_points)
                .into_iter()
                .filter(|t| *t >= from)
                .map(|t| (t, "jump")),
        );
    }
    triggers.extend(
        states_of(db, org, market)
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
    let at = |t: i64| DateTime::from_timestamp(t, 0).unwrap_or_default();
    let here = trades_where(
        db,
        "org_id = $1 AND condition_id = $2 AND ts >= $3 AND ts <= $4",
        vec![
            org.into(),
            m.condition_id.clone().into(),
            at(from - window).into(),
            at(to).into(),
        ],
    )
    .await?;
    let wallets: Vec<String> = here
        .iter()
        .map(|t| t.wallet.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let all = trades_where(
        db,
        "org_id = $1 AND wallet = ANY($2)",
        vec![org.into(), wallets.into()],
    )
    .await?;
    Ok(pm::early_bets(&here, &all, &triggers, &th))
}

/// Detector name of an early bet in a case (D924).
pub const EARLY_BET: &str = "early_bet@1";

/// An early bet found on a market member of a case: (market, bet).
pub async fn case_detections(
    db: &DatabaseConnection,
    org: i64,
    members: &HashMap<Uuid, crate::models::_entities::objects::Model>,
    from: i64,
    to: i64,
) -> Result<Vec<(Uuid, EarlyBet)>, DbErr> {
    let mut out = Vec::new();
    for (id, o) in members.iter().filter(|(_, o)| o.type_key == T_MARKET) {
        for b in early_bets(db, org, *id, &o.properties, from, to).await? {
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
