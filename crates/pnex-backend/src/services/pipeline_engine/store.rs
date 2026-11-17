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
            "waiting" => track::State::Waiting,
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
        track::State::Waiting => "waiting",
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
            pnex_core::pipeline::Step::Fetch(f) => Some(f),
            _ => None,
        })
        .flat_map(|f| {
            let grant = f
                .secret
                .iter()
                .flat_map(|s| s.token.iter())
                .flat_map(|t| [t.user.as_str(), t.password.as_str()]);
            let names = f.secret.iter().map(|s| s.name.as_str());
            names.chain(grant).chain(f.proxy.as_deref())
        })
        .filter(|n| !n.is_empty())
        .collect();
    for name in names {
        use crate::services::secrets::store as vault;
        // A key the org has not set: the fetch that needs it says so.
        let Ok(Some(row)) = vault::find_by_name(db, Some(org), name).await else {
            continue;
        };
        // Set but unreadable: kept empty, so that the fetch refuses instead of
        // calling without it (a proxy skipped would be a direct call).
        let value = match crate::services::markets::keyring() {
            Some(ring) => vault::reveal(db, ring, Some(org), row.id).await.ok(),
            None => None,
        };
        out.secrets.insert(
            name.to_string(),
            value.map(|v| v.trim().to_string()).unwrap_or_default(),
        );
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
    /// The application, for the targets that write media; an import's run
    /// has none.
    pub ctx: Option<&'a AppContext>,
    /// `<pack>/<key>` of the pipeline run.
    pub pipeline: &'a str,
    /// The member who asked the run: a navigation plays in their browser
    /// agent (D782).
    pub user: Option<i64>,
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
    async fn has_table(&mut self, table: &str) -> Result<bool, Failure> {
        self.db
            .query_one_raw(stmt(
                "SELECT 1 AS one FROM datasets WHERE org_id = $1 AND name = $2",
                vec![self.org.into(), table.into()],
            ))
            .await
            .map(|r| r.is_some())
            .map_err(|_| fail("storage", json!({ "table": table })))
    }

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

    async fn media(
        &mut self,
        target: &pnex_core::pipeline::doc::MediaTarget,
        rows: Vec<serde_json::Map<String, Value>>,
    ) -> Result<u64, Failure> {
        use crate::services::crawl::{self, documents};
        let Some(ctx) = self.ctx else {
            return Err(fail("step_unsupported", json!({ "step": "write.media" })));
        };
        let mut kept = 0;
        for r in rows {
            let text = |k: &str| {
                r.get(k)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
            };
            let (Some(body), Some(url), Some(kind), Some(by), Some(key)) = (
                text("text"),
                text("source_url"),
                text("type_key"),
                text("key_property"),
                text("key"),
            ) else {
                continue;
            };
            let Some((object, _)) = object_by(self.db, self.org, &kind, &by, &key).await? else {
                continue;
            };
            // A file name: an address in it keeps its words, not its slashes.
            let name: String = text("name")
                .unwrap_or_else(|| "document.txt".into())
                .chars()
                .map(|c| {
                    if matches!(c, '/' | '\\' | ':' | '?' | '*' | '"' | '<' | '>' | '|') {
                        '_'
                    } else {
                        c
                    }
                })
                .collect();
            // The upload allowlist reads the extension: the body is plain text.
            let name = match name.ends_with(".txt") {
                true => name,
                false => format!("{name}.txt"),
            };
            let body = body.into_bytes();
            let sha = crawl::sha256_hex(&body);
            let stored = async {
                let asset = documents::store(
                    ctx,
                    self.org,
                    &target.source,
                    &url,
                    &name,
                    "text/plain",
                    &sha,
                    body,
                )
                .await
                .map_err(|e| format!("store: {e}"))?;
                documents::link(ctx, self.org, &target.link_type, object, asset, &url, &sha)
                    .await
                    .map_err(|e| format!("link: {e}"))?;
                kept_for(self.db, self.org, asset, object, target)
                    .await
                    .map_err(|e| format!("kept: {e}"))
            }
            .await;
            match stored {
                Ok(()) => kept += 1,
                Err(e) => {
                    tracing::warn!(org = self.org, url = %url, error = %e, "pipeline document not kept");
                    return Err(fail("storage", json!({ "what": "media" })));
                }
            }
        }
        Ok(kept)
    }

    async fn proposals(
        &mut self,
        target: &pnex_core::pipeline::doc::ProposalsTarget,
        rows: Vec<serde_json::Map<String, Value>>,
    ) -> Result<u64, Failure> {
        use pnex_core::ontology::proposal::{ProposalInput, ProposalKind};
        let failed = || fail("storage", json!({ "what": "proposals" }));
        let mut made = 0;
        for r in rows {
            let text = |k: &str| {
                r.get(k)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
            };
            // A JSON object written as text by a function or a query.
            let json_of = |k: &str| match r.get(k) {
                Some(Value::String(t)) => serde_json::from_str(t).unwrap_or(Value::Null),
                Some(v) => v.clone(),
                None => Value::Null,
            };
            let (Some(kind), Some(by), Some(key)) =
                (text("type_key"), text("key_property"), text("key"))
            else {
                continue;
            };
            // The object is gone or was never known: nothing to propose.
            let Some((id, version)) = object_by(self.db, self.org, &kind, &by, &key).await? else {
                continue;
            };
            let values = json_of("values").as_object().cloned().unwrap_or_default();
            let mut input = ProposalInput {
                kind: Some(ProposalKind::ObjectUpdate),
                type_key: kind,
                target_id: Some(id.to_string()),
                source_id: None,
                link_target_id: None,
                title: text("title"),
                values,
                base_version: Some(version),
                evidence: json_of("evidence"),
                origin: target.origin.clone(),
                confidence: target.confidence,
            };
            if let Some(link) = text("link_type") {
                let (Some(to_type), Some(to_by), Some(to_key)) =
                    (text("to_type"), text("to_property"), text("to_key"))
                else {
                    continue;
                };
                let Some((to, _)) = object_by(self.db, self.org, &to_type, &to_by, &to_key).await?
                else {
                    continue;
                };
                input.kind = Some(ProposalKind::LinkCreate);
                input.type_key = link;
                input.target_id = None;
                input.source_id = Some(id.to_string());
                input.link_target_id = Some(to.to_string());
                input.base_version = None;
            }
            match crate::services::ontology::proposals::propose(self.db, self.org, &input).await {
                Ok(_) => made += 1,
                Err(crate::services::ontology::OntologyError::Db(_)) => return Err(failed()),
                Err(e) => tracing::info!(org = self.org, error = %e, "pipeline proposal refused"),
            }
        }
        Ok(made)
    }

    async fn objects(
        &mut self,
        target: &pnex_core::pipeline::doc::ObjectsTarget,
        rows: Vec<serde_json::Map<String, Value>>,
    ) -> Result<u64, Failure> {
        // An entry of a type the org does not have is left out with its
        // links: a pack enriches what another pack brought, when it is there.
        let schema = crate::services::ontology::schema(self.db, self.org)
            .await
            .map_err(|_| fail("storage", json!({ "what": "objects" })))?;
        let objects: Vec<_> = target
            .objects
            .iter()
            .filter(|o| schema.object_type(&o.type_key).is_some())
            .collect();
        let links: Vec<_> = target
            .links
            .iter()
            .filter(|l| {
                schema.link_type(&l.link_type).is_some()
                    && [&l.from, &l.to]
                        .iter()
                        .all(|a| objects.iter().any(|o| &o.alias == *a))
            })
            .collect();
        if objects.is_empty() {
            return Ok(0);
        }
        let mapping: pnex_core::ontology::import::Mapping = serde_json::from_value(json!({
            "key": self.pipeline.replace('/', "_"),
            "name": self.pipeline,
            "objects": objects,
            "links": links,
        }))
        .map_err(|_| fail("internal", Value::Null))?;
        // One provenance per pipeline: its writes find their own objects again.
        let provenance = Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("pipeline:{}", self.pipeline).as_bytes(),
        );
        let rows: Vec<Value> = rows.into_iter().map(Value::Object).collect();
        let source_ref = format!("pipeline:{}", self.pipeline);
        crate::services::ontology::imports::write_rows(
            self.db, self.org, provenance, &source_ref, &mapping, &rows,
        )
        .await
        .map(|n| n as u64)
        .map_err(|e| {
            tracing::info!(org = self.org, pipeline = self.pipeline, error = %e, "pipeline objects not written");
            fail("objects_refused", Value::Null)
        })
    }

    async fn browse(
        &mut self,
        b: &pnex_core::pipeline::doc::Browse,
        row: &super::Row,
        item: &str,
    ) -> Result<Option<Vec<super::Row>>, Failure> {
        use crate::services::site_recipes::purpose;
        // shortcut: an import's run has no application, so it cannot browse;
        // pass the context to imports when a pack's import needs a navigation.
        let Some(ctx) = self.ctx else {
            return Err(fail("step_unsupported", json!({ "step": "browse" })));
        };
        let params: std::collections::BTreeMap<String, String> = row
            .cols
            .keys()
            .map(|k| (k.clone(), row.text(k).unwrap_or_default()))
            .collect();
        let recipe = pnex_core::site_recipe::Recipe {
            name: format!("{} · {}", self.pipeline, b.as_),
            site: b.site.clone(),
            allow_hosts: Vec::new(),
            params: params.keys().cloned().collect(),
            browser: None,
            captures: None,
            session: b.session.clone(),
            steps: b.steps.clone(),
        };
        // JSON is YAML: the run keeps the text of what it played.
        let yaml = serde_json::to_string(&recipe).map_err(|_| fail("internal", Value::Null))?;
        let why = purpose::Purpose::Pipeline {
            pipeline: self.pipeline.to_string(),
            item: item.to_string(),
        };
        match purpose::start(ctx, self.org, self.user, &yaml, params, &why).await {
            Ok(_) => Ok(None),
            Err(crate::services::site_recipes::StartError::NoAgent) => {
                Err(fail("no_agent", Value::Null))
            }
            Err(crate::services::site_recipes::StartError::DailyCap { max }) => {
                Err(fail("daily_cap", json!({ "max": max })))
            }
            Err(e) => {
                tracing::info!(org = self.org, pipeline = self.pipeline, error = ?e, "navigation not started");
                Err(fail("browse_refused", Value::Null))
            }
        }
    }
}

