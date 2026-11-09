//! `connector_query` (data-pipelines.md D352): runs one read-only query on an outside
//! database and streams its result as CSV into the staging, where the next steps of
//! the import read it like a downloaded file. The result is untrusted data.

use std::time::Duration;

use chrono::Utc;
use futures_util::TryStreamExt;
use pnex_core::connectors::rest::{self, Paging, RestColumn};
use pnex_core::connectors::QUERY_MAX_BYTES;
use pnex_core::err_codes;
use pnex_core::ingest::{cursor_after, IngestConfig, IngestMsg, StagedEntry, CURSOR_PLACEHOLDER};
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Connection, Executor, SqlSafeStr};
use uuid::Uuid;

use super::{stmt, unavailable, Cx, Outcome};
use crate::services::connectors;

const QUERY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const FILE_NAME: &str = "query.csv";

/// R2: what an import reads from a connector goes into a table every member reads,
/// so its author must still hold the right to use connectors. Checked on each run:
/// a schedule keeps running after the author's role changed.
async fn author_may_use(cx: &Cx<'_>) -> bool {
    use crate::models::_entities::{
        organization_members as om, sea_orm_active_enums::OrgMemberRole,
    };
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let Some(import) = cx
        .node
        .node_id
        .strip_prefix("import:")
        .and_then(|i| i.parse::<Uuid>().ok())
    else {
        return false;
    };
    let author: Option<i64> = cx
        .db
        .query_one_raw(stmt(
            "SELECT created_by FROM object_imports WHERE org_id = $1 AND id = $2",
            vec![cx.node.org_id.into(), import.into()],
        ))
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get("", "created_by").ok());
    let Some(author) = author else {
        return false;
    };
    let role: Option<OrgMemberRole> = om::Entity::find()
        .filter(om::Column::OrgId.eq(cx.node.org_id))
        .filter(om::Column::UserId.eq(author))
        .one(cx.db)
        .await
        .ok()
        .flatten()
        .map(|m| m.role);
    role.is_some_and(|r| crate::services::iam::authorize(&r, "connectors:use").allowed())
}

/// The cursor the import last committed and the incremental reads since a full one.
async fn cursor_state(cx: &Cx<'_>) -> (Option<String>, u32) {
    let Some(import) = cx
        .node
        .node_id
        .strip_prefix("import:")
        .and_then(|i| i.parse::<Uuid>().ok())
    else {
        return (None, 0);
    };
    let row = cx
        .db
        .query_one_raw(stmt(
            "SELECT cursor, cursor_runs FROM object_imports WHERE org_id = $1 AND id = $2",
            vec![cx.node.org_id.into(), import.into()],
        ))
        .await
        .ok()
        .flatten();
    let Some(row) = row else {
        return (None, 0);
    };
    let cursor: Option<String> = row.try_get("", "cursor").ok().flatten();
    let runs: i32 = row.try_get("", "cursor_runs").unwrap_or(0);
    (cursor, runs.max(0) as u32)
}

/// The value this run reads past: the start on a full read, the stored one otherwise.
struct Read {
    full: bool,
    from: String,
}

fn refused(code: &'static str) -> Outcome {
    Outcome::fail(code, Value::Null)
}

