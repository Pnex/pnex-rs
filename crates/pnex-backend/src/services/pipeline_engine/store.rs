//! The server side of the pipeline engine (pipeline-engine.md D937–D946):
//! the pipelines shipped in the packs, the registry of their rows, the
//! network under the egress guard, the org's tables, and the scheduler.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use std::sync::Arc;

use chrono::{DateTime, NaiveDateTime, Utc};
use include_dir::{include_dir, Dir};
use loco_rs::app::AppContext;
use pnex_core::pipeline::doc::{duration_ms, Document};
use pnex_core::pipeline::track;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::{json, Value};
use uuid::Uuid;

use super::blob::{Batches, Blob, Spool};
use super::{fail, run, Failure, Inputs, Req, RunOutcome, Sink, Transport};
use crate::services::artifact_store::S3Store;
use crate::services::datasets::{self, DatasetError};
use crate::services::db_lock::{ns, TenantLock};

static PACKS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/packs");

/// A pipeline a pack ships (D947): `packs/<pack>/pipelines/<key>.yaml`.
#[derive(Debug, Clone)]
pub struct Shipped {
    pub pack: String,
    pub doc: Document,
}

impl Shipped {
    /// Its name in the registry: `<pack>/<key>`.
    pub fn id(&self) -> String {
        format!("{}/{}", self.pack, self.doc.key)
    }
}

/// Every pipeline of the shipped packs, parsed once.
pub fn shipped() -> &'static [Shipped] {
    static ALL: LazyLock<Vec<Shipped>> = LazyLock::new(|| {
        let mut out = Vec::new();
        for pack in PACKS_DIR.dirs() {
            let key = pack.path().to_string_lossy().into_owned();
            let Some(dir) = PACKS_DIR.get_dir(format!("{key}/pipelines")) else {
                continue;
            };
            for f in dir.files() {
                let text = f.contents_utf8().expect("a shipped pipeline is UTF-8");
                let doc: Document = serde_saphyr::from_str(text)
                    .unwrap_or_else(|e| panic!("{}: {e}", f.path().display()));
                out.push(Shipped {
                    pack: key.clone(),
                    doc,
                });
            }
        }
        out
    });
    &ALL
}

/// A fixture of a pack, for its replay tests (D945).
pub fn fixture(pack: &str, name: &str) -> Option<&'static [u8]> {
    PACKS_DIR
        .get_file(format!("{pack}/fixtures/{name}"))
        .map(|f| f.contents())
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres, sql, values)
}

/// The state of the rows of one pipeline in one org.
pub async fn known(
    db: &impl ConnectionTrait,
    org: i64,
    pipeline: &str,
) -> Result<(HashMap<String, track::Item>, HashMap<String, String>), sea_orm::DbErr> {
    let rows = db
        .query_all_raw(stmt(
            "SELECT item, state, retry_after, failures, sha256 FROM pipeline_items \
             WHERE org_id = $1 AND pipeline = $2 AND item <> ''",
            vec![org.into(), pipeline.into()],
        ))
        .await?;
    let (mut items, mut shas) = (HashMap::new(), HashMap::new());
    for r in rows {
        let key: String = r.try_get("", "item")?;
        if let Some(fetch) = key.strip_prefix("@sha:") {
            if let Ok(Some(sha)) = r.try_get::<Option<String>>("", "sha256") {
                shas.insert(fetch.to_string(), sha);
            }
            continue;
        }
        let state = match r.try_get::<String>("", "state")?.as_str() {
            "done" => track::State::Done,
            "empty" => track::State::Empty,
            _ => track::State::Failed,
        };
        let retry: Option<DateTime<Utc>> = r.try_get("", "retry_after")?;
        items.insert(
            key.clone(),
            track::Item {
                key,
                state,
                retry_after_us: retry.map(|t| t.timestamp_micros()),
                failures: r.try_get::<i32>("", "failures")?.max(0) as u32,
            },
        );
    }
    Ok((items, shas))
}