/// The live object of a type whose property holds `value`: its id and version.
/// What a media target asks once a document is kept: its purge date, and
/// its id stamped in place on the object (a stamp, not an edit).
async fn kept_for(
    db: &DatabaseConnection,
    org: i64,
    asset: Uuid,
    object: Uuid,
    target: &pnex_core::pipeline::doc::MediaTarget,
) -> Result<(), sea_orm::DbErr> {
    if let Some(days) = target.keep_days {
        db.execute_raw(stmt(
            "UPDATE media_assets SET metadata = coalesce(metadata, '{}'::jsonb) \
               || jsonb_build_object('purge_after', (now() + make_interval(days => $3::int))::date::text) \
             WHERE id = $1 AND org_id = $2",
            vec![asset.into(), org.into(), (days as i32).into()],
        ))
        .await?;
    }
    if let Some(property) = &target.stamp {
        db.execute_raw(stmt(
            "UPDATE objects SET properties = properties || jsonb_build_object($3::text, $4::text) \
             WHERE org_id = $1 AND id = $2 AND valid_to IS NULL",
            vec![
                org.into(),
                object.into(),
                property.clone().into(),
                asset.to_string().into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

/// A live object of `kind` whose property `by` (or its `id`) is `value`.
async fn object_by(
    db: &DatabaseConnection,
    org: i64,
    kind: &str,
    by: &str,
    value: &str,
) -> Result<Option<(Uuid, i32)>, Failure> {
    let failed = || fail("storage", json!({ "what": "proposals" }));
    let sql = match by {
        "id" => {
            "SELECT id, version FROM objects WHERE org_id = $1 AND type_key = $2 \
                   AND valid_to IS NULL AND $3 = 'id' AND id::text = $4 LIMIT 1"
        }
        _ => {
            "SELECT id, version FROM objects WHERE org_id = $1 AND type_key = $2 \
                AND valid_to IS NULL AND properties ->> $3 = $4 ORDER BY id LIMIT 1"
        }
    };
    let Some(row) = db
        .query_one_raw(stmt(
            sql,
            vec![org.into(), kind.into(), by.into(), value.into()],
        ))
        .await
        .map_err(|_| failed())?
    else {
        return Ok(None);
    };
    let id = row.try_get::<Uuid>("", "id").map_err(|_| failed())?;
    let version = row.try_get::<i32>("", "version").map_err(|_| failed())?;
    Ok(Some((id, version)))
}

/// The network: egress guard (R8), redirects on the same host only (D942),
/// one call per `every` per host for the whole process (D938).
pub struct HttpTransport;

// shortcut: the cadence is per process; share it across pods (Valkey) when
// the pipelines run on more than one.
static LAST_CALL: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

fn builder() -> reqwest::ClientBuilder {
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
}

/// The client of a request: the shared one, or one per proxy of an org.
fn client(r: &Req) -> Result<reqwest::Client, Failure> {
    static HTTP: LazyLock<reqwest::Client> =
        LazyLock::new(|| builder().build().expect("reqwest client"));
    static PROXIED: LazyLock<Mutex<HashMap<String, reqwest::Client>>> =
        LazyLock::new(Default::default);
    let Some(proxy) = &r.proxy else {
        return Ok(HTTP.clone());
    };
    let mut cache = PROXIED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.get(proxy.as_str()) {
        return Ok(c.clone());
    }
    // The proxy's address may carry its credentials: never in an error.
    let invalid = || fail("proxy_invalid", Value::Null);
    let c = reqwest::Proxy::all(proxy.clone())
        .ok()
        .and_then(|p| builder().proxy(p).build().ok())
        .ok_or_else(invalid)?;
    cache.insert(proxy.to_string(), c.clone());
    Ok(c)
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

/// The host of a paced request: `None` without a cadence, or for a test's
/// mock, which has no rate to keep.
fn paced_host(url: &reqwest::Url, every_ms: u64) -> Option<String> {
    let host = url.host_str()?;
    let mocked = TEST_ROUTES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(host);
    (every_ms > 0 && !mocked).then(|| host.to_string())
}

// Paced hosts that pushed back: pushes in a row and when they may be asked again.
// shortcut: per process, and only on get and form posts; share it like LAST_CALL.
static COOLDOWN: LazyLock<Mutex<HashMap<String, (u32, Instant)>>> = LazyLock::new(Default::default);

/// The pause after the n-th push back in a row: a minute, doubled each
/// time, an hour at most.
pub(super) fn cooldown(pushes: u32) -> Duration {
    Duration::from_secs((60u64 << pushes.saturating_sub(1).min(6)).min(3600))
}

/// Refuses a paced host still cooling down, without a request.
fn cooled(url: &reqwest::Url, every_ms: u64) -> Result<(), Failure> {
    let Some(host) = paced_host(url, every_ms) else {
        return Ok(());
    };
    let now = Instant::now();
    let cooling = COOLDOWN.lock().unwrap_or_else(|e| e.into_inner());
    match cooling.get(&host).filter(|(_, until)| *until > now) {
        Some((_, until)) => Err(fail(
            "throttled",
            json!({ "host": host, "seconds": (*until - now).as_secs() }),
        )),
        None => Ok(()),
    }
}

/// A push back lengthens the host's pause; an answer ends it.
fn settle<T>(url: &reqwest::Url, every_ms: u64, out: &Result<T, Failure>) {
    let Some(host) = paced_host(url, every_ms) else {
        return;
    };
    let mut cooling = COOLDOWN.lock().unwrap_or_else(|e| e.into_inner());
    match out {
        Ok(_) => {
            cooling.remove(&host);
        }
        Err(f) if super::throttled(f) => {
            let pushes = cooling.get(&host).map_or(0, |(n, _)| *n) + 1;
            cooling.insert(host, (pushes, Instant::now() + cooldown(pushes)));
        }
        Err(_) => {}
    }
}

async fn wait_turn(url: &reqwest::Url, every_ms: u64) {
    let Some(host) = paced_host(url, every_ms) else {
        return;
    };
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
    // A range past the last record holds nothing, like a missing page.
    if matches!(
        resp.status(),
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::RANGE_NOT_SATISFIABLE
    ) {
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
        cooled(url, r.every_ms)?;
        wait_turn(url, r.every_ms).await;
        let out = body(url, dressed(client(r)?.get(routed(url)), r), r.max_bytes).await;
        settle(url, r.every_ms, &out);
        out
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
        cooled(url, r.every_ms)?;
        wait_turn(url, r.every_ms).await;
        let mut req = client(r)?.post(routed(url)).form(fields);
        // The page a form came from, as a browser sends it; never another host's.
        if let Some(p) = referer.filter(|p| p.host_str() == url.host_str()) {
            req = req.header(reqwest::header::REFERER, p.as_str());
        }
        let out = body(url, dressed(req, r), r.max_bytes).await;
        settle(url, r.every_ms, &out);
        out
    }

    async fn post_json(
        &self,
        url: &reqwest::Url,
        json_body: &Value,
        r: &Req,
    ) -> Result<Option<Blob>, Failure> {
        wait_turn(url, r.every_ms).await;
        let req = client(r)?.post(routed(url)).json(json_body);
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
    run_with(None, db, store, org, sp, transport, now, journal).await
}

/// [`run_for_org`] with the application: every write target works.
pub async fn run_in(
    ctx: &AppContext,
    org: i64,
    sp: &Shipped,
    transport: &impl Transport,
    now: DateTime<Utc>,
) -> Result<RunOutcome, DatasetError> {
    let store = datasets::store_of(&ctx.config)?;
    Ok(run_with(Some(ctx), &ctx.db, &store, org, sp, transport, now, None).await?)
}

#[allow(clippy::too_many_arguments)]
async fn run_with(
    ctx: Option<&AppContext>,
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    sp: &Shipped,
    transport: &impl Transport,
    now: DateTime<Utc>,
    journal: Option<&mut Vec<super::StepRecord>>,
) -> Result<RunOutcome, sea_orm::DbErr> {
    run_core(
        ctx,
        db,
        store,
        org,
        sp,
        transport,
        now,
        journal,
        Vec::new(),
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_given(
    ctx: Option<&AppContext>,
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    sp: &Shipped,
    transport: &impl Transport,
    now: DateTime<Utc>,
    asked: Vec<serde_json::Map<String, Value>>,
    user: Option<i64>,
) -> Result<RunOutcome, sea_orm::DbErr> {
    run_core(ctx, db, store, org, sp, transport, now, None, asked, user).await
}

#[allow(clippy::too_many_arguments)]
async fn run_core(
    ctx: Option<&AppContext>,
    db: &DatabaseConnection,
    store: &S3Store,
    org: i64,
    sp: &Shipped,
    transport: &impl Transport,
    now: DateTime<Utc>,
    journal: Option<&mut Vec<super::StepRecord>>,
    asked: Vec<serde_json::Map<String, Value>>,
    user: Option<i64>,
) -> Result<RunOutcome, sea_orm::DbErr> {
    let id = sp.id();
    let (items, shas) = known(db, org, &id).await?;
    let mut inputs = inputs(db, org, &sp.doc, now.naive_utc(), shas).await?;
    inputs.functions = call_functions(&sp.pack, &sp.doc);
    inputs.asked = asked;
    let mut sink = DatasetSink {
        db,
        store,
        org,
        ctx,
        pipeline: &id,
        user,
    };
    let sql = super::sql::WorkerSql { db, store, org };
    let out = run(
        &sp.doc, &inputs, transport, &mut sink, &sql, &items, None, None, journal,
    )
    .await;
    record(db, org, &id, &out).await?;
    for (item, failure, _) in &out.items {
        if let Some(f) = failure.as_ref().filter(|f| f.code != super::WAITING) {
            tracing::info!(org, pipeline = %id, item = %item.key, code = f.code, args = %f.args, "pipeline row failed");
        }
    }
    if let Some(f) = &out.failure {
        tracing::warn!(org, pipeline = %id, code = f.code, args = %f.args, "pipeline run failed");
    }
    Ok(out)
}

/// Runs a pipeline of a pack for the rows a route asks (its `asked` axis,
/// D996), now, under the org's pipeline lock; what it did is kept like any
/// run's. A row whose navigation still runs is left as it is.
pub async fn run_asked(
    ctx: &AppContext,
    org: i64,
    user: Option<i64>,
    pipeline: &str,
    asked: Vec<serde_json::Map<String, Value>>,
) -> Result<RunOutcome, DatasetError> {
    let sp = shipped()
        .iter()
        .find(|s| s.id() == pipeline)
        .ok_or(DatasetError::NotFound)?;
    let lock = TenantLock::acquire(&ctx.db, ns::PIPELINE_RUN, org, Duration::from_secs(30))
        .await
        .map_err(|_| DatasetError::TransactionOpen)?;
    let store = datasets::store_of(&ctx.config)?;
    let out = run_given(
        Some(ctx),
        &ctx.db,
        &store,
        org,
        sp,
        &HttpTransport,
        Utc::now(),
        asked,
        user,
    )
    .await;
    lock.release().await;
    Ok(out?)
}

/// Resumes the row `item` of a pipeline once its navigation ended (D995):
/// `extracts` = (name, text) of the navigation, `None` when it failed. A row
/// no longer waiting (resumed already, taken again) is left as it is.
/// Whether the extracts may stay with the navigation (`keep`).
pub async fn resume_row(
    ctx: &AppContext,
    org: i64,
    pipeline: &str,
    item: &str,
    extracts: Option<Vec<(String, String)>>,
) -> Result<bool, DatasetError> {
    let Some(sp) = shipped().iter().find(|s| s.id() == pipeline) else {
        return Ok(true);
    };
    let keep = match sp.doc.steps.get(1) {
        Some(pnex_core::pipeline::doc::Step::Browse(b)) => b.keep,
        _ => true,
    };
    let lock = TenantLock::acquire(&ctx.db, ns::PIPELINE_RUN, org, Duration::from_secs(120))
        .await
        .map_err(|_| DatasetError::TransactionOpen)?;
    let out = resume_locked(ctx, org, sp, item, extracts).await;
    lock.release().await;
    out.map(|_| keep)
}

async fn resume_locked(
    ctx: &AppContext,
    org: i64,
    sp: &Shipped,
    item: &str,
    extracts: Option<Vec<(String, String)>>,
) -> Result<(), DatasetError> {
    let id = sp.id();
    let waiting = ctx
        .db
        .query_one_raw(stmt(
            "SELECT args FROM pipeline_items \
             WHERE org_id = $1 AND pipeline = $2 AND item = $3 AND state = 'waiting'",
            vec![org.into(), id.clone().into(), item.into()],
        ))
        .await?;
    let Some(args) = waiting.and_then(|r| r.try_get::<Value>("", "args").ok()) else {
        return Ok(());
    };
    let row = match args.get("row") {
        Some(Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    let (items, shas) = known(&ctx.db, org, &id).await?;
    let mut inputs = inputs(&ctx.db, org, &sp.doc, Utc::now().naive_utc(), shas).await?;
    inputs.functions = call_functions(&sp.pack, &sp.doc);
    let store = datasets::store_of(&ctx.config)?;
    let mut sink = DatasetSink {
        db: &ctx.db,
        store: &store,
        org,
        ctx: Some(ctx),
        pipeline: &id,
        user: None,
    };
    let sql = super::sql::WorkerSql {
        db: &ctx.db,
        store: &store,
        org,
    };
    let extracts = extracts.map(super::extract_rows);
    let (it, failure, rows) = super::resume(
        &sp.doc,
        &inputs,
        &HttpTransport,
        &mut sink,
        &sql,
        items.get(item),
        item,
        row,
        extracts,
    )
    .await;
    if let Some(f) = &failure {
        tracing::info!(org, pipeline = %id, item, code = f.code, args = %f.args, "pipeline row failed");
    }
    let state = match it.state {
        track::State::Done => "done",
        track::State::Empty => "empty",
        track::State::Failed => "failed",
        track::State::Waiting => "waiting",
    };
    save(
        &ctx.db,
        org,
        &id,
        item,
        state,
        failure.as_ref(),
        it.retry_after_us,
        it.failures,
        rows,
        None,
    )
    .await?;
    Ok(())
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
                run_with(Some(ctx), &ctx.db, &store, org, sp, transport, now, None).await?;
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