pub(super) async fn run(cx: &Cx<'_>, cfg: &IngestConfig, _msg: IngestMsg) -> Outcome {
    if !author_may_use(cx).await {
        return refused(err_codes::CONNECTOR_FORBIDDEN);
    }
    let Some(ring) = crate::services::markets::keyring() else {
        return refused(err_codes::CONNECTOR_UNREACHABLE);
    };
    let room = match cx.room().await {
        Ok(n) => n,
        Err(o) => return *o,
    };
    let cap = cfg.max_bytes();
    let target = connectors::target(cx.db, ring, cx.node.org_id, Some(&cfg.connector), None).await;
    let read = match &cfg.cursor {
        Some(c) => {
            let (stored, runs) = cursor_state(cx).await;
            let full = c.full(stored.as_deref(), runs);
            Some(Read {
                full,
                from: if full {
                    c.start.clone()
                } else {
                    stored.unwrap_or_default()
                },
            })
        }
        None => None,
    };
    let target = match target {
        Ok(connectors::Target::Pg(t)) => t,
        Ok(connectors::Target::Rest(mut t)) => return rest(cx, cfg, &mut t, cap, room, read).await,
        Ok(connectors::Target::Webhook { id, .. }) => return inbox(cx, cfg, id, cap, room).await,
        Ok(connectors::Target::My(t)) => return mysql(cx, cfg, *t, cap, room, read).await,
        Err(code) => return refused(code),
    };
    if cfg.query.trim().is_empty() || cfg.query.len() > QUERY_MAX_BYTES {
        return refused(err_codes::CONNECTOR_QUERY_INVALID);
    }
    let mut conn = match target.connect(QUERY_TIMEOUT).await {
        Ok(c) => c,
        Err(code) => return refused(code),
    };
    let mut query = cfg.query.clone();
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
            (Ok(_), Ok(lit)) => query = query.replace(CURSOR_PLACEHOLDER, &lit),
            _ => return refused(err_codes::CONNECTOR_UNREACHABLE),
        }
    }
    // The extended protocol parses one statement alone. Parsed, it cannot close the
    // parenthesis below or chain another statement; the driver would run those. The
    // SQL is the admin's on purpose, on their own database.
    if let Err(e) = (&mut conn)
        .prepare(sqlx::AssertSqlSafe(query.clone()).into_sql_str())
        .await
    {
        // The server's text can quote the data: the code stays, the text in the logs.
        let state = e
            .as_database_error()
            .and_then(|d| d.code())
            .map(|c| c.to_string());
        tracing::info!(error = %e, "connector query refused");
        return Outcome::fail(
            err_codes::CONNECTOR_QUERY_INVALID,
            json!({ "sqlstate": state }),
        );
    }
    let copy = format!("COPY ({query}\n) TO STDOUT (FORMAT csv, HEADER true)");
    let key = cx.key("raw", FILE_NAME);
    let Ok(mut w) = cx.store.writer(&key).await else {
        return unavailable();
    };
    let (mut hasher, mut bytes) = (Sha256::new(), 0u64);
    let copied = async {
        let mut stream = conn.copy_out_raw(&copy).await.map_err(|e| {
            tracing::info!(error = %e, "connector query failed");
            refused(err_codes::CONNECTOR_QUERY_INVALID)
        })?;
        loop {
            match stream.try_next().await {
                Ok(Some(chunk)) => {
                    bytes += chunk.len() as u64;
                    if bytes > cap {
                        let args = json!({ "bytes": bytes, "max": cap });
                        return Err(Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, args));
                    }
                    if bytes > room {
                        return Err(Outcome::fail(err_codes::INGEST_STAGING_QUOTA, Value::Null));
                    }
                    hasher.update(&chunk);
                    if w.write(chunk).await.is_err() {
                        return Err(unavailable());
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) => {
                    tracing::info!(error = %e, "connector query stopped");
                    return Err(refused(err_codes::CONNECTOR_QUERY_INVALID));
                }
            }
        }
    }
    .await;
    if let Err(out) = copied {
        let _ = w.abort().await;
        return out;
    }
    if w.close().await.is_err() {
        return unavailable();
    }
    let (mut top, mut rows) = (String::new(), None);
    if let (Some(c), Some(_)) = (&cfg.cursor, &read) {
        let ident = sqlx::query_scalar::<_, String>("SELECT quote_ident($1::text)")
            .bind(&c.column)
            .fetch_one(&mut conn)
            .await;
        let Ok(ident) = ident else {
            return refused(err_codes::CONNECTOR_UNREACHABLE);
        };
        let sql = format!("SELECT max(q.{ident})::text, count(*) FROM ({query}\n) q");
        match sqlx::query_as::<_, (Option<String>, i64)>(sqlx::AssertSqlSafe(sql))
            .fetch_one(&mut conn)
            .await
        {
            Ok((v, n)) => {
                top = v.unwrap_or_default();
                rows = Some(n.max(0) as u64);
            }
            Err(e) => {
                tracing::info!(error = %e, "connector cursor not read");
                return Outcome::fail(
                    err_codes::CONNECTOR_QUERY_INVALID,
                    json!({ "cursor": c.column }),
                );
            }
        }
        let _ = (&mut conn).execute("COMMIT").await;
    }
    let _ = conn.close().await;
    staged(cfg, key, bytes, hasher, read.map(|r| r.full), top, rows)
}