async fn save(
    db: &impl ConnectionTrait,
    org: i64,
    pipeline: &str,
    item: &str,
    state: &str,
    failure: Option<&Failure>,
    retry_after_us: Option<i64>,
    failures: u32,
    rows: u64,
    sha: Option<&str>,
) -> Result<(), sea_orm::DbErr> {
    db.execute_raw(stmt(
        "INSERT INTO pipeline_items \
           (org_id, pipeline, item, state, code, args, retry_after, failures, rows, sha256, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, to_timestamp($7::float8 / 1e6), $8, $9, $10, now()) \
         ON CONFLICT (org_id, pipeline, item) DO UPDATE SET state = EXCLUDED.state, \
           code = EXCLUDED.code, args = EXCLUDED.args, retry_after = EXCLUDED.retry_after, \
           failures = EXCLUDED.failures, rows = EXCLUDED.rows, sha256 = EXCLUDED.sha256, \
           updated_at = now()",
        vec![
            org.into(),
            pipeline.into(),
            item.into(),
            state.into(),
            failure.map(|f| f.code.to_string()).into(),
            failure.map(|f| f.args.clone()).into(),
            retry_after_us.map(|us| us as f64).into(),
            (failures as i32).into(),
            (rows as i64).into(),
            sha.map(str::to_string).into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Keeps what a run did: each row taken, the bytes read, the run itself.
pub async fn record(
    db: &impl ConnectionTrait,
    org: i64,
    pipeline: &str,
    out: &RunOutcome,
) -> Result<(), sea_orm::DbErr> {
    let state = |s: track::State| match s {
        track::State::Done => "done",
        track::State::Empty => "empty",
        track::State::Failed => "failed",
    };
    for (item, failure, rows) in &out.items {
        let key = item.key.as_str();
        save(
            db,
            org,
            pipeline,
            key,
            state(item.state),
            failure.as_ref(),
            item.retry_after_us,
            item.failures,
            *rows,
            None,
        )
        .await?;
    }
    // The bytes of a source are kept only once the run that read them wrote.
    if out.failure.is_none() {
        for (fetch, sha) in &out.fresh_shas {
            let key = format!("@sha:{fetch}");
            save(db, org, pipeline, &key, "done", None, None, 0, 0, Some(sha)).await?;
        }
    }
    let run_state = match (&out.failure, out.written) {
        (Some(_), _) => "failed",
        (None, 0) => "empty",
        (None, _) => "done",
    };
    save(
        db,
        org,
        pipeline,
        "",
        run_state,
        out.failure.as_ref(),
        None,
        0,
        out.written,
        None,
    )
    .await
}

/// The Starlark functions of a shipped pack, by name: what its pipelines'
/// `call` steps run.
pub fn pack_functions(pack: &str) -> HashMap<String, String> {
    crate::services::ontology::packs::shipped()
        .into_iter()
        .find(|p| p.key == pack)
        .map(|p| {
            p.pipeline_functions
                .into_iter()
                .filter(|f| f.language == pnex_core::functions::FunctionLanguage::Starlark)
                .map(|f| (f.name, f.code))
                .collect()
        })
        .unwrap_or_default()
}

/// The code of the functions a document's calls name: `<name>` in its own
/// pack, `<pack>/<name>` in another shipped pack (one its pack builds on).
pub fn call_functions(own: &str, doc: &Document) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for s in &doc.steps {
        if let pnex_core::pipeline::Step::Call(c) = s {
            let (pack, name) = c
                .function
                .split_once('/')
                .unwrap_or((own, c.function.as_str()));
            if let Some(code) = pack_functions(pack).remove(name) {
                out.insert(c.function.clone(), code);
            }
        }
    }
    out
}

/// The settings and objects a document reads, as the org has them.
pub async fn inputs(
    db: &DatabaseConnection,
    org: i64,
    doc: &Document,
    now: NaiveDateTime,
    shas: HashMap<String, String>,
) -> Result<Inputs, sea_orm::DbErr> {
    let (settings, objects) = super::wanted(doc);
    let mut out = Inputs {
        now,
        shas,
        ..Default::default()
    };
    for key in settings {
        let v = crate::services::ontology::packs::setting_value(db, org, &key).await?;
        let text = match v {
            Some(Value::String(s)) => s,
            Some(Value::Null) | None => continue,
            Some(v) => v.to_string(),
        };
        out.settings.insert(key, text);
    }
    // Only shipped documents run here: their keys go to their own hosts (D942).
    let names: std::collections::BTreeSet<&str> = doc
        .steps
        .iter()
        .filter_map(|s| match s {
            pnex_core::pipeline::Step::Fetch(f) => f.secret.as_ref(),
            _ => None,
        })
        .flat_map(|s| {
            let grant = s
                .token
                .iter()
                .flat_map(|t| [t.user.as_str(), t.password.as_str()]);
            std::iter::once(s.name.as_str()).chain(grant)
        })
        .filter(|n| !n.is_empty())
        .collect();
    for name in names {
        use crate::services::secrets::store as vault;
        let Some(ring) = crate::services::markets::keyring() else {
            break;
        };
        // A key the org has not set: the fetch that needs it says so.
        let Ok(Some(row)) = vault::find_by_name(db, Some(org), name).await else {
            continue;
        };
        if let Ok(v) = vault::reveal(db, ring, Some(org), row.id).await {
            out.secrets.insert(name.to_string(), v.trim().to_string());
        }
    }
    for o in objects {
        let codes: Vec<String> = o
            .codes
            .as_ref()
            .and_then(|c| out.settings.get(&c.setting))
            .map(|raw| pnex_core::ontology::import::PerObjectSetting::codes(raw))
            .unwrap_or_default();
        // Linked: an open link of one of the types, either way; with codes,
        // the other end holds one. Without links, the object itself does.
        let mut sql = String::from(
            "SELECT DISTINCT ON (properties ->> $3) properties ->> $3 AS v, title, properties AS p \
             FROM objects \
             WHERE org_id = $1 AND type_key = $2 AND valid_to IS NULL AND properties ? $3 \
               AND (cardinality($5::text[]) = 0 OR cardinality($4::text[]) > 0 \
                 OR upper(properties ->> $6) = ANY($5)) \
               AND (cardinality($4::text[]) = 0 OR EXISTS (SELECT 1 FROM resource_edges e \
                 WHERE e.org_id = $1 AND e.relation = ANY($4) \
                   AND (e.source_object_id = objects.id OR e.target_object_id = objects.id) \
                   AND e.valid_from <= now() AND (e.valid_to IS NULL OR e.valid_to > now()) \
                   AND (cardinality($5::text[]) = 0 OR EXISTS (SELECT 1 FROM objects o \
                     WHERE o.org_id = $1 AND o.valid_to IS NULL \
                       AND o.id = CASE WHEN e.source_object_id = objects.id \
                         THEN e.target_object_id ELSE e.source_object_id END \
                       AND upper(o.properties ->> $6) = ANY($5)))))",
        );
        let mut values: Vec<sea_orm::Value> = vec![
            org.into(),
            o.type_key.clone().into(),
            o.property.clone().into(),
            o.linked.clone().into(),
            codes.into(),
            o.codes.as_ref().map(|c| c.property.clone()).into(),
        ];
        for (p, allowed) in &o.only {
            let n = values.len();
            sql.push_str(&format!(" AND properties ->> ${} = ANY(${})", n + 1, n + 2));
            values.push(p.clone().into());
            values.push(allowed.clone().into());
        }
        sql.push_str(" ORDER BY properties ->> $3");
        let rows = db.query_all_raw(stmt(&sql, values)).await?;
        let mut found = Vec::new();
        let mut extra = HashMap::new();
        for r in &rows {
            let Ok(v) = r.try_get::<String>("", "v") else {
                continue;
            };
            let mut cols = serde_json::Map::new();
            if let (Some(col), Ok(t)) = (&o.title, r.try_get::<String>("", "title")) {
                cols.insert(col.clone(), Value::String(t));
            }
            let props: Value = r.try_get("", "p").unwrap_or(Value::Null);
            for c in &o.columns {
                // Text, as the other axes give it.
                let cell = match props.get(c) {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Null) | None => None,
                    Some(other) => Some(other.to_string()),
                };
                if let Some(cell) = cell {
                    cols.insert(c.clone(), Value::String(cell));
                }
            }
            extra.insert(v.clone(), cols);
            found.push(v);
        }
        if o.title.is_some() || !o.columns.is_empty() {
            out.extra.insert(o.key(), extra);
        }
        out.objects.insert(o.key(), found);
    }
    Ok(out)
}

/// Writes into the org's tables, each `append` one transaction.
pub struct DatasetSink<'a> {
    pub db: &'a DatabaseConnection,
    pub store: &'a S3Store,
    pub org: i64,
}

async fn table_of(db: &DatabaseConnection, org: i64, name: &str) -> Result<Uuid, DatasetError> {
    let find = || async {
        db.query_one_raw(stmt(
            "SELECT id FROM datasets WHERE org_id = $1 AND name = $2",
            vec![org.into(), name.into()],
        ))
        .await?
        .map(|r| r.try_get::<Uuid>("", "id"))
        .transpose()
    };
    if let Some(id) = find().await? {
        return Ok(id);
    }
    match datasets::create(db, org, name).await {
        Ok(d) => d.id.parse().map_err(|_| DatasetError::NotFound),
        // Created meanwhile by another pod.
        Err(DatasetError::NameTaken) => find().await?.ok_or(DatasetError::NotFound),
        Err(e) => Err(e),
    }
}

impl Sink for DatasetSink<'_> {
    async fn append(
        &mut self,
        table: &str,
        replace: bool,
        files: Vec<Arc<Batches>>,
    ) -> Result<u64, Failure> {
        let out = async {
            let id = table_of(self.db, self.org, table).await?;
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            // Read back batch by batch: a month of ticks never sits in memory.
            let reader = tokio::task::spawn_blocking(move || -> Result<(), Failure> {
                for f in files {
                    for b in f.read()? {
                        if tx.blocking_send(Some(b?)).is_err() {
                            return Ok(());
                        }
                    }
                }
                let _ = tx.blocking_send(None);
                Ok(())
            });
            let kind = if replace {
                pnex_core::datasets::DatasetTransactionKind::Snapshot
            } else {
                pnex_core::datasets::DatasetTransactionKind::Append
            };
            let written = datasets::write_stream(self.db, self.store, self.org, id, kind, rx).await;
            if let Ok(Err(f)) = reader.await {
                tracing::warn!(code = f.code, "pipeline batches not read back");
            }
            written
        }
        .await;
        match out {
            Ok(t) => Ok(t.map_or(0, |t| t.rows.max(0) as u64)),
            Err(DatasetError::QuotaExceeded) => Err(fail("quota", json!({ "table": table }))),
            Err(DatasetError::SchemaMismatch { columns }) => Err(fail(
                "schema",
                json!({ "table": table, "columns": columns }),
            )),
            Err(e) => {
                tracing::warn!(
                    org = self.org,
                    table,
                    code = e.code(),
                    "pipeline table not written"
                );
                Err(fail("storage", json!({ "table": table })))
            }
        }
    }

    async fn ranges(&mut self, rows: Vec<serde_json::Map<String, Value>>) -> Result<u64, Failure> {
        use crate::services::time_ranges::{self, import};
        let failed = || fail("storage", json!({ "what": "time_ranges" }));
        let scope = time_ranges::resolve_scope(self.db, self.org, "org", "")
            .await
            .map_err(|_| failed())?;
        let rows = rows
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let text = |k: &str| {
                    r.get(k)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .filter(|v| !v.trim().is_empty())
                };
                import::Row {
                    line: i as u32 + 1,
                    input: pnex_core::time_range::TimeRangeInput {
                        external_id: text("external_id"),
                        label: text("label"),
                        planned_start: text("planned_start"),
                        planned_end: text("planned_end"),
                        category: text("category"),
                        source_url: text("source_url"),
                        attrs: text("description").map(|d| json!({ "description": d })),
                        ..Default::default()
                    },
                }
            })
            .collect();
        let out = import::run(self.db, self.org, &scope, rows)
            .await
            .map_err(|_| failed())?;
        if out.rejected > 0 {
            tracing::info!(org = self.org, rejected = out.rejected, errors = ?out.errors, "pipeline time ranges refused");
        }
        Ok(u64::from(out.created + out.updated))
    }
}

