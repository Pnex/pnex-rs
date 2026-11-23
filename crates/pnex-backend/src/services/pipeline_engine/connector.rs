//! The `connector` step (pipelines.md D352, D793, D795): one read of a
//! connector of the org, its result as CSV in a file of the run, for a
//! `read`. The result is untrusted data. The text of an outside server stays
//! in the logs: a failure carries codes only (R4).

use std::time::Duration;

use futures_util::TryStreamExt;
use pnex_core::connectors::rest::{self, Paging, RestColumn};
use pnex_core::connectors::{cursor_after, QUERY_MAX_BYTES};
use pnex_core::err_codes;
use pnex_core::pipeline::doc::Connector;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{json, Value};
use sqlx::{Connection, Executor, SqlSafeStr};
use uuid::Uuid;

use super::blob::{Blob, Spool};
use super::{fail, Failure};
use crate::services::connectors::{self, Target};

const QUERY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Bytes one read keeps at most.
const MAX_BYTES: u64 = pnex_core::ingest::DOWNLOAD_MAX_BYTES;
/// Calls read from an inbox per query.
const INBOX_BATCH: i64 = 1_000;
/// Records one run reads at most; the next run goes on from there.
const INBOX_RUN_MAX: u64 = 100_000;

/// The cursor a step kept: its last value and the incremental reads since
/// the last full one.
#[derive(Debug, Clone, Default)]
pub struct Kept {
    pub value: Option<String>,
    pub runs: u32,
}

/// What a read moves the cursor to once the run succeeds. `value` empty:
/// nothing new, the kept value stays.
#[derive(Debug, Clone, PartialEq)]
pub struct Next {
    pub value: String,
    pub full: bool,
}