/// The outcome of a step whose CSV is written: the next steps read it like a file.
fn staged(
    cfg: &IngestConfig,
    key: String,
    bytes: u64,
    hasher: Sha256,
    full_read: Option<bool>,
    cursor: String,
    read_rows: Option<u64>,
) -> Outcome {
    let sha256 = hex::encode(hasher.finalize());
    // No host or user in the journal (R4).
    let manifest = json!({
        "connector": cfg.connector,
        "bytes": bytes,
        "sha256": sha256,
        "fetched_at": Utc::now().to_rfc3339(),
        "full_read": full_read,
        "cursor": cursor,
        "read_rows": read_rows,
    });
    let entry = StagedEntry {
        name: FILE_NAME.to_string(),
        staging_key: key.clone(),
        bytes,
    };
    let mut out = Outcome::ok(
        IngestMsg {
            raw_key: key.clone(),
            staging_key: key,
            full_read,
            cursor,
            read_rows,
            ..Default::default()
        },
        manifest,
    );
    out.written = vec![entry];
    out.sha256 = Some(sha256);
    out.keep_until = Some(Utc::now() + chrono::Duration::days(cfg.keep_raw_days() as i64));
    out
}

/// Reads a REST API page after page into one CSV (connectors.md D793). Bodies count
/// against `cap`, the CSV against the staging room.
async fn rest(
    cx: &Cx<'_>,
    cfg: &IngestConfig,
    t: &mut connectors::RestTarget,
    cap: u64,
    room: u64,
    read: Option<Read>,
) -> Outcome {
    let Some(q) = cfg.rest.as_ref().filter(|q| q.check().is_none()) else {
        return refused(err_codes::CONNECTOR_QUERY_INVALID);
    };
    let key = cx.key("raw", FILE_NAME);
    let Ok(mut w) = cx.store.writer(&key).await else {
        return unavailable();
    };
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let since = match (&cfg.cursor, &read) {
        (Some(c), Some(r)) if !r.from.is_empty() => Some((c.param.clone(), r.from.clone())),
        _ => None,
    };
    let mut top = String::new();
    let mut count = 0u64;
    let pages = async {
        let first = t.url(&q.path);
        let (mut received, mut columns) = (0u64, None::<Vec<RestColumn>>);
        let (mut cursor, mut link) = (None::<String>, None::<reqwest::Url>);
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
                        if received > cap {
                            let args = json!({ "bytes": received, "max": cap });
                            return Err(Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, args));
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
            count += records.len() as u64;
            for r in records {
                csv.push_str(&rest::csv_line(
                    cols.iter().map(|c| rest::cell(r, &c.pointer)),
                ));
                if let Some(c) = &cfg.cursor {
                    let v = rest::cell(r, &c.column);
                    if !v.is_empty() && (top.is_empty() || cursor_after(&v, &top)) {
                        top = v;
                    }
                }
            }
            written += csv.len() as u64;
            if written > room {
                return Err(Outcome::fail(err_codes::INGEST_STAGING_QUOTA, Value::Null));
            }
            hasher.update(csv.as_bytes());
            if w.write(csv.into_bytes()).await.is_err() {
                return Err(unavailable());
            }
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
                return Ok(());
            }
        }
        let args = json!({ "max": q.max_pages() });
        Err(Outcome::fail(err_codes::CONNECTOR_REST_PAGE_LIMIT, args))
    }
    .await;
    if let Err(out) = pages {
        let _ = w.abort().await;
        return out;
    }
    if w.close().await.is_err() {
        return unavailable();
    }
    let rows = read.as_ref().map(|_| count);
    staged(cfg, key, written, hasher, read.map(|r| r.full), top, rows)
}