/// The network: egress guard (R8), redirects on the same host only (D942),
/// one call per `every` per host for the whole process (D938).
pub struct HttpTransport;

// shortcut: the cadence is per process; share it across pods (Valkey) when
// the pipelines run on more than one.
static LAST_CALL: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

fn client() -> &'static reqwest::Client {
    static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .redirect(reqwest::redirect::Policy::custom(|a| {
                let same = a.previous().first().map(|u| u.host_str()) == Some(a.url().host_str());
                if same && a.previous().len() < 5 {
                    a.follow()
                } else {
                    a.stop()
                }
            }))
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(120))
            .user_agent(concat!("PNEX-Pipeline/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    });
    &HTTP
}

// Hosts the tests point at their mock server (never set outside tests).
static TEST_ROUTES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// Sends the requests of `host` to `base` (`http://127.0.0.1:port`): the
/// integration tests' mock of a source a shipped pipeline names.
pub fn route_for_tests(host: &str, base: &str) {
    let mut routes = TEST_ROUTES.lock().unwrap_or_else(|e| e.into_inner());
    routes.insert(
        host.to_ascii_lowercase(),
        base.trim_end_matches('/').to_string(),
    );
}

fn routed(url: &reqwest::Url) -> reqwest::Url {
    let routes = TEST_ROUTES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(base) = url.host_str().and_then(|h| routes.get(h)) else {
        return url.clone();
    };
    let rest = &url.as_str()[url.origin().ascii_serialization().len()..];
    reqwest::Url::parse(&format!("{base}{rest}")).unwrap_or_else(|_| url.clone())
}

async fn wait_turn(url: &reqwest::Url, every_ms: u64) {
    let mocked = {
        let routes = TEST_ROUTES.lock().unwrap_or_else(|e| e.into_inner());
        url.host_str().is_some_and(|h| routes.contains_key(h))
    };
    // A test's mock has no rate to keep.
    if every_ms == 0 || mocked {
        return;
    }
    let host = url.host_str().unwrap_or_default().to_string();
    let wait = {
        let mut last = LAST_CALL.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let at = last
            .get(&host)
            .map(|t| *t + Duration::from_millis(every_ms))
            .filter(|t| *t > now)
            .unwrap_or(now);
        last.insert(host, at);
        at - now
    };
    tokio::time::sleep(wait).await;
}

async fn body(
    url: &reqwest::Url,
    req: reqwest::RequestBuilder,
    max: u64,
) -> Result<Option<Blob>, Failure> {
    let unreachable = || fail("unreachable", json!({ "url": url.as_str() }));
    let mut resp = req.send().await.map_err(|e| {
        tracing::info!(url = %url, error = %e.without_url(), "pipeline source unreachable");
        unreachable()
    })?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(fail(
            "http_status",
            json!({ "url": url.as_str(), "status": resp.status().as_u16() }),
        ));
    }
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(fail("too_large", json!({ "max": max })));
    }
    let mut out = Spool::new(max);
    while let Some(chunk) = resp.chunk().await.map_err(|_| unreachable())? {
        out.push(&chunk)?;
    }
    Ok(Some(out.finish()))
}