fn refused(code: &'static str) -> Failure {
    fail(code, Value::Null)
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> sea_orm::Statement {
    sea_orm::Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

/// The cursor a step of a pipeline kept.
pub async fn kept(db: &DatabaseConnection, org: i64, pipeline: &str, step: &str) -> Kept {
    let row = db
        .query_one_raw(stmt(
            "SELECT value, runs FROM pipeline_cursors WHERE org_id = $1 AND pipeline = $2 AND step = $3",
            vec![org.into(), pipeline.into(), step.into()],
        ))
        .await
        .ok()
        .flatten();
    match row {
        Some(r) => Kept {
            value: r.try_get("", "value").ok(),
            runs: r.try_get::<i32>("", "runs").unwrap_or(0).max(0) as u32,
        },
        None => Kept::default(),
    }
}

/// Keeps what a successful run read up to.
pub async fn keep(
    db: &DatabaseConnection,
    org: i64,
    pipeline: &str,
    cursors: &[(String, Next)],
) -> Result<(), sea_orm::DbErr> {
    for (step, next) in cursors {
        if next.value.is_empty() {
            // Nothing new: one more incremental read, the value stays.
            db.execute_raw(stmt(
                "UPDATE pipeline_cursors SET runs = runs + 1, updated_at = now() \
                 WHERE org_id = $1 AND pipeline = $2 AND step = $3",
                vec![org.into(), pipeline.into(), step.as_str().into()],
            ))
            .await?;
            continue;
        }
        db.execute_raw(stmt(
            "INSERT INTO pipeline_cursors (org_id, pipeline, step, value, runs) \
             VALUES ($1, $2, $3, $4, CASE WHEN $5 THEN 0 ELSE 1 END) \
             ON CONFLICT (org_id, pipeline, step) DO UPDATE SET value = EXCLUDED.value, \
               runs = CASE WHEN $5 THEN 0 ELSE pipeline_cursors.runs + 1 END, updated_at = now()",
            vec![
                org.into(),
                pipeline.into(),
                step.as_str().into(),
                next.value.as_str().into(),
                next.full.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

/// The value this run reads past: the start on a full read, the kept one otherwise.
struct Read {
    full: bool,
    from: String,
}

/// Reads the connector `c` names. `None` in the second place: no cursor.
pub async fn read(
    db: &DatabaseConnection,
    org: i64,
    c: &Connector,
    kept: Kept,
) -> Result<(Blob, Option<Next>), Failure> {
    let Some(ring) = crate::services::markets::keyring() else {
        return Err(refused(err_codes::CONNECTOR_UNREACHABLE));
    };
    let read = c.cursor.as_ref().map(|cur| {
        let full = cur.full(kept.value.as_deref(), kept.runs);
        Read {
            full,
            from: if full {
                cur.start.clone()
            } else {
                kept.value.clone().unwrap_or_default()
            },
        }
    });
    match connectors::target(db, ring, org, Some(&c.name), None).await {
        Ok(Target::Pg(t)) => pg(c, *t, read).await,
        Ok(Target::My(t)) => mysql(c, *t, read).await,
        Ok(Target::Rest(mut t)) => rest(c, &mut t, read).await,
        Ok(Target::Webhook { id, .. }) => inbox(db, org, c, id, kept).await,
        Err(code) => Err(refused(code)),
    }
}

fn invalid_sql(e: &sqlx::Error) -> Failure {
    // The server's text can quote the data: the code stays, the text in the logs.
    let state = e
        .as_database_error()
        .and_then(|d| d.code())
        .map(|c| c.to_string());
    tracing::info!(error = %e, "connector query refused");
    fail(
        err_codes::CONNECTOR_QUERY_INVALID,
        json!({ "sqlstate": state }),
    )
}

fn checked_query(c: &Connector) -> Result<String, Failure> {
    if c.query.trim().is_empty() || c.query.len() > QUERY_MAX_BYTES {
        return Err(refused(err_codes::CONNECTOR_QUERY_INVALID));
    }
    Ok(c.query.clone())
}

fn too_large() -> Failure {
    fail("too_large", json!({ "max": MAX_BYTES }))
}

/// One read-only statement on a PostgreSQL connector, copied out as CSV.
async fn pg(
    c: &Connector,
    target: connectors::PgTarget,
    read: Option<Read>,
) -> Result<(Blob, Option<Next>), Failure> {
    let mut query = checked_query(c)?;
    let mut conn = target.connect(QUERY_TIMEOUT).await.map_err(refused)?;
    if let Some(read) = &read {
        // One snapshot for the rows and their highest cursor: a row committed between
        // the two reads can be neither missed nor counted twice.
        let begun = (&mut conn)
            .execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .await;
        // The source quotes the value: it comes from its own data, never trusted.
        let literal = sqlx::query_scalar::<_, String>("SELECT quote_literal($1::text)")
            .bind(&read.from)
            .fetch_one(&mut conn)
            .await;
        match (begun, literal) {
            (Ok(_), Ok(lit)) => {
                query = query.replace(pnex_core::connectors::CURSOR_PLACEHOLDER, &lit)
            }
            _ => return Err(refused(err_codes::CONNECTOR_UNREACHABLE)),
        }
    }
    // The extended protocol parses one statement alone. Parsed, it cannot close the
    // parenthesis below or chain another statement; the driver would run those. The
    // SQL is the admin's on purpose, on their own database.
    (&mut conn)
        .prepare(sqlx::AssertSqlSafe(query.clone()).into_sql_str())
        .await
        .map_err(|e| invalid_sql(&e))?;
    let copy = format!("COPY ({query}\n) TO STDOUT (FORMAT csv, HEADER true)");
    let mut out = Spool::new(MAX_BYTES);
    {
        let mut stream = conn.copy_out_raw(&copy).await.map_err(|e| {
            tracing::info!(error = %e, "connector query failed");
            refused(err_codes::CONNECTOR_QUERY_INVALID)
        })?;
        loop {
            match stream.try_next().await {
                Ok(Some(chunk)) => out.push(&chunk)?,
                Ok(None) => break,
                Err(e) => {
                    tracing::info!(error = %e, "connector query stopped");
                    return Err(refused(err_codes::CONNECTOR_QUERY_INVALID));
                }
            }
        }
    }
    let next = match (&c.cursor, read) {
        (Some(cur), Some(read)) => {
            let ident = sqlx::query_scalar::<_, String>("SELECT quote_ident($1::text)")
                .bind(&cur.column)
                .fetch_one(&mut conn)
                .await
                .map_err(|_| refused(err_codes::CONNECTOR_UNREACHABLE))?;
            let sql = format!("SELECT max(q.{ident})::text FROM ({query}\n) q");
            let top = sqlx::query_scalar::<_, Option<String>>(sqlx::AssertSqlSafe(sql))
                .fetch_one(&mut conn)
                .await
                .map_err(|e| {
                    tracing::info!(error = %e, "connector cursor not read");
                    fail(
                        err_codes::CONNECTOR_QUERY_INVALID,
                        json!({ "cursor": cur.column }),
                    )
                })?;
            let _ = (&mut conn).execute("COMMIT").await;
            Some(Next {
                value: top.unwrap_or_default(),
                full: read.full,
            })
        }
        _ => None,
    };
    let _ = conn.close().await;
    Ok((out.finish(), next))
}

/// One read-only statement on a MySQL connector, its rows as CSV (D792). No COPY:
/// rows come by the text protocol, each value as the server writes it.
async fn mysql(
    c: &Connector,
    target: connectors::MyTarget,
    read: Option<Read>,
) -> Result<(Blob, Option<Next>), Failure> {
    use sqlx::{Column, Row, Statement};
    let mut query = checked_query(c)?;
    let mut conn = target.connect(QUERY_TIMEOUT).await.map_err(refused)?;
    if let Some(read) = &read {
        // One snapshot for the rows and their highest cursor, as for PostgreSQL.
        let begun = (&mut conn)
            .execute("START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY")
            .await;
        // The source quotes the value: it comes from its own data, never trusted.
        let literal = sqlx::query_scalar::<_, String>("SELECT QUOTE(?)")
            .bind(&read.from)
            .fetch_one(&mut conn)
            .await;
        match (begun, literal) {
            (Ok(_), Ok(lit)) => {
                query = query.replace(pnex_core::connectors::CURSOR_PLACEHOLDER, &lit)
            }
            _ => return Err(refused(err_codes::CONNECTOR_UNREACHABLE)),
        }
    }
    // A prepared statement is one statement; it must return rows, which refuses a
    // `SET` that would change the session.
    let names: Vec<String> = match (&mut conn)
        .prepare(sqlx::AssertSqlSafe(query.clone()).into_sql_str())
        .await
    {
        Ok(p) if !p.columns().is_empty() => {
            p.columns().iter().map(|c| c.name().to_string()).collect()
        }
        Ok(_) => return Err(refused(err_codes::CONNECTOR_QUERY_INVALID)),
        Err(e) => return Err(invalid_sql(&e)),
    };
    let mut out = Spool::new(MAX_BYTES);
    {
        let mut chunk = rest::csv_line(&names);
        let mut stream = sqlx::raw_sql(sqlx::AssertSqlSafe(query.clone())).fetch(&mut conn);
        loop {
            let row = match stream.try_next().await {
                Ok(row) => row,
                Err(e) => {
                    tracing::info!(error = %e, "connector query stopped");
                    return Err(refused(err_codes::CONNECTOR_QUERY_INVALID));
                }
            };
            if let Some(row) = &row {
                let cells = (0..row.len()).map(|i| {
                    row.try_get_unchecked::<Option<Vec<u8>>, _>(i)
                        .ok()
                        .flatten()
                        .map(|b| String::from_utf8_lossy(&b).into_owned())
                        .unwrap_or_default()
                });
                chunk.push_str(&rest::csv_line(cells));
            }
            if chunk.len() >= 1 << 20 || (row.is_none() && !chunk.is_empty()) {
                out.push(chunk.as_bytes())?;
                chunk.clear();
            }
            if row.is_none() {
                break;
            }
        }
    }
    let next = match (&c.cursor, read) {
        (Some(cur), Some(read)) => {
            let ident = format!("`{}`", cur.column.replace('`', "``"));
            let sql = format!("SELECT CAST(MAX(q.{ident}) AS CHAR) FROM ({query}\n) q");
            let top = sqlx::query_scalar::<_, Option<String>>(sqlx::AssertSqlSafe(sql))
                .fetch_one(&mut conn)
                .await
                .map_err(|e| {
                    tracing::info!(error = %e, "connector cursor not read");
                    fail(
                        err_codes::CONNECTOR_QUERY_INVALID,
                        json!({ "cursor": cur.column }),
                    )
                })?;
            let _ = (&mut conn).execute("COMMIT").await;
            Some(Next {
                value: top.unwrap_or_default(),
                full: read.full,
            })
        }
        _ => None,
    };
    let _ = conn.close().await;
    Ok((out.finish(), next))
}

/// Reads a REST API page after page into one CSV (D793).
async fn rest(
    c: &Connector,
    t: &mut connectors::RestTarget,
    read: Option<Read>,
) -> Result<(Blob, Option<Next>), Failure> {
    let Some(q) = c.rest.as_ref().filter(|q| q.check().is_none()) else {
        return Err(refused(err_codes::CONNECTOR_QUERY_INVALID));
    };
    let since = match (&c.cursor, &read) {
        (Some(cur), Some(r)) if !r.from.is_empty() => Some((cur.param.clone(), r.from.clone())),
        _ => None,
    };
    let mut out = Spool::new(MAX_BYTES);
    let mut top = String::new();
    let first = t.url(&q.path);
    let (mut received, mut columns) = (0u64, None::<Vec<RestColumn>>);
    let (mut cursor, mut link) = (None::<String>, None::<reqwest::Url>);
    let mut done = false;
    for n in 0..q.max_pages() {
        let (url, params) = match link.take() {
            Some(next) => (next, Vec::new()),
            None => {
                let mut p = q.page_params(n, cursor.as_deref());
                if let Some((k, v)) = &since {
                    p.retain(|(x, _)| x != k);
                    p.push((k.clone(), v.clone()));
                }
                (first.clone(), p)
            }
        };
        let mut res = t.get(url.clone(), &params).await.map_err(refused)?;
        let next = res
            .headers()
            .get(reqwest::header::LINK)
            .and_then(|v| v.to_str().ok())
            .and_then(rest::next_link);
        let mut body = Vec::new();
        loop {
            match res.chunk().await {
                Ok(Some(chunk)) => {
                    received += chunk.len() as u64;
                    if received > MAX_BYTES {
                        return Err(too_large());
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::info!(error = %e, "connector page stopped");
                    return Err(refused(err_codes::CONNECTOR_UNREACHABLE));
                }
            }
        }
        let page: Value = serde_json::from_slice(&body)
            .map_err(|_| refused(err_codes::CONNECTOR_REST_NOT_JSON))?;
        drop(body);
        let records = q
            .records(&page)
            .ok_or_else(|| refused(err_codes::CONNECTOR_REST_NO_RECORDS))?;
        let mut csv = String::new();
        let cols = columns.get_or_insert_with(|| {
            let c = q.columns_for(records.first());
            csv.push_str(&rest::csv_line(c.iter().map(|c| c.name.as_str())));
            c
        });
        for r in records {
            csv.push_str(&rest::csv_line(
                cols.iter().map(|c| rest::cell(r, &c.pointer)),
            ));
            if let Some(cur) = &c.cursor {
                let v = rest::cell(r, &cur.column);
                if !v.is_empty() && (top.is_empty() || cursor_after(&v, &top)) {
                    top = v;
                }
            }
        }
        out.push(csv.as_bytes())?;
        let short = q.page_size.is_some_and(|s| records.len() < s as usize);
        let more = match q.paging {
            Paging::None => false,
            Paging::Page | Paging::Offset => !records.is_empty() && !short,
            Paging::Cursor => match q.next_cursor(&page) {
                // The same cursor twice would loop until the page limit.
                Some(c) if cursor.as_deref() != Some(c.as_str()) => {
                    cursor = Some(c);
                    true
                }
                _ => false,
            },
            Paging::Link => match next {
                Some(href) => {
                    let next = url
                        .join(&href)
                        .map_err(|_| refused(err_codes::CONNECTOR_REST_HOST_CHANGED))?;
                    if !t.same_origin(&next) {
                        return Err(refused(err_codes::CONNECTOR_REST_HOST_CHANGED));
                    }
                    link = Some(next);
                    true
                }
                None => false,
            },
        };
        if !more {
            done = true;
            break;
        }
    }
    if !done {
        return Err(fail(
            err_codes::CONNECTOR_REST_PAGE_LIMIT,
            json!({ "max": q.max_pages() }),
        ));
    }
    let next = read.map(|r| Next {
        value: top,
        full: r.full,
    });
    Ok((out.finish(), next))
}

/// Reads the calls a webhook kept past the last one read (D795). The cursor
/// is the inbox id, implicit: the first run reads all, then each run only the
/// new calls. Shape from `rest.records` and `rest.columns`: without them a
/// call is one record, or a list of records.
async fn inbox(
    db: &DatabaseConnection,
    org: i64,
    c: &Connector,
    connector: Uuid,
    kept: Kept,
) -> Result<(Blob, Option<Next>), Failure> {
    let unavailable = |_| refused(err_codes::CONNECTOR_UNREACHABLE);
    let shape = c.rest.clone().unwrap_or_default();
    if shape.check().is_some() {
        return Err(refused(err_codes::CONNECTOR_QUERY_INVALID));
    }
    let from: i64 = kept
        .value
        .as_deref()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // Calls past the retention go, read or not (pipelines.md D795).
    let _ = db
        .execute_raw(stmt(
            "DELETE FROM connector_inbox WHERE connector_id = $1 \
               AND received_at < now() - make_interval(days => $2::int)",
            vec![
                connector.into(),
                (crate::services::hooks::INBOX_DAYS as i32).into(),
            ],
        ))
        .await;
    let mut out = Spool::new(MAX_BYTES);
    let (mut count, mut last) = (0u64, from);
    let mut columns = None::<Vec<RestColumn>>;
    loop {
        let rows = db
            .query_all_raw(stmt(
                "SELECT id, body::text AS body FROM connector_inbox \
                 WHERE org_id = $1 AND connector_id = $2 AND id > $3 ORDER BY id LIMIT $4",
                vec![
                    org.into(),
                    connector.into(),
                    last.into(),
                    INBOX_BATCH.into(),
                ],
            ))
            .await
            .map_err(unavailable)?;
        if rows.is_empty() {
            break;
        }
        let mut csv = String::new();
        for row in &rows {
            let id: i64 = row.try_get("", "id").map_err(unavailable)?;
            let text: String = row.try_get("", "body").map_err(unavailable)?;
            let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let records: Vec<&Value> = match body.pointer(&shape.records) {
                Some(Value::Array(a)) => a.iter().collect(),
                Some(v @ Value::Object(_)) => vec![v],
                _ => Vec::new(),
            };
            let cols = columns.get_or_insert_with(|| {
                let c = shape.columns_for(records.first().copied());
                csv.push_str(&rest::csv_line(c.iter().map(|c| c.name.as_str())));
                c
            });
            for r in records {
                csv.push_str(&rest::csv_line(
                    cols.iter().map(|c| rest::cell(r, &c.pointer)),
                ));
                count += 1;
            }
            last = id;
        }
        out.push(csv.as_bytes())?;
        if count >= INBOX_RUN_MAX || (rows.len() as i64) < INBOX_BATCH {
            break;
        }
    }
    let next = Next {
        value: if last > from {
            last.to_string()
        } else {
            String::new()
        },
        full: kept.value.is_none(),
    };
    Ok((out.finish(), Some(next)))
}