/// Calls read from an inbox per query.
const INBOX_BATCH: i64 = 1_000;
/// Records one run reads at most; the next run goes on from there.
const INBOX_RUN_MAX: u64 = 100_000;

/// Reads the calls a webhook kept past the last one read into a CSV (D795). The
/// cursor is the inbox id, implicit: the first run reads all, then each run only
/// the new calls, added to the table. Shape from `rest.records` and `rest.columns`:
/// without them a call is one record, or a list of records.
async fn inbox(cx: &Cx<'_>, cfg: &IngestConfig, connector: Uuid, cap: u64, room: u64) -> Outcome {
    let shape = cfg.rest.clone().unwrap_or_default();
    if shape.check().is_some() {
        return refused(err_codes::CONNECTOR_QUERY_INVALID);
    }
    let (stored, _) = cursor_state(cx).await;
    let from: i64 = stored.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0);
    // Calls past the retention go, read or not (connectors.md D795).
    let _ = cx
        .db
        .execute_raw(stmt(
            "DELETE FROM connector_inbox WHERE connector_id = $1 \
               AND received_at < now() - make_interval(days => $2::int)",
            vec![
                connector.into(),
                (crate::services::hooks::INBOX_DAYS as i32).into(),
            ],
        ))
        .await;
    let key = cx.key("raw", FILE_NAME);
    let Ok(mut w) = cx.store.writer(&key).await else {
        return unavailable();
    };
    let mut hasher = Sha256::new();
    let (mut written, mut received, mut count) = (0u64, 0u64, 0u64);
    let mut last = from;
    let mut columns = None::<Vec<RestColumn>>;
    let read = async {
        loop {
            let rows = cx
                .db
                .query_all_raw(stmt(
                    "SELECT id, body::text AS body FROM connector_inbox \
                     WHERE org_id = $1 AND connector_id = $2 AND id > $3 ORDER BY id LIMIT $4",
                    vec![
                        cx.node.org_id.into(),
                        connector.into(),
                        last.into(),
                        INBOX_BATCH.into(),
                    ],
                ))
                .await
                .map_err(|_| unavailable())?;
            if rows.is_empty() {
                return Ok(());
            }
            let mut csv = String::new();
            for row in &rows {
                let id: i64 = row.try_get("", "id").map_err(|_| unavailable())?;
                let text: String = row.try_get("", "body").map_err(|_| unavailable())?;
                received += text.len() as u64;
                if received > cap {
                    let args = json!({ "bytes": received, "max": cap });
                    return Err(Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, args));
                }
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
            written += csv.len() as u64;
            if written > room {
                return Err(Outcome::fail(err_codes::INGEST_STAGING_QUOTA, Value::Null));
            }
            hasher.update(csv.as_bytes());
            if w.write(csv.into_bytes()).await.is_err() {
                return Err(unavailable());
            }
            if count >= INBOX_RUN_MAX || (rows.len() as i64) < INBOX_BATCH {
                return Ok(());
            }
        }
    }
    .await;
    if let Err(out) = read {
        let _ = w.abort().await;
        return out;
    }
    if w.close().await.is_err() {
        return unavailable();
    }
    let top = if last > from {
        last.to_string()
    } else {
        String::new()
    };
    // An empty inbox is "nothing new", even before the first read: a first version
    // without a call would have no real columns for the next ones.
    let full = stored.is_none() && count > 0;
    staged(cfg, key, written, hasher, Some(full), top, Some(count))
}