/// The headers and the secret parameter of a request; an invalid header
/// value fails the request instead of going out raw.
fn dressed(mut b: reqwest::RequestBuilder, r: &Req) -> reqwest::RequestBuilder {
    for (k, v) in &r.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    if let Some((k, v)) = &r.secret_query {
        b = b.query(&[(k, v)]);
    }
    b
}

/// The client of a streamed file: it follows redirects by hand, each to a
/// host of the document (a release served from a storage host).
fn plain_client() -> &'static reqwest::Client {
    static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
        pnex_core::egress::guarded(reqwest::Client::builder())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(120))
            .user_agent(concat!("PNEX-Pipeline/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client")
    });
    &HTTP
}

impl Transport for HttpTransport {
    async fn get(&self, url: &reqwest::Url, r: &Req) -> Result<Option<Blob>, Failure> {
        wait_turn(url, r.every_ms).await;
        body(url, dressed(client().get(routed(url)), r), r.max_bytes).await
    }

    async fn stream(
        &self,
        url: &reqwest::Url,
        r: &Req,
        out: &super::ChunkTx,
    ) -> Result<bool, Failure> {
        wait_turn(url, r.every_ms).await;
        let unreachable = || fail("unreachable", json!({ "url": url.as_str() }));
        let mut at = url.clone();
        let mut resp = None;
        for _ in 0..5 {
            let x = dressed(plain_client().get(routed(&at)), r)
                .send()
                .await
                .map_err(|e| {
                    tracing::info!(url = %url, error = %e.without_url(), "pipeline source unreachable");
                    unreachable()
                })?;
            if !x.status().is_redirection() {
                resp = Some(x);
                break;
            }
            let next = x
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
                .and_then(|l| at.join(l).ok())
                .ok_or_else(unreachable)?;
            // Only to a host the document declares (D942), never plain HTTP.
            let host = next.host_str().unwrap_or_default().to_ascii_lowercase();
            if next.scheme() != "https" || !r.hosts.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
                return Err(fail("host_not_declared", json!({ "host": host })));
            }
            at = next;
        }
        let mut resp = resp.ok_or_else(unreachable)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !resp.status().is_success() {
            return Err(fail(
                "http_status",
                json!({ "url": url.as_str(), "status": resp.status().as_u16() }),
            ));
        }
        while let Some(chunk) = resp.chunk().await.map_err(|_| unreachable())? {
            if out.send(Ok(chunk.to_vec())).await.is_err() {
                return Err(fail("closed", Value::Null));
            }
        }
        Ok(true)
    }

    async fn post_form(
        &self,
        url: &reqwest::Url,
        referer: Option<&reqwest::Url>,
        fields: &[(String, String)],
        r: &Req,
    ) -> Result<Option<Blob>, Failure> {
        wait_turn(url, r.every_ms).await;
        let mut req = client().post(routed(url)).form(fields);
        // The page a form came from, as a browser sends it; never another host's.
        if let Some(p) = referer.filter(|p| p.host_str() == url.host_str()) {
            req = req.header(reqwest::header::REFERER, p.as_str());
        }
        body(url, dressed(req, r), r.max_bytes).await
    }
}

