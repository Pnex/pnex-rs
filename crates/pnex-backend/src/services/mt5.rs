//! MetaTrader 5 terminals as registered data sources (markets-research.md
//! appendix E, D847–D849). A terminal is not a device: it borrows only the
//! device journey (a record made in the UI, generated code carrying its own
//! token, an announced catalogue, a configuration decided by the server, a
//! presence). Its calls wait in `mt5_inbox` until the markets sweep writes
//! them into `mt5_ohlcv_1m` and `mt5_ticks`. Read only: never an order.
//!
//! The org always comes from the principal (R1): the org context for the
//! UI's routes, the terminal's own row for the EA's. Only the hash of a
//! token is kept; the token itself appears once, in the generated file
//! (R4, R16).

use pnex_core::err_codes;
use pnex_core::mt5::{self, Mt5Config, Mt5Symbol, Mt5Terminal, Mt5TerminalInput};
use rand::RngExt;
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, QueryResult, SqlErr, Statement};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum Mt5Error {
    #[error("invalid {field}: {token}")]
    Invalid { field: String, token: String },
    #[error("terminal not found")]
    NotFound,
    #[error("terminal name taken")]
    NameTaken,
    #[error("terminal changed since read")]
    VersionConflict { current: i32 },
    #[error("too many terminals")]
    TooMany,
    #[error(transparent)]
    Db(#[from] DbErr),
}

fn invalid(field: &str, token: &str) -> Mt5Error {
    Mt5Error::Invalid {
        field: field.into(),
        token: token.into(),
    }
}

/// A terminal authenticated by its token: the org comes from here (R1).
#[derive(Debug, Clone, Copy)]
pub struct TerminalPrincipal {
    pub id: Uuid,
    pub org_id: i64,
}

/// Why a call of a terminal is not kept.
#[derive(Debug, PartialEq, Eq)]
pub enum CallRefusal {
    /// Not a call of this API; the machine token says why.
    Body(&'static str),
    /// Too many calls waiting: the EA sends them again later.
    Full,
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

fn new_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}{hex}", mt5::TOKEN_PREFIX)
}

fn hash(token: &str) -> String {
    Sha256::digest(token.trim().as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn iso(r: &QueryResult, column: &str) -> Result<Option<String>, DbErr> {
    let at: Option<chrono::DateTime<chrono::Utc>> = r.try_get("", column)?;
    Ok(at.map(|t| t.to_rfc3339()))
}

const SELECT: &str = "SELECT t.id, t.name, t.broker, t.server, t.account, t.bars, t.ticks, \
       t.token_hash IS NOT NULL AS has_token, t.last_seen_at, t.ea_version, t.terminal_build, \
       t.backfill_days, t.backfill_requested_at, t.version, t.created_at, t.updated_at, \
       EXTRACT(EPOCH FROM now() - t.last_seen_at)::bigint AS age, \
       (SELECT count(*) FROM mt5_symbols s WHERE s.terminal_id = t.id) AS symbols \
     FROM mt5_terminals t";

fn view(r: &QueryResult) -> Result<Mt5Terminal, DbErr> {
    let id: Uuid = r.try_get("", "id")?;
    Ok(Mt5Terminal {
        id: id.to_string(),
        name: r.try_get("", "name")?,
        broker: r.try_get("", "broker")?,
        server: r.try_get("", "server")?,
        account: r.try_get("", "account")?,
        bars: r.try_get("", "bars")?,
        ticks: r.try_get("", "ticks")?,
        has_token: r.try_get("", "has_token")?,
        presence: mt5::presence(r.try_get("", "age")?),
        last_seen_at: iso(r, "last_seen_at")?,
        ea_version: r.try_get("", "ea_version")?,
        terminal_build: r.try_get("", "terminal_build")?,
        symbols: r.try_get("", "symbols")?,
        backfill_days: r.try_get("", "backfill_days")?,
        backfill_requested_at: iso(r, "backfill_requested_at")?,
        version: r.try_get("", "version")?,
        created_at: iso(r, "created_at")?.unwrap_or_default(),
        updated_at: iso(r, "updated_at")?.unwrap_or_default(),
    })
}

/// A text field of a terminal: trimmed, no control character, bounded.
fn text(field: &str, raw: &str, required: bool) -> Result<String, Mt5Error> {
    let v = raw.trim();
    if required && v.is_empty() {
        return Err(invalid(field, err_codes::FIELD_REQUIRED));
    }
    if v.chars().count() > mt5::NAME_MAX {
        return Err(invalid(
            field,
            &format!("{}:{}", err_codes::FIELD_MAX_LENGTH, mt5::NAME_MAX),
        ));
    }
    if v.chars().any(char::is_control) {
        return Err(invalid(field, err_codes::FIELD_INVALID));
    }
    Ok(v.to_string())
}

fn patterns(field: &str, raw: &str) -> Result<String, Mt5Error> {
    mt5::clean_patterns(raw).map_err(|_| invalid(field, err_codes::FIELD_INVALID))
}

struct Clean {
    name: String,
    broker: String,
    server: String,
    account: String,
    bars: Option<String>,
    ticks: Option<String>,
}

fn clean(input: &Mt5TerminalInput) -> Result<Clean, Mt5Error> {
    Ok(Clean {
        name: text("name", &input.name, true)?,
        broker: text("broker", &input.broker, false)?,
        server: text("server", &input.server, false)?,
        account: text("account", &input.account, false)?,
        bars: input
            .bars
            .as_deref()
            .map(|b| patterns("bars", b))
            .transpose()?,
        ticks: input
            .ticks
            .as_deref()
            .map(|t| patterns("ticks", t))
            .transpose()?,
    })
}

fn taken(e: DbErr) -> Mt5Error {
    match e.sql_err() {
        Some(SqlErr::UniqueConstraintViolation(_)) => Mt5Error::NameTaken,
        _ => Mt5Error::Db(e),
    }
}

pub async fn list<C: ConnectionTrait>(
    db: &C,
    org: i64,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Mt5Terminal>), Mt5Error> {
    let count: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM mt5_terminals WHERE org_id = $1",
            vec![org.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!("{SELECT} WHERE t.org_id = $1 ORDER BY t.name LIMIT $2 OFFSET $3"),
            vec![org.into(), limit.into(), offset.into()],
        ))
        .await?;
    let list = rows.iter().map(view).collect::<Result<Vec<_>, _>>()?;
    Ok((count.max(0), list))
}

pub async fn get<C: ConnectionTrait>(db: &C, org: i64, id: Uuid) -> Result<Mt5Terminal, Mt5Error> {
    let row = db
        .query_one_raw(stmt(
            &format!("{SELECT} WHERE t.org_id = $1 AND t.id = $2"),
            vec![org.into(), id.into()],
        ))
        .await?
        .ok_or(Mt5Error::NotFound)?;
    Ok(view(&row)?)
}

/// A new terminal, without a token until its EA is generated. Its bars are
/// the whole catalogue unless chosen (Q-MT3), its ticks none (Q-MT4).
pub async fn create<C: ConnectionTrait>(
    db: &C,
    org: i64,
    input: &Mt5TerminalInput,
) -> Result<Mt5Terminal, Mt5Error> {
    let c = clean(input)?;
    let n: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM mt5_terminals WHERE org_id = $1",
            vec![org.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    if n >= mt5::TERMINALS_MAX {
        return Err(Mt5Error::TooMany);
    }
    let id = Uuid::new_v4();
    db.execute_raw(stmt(
        "INSERT INTO mt5_terminals (id, org_id, name, broker, server, account, bars, ticks) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        vec![
            id.into(),
            org.into(),
            c.name.into(),
            c.broker.into(),
            c.server.into(),
            c.account.into(),
            c.bars.unwrap_or_else(|| "*".into()).into(),
            c.ticks.unwrap_or_default().into(),
        ],
    ))
    .await
    .map_err(taken)?;
    get(db, org, id).await
}

/// Replaces the terminal on the version that was read.
pub async fn update<C: ConnectionTrait>(
    db: &C,
    org: i64,
    id: Uuid,
    input: &Mt5TerminalInput,
) -> Result<Mt5Terminal, Mt5Error> {
    let c = clean(input)?;
    let Some(expected) = input.expected_version else {
        return Err(invalid("expected_version", err_codes::FIELD_REQUIRED));
    };
    let current = get(db, org, id).await?;
    let done = db
        .execute_raw(stmt(
            "UPDATE mt5_terminals SET name = $3, broker = $4, server = $5, account = $6, \
               bars = $7, ticks = $8, version = version + 1, updated_at = now() \
             WHERE org_id = $1 AND id = $2 AND version = $9",
            vec![
                org.into(),
                id.into(),
                c.name.into(),
                c.broker.into(),
                c.server.into(),
                c.account.into(),
                c.bars.unwrap_or(current.bars).into(),
                c.ticks.unwrap_or(current.ticks).into(),
                expected.into(),
            ],
        ))
        .await
        .map_err(taken)?;
    if done.rows_affected() == 0 {
        let now = get(db, org, id).await?;
        return Err(Mt5Error::VersionConflict {
            current: now.version,
        });
    }
    get(db, org, id).await
}

pub async fn delete<C: ConnectionTrait>(db: &C, org: i64, id: Uuid) -> Result<(), Mt5Error> {
    let done = db
        .execute_raw(stmt(
            "DELETE FROM mt5_terminals WHERE org_id = $1 AND id = $2",
            vec![org.into(), id.into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(Mt5Error::NotFound);
    }
    Ok(())
}

/// A new token for the terminal; the previous one stops working at once.
/// The token is returned this once, for the generated EA only.
pub async fn new_ea_token<C: ConnectionTrait>(
    db: &C,
    org: i64,
    id: Uuid,
) -> Result<(Mt5Terminal, String), Mt5Error> {
    let token = new_token();
    let done = db
        .execute_raw(stmt(
            "UPDATE mt5_terminals SET token_hash = $3, updated_at = now() WHERE org_id = $1 AND id = $2",
            vec![org.into(), id.into(), hash(&token).into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(Mt5Error::NotFound);
    }
    Ok((get(db, org, id).await?, token))
}

/// Asks the terminal to send again `days` of 1-minute bars of its selection.
pub async fn request_backfill<C: ConnectionTrait>(
    db: &C,
    org: i64,
    id: Uuid,
    days: u32,
) -> Result<Mt5Terminal, Mt5Error> {
    if !(1..=mt5::BACKFILL_DAYS_MAX).contains(&days) {
        return Err(invalid("days", err_codes::FIELD_INVALID));
    }
    let done = db
        .execute_raw(stmt(
            "UPDATE mt5_terminals SET backfill_days = $3, backfill_requested_at = now(), \
               updated_at = now() WHERE org_id = $1 AND id = $2",
            vec![org.into(), id.into(), (days as i32).into()],
        ))
        .await?;
    if done.rows_affected() == 0 {
        return Err(Mt5Error::NotFound);
    }
    get(db, org, id).await
}

/// The terminal's catalogue, filtered by a part of the symbol or of its
/// description, with what its selection sends.
pub async fn symbols<C: ConnectionTrait>(
    db: &C,
    org: i64,
    id: Uuid,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<(i64, Vec<Mt5Symbol>), Mt5Error> {
    let t = get(db, org, id).await?;
    let like = format!(
        "%{}%",
        q.unwrap_or("")
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    );
    let filter = "terminal_id = $1 AND (symbol ILIKE $2 OR description ILIKE $2)";
    let count: i64 = db
        .query_one_raw(stmt(
            &format!("SELECT count(*) AS n FROM mt5_symbols WHERE {filter}"),
            vec![id.into(), like.clone().into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    let rows = db
        .query_all_raw(stmt(
            &format!(
                "SELECT symbol, description, path, base, profit, digits, contract_size, first_m1 \
                 FROM mt5_symbols WHERE {filter} ORDER BY symbol LIMIT $3 OFFSET $4"
            ),
            vec![id.into(), like.into(), limit.into(), offset.into()],
        ))
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let symbol: String = r.try_get("", "symbol")?;
        out.push(Mt5Symbol {
            bars: mt5::selected(&t.bars, &symbol),
            ticks: mt5::selected(&t.ticks, &symbol),
            description: r.try_get("", "description")?,
            path: r.try_get("", "path")?,
            base: r.try_get("", "base")?,
            profit: r.try_get("", "profit")?,
            digits: r.try_get("", "digits")?,
            contract_size: r.try_get("", "contract_size")?,
            first_m1: iso(r, "first_m1")?,
            symbol,
        });
    }
    Ok((count.max(0), out))
}

/// The terminal of a token, `None` for an unknown or replaced one.
pub async fn authenticate<C: ConnectionTrait>(
    db: &C,
    token: &str,
) -> Result<Option<TerminalPrincipal>, DbErr> {
    if !token.trim().starts_with(mt5::TOKEN_PREFIX) {
        return Ok(None);
    }
    let Some(r) = db
        .query_one_raw(stmt(
            "SELECT id, org_id FROM mt5_terminals WHERE token_hash = $1",
            vec![hash(token).into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    Ok(Some(TerminalPrincipal {
        id: r.try_get("", "id")?,
        org_id: r.try_get("", "org_id")?,
    }))
}

/// A call of the EA: the terminal is up; what it says of itself is kept.
async fn seen<C: ConnectionTrait>(db: &C, p: TerminalPrincipal, body: &Value) -> Result<(), DbErr> {
    let version: Option<String> = body.get("ea_version").and_then(Value::as_str).map(|v| {
        v.chars()
            .filter(|c| c.is_ascii_graphic())
            .take(16)
            .collect()
    });
    let build = body.get("build").and_then(Value::as_i64);
    db.execute_raw(stmt(
        "UPDATE mt5_terminals SET last_seen_at = now(), \
           ea_version = coalesce($3, ea_version), terminal_build = coalesce($4, terminal_build) \
         WHERE id = $1 AND org_id = $2",
        vec![p.id.into(), p.org_id.into(), version.into(), build.into()],
    ))
    .await?;
    Ok(())
}

/// The catalogue a terminal announces: each symbol upserted, none removed
/// (a symbol the broker drops stays known with its last announce).
pub async fn announce<C: ConnectionTrait>(
    db: &C,
    p: TerminalPrincipal,
    body: &Value,
) -> Result<Result<usize, CallRefusal>, DbErr> {
    let list = match mt5::announced(body) {
        Ok(l) => l,
        Err(token) => return Ok(Err(CallRefusal::Body(token))),
    };
    seen(db, p, body).await?;
    for chunk in list.chunks(500) {
        let mut sql = String::from(
            "INSERT INTO mt5_symbols (terminal_id, symbol, description, path, base, profit, \
               digits, contract_size, first_m1) VALUES ",
        );
        let mut values: Vec<sea_orm::Value> = Vec::with_capacity(chunk.len() * 9);
        for (i, a) in chunk.iter().enumerate() {
            let k = i * 9;
            if i > 0 {
                sql.push(',');
            }
            sql.push_str(&format!(
                "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, to_timestamp(${}))",
                k + 1,
                k + 2,
                k + 3,
                k + 4,
                k + 5,
                k + 6,
                k + 7,
                k + 8,
                k + 9
            ));
            values.extend([
                p.id.into(),
                a.symbol.clone().into(),
                a.description.clone().into(),
                a.path.clone().into(),
                a.base.clone().into(),
                a.profit.clone().into(),
                a.digits.into(),
                a.contract_size.into(),
                a.first_m1.map(|t| t as f64).into(),
            ]);
        }
        sql.push_str(
            " ON CONFLICT (terminal_id, symbol) DO UPDATE SET description = EXCLUDED.description, \
               path = EXCLUDED.path, base = EXCLUDED.base, profit = EXCLUDED.profit, \
               digits = EXCLUDED.digits, contract_size = EXCLUDED.contract_size, \
               first_m1 = EXCLUDED.first_m1, announced_at = now()",
        );
        db.execute_raw(stmt(&sql, values)).await?;
    }
    // A broker or server left empty in the UI takes what the terminal says.
    let field = |k: &str| -> String {
        body.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(mt5::NAME_MAX)
            .collect()
    };
    db.execute_raw(stmt(
        "UPDATE mt5_terminals SET broker = CASE WHEN broker = '' THEN $3 ELSE broker END, \
           server = CASE WHEN server = '' THEN $4 ELSE server END WHERE id = $1 AND org_id = $2",
        vec![
            p.id.into(),
            p.org_id.into(),
            field("broker").into(),
            field("server").into(),
        ],
    ))
    .await?;
    Ok(Ok(list.len()))
}

/// What the EA reads: its selection resolved on its catalogue.
pub async fn config<C: ConnectionTrait>(db: &C, p: TerminalPrincipal) -> Result<Mt5Config, DbErr> {
    seen(db, p, &Value::Null).await?;
    let Some(t) = db
        .query_one_raw(stmt(
            "SELECT bars, ticks, backfill_days, \
               EXTRACT(EPOCH FROM backfill_requested_at)::bigint AS asked \
             FROM mt5_terminals WHERE id = $1 AND org_id = $2",
            vec![p.id.into(), p.org_id.into()],
        ))
        .await?
    else {
        return Ok(mt5::config(&[], "", "", None));
    };
    let catalogue: Vec<String> = db
        .query_all_raw(stmt(
            "SELECT symbol FROM mt5_symbols WHERE terminal_id = $1 ORDER BY symbol",
            vec![p.id.into()],
        ))
        .await?
        .iter()
        .filter_map(|r| r.try_get("", "symbol").ok())
        .collect();
    let bars: String = t.try_get("", "bars")?;
    let ticks: String = t.try_get("", "ticks")?;
    let days: i32 = t.try_get("", "backfill_days")?;
    let asked: Option<i64> = t.try_get("", "asked")?;
    let backfill = asked.map(|at| (at, at - i64::from(days) * 86_400));
    Ok(mt5::config(&catalogue, &bars, &ticks, backfill))
}

/// Keeps one call of bars and ticks for the markets sweep; a call without
/// data is the heartbeat and keeps nothing.
pub async fn ingest<C: ConnectionTrait>(
    db: &C,
    p: TerminalPrincipal,
    body: Value,
) -> Result<Result<(), CallRefusal>, DbErr> {
    let batch = match pnex_core::markets::history::mt5_payload(&body) {
        Ok(b) => b,
        Err(token) => return Ok(Err(CallRefusal::Body(token))),
    };
    seen(db, p, &body).await?;
    if batch.bars.is_empty() && batch.ticks.is_empty() {
        return Ok(Ok(()));
    }
    let waiting: i64 = db
        .query_one_raw(stmt(
            "SELECT count(*) AS n FROM mt5_inbox WHERE org_id = $1",
            vec![p.org_id.into()],
        ))
        .await?
        .map(|r| r.try_get("", "n"))
        .transpose()?
        .unwrap_or(0);
    if waiting >= mt5::INBOX_MAX {
        return Ok(Err(CallRefusal::Full));
    }
    db.execute_raw(stmt(
        "INSERT INTO mt5_inbox (org_id, terminal_id, body) VALUES ($1, $2, $3)",
        vec![p.org_id.into(), p.id.into(), body.into()],
    ))
    .await?;
    Ok(Ok(()))
}