/// One read-only statement on a MySQL connector, its rows as CSV (D792). No COPY:
/// rows come by the text protocol, each value as the server writes it.
async fn mysql(
    cx: &Cx<'_>,
    cfg: &IngestConfig,
    t: connectors::MyTarget,
    cap: u64,
    room: u64,
    read: Option<Read>,
) -> Outcome {
    use sqlx::{Column, Row, Statement};
    if cfg.query.trim().is_empty() || cfg.query.len() > QUERY_MAX_BYTES {
        return refused(err_codes::CONNECTOR_QUERY_INVALID);
    }
    let mut conn = match t.connect(QUERY_TIMEOUT).await {
        Ok(c) => c,
        Err(code) => return refused(code),
    };
    let mut query = cfg.query.clone();
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
            (Ok(_), Ok(lit)) => query = query.replace(CURSOR_PLACEHOLDER, &lit),
            _ => return refused(err_codes::CONNECTOR_UNREACHABLE),
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
        Ok(_) => return refused(err_codes::CONNECTOR_QUERY_INVALID),
        Err(e) => {
            let state = e
                .as_database_error()
                .and_then(|d| d.code())
                .map(|c| c.to_string());
            tracing::info!(error = %e, "connector query refused");
            return Outcome::fail(
                err_codes::CONNECTOR_QUERY_INVALID,
                json!({ "sqlstate": state }),
            );
        }
    };
    let key = cx.key("raw", FILE_NAME);
    let Ok(mut w) = cx.store.writer(&key).await else {
        return unavailable();
    };
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let copied = async {
        let mut chunk = rest::csv_line(&names);
        let mut stream = sqlx::raw_sql(sqlx::AssertSqlSafe(query.clone())).fetch(&mut conn);
        loop {
            let row = match stream.try_next().await {
                Ok(Some(row)) => Some(row),
                Ok(None) => None,
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
                bytes += chunk.len() as u64;
                if bytes > cap {
                    let args = json!({ "bytes": bytes, "max": cap });
                    return Err(Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, args));
                }
                if bytes > room {
                    return Err(Outcome::fail(err_codes::INGEST_STAGING_QUOTA, Value::Null));
                }
                hasher.update(chunk.as_bytes());
                if w.write(std::mem::take(&mut chunk).into_bytes())
                    .await
                    .is_err()
                {
                    return Err(unavailable());
                }
            }
            if row.is_none() {
                return Ok(());
            }
        }
    }
    .await;
    if let Err(out) = copied {
        let _ = w.abort().await;
        return out;
    }
    if w.close().await.is_err() {
        return unavailable();
    }
    let (mut top, mut rows) = (String::new(), None);
    if let (Some(c), Some(_)) = (&cfg.cursor, &read) {
        let ident = format!("`{}`", c.column.replace('`', "``"));
        let sql = format!("SELECT CAST(MAX(q.{ident}) AS CHAR), COUNT(*) FROM ({query}\n) q");
        match sqlx::query_as::<_, (Option<String>, i64)>(sqlx::AssertSqlSafe(sql))
            .fetch_one(&mut conn)
            .await
        {
            Ok((v, n)) => {
                top = v.unwrap_or_default();
                rows = Some(n.max(0) as u64);
            }
            Err(e) => {
                tracing::info!(error = %e, "connector cursor not read");
                return Outcome::fail(
                    err_codes::CONNECTOR_QUERY_INVALID,
                    json!({ "cursor": c.column }),
                );
            }
        }
        let _ = (&mut conn).execute("COMMIT").await;
    }
    let _ = conn.close().await;
    staged(cfg, key, bytes, hasher, read.map(|r| r.full), top, rows)
}