/// The steps of an import's run in its history (the `ingest_steps` journal
/// an import's steps write, read by `ingest::run_steps`): one line per step,
/// its rows and the bytes it read, its error with its code and arguments.
pub async fn journal(
    db: &impl ConnectionTrait,
    org: i64,
    import_id: Uuid,
    run_id: Uuid,
    steps: &[super::StepRecord],
) -> Result<(), sea_orm::DbErr> {
    for s in steps {
        let mut manifest = json!({ "rows": s.rows });
        if let Some(u) = &s.url {
            manifest["url"] = json!(u);
        }
        if let Some(f) = &s.failure {
            manifest["args"] = f.args.clone();
        }
        db.execute_raw(stmt(
            "INSERT INTO ingest_steps (id, org_id, flow_id, flow_version, node_id, run_id, step, \
               port, code, manifest, bytes) VALUES ($1, $2, 0, 0, $3, $4, $5, $6, $7, $8, $9)",
            vec![
                Uuid::new_v4().into(),
                org.into(),
                format!("import:{import_id}").into(),
                run_id.into(),
                s.step.into(),
                // Ports of the step journal: 0 done, 1 error.
                i16::from(s.failure.is_some()).into(),
                s.failure.as_ref().map(|f| f.code.to_string()).into(),
                pnex_core::ingest::bounded_manifest(manifest).into(),
                (s.bytes as i64).into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

/// Runs one pipeline for one org now, and keeps what it did.
pub async fn run_for_org(
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    sp: &Shipped,
    transport: &impl Transport,
    now: DateTime<Utc>,
    journal: Option<&mut Vec<super::StepRecord>>,
) -> Result<RunOutcome, sea_orm::DbErr> {
    let id = sp.id();
    let (items, shas) = known(db, org, &id).await?;
    let mut inputs = inputs(db, org, &sp.doc, now.naive_utc(), shas).await?;
    inputs.functions = call_functions(&sp.pack, &sp.doc);
    let mut sink = DatasetSink { db, store, org };
    let sql = super::sql::WorkerSql { db, store, org };
    let out = run(
        &sp.doc, &inputs, transport, &mut sink, &sql, &items, None, None, journal,
    )
    .await;
    record(db, org, &id, &out).await?;
    for (item, failure, _) in &out.items {
        if let Some(f) = failure {
            tracing::info!(org, pipeline = %id, item = %item.key, code = f.code, args = %f.args, "pipeline row failed");
        }
    }
    if let Some(f) = &out.failure {
        tracing::warn!(org, pipeline = %id, code = f.code, args = %f.args, "pipeline run failed");
    }
    Ok(out)
}

async fn last_run(
    db: &DatabaseConnection,
    org: i64,
    pipeline: &str,
) -> Result<Option<DateTime<Utc>>, sea_orm::DbErr> {
    db.query_one_raw(stmt(
        "SELECT updated_at FROM pipeline_items WHERE org_id = $1 AND pipeline = $2 AND item = ''",
        vec![org.into(), pipeline.into()],
    ))
    .await?
    .map(|r| r.try_get("", "updated_at"))
    .transpose()
}

/// One pass of the scheduler (D946): every scheduled pipeline of every pack
/// an org installed, when its last run is older than its schedule.
pub async fn sweep(
    ctx: &AppContext,
    transport: &impl Transport,
    now: DateTime<Utc>,
) -> Result<(), DatasetError> {
    let store = datasets::store_of(&ctx.config)?;
    for sp in shipped() {
        let Some(every) = sp.doc.schedule.as_deref().and_then(duration_ms) else {
            continue;
        };
        let orgs: Vec<i64> = ctx
            .db
            .query_all_raw(stmt(
                "SELECT org_id FROM packs WHERE key = $1 ORDER BY org_id",
                vec![sp.pack.clone().into()],
            ))
            .await?
            .iter()
            .filter_map(|r| r.try_get("", "org_id").ok())
            .collect();
        for org in orgs {
            if sp.doc.large && datasets::quota_override(&ctx.db, org).await?.is_none() {
                continue;
            }
            let Ok(lock) =
                TenantLock::acquire(&ctx.db, ns::PIPELINE_RUN, org, Duration::from_secs(1)).await
            else {
                continue;
            };
            let due = last_run(&ctx.db, org, &sp.id())
                .await?
                .is_none_or(|t| (now - t).num_milliseconds() >= every as i64);
            if due {
                run_for_org(&ctx.db, &store, org, sp, transport, now, None).await?;
            }
            lock.release().await;
        }
    }
    Ok(())
}

/// The scheduler, once a minute.
pub fn spawn(ctx: &AppContext) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        loop {
            tick.tick().await;
            if let Err(e) = sweep(&ctx, &HttpTransport, Utc::now()).await {
                tracing::warn!(code = e.code(), "pipeline scheduler pass stopped");
            }
        }
    });
}
