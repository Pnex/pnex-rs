//! The pipeline engine (pipelines.md D934–D953): runs a pipeline
//! document step by step over named working tables. The transport, the
//! sink and the registry are given by the caller: the server passes the
//! network, the org's tables and `pipeline_items`; the replay tests (D945)
//! pass recorded answers and memory.

pub mod blob;
mod files;
pub mod flow_ask;
pub mod read;
pub mod sql;
pub mod store;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::NaiveDateTime;
use pnex_core::pipeline::doc::{Axis, Case, Document, Enumerate, Fetch, Method, Step};
use pnex_core::pipeline::{periods, template, track};
use serde_json::{json, Map, Value};

/// Rows one enumeration produces at most.
pub const ENUMERATE_MAX: usize = 100_000;
/// Body of a fetch when the step names no cap.
pub const FETCH_DEFAULT_BYTES: u64 = 256 * 1024 * 1024;
/// Rows of a tracked enumeration a run takes when the step says nothing.
pub const PER_RUN_DEFAULT: u32 = 10;

/// Why a step stopped: a machine code and its arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub code: &'static str,
    pub args: Value,
}

pub fn fail(code: &'static str, args: Value) -> Failure {
    Failure { code, args }
}

/// Whether a source pushed back (429, 503, a dropped connection, or a host
/// still cooling down): the run stops instead of asking it for the next rows.
pub fn throttled(f: &Failure) -> bool {
    match f.code {
        "unreachable" | "throttled" => true,
        "http_status" => matches!(f.args["status"].as_u64(), Some(429 | 503)),
        _ => false,
    }
}

/// A row of a working table: its columns and, for a fetched or unpacked
/// row, the bytes of its file.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub cols: Map<String, Value>,
    pub file: Option<Arc<blob::Blob>>,
}

impl Row {
    pub fn text(&self, name: &str) -> Option<String> {
        match self.cols.get(name)? {
            Value::String(s) => Some(s.clone()),
            Value::Null => None,
            v => Some(v.to_string()),
        }
    }
}

/// A working table (D935): rows of the network steps, or typed batches of
/// a `read`.
#[derive(Debug, Clone)]
pub enum Table {
    Rows(Vec<Row>),
    Batches(Vec<Arc<blob::Batches>>),
}

impl Table {
    pub fn len(&self) -> usize {
        match self {
            Table::Rows(r) => r.len(),
            Table::Batches(b) => b.iter().map(|b| b.rows as usize).sum(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Chunks of a streamed body; `Err` tells the reader the stream was cut.
pub type ChunkTx = tokio::sync::mpsc::Sender<Result<Vec<u8>, ()>>;

/// Where requests go: the network under the egress guard, or recorded
/// answers. `None` is a 404.
#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn get(&self, url: &reqwest::Url, req: &Req) -> Result<Option<blob::Blob>, Failure>;

    /// Sends the body to `out` as it arrives, never kept; `false` is a 404.
    /// By default the answer of `get` in chunks (recorded answers).
    async fn stream(&self, url: &reqwest::Url, req: &Req, out: &ChunkTx) -> Result<bool, Failure> {
        let Some(body) = self.get(url, req).await? else {
            return Ok(false);
        };
        for c in body.bytes(u64::MAX)?.chunks(64 * 1024) {
            if out.send(Ok(c.to_vec())).await.is_err() {
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
        req: &Req,
    ) -> Result<Option<blob::Blob>, Failure>;

    /// Posts a JSON body. By default the answer recorded for the address.
    async fn post_json(
        &self,
        url: &reqwest::Url,
        _body: &Value,
        req: &Req,
    ) -> Result<Option<blob::Blob>, Failure> {
        self.get(url, req).await
    }
}

/// How one request is sent.
#[derive(Debug, Clone, Default)]
pub struct Req {
    /// One call per this many ms on the host (D938).
    pub every_ms: u64,
    pub max_bytes: u64,
    pub headers: Vec<(String, String)>,
    /// A vault key as a query parameter: added when sent, never part of the
    /// address kept or logged.
    pub secret_query: Option<(String, String)>,
    /// Hosts a streamed request may be redirected to: the document's (D942).
    pub hosts: Vec<String>,
    /// The org's proxy (`proxy` of the fetch), checked by the egress guard.
    pub proxy: Option<reqwest::Url>,
}

/// Where a `write` puts its rows: the org's tables, or memory.
#[allow(async_fn_in_trait)]
pub trait Sink {
    /// Appends the batches to the table as one transaction; the rows written.
    /// `replace`: the transaction is the whole table from its commit on.
    async fn append(
        &mut self,
        table: &str,
        replace: bool,
        batches: Vec<Arc<blob::Batches>>,
    ) -> Result<u64, Failure>;

    /// Whether the org holds this table already.
    async fn has_table(&mut self, table: &str) -> Result<bool, Failure>;

    /// The rows of an org table when they come from elsewhere than the
    /// worker (a replay's fixture); `None`: the worker reads it.
    async fn org_table(&mut self, _table: &str) -> Result<Option<Arc<blob::Batches>>, Failure> {
        Ok(None)
    }

    /// Upserts the org's time ranges by `external_id`; the rows written.
    async fn ranges(&mut self, rows: Vec<Map<String, Value>>) -> Result<u64, Failure>;

    /// Keeps documents of the org linked to objects; the documents kept.
    async fn media(
        &mut self,
        target: &pnex_core::pipeline::doc::MediaTarget,
        rows: Vec<Map<String, Value>>,
    ) -> Result<u64, Failure>;

    /// Files proposals to validate; the proposals made.
    async fn proposals(
        &mut self,
        target: &pnex_core::pipeline::doc::ProposalsTarget,
        rows: Vec<Map<String, Value>>,
    ) -> Result<u64, Failure>;

    /// Creates and updates the org's objects and links the rows map to
    /// (D941), without preview; the objects and links written.
    async fn objects(
        &mut self,
        target: &pnex_core::pipeline::doc::ObjectsTarget,
        rows: Vec<Map<String, Value>>,
    ) -> Result<u64, Failure>;

    /// Asks the org's AI provider for the extractor's fields in the `text`
    /// column of each row (D841); the rows of the items kept, by
    /// [`item_rows`].
    async fn model(
        &mut self,
        step: &pnex_core::pipeline::doc::Model,
        extractor: &pnex_core::extractor::Extractor,
        rows: Vec<Map<String, Value>>,
    ) -> Result<Vec<Row>, Failure>;

    /// Asks these rows again of the pipeline once the run ends; the rows.
    async fn ask(&mut self, rows: Vec<Map<String, Value>>) -> Result<u64, Failure>;

    /// Plays a navigation for the row `item` (D995): its extracts as
    /// `{name, text}` rows when known now (a replay), `None` once started:
    /// the row waits and the end of the navigation resumes it.
    async fn browse(
        &mut self,
        b: &pnex_core::pipeline::doc::Browse,
        row: &Row,
        item: &str,
    ) -> Result<Option<Vec<Row>>, Failure>;
}

/// The rows of a document's items (D841): the input row's columns but its
/// text, the item's rank, its values and quotes as JSON text, and what was
/// left out of the whole document (`rejected` values, `parts_skipped`).
pub fn item_rows(
    input: &Map<String, Value>,
    text: &str,
    checked: &pnex_core::extractor::Checked,
    parts_skipped: usize,
) -> Vec<Row> {
    checked
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut cols: Map<String, Value> = input
                .iter()
                .filter(|(k, _)| k.as_str() != text)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            cols.insert("item".into(), Value::String(i.to_string()));
            cols.insert(
                "values".into(),
                Value::String(Value::Object(item.values.clone()).to_string()),
            );
            cols.insert(
                "evidence".into(),
                Value::String(Value::Object(item.quotes.clone()).to_string()),
            );
            cols.insert(
                "rejected".into(),
                Value::String(checked.rejected.len().to_string()),
            );
            cols.insert(
                "parts_skipped".into(),
                Value::String(parts_skipped.to_string()),
            );
            Row { cols, file: None }
        })
        .collect()
}

/// The code of the failure that pauses a row on its navigation (D995).
pub const WAITING: &str = "waiting";

/// The `{name, text}` rows of a navigation's extracts.
pub fn extract_rows(extracts: impl IntoIterator<Item = (String, String)>) -> Vec<Row> {
    extracts
        .into_iter()
        .map(|(name, text)| Row {
            cols: Map::from_iter([
                ("name".to_string(), Value::String(name)),
                ("text".to_string(), Value::String(text)),
            ]),
            file: None,
        })
        .collect()
}

/// An empty table of text columns.
fn empty_text(cols: &[String]) -> Result<Arc<blob::Batches>, Failure> {
    use arrow_schema::{DataType, Field, Schema};
    let fields: Vec<Field> = cols
        .iter()
        .map(|c| Field::new(c, DataType::Utf8, true))
        .collect();
    blob::BatchWriter::new(Arc::new(Schema::new(fields)))?.finish()
}

/// The rows of a table as JSON records, timestamps in RFC 3339 UTC.
fn records_of(files: &[Arc<blob::Batches>]) -> Result<Vec<Map<String, Value>>, Failure> {
    let mut out = Vec::new();
    for f in files {
        let utc: Vec<bool> = f
            .schema
            .fields()
            .iter()
            .map(|x| matches!(x.data_type(), arrow_schema::DataType::Timestamp(..)))
            .collect();
        for b in f.read()? {
            let rows = crate::services::datasets::parquet::records(&b?)
                .map_err(|_| fail("storage", json!({ "what": "batches" })))?;
            for row in rows {
                let Value::Object(mut row) = row else {
                    continue;
                };
                for (field, ts) in f.schema.fields().iter().zip(&utc) {
                    // `2026-12-10T13:30:00` read back from a UTC column.
                    if let (true, Some(Value::String(t))) = (ts, row.get_mut(field.name())) {
                        if !t.ends_with('Z') && !t.contains('+') {
                            t.push('Z');
                        }
                    }
                }
                out.push(row);
            }
        }
    }
    Ok(out)
}

/// What a run reads besides the network, loaded before it starts.
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    /// `<pack>.<key>` → value of the org's settings the document names.
    pub settings: HashMap<String, String>,
    /// Values of the org's live objects, by `ObjectsAxis::key`.
    pub objects: HashMap<String, Vec<String>>,
    /// Same key → value → the axis' title and other columns, by column.
    pub extra: HashMap<String, HashMap<String, Map<String, Value>>>,
    /// Starlark code of the functions of the document's pack, by name.
    pub functions: HashMap<String, String>,
    /// Extractors the document's `model` steps name, by name (D841).
    pub extractors: HashMap<String, pnex_core::extractor::Extractor>,
    pub now: NaiveDateTime,
    /// Vault name → value of the secrets a shipped document names.
    pub secrets: HashMap<String, String>,
    /// `<fetch as>:<url>` → sha256 of the bytes last read (`skip_if_unchanged`).
    pub shas: HashMap<String, String>,
    /// The rows the start of the run gives to its `asked` axis (D996).
    pub asked: Vec<Map<String, Value>>,
}

/// The settings and objects a document reads.
pub fn wanted(doc: &Document) -> (Vec<String>, Vec<pnex_core::pipeline::doc::ObjectsAxis>) {
    let (mut settings, mut objects) = (doc.requires.clone(), doc.object_hosts.clone());
    for step in &doc.steps {
        match step {
            Step::Enumerate(e) => {
                for a in &e.cross {
                    settings.extend(a.setting.iter().chain(&a.from_setting).cloned());
                    if let Some(o) = &a.objects {
                        settings.extend(o.codes.iter().map(|c| c.setting.clone()));
                        objects.push(o.clone());
                    }
                }
            }
            Step::Fetch(f) => {
                let templates = f.params.values().chain(f.headers.values());
                for t in std::iter::once(&f.url).chain(templates) {
                    settings.extend(
                        template::names(t)
                            .into_iter()
                            .filter_map(|n| n.strip_prefix("setting.").map(str::to_string)),
                    );
                }
            }
            _ => {}
        }
    }
    settings.sort();
    settings.dedup();
    (settings, objects)
}

fn lookup(row: &Row, inputs: &Inputs, name: &str) -> Option<String> {
    // The run's day and instant, UTC: a window relative to the run.
    match name {
        "run_date" => return Some(inputs.now.date().to_string()),
        "run_now" => return Some(inputs.now.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        _ => {}
    }
    match name.strip_prefix("setting.") {
        // An empty setting is no value: a source that needs it is not asked.
        Some(key) => inputs
            .settings
            .get(key)
            .filter(|v| !v.trim().is_empty())
            .cloned(),
        None => row.text(name),
    }
}

/// A start or end of periods: a template of the row, then the forms of
/// `parse_start_at`.
fn instant(
    raw: &str,
    per: pnex_core::pipeline::doc::Period,
    name: &str,
    row: &Row,
    inputs: &Inputs,
) -> Result<chrono::NaiveDateTime, Failure> {
    let text = template::fill(raw, false, |n| lookup(row, inputs, n))
        .map_err(|n| fail("missing_value", json!({ "name": n })))?;
    // A row's instant is RFC 3339 (`2024-03-05T00:00:00Z`).
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&text) {
        return Ok(t.naive_utc());
    }
    periods::parse_start_at(per, &text, inputs.now)
        .ok_or_else(|| fail("invalid_value", json!({ "name": name, "value": text })))
}

/// The values of one axis for one row: (label, columns) per value.
fn axis_values(
    a: &Axis,
    inputs: &Inputs,
    row: &Row,
) -> Result<Vec<(String, Map<String, Value>)>, Failure> {
    if let Some(per) = a.periods {
        let from = a
            .from_setting
            .as_ref()
            .and_then(|k| inputs.settings.get(k))
            .filter(|v| !v.trim().is_empty())
            .or(a.from.as_ref());
        // A source whose start is an empty setting is not read.
        let Some(from) = from else {
            return Ok(Vec::new());
        };
        let start = instant(from, per, &a.as_, row, inputs)?;
        let end = match &a.to {
            Some(t) => instant(t, per, &a.as_, row, inputs)?,
            None => inputs.now,
        };
        return Ok(periods::starts(per, start, end, a.closed_only)
            .into_iter()
            .map(|s| {
                let parts = periods::parts(per, s, &a.as_);
                let label = parts[0].1.clone();
                let cols = parts
                    .into_iter()
                    .map(|(k, v)| (k, Value::String(v)))
                    .collect();
                (label, cols)
            })
            .collect());
    }
    let raw: Vec<String> = if let Some([lo, hi]) = a.range {
        (lo..=hi).map(|n| n.to_string()).collect()
    } else if let Some(k) = &a.setting {
        inputs
            .settings
            .get(k)
            .map(|v| v.split(',').map(str::to_string).collect())
            .unwrap_or_default()
    } else if let Some(o) = &a.objects {
        inputs.objects.get(&o.key()).cloned().unwrap_or_default()
    } else if a.asked.is_some() {
        inputs
            .asked
            .iter()
            .filter_map(|r| r.get(&a.as_).and_then(Value::as_str).map(str::to_string))
            .collect()
    } else {
        a.values.clone()
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for v in raw {
        if a.exclude
            .iter()
            .any(|g| pnex_core::pipeline::glob_match(g, v.trim()))
        {
            continue;
        }
        let v = match &a.split {
            Some(sep) => v.split(sep.as_str()).next().unwrap_or_default().to_string(),
            None => v,
        };
        let v = match a.case {
            Some(Case::Upper) => v.trim().to_uppercase(),
            Some(Case::Lower) => v.trim().to_lowercase(),
            None => v.trim().to_string(),
        };
        if v.is_empty() || !seen.insert(v.clone()) {
            continue;
        }
        let mut cols = Map::new();
        cols.insert(a.as_.clone(), Value::String(v.clone()));
        if let Some(asked) = &a.asked {
            let given = inputs
                .asked
                .iter()
                .find(|r| r.get(&a.as_).and_then(Value::as_str).map(str::trim) == Some(v.as_str()));
            for col in asked {
                let cell = given.and_then(|r| r.get(col)).cloned();
                cols.insert(col.clone(), cell.unwrap_or(Value::Null));
            }
        }
        if let Some(o) = &a.objects {
            let extra = inputs.extra.get(&o.key()).and_then(|e| e.get(&v));
            for col in o.title.iter().chain(&o.columns) {
                let cell = extra.and_then(|e| e.get(col)).cloned();
                cols.insert(col.clone(), cell.unwrap_or(Value::Null));
            }
        }
        // A value no glob of a derived column matches is left out.
        let derived: Option<Vec<(String, Value)>> = a
            .derive
            .iter()
            .map(|(col, rules)| {
                rules
                    .iter()
                    .find(|[glob, _]| pnex_core::pipeline::glob_match(glob, &v))
                    .map(|[_, value]| (col.clone(), Value::String(value.clone())))
            })
            .collect();
        let Some(derived) = derived else { continue };
        cols.extend(derived);
        out.push((v, cols));
    }
    Ok(out)
}

/// The rows of an enumeration with their keys (the labels of the axes
/// joined by `/`): each row of `base` crossed with the axes.
pub fn enumerate(
    e: &Enumerate,
    inputs: &Inputs,
    base: Vec<Row>,
) -> Result<Vec<(String, Row)>, Failure> {
    let mut out: Vec<(String, Row)> = base.into_iter().map(|r| (String::new(), r)).collect();
    for a in &e.cross {
        let mut next = Vec::new();
        for (key, row) in &out {
            for (label, cols) in axis_values(a, inputs, row)? {
                if next.len() >= ENUMERATE_MAX {
                    return Err(fail("too_many_rows", json!({ "max": ENUMERATE_MAX })));
                }
                let mut r = row.clone();
                r.cols.extend(cols);
                let key = if key.is_empty() {
                    label
                } else {
                    format!("{key}/{label}")
                };
                next.push((key, r));
            }
        }
        out = next;
    }
    Ok(out)
}

/// What one step did, for the run history: its rows, the bytes it read, the
/// first address it fetched, or why it stopped.
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub step: &'static str,
    pub rows: u64,
    pub bytes: u64,
    pub url: Option<String>,
    pub failure: Option<Failure>,
}

impl StepRecord {
    fn of(step: &Step, result: &Result<Option<Table>, Failure>, written: u64) -> Self {
        let mut r = StepRecord {
            step: step.name(),
            rows: written,
            bytes: 0,
            url: None,
            failure: None,
        };
        match result {
            Ok(Some(t)) => {
                r.rows = t.len() as u64;
                if let Table::Rows(rows) = t {
                    r.bytes = rows
                        .iter()
                        .filter_map(|x| x.file.as_ref())
                        .map(|f| f.len())
                        .sum();
                    if matches!(step, Step::Fetch(_)) {
                        r.url = rows.first().and_then(|x| x.text("url"));
                    }
                }
            }
            Ok(None) => {}
            Err(f) => r.failure = Some(f.clone()),
        }
        r
    }
}

/// One pass of the steps of a document over its working tables.
pub struct Exec<'a, T, S, Q> {
    pub doc: &'a Document,
    pub transport: &'a T,
    pub sink: &'a mut S,
    pub sql: &'a Q,
    pub inputs: &'a Inputs,
    /// Every working table of the pass, kept for the replay tests.
    pub keep: Option<&'a mut HashMap<String, Vec<Table>>>,
    /// One line per step run (an import's run history).
    pub journal: Option<&'a mut Vec<StepRecord>>,
    /// Sha256 of the bytes read by a `skip_if_unchanged` fetch.
    pub fresh_shas: HashMap<String, String>,
    /// Tokens got by a fetch's grant during the pass, by grant address.
    pub tokens: HashMap<String, String>,
    /// Tables a streamed fetch already gave for the steps after it.
    pub ahead: HashMap<String, Table>,
    /// Key of the tracked row the pass runs for (a navigation's resume).
    pub item: Option<String>,
}

impl<T: Transport, S: Sink, Q: sql::SqlRunner> Exec<'_, T, S, Q> {
    /// Hosts of the addresses the org's objects hold (`object_hosts`, D975).
    fn object_hosts(&self) -> Vec<String> {
        self.doc
            .object_hosts
            .iter()
            .filter_map(|a| self.inputs.objects.get(&a.key()))
            .flatten()
            .filter_map(|u| {
                Some(
                    reqwest::Url::parse(u)
                        .ok()?
                        .host_str()?
                        .to_ascii_lowercase(),
                )
            })
            .collect()
    }

    fn hosts_allow(&self, url: &reqwest::Url) -> Result<(), Failure> {
        self.host_in(url, true)
    }

    /// `with_objects`: the org's object hosts count too; never for a request
    /// carrying a secret.
    fn host_in(&self, url: &reqwest::Url, with_objects: bool) -> Result<(), Failure> {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let declared = self
            .doc
            .hosts
            .iter()
            .any(|h| h.trim().eq_ignore_ascii_case(&host));
        let of_objects = !declared && with_objects && self.object_hosts().contains(&host);
        if !matches!(url.scheme(), "http" | "https") || !(declared || of_objects) {
            return Err(fail("host_not_declared", json!({ "host": host })));
        }
        // An org typed this host: an IP literal skips the guarded resolver (R8).
        if let Some(code) = of_objects
            .then(|| pnex_core::egress::check_url(url))
            .flatten()
        {
            return Err(fail(code, json!({ "host": host })));
        }
        Ok(())
    }

    /// The document's hosts and the org's object hosts (streamed redirects).
    fn all_hosts(&self) -> Vec<String> {
        let mut hosts = self.doc.hosts.clone();
        hosts.extend(self.object_hosts());
        hosts
    }

    /// The rows of a keyed write the table does not hold yet, each key once.
    async fn unseen(
        &mut self,
        t: &pnex_core::pipeline::doc::TableTarget,
        files: Vec<Arc<blob::Batches>>,
    ) -> Result<Vec<Arc<blob::Batches>>, Failure> {
        let file = match files.len() {
            0 => return Ok(files),
            1 => files[0].clone(),
            _ => blocking(move || blob::merged(&files)).await?,
        };
        if file.rows == 0 {
            return Ok(Vec::new());
        }
        let columns: Vec<String> = file
            .schema
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        if let Some(k) = t.key.iter().find(|k| !columns.contains(k)) {
            return Err(fail("unknown_column", json!({ "column": k })));
        }
        let q = |c: &str| format!("\"{c}\"");
        let cols: Vec<String> = columns.iter().map(|c| format!("f.{}", q(c))).collect();
        let keys: Vec<String> = t.key.iter().map(|k| q(k)).collect();
        let mut sql = format!(
            "SELECT {} FROM (SELECT *, row_number() OVER (PARTITION BY {}) AS pnex_rank \
             FROM pnex_fresh) AS f WHERE f.pnex_rank = 1",
            cols.join(", "),
            keys.join(", ")
        );
        let mut tables = std::collections::BTreeMap::new();
        if self.sink.has_table(&t.name).await? {
            let same: Vec<String> = keys.iter().map(|k| format!("k.{k} = f.{k}")).collect();
            sql.push_str(&format!(
                " AND NOT EXISTS (SELECT 1 FROM pnex_kept AS k WHERE {})",
                same.join(" AND ")
            ));
            tables.insert("pnex_kept".to_string(), t.name.clone());
        }
        let out = self
            .sql
            .run(&sql, vec![("pnex_fresh".to_string(), file)], &tables)
            .await?;
        Ok(out.into_iter().collect())
    }

    async fn fetch(&mut self, f: &Fetch, input: Vec<Row>) -> Result<Vec<Row>, Failure> {
        let mut out = Vec::new();
        for row in input {
            let Some(pages) = &f.pages else {
                out.extend(self.fetch_one(f, &row, None).await?);
                continue;
            };
            // Pages from 1 until one holds no record or the last one allowed.
            for page in pages.from..pages.from + pages.max {
                let Some(next) = self.fetch_one(f, &row, Some(page)).await? else {
                    break;
                };
                let (file, until) = (next.file.clone(), pages.until.clone());
                let records = blocking(move || {
                    let doc: Value = match &file {
                        Some(b) => serde_json::from_slice(&b.bytes(read::READ_MAX_BYTES as u64)?)
                            .unwrap_or(Value::Null),
                        None => Value::Null,
                    };
                    Ok(doc
                        .pointer(&until)
                        .and_then(Value::as_array)
                        .map_or(0, Vec::len))
                });
                let records = records.await?;
                if records == 0 {
                    break;
                }
                out.push(next);
                // A page of records shorter than asked is the last.
                if pages.size.is_some_and(|n| records < n as usize) {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// The token of a grant, asked once per pass: the org's user and
    /// password go only to the grant's address, on a host of the document.
    async fn token(
        &mut self,
        t: &pnex_core::pipeline::doc::TokenGrant,
        every: u64,
    ) -> Result<String, Failure> {
        if let Some(v) = self.tokens.get(&t.url) {
            return Ok(v.clone());
        }
        let secret = |name: &str| {
            self.inputs
                .secrets
                .get(name)
                .filter(|v| !v.is_empty())
                .cloned()
                .ok_or_else(|| fail("secret_required", json!({ "name": name })))
        };
        let (user, password) = (secret(&t.user)?, secret(&t.password)?);
        let (user_field, password_field) = t.kind.names();
        let url = reqwest::Url::parse(&t.url)
            .map_err(|_| fail("invalid_value", json!({ "name": "token.url" })))?;
        self.host_in(&url, false)?;
        let mut fields: Vec<(String, String)> =
            vec![(user_field.into(), user), (password_field.into(), password)];
        fields.extend(t.fields.iter().map(|(k, v)| (k.clone(), v.clone())));
        let req = Req {
            every_ms: every,
            max_bytes: 64 * 1024,
            ..Default::default()
        };
        // A refusal says nothing of the secrets: only that the key is missing or wrong.
        let refused = || fail("secret_required", json!({ "name": t.user }));
        let body = self
            .transport
            .post_form(&url, None, &fields, &req)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        let doc: Value = serde_json::from_slice(&body.bytes(64 * 1024)?).map_err(|_| refused())?;
        let token = doc
            .get(&t.field)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(refused)?
            .to_string();
        self.tokens.insert(t.url.clone(), token.clone());
        Ok(token)
    }

    /// One answer for one row (one page of it): `None` when there is none.
    /// The address of a row (its parameters added, its host declared) and
    /// the headers sent.
    fn address(
        &self,
        f: &Fetch,
        row: &Row,
    ) -> Result<(reqwest::Url, Vec<(String, String)>), Failure> {
        let filled = |t: &str, address: bool| {
            template::fill(t, address, |n| lookup(row, self.inputs, n))
                .map_err(|n| fail("missing_value", json!({ "name": n })))
        };
        let raw = filled(&f.url, true)?;
        let mut url = reqwest::Url::parse(&raw)
            .map_err(|_| fail("invalid_value", json!({ "name": "url", "value": raw })))?;
        self.hosts_allow(&url)?;
        for (k, v) in &f.params {
            let v = filled(v, false)?;
            url.query_pairs_mut().append_pair(k, &v);
        }
        let mut headers = Vec::new();
        for (k, v) in &f.headers {
            // A header carries what a source asks of the caller (its contact).
            let value = filled(v, false).map_err(|f| fail("contact_required", f.args))?;
            headers.push((k.clone(), value));
        }
        Ok((url, headers))
    }

    /// A streamed fetch run with the `unpack` and `read` it feeds: each
    /// row's parts are one stream, its tar entries read as they pass,
    /// nothing kept but the typed rows. The rows fetched, without a file.
    async fn streamed(&mut self, f: &Fetch, input: Vec<Row>) -> Result<Vec<Row>, Failure> {
        let (u, r) = self
            .doc
            .streamed(&f.as_)
            .ok_or_else(|| fail("step_unsupported", json!({ "step": "fetch.stream" })))?;
        let every = f
            .every
            .as_deref()
            .and_then(pnex_core::pipeline::doc::duration_ms)
            .unwrap_or(0);
        let max = f.max_bytes.unwrap_or(FETCH_DEFAULT_BYTES);
        let parts = if f.parts.is_empty() {
            vec![String::new()]
        } else {
            f.parts.clone()
        };
        let (mut fetched, mut typed) = (Vec::new(), Vec::new());
        for row in input {
            let mut addresses = Vec::new();
            for p in &parts {
                let mut with_part = row.clone();
                with_part
                    .cols
                    .insert("part".into(), Value::String(p.clone()));
                addresses.push(self.address(f, &with_part)?);
            }
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            let (u2, r2, row2) = (u.clone(), r.clone(), row.clone());
            let decode =
                tokio::task::spawn_blocking(move || files::unpack_stream(&u2, &r2, &row2, rx, max));
            let mut pumped = Ok(false);
            for (i, (url, headers)) in addresses.iter().enumerate() {
                let req = Req {
                    every_ms: every,
                    max_bytes: max,
                    headers: headers.clone(),
                    hosts: self.all_hosts(),
                    ..Default::default()
                };
                pumped = self.transport.stream(url, &req, &tx).await;
                match &pumped {
                    // A missing part after the first: the previous was the last.
                    Ok(false) if i > 0 => pumped = Ok(true),
                    Ok(true) => continue,
                    _ => {}
                }
                break;
            }
            if pumped.is_err() {
                let _ = tx.send(Err(())).await;
            }
            drop(tx);
            let decoded = decode.await.map_err(|_| fail("internal", Value::Null))?;
            let files = match (pumped, decoded) {
                // The reader stopped first: its failure is the cause.
                (Err(p), Err(d)) if p.code == "closed" => return Err(d),
                (Err(p), _) => return Err(p),
                (Ok(false), _) if f.missing_is_empty => continue,
                (Ok(false), _) => {
                    let url = addresses
                        .first()
                        .map(|a| a.0.to_string())
                        .unwrap_or_default();
                    return Err(fail("not_found", json!({ "url": url })));
                }
                (Ok(true), d) => d?,
            };
            typed.extend(files);
            let mut next = row;
            let first = addresses
                .first()
                .map(|a| a.0.to_string())
                .unwrap_or_default();
            next.cols.insert("url".into(), Value::String(first));
            fetched.push(next);
        }
        // The entries are never kept: their table stays empty.
        self.ahead.insert(u.as_.clone(), Table::Rows(Vec::new()));
        self.ahead.insert(r.as_.clone(), Table::Batches(typed));
        Ok(fetched)
    }

    async fn fetch_one(
        &mut self,
        f: &Fetch,
        row: &Row,
        page: Option<u32>,
    ) -> Result<Option<Row>, Failure> {
        let every = f
            .every
            .as_deref()
            .and_then(pnex_core::pipeline::doc::duration_ms)
            .unwrap_or(0);
        let max = f.max_bytes.unwrap_or(FETCH_DEFAULT_BYTES);
        let filled = |t: &str, address: bool| {
            template::fill(t, address, |n| lookup(row, self.inputs, n))
                .map_err(|n| fail("missing_value", json!({ "name": n })))
        };
        let (mut url, headers) = self.address(f, row)?;
        if let (Some(p), Some(n)) = (&f.pages, page) {
            let value = match p.size {
                // A range of records: `0-149`, `150-299`…
                Some(size) => {
                    let start = (n - p.from) * size;
                    format!("{start}-{}", start + size - 1)
                }
                None => n.to_string(),
            };
            url.query_pairs_mut().append_pair(&p.param, &value);
        }
        let mut req = Req {
            every_ms: every,
            max_bytes: max,
            headers,
            ..Default::default()
        };
        if let Some(raw) = f.proxy.as_ref().and_then(|n| self.inputs.secrets.get(n)) {
            // Never the address in the failure: it may carry credentials.
            let invalid = || fail("proxy_invalid", json!({ "name": f.proxy }));
            let p = reqwest::Url::parse(raw).map_err(|_| invalid())?;
            if !matches!(p.scheme(), "http" | "https") {
                return Err(invalid());
            }
            if let Some(code) = pnex_core::egress::check_url(&p) {
                return Err(fail(code, json!({ "name": f.proxy })));
            }
            req.proxy = Some(p);
        }
        if let Some(sec) = &f.secret {
            self.host_in(&url, false)?;
            let value = match &sec.token {
                Some(t) => self.token(t, every).await?,
                None => self
                    .inputs
                    .secrets
                    .get(&sec.name)
                    .filter(|v| !v.is_empty())
                    .cloned()
                    .ok_or_else(|| fail("secret_required", json!({ "name": sec.name })))?,
            };
            let value = &value;
            match (&sec.param, &sec.header) {
                (Some(p), _) => req.secret_query = Some((p.clone(), value.clone())),
                (None, Some(h)) => {
                    let v = match &sec.scheme {
                        Some(s) => format!("{s} {value}"),
                        None => value.clone(),
                    };
                    req.headers.push((h.clone(), v));
                }
                (None, None) => return Err(fail("secret_required", json!({ "name": sec.name }))),
            }
        }
        // A JSON object, or its text (a column a `sql` step built).
        let json_body = match f.json_from.as_ref().and_then(|c| row.cols.get(c)) {
            Some(Value::String(t)) => Some(
                serde_json::from_str::<Value>(t)
                    .map_err(|_| fail("invalid_value", json!({ "name": "json_from" })))?,
            ),
            other => other.cloned(),
        };
        let body = match (f.method, json_body) {
            (Method::Get, _) => self.transport.get(&url, &req).await?,
            (Method::Post, Some(b)) => self.transport.post_json(&url, &b, &req).await?,
            (Method::Post, None) => {
                let fields: Vec<(String, String)> = f
                    .form_from
                    .as_ref()
                    .and_then(|c| row.cols.get(c))
                    .and_then(Value::as_object)
                    .map(|o| {
                        o.iter()
                            .map(|(k, v)| {
                                let v = v.as_str().map_or_else(|| v.to_string(), str::to_string);
                                (k.clone(), v)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let referer = row.text("url").and_then(|u| reqwest::Url::parse(&u).ok());
                self.transport
                    .post_form(&url, referer.as_ref(), &fields, &req)
                    .await?
            }
        };
        let Some(body) = body else {
            if f.missing_is_empty {
                return Ok(None);
            }
            return Err(fail("not_found", json!({ "url": url.as_str() })));
        };
        if body.is_empty() {
            return Ok(None);
        }
        let body = Arc::new(body);
        let sha = match (&f.checksum_url, f.skip_if_unchanged) {
            (None, false) => None,
            _ => {
                let b = body.clone();
                Some(blocking(move || b.sha256()).await?)
            }
        };
        if let (Some(t), Some(sha)) = (&f.checksum_url, &sha) {
            let curl = reqwest::Url::parse(&filled(t, true)?)
                .map_err(|_| fail("invalid_value", json!({ "name": "checksum_url" })))?;
            self.hosts_allow(&curl)?;
            // A file published without its checksum is read unchecked.
            let small = Req {
                every_ms: every,
                max_bytes: 4096,
                ..Default::default()
            };
            if let Some(text) = self.transport.get(&curl, &small).await? {
                let text = String::from_utf8_lossy(&text.bytes(4096)?).into_owned();
                let want = text
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if &want != sha {
                    return Err(fail("checksum", json!({ "url": url.as_str() })));
                }
            }
        }
        if let (true, Some(sha)) = (f.skip_if_unchanged, sha) {
            let key = format!("{}:{url}", f.as_);
            if self.inputs.shas.get(&key) == Some(&sha) {
                return Ok(None);
            }
            self.fresh_shas.insert(key, sha);
        }
        let mut next = row.clone();
        next.cols
            .insert("url".into(), Value::String(url.to_string()));
        if let Some(n) = page {
            next.cols
                .insert("page".into(), Value::String(n.to_string()));
        }
        next.file = Some(body);
        Ok(Some(next))
    }

    /// An empty table with the columns its step declares: the axes of an
    /// enumeration (text), the columns of a read. `None` for another step.
    fn empty_of(&self, name: &str) -> Result<Option<Arc<blob::Batches>>, Failure> {
        use arrow_schema::{DataType, Field, Schema};
        let fields: Vec<Field> = match self.doc.steps.iter().find(|s| s.output() == Some(name)) {
            Some(Step::Enumerate(e)) => e
                .cross
                .iter()
                .flat_map(|a| {
                    let extra: Vec<String> = a
                        .objects
                        .iter()
                        .flat_map(|o| o.title.iter().chain(&o.columns).cloned())
                        .collect();
                    std::iter::once(a.as_.clone())
                        .chain(a.derive.keys().cloned())
                        .chain(extra)
                })
                .map(|n| Field::new(n, DataType::Utf8, true))
                .collect(),
            Some(Step::Read(r)) if !r.columns.is_empty() => read::fields(r),
            // A navigation that extracted nothing.
            Some(Step::Browse(_)) => ["name", "text"]
                .map(|n| Field::new(n, DataType::Utf8, true))
                .to_vec(),
            _ => return Ok(None),
        };
        blob::BatchWriter::new(Arc::new(Schema::new(fields)))?
            .finish()
            .map(Some)
    }

    fn rows<'t>(tables: &'t HashMap<String, Table>, name: &str) -> Result<&'t [Row], Failure> {
        match tables.get(name) {
            Some(Table::Rows(r)) => Ok(r),
            _ => Err(fail("not_rows", json!({ "table": name }))),
        }
    }

    /// The rows an enumeration or a fetch walks: a query's or a call's
    /// result is read back as rows, its values as text.
    fn walked(tables: &HashMap<String, Table>, name: &str) -> Result<Vec<Row>, Failure> {
        let files = match tables.get(name) {
            Some(Table::Rows(r)) => return Ok(r.clone()),
            Some(Table::Batches(files)) => files,
            None => return Err(fail("not_rows", json!({ "table": name }))),
        };
        let mut out = Vec::new();
        for f in files {
            for b in f.read()? {
                let records = crate::services::datasets::parquet::records(&b?)
                    .map_err(|_| fail("unreadable", json!({ "table": name })))?;
                for r in records {
                    if out.len() >= ENUMERATE_MAX {
                        return Err(fail("too_many_rows", json!({ "max": ENUMERATE_MAX })));
                    }
                    let cols = match r {
                        Value::Object(o) => o,
                        _ => Map::new(),
                    };
                    out.push(Row { cols, file: None });
                }
            }
        }
        Ok(out)
    }

    /// One step over the working tables: its table, `None` for a write.
    async fn step(
        &mut self,
        step: &Step,
        tables: &HashMap<String, Table>,
        written: &mut u64,
    ) -> Result<Option<Table>, Failure> {
        if let Some(t) = step.output().and_then(|o| self.ahead.remove(o)) {
            return Ok(Some(t));
        }
        let out = match step {
            Step::Enumerate(e) => Table::Rows(
                enumerate(
                    e,
                    self.inputs,
                    match &e.for_each {
                        Some(t) => Self::walked(tables, t)?,
                        None => vec![Row::default()],
                    },
                )?
                .into_iter()
                .map(|(_, r)| r)
                .collect(),
            ),
            Step::Fetch(f) if f.stream => {
                let input = match &f.for_each {
                    Some(t) => Self::walked(tables, t)?,
                    None => vec![Row::default()],
                };
                Table::Rows(self.streamed(f, input).await?)
            }
            Step::Fetch(f) => {
                let input = match &f.for_each {
                    Some(t) => Self::walked(tables, t)?,
                    None => vec![Row::default()],
                };
                Table::Rows(self.fetch(f, input).await?)
            }
            Step::Extract(e) => {
                let (e, input) = (e.clone(), Self::rows(tables, &e.from)?.to_vec());
                Table::Rows(blocking(move || files::extract(&e, &input)).await?)
            }
            Step::Unpack(u) => {
                let (u, input) = (u.clone(), Self::rows(tables, &u.from)?.to_vec());
                Table::Rows(blocking(move || files::unpack(&u, &input)).await?)
            }
            Step::Read(r) => {
                let (r, mut input) = (r.clone(), Self::rows(tables, &r.from)?.to_vec());
                // A column's value may name the run's day or instant.
                for row in &mut input {
                    for name in ["run_date", "run_now"] {
                        if let Some(v) = lookup(&Row::default(), self.inputs, name) {
                            row.cols.entry(name).or_insert(Value::String(v));
                        }
                    }
                }
                Table::Batches(blocking(move || read::read(&r, &input)).await?)
            }
            Step::Write(w) => {
                let batches = match tables.get(&w.from) {
                    Some(Table::Batches(b)) => b.clone(),
                    Some(Table::Rows(r)) => {
                        let r = r.clone();
                        blocking(move || read::rows_batch(&r))
                            .await?
                            .into_iter()
                            .collect()
                    }
                    None => return Err(fail("not_rows", json!({ "table": w.from }))),
                };
                if let Some(t) = &w.table {
                    let batches = match t.key.is_empty() {
                        true => batches,
                        false => self.unseen(t, batches).await?,
                    };
                    if batches.iter().any(|b| b.rows > 0) {
                        *written += self.sink.append(&t.name, t.replace, batches).await?;
                    }
                } else if w.time_ranges.is_some() {
                    let rows = blocking(move || records_of(&batches)).await?;
                    *written += self.sink.ranges(rows).await?;
                } else if let Some(m) = &w.media {
                    let rows = blocking(move || records_of(&batches)).await?;
                    *written += self.sink.media(m, rows).await?;
                } else if let Some(p) = &w.proposals {
                    let rows = blocking(move || records_of(&batches)).await?;
                    *written += self.sink.proposals(p, rows).await?;
                } else if w.ask.is_some() {
                    let rows = blocking(move || records_of(&batches)).await?;
                    *written += self.sink.ask(rows).await?;
                } else if let Some(o) = w.objects.as_ref().filter(|o| !o.snapshot) {
                    let rows = blocking(move || records_of(&batches)).await?;
                    *written += self.sink.objects(o, rows).await?;
                } else {
                    // shortcut: no snapshot outside an import; add closing when a
                    // pipeline must close what its rows no longer hold.
                    return Err(fail("step_unsupported", json!({ "step": "write.objects" })));
                }
                return Ok(None);
            }
            Step::Sql(q) => {
                // The working tables the query names, as whole words.
                let words: std::collections::HashSet<&str> = q
                    .sql
                    .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .collect();
                let mut staged = Vec::new();
                let mut unknown_empty = false;
                for (name, t) in tables {
                    if !words.contains(name.as_str()) {
                        continue;
                    }
                    let file = match t {
                        // An empty table keeps the columns its step declares:
                        // `WHERE NOT EXISTS (SELECT 1 FROM it)` still reads.
                        _ if t.is_empty() => match self.empty_of(name)? {
                            Some(f) => f,
                            None => {
                                unknown_empty = true;
                                continue;
                            }
                        },
                        Table::Batches(files) if files.len() == 1 => files[0].clone(),
                        Table::Batches(files) => {
                            let files = files.clone();
                            blocking(move || blob::merged(&files)).await?
                        }
                        Table::Rows(r) => {
                            let r = r.clone();
                            match blocking(move || read::rows_batch(&r)).await? {
                                Some(f) => f,
                                None => continue,
                            }
                        }
                    };
                    staged.push((name.clone(), file));
                }
                // Tables of the org: given by the sink, read by the worker, or
                // missing (empty with the declared columns, else no row).
                let mut org_tables = std::collections::BTreeMap::new();
                for (alias, name) in &q.tables {
                    if let Some(file) = self.sink.org_table(name).await? {
                        staged.push((alias.clone(), file));
                    } else if self.sink.has_table(name).await? {
                        org_tables.insert(alias.clone(), name.clone());
                    } else if let Some(cols) = q.columns.get(alias) {
                        staged.push((alias.clone(), empty_text(cols)?));
                    } else {
                        unknown_empty = true;
                    }
                }
                // A table with no row and no declared column: the query gives no row.
                let out = match unknown_empty {
                    true => None,
                    false => self.sql.run(&q.sql, staged, &org_tables).await?,
                };
                Table::Batches(out.into_iter().collect())
            }
            Step::Call(c) => {
                let code =
                    self.inputs.functions.get(&c.function).ok_or_else(|| {
                        fail("function_unknown", json!({ "function": c.function }))
                    })?;
                let input = match tables.get(&c.from) {
                    None => return Err(fail("unknown_table", json!({ "table": c.from }))),
                    Some(t) if t.is_empty() => None,
                    Some(Table::Batches(files)) if files.len() == 1 => Some(files[0].clone()),
                    Some(Table::Batches(files)) => {
                        let files = files.clone();
                        Some(blocking(move || blob::merged(&files)).await?)
                    }
                    Some(Table::Rows(r)) => {
                        let r = r.clone();
                        blocking(move || read::rows_batch(&r)).await?
                    }
                };
                // The pack settings the document reads, as `pack_setting` gives them.
                let settings: serde_json::Map<String, Value> = self
                    .inputs
                    .settings
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect();
                let out = self.sql.call(code, &settings, input).await?;
                Table::Batches(out.into_iter().collect())
            }
            Step::Model(m) => {
                let ex = self.inputs.extractors.get(&m.extractor).ok_or_else(|| {
                    fail("extractor_unknown", json!({ "extractor": m.extractor }))
                })?;
                let rows = match tables.get(&m.from) {
                    None => return Err(fail("unknown_table", json!({ "table": m.from }))),
                    Some(Table::Rows(r)) => r.iter().map(|r| r.cols.clone()).collect(),
                    Some(Table::Batches(files)) => {
                        let files = files.clone();
                        blocking(move || records_of(&files)).await?
                    }
                };
                Table::Rows(self.sink.model(m, ex, rows).await?)
            }
            Step::Browse(b) => {
                let (Some(item), Some(Table::Rows(rows))) = (
                    self.item.clone(),
                    b.for_each.as_ref().and_then(|t| tables.get(t)),
                ) else {
                    return Err(fail("browse_position", Value::Null));
                };
                let Some(row) = rows.first() else {
                    return Ok(Some(Table::Rows(Vec::new())));
                };
                match self.sink.browse(b, row, &item).await? {
                    Some(extracts) => Table::Rows(extracts),
                    None => return Err(fail(WAITING, Value::Null)),
                }
            }
        };
        Ok(Some(out))
    }

    /// Runs the steps from `from` on; the rows written.
    pub async fn pass(
        &mut self,
        from: usize,
        mut tables: HashMap<String, Table>,
    ) -> Result<u64, Failure> {
        let doc = self.doc;
        let mut written = 0;
        for step in &doc.steps[from..] {
            let before = written;
            let result = self.step(step, &tables, &mut written).await;
            if let Some(j) = self.journal.as_deref_mut() {
                j.push(StepRecord::of(step, &result, written - before));
            }
            let Some(out) = result? else { continue };
            let name = step.output().unwrap_or_default().to_string();
            if let Some(keep) = self.keep.as_deref_mut() {
                keep.entry(name.clone()).or_default().push(out.clone());
            }
            tables.insert(name, out);
        }
        Ok(written)
    }
}

/// Resumes a row its navigation paused (D995): the steps after `browse`,
/// with the row and the navigation's extracts (`None`: it failed). The new
/// state of the row, its failure, the rows written.
#[allow(clippy::too_many_arguments)]
pub async fn resume<T: Transport, S: Sink, Q: sql::SqlRunner>(
    doc: &Document,
    inputs: &Inputs,
    transport: &T,
    sink: &mut S,
    sql: &Q,
    previous: Option<&track::Item>,
    key: &str,
    row: Map<String, Value>,
    extracts: Option<Vec<Row>>,
) -> (track::Item, Option<Failure>, u64) {
    let now_us = inputs.now.and_utc().timestamp_micros();
    let (Some(Step::Enumerate(e)), Some(Step::Browse(b))) = (doc.steps.first(), doc.steps.get(1))
    else {
        let f = fail("browse_position", Value::Null);
        let item = track::after(
            key,
            previous,
            track::State::Failed,
            false,
            None,
            None,
            now_us,
        );
        return (item, Some(f), 0);
    };
    let settled = e.empty == pnex_core::pipeline::doc::Empty::Settled;
    let Some(extracts) = extracts else {
        let item = track::after(
            key,
            previous,
            track::State::Failed,
            settled,
            e.retry_days,
            e.again_days,
            now_us,
        );
        return (item, Some(fail("browse_failed", Value::Null)), 0);
    };
    let mut exec = Exec {
        doc,
        transport,
        sink,
        sql,
        inputs,
        keep: None,
        journal: None,
        fresh_shas: HashMap::new(),
        tokens: HashMap::new(),
        ahead: HashMap::new(),
        item: Some(key.to_string()),
    };
    let tables = HashMap::from([
        (
            e.as_.clone(),
            Table::Rows(vec![Row {
                cols: row,
                file: None,
            }]),
        ),
        (b.as_.clone(), Table::Rows(extracts)),
    ]);
    let (state, failure, rows) = match exec.pass(2, tables).await {
        Ok(0) => (track::State::Empty, None, 0),
        Ok(n) => (track::State::Done, None, n),
        Err(f) => (track::State::Failed, Some(f), 0),
    };
    let item = track::after(
        key,
        previous,
        state,
        settled,
        e.retry_days,
        e.again_days,
        now_us,
    );
    (item, failure, rows)
}

async fn blocking<R: Send + 'static>(
    f: impl FnOnce() -> Result<R, Failure> + Send + 'static,
) -> Result<R, Failure> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| fail("internal", Value::Null))?
}

/// The outcome of a run: the new state of each row taken, and of the run.
#[derive(Debug, Default)]
pub struct RunOutcome {
    /// (item, state, failure) of each row taken.
    pub items: Vec<(track::Item, Option<Failure>, u64)>,
    pub written: u64,
    pub fresh_shas: HashMap<String, String>,
    /// A failure outside any row (an untracked run, a bad enumeration).
    pub failure: Option<Failure>,
}

/// Runs a document once. A tracked first step (D937) gives the rows: the
/// run takes those due (`known` holds their states), each through the
/// other steps on its own, so that a failing row never stops the others;
/// only a source pushing back ([`throttled`]) ends the run there.
#[allow(clippy::too_many_arguments)]
pub async fn run<T: Transport, S: Sink, Q: sql::SqlRunner>(
    doc: &Document,
    inputs: &Inputs,
    transport: &T,
    sink: &mut S,
    sql: &Q,
    known: &HashMap<String, track::Item>,
    max_items: Option<usize>,
    mut keep: Option<&mut HashMap<String, Vec<Table>>>,
    mut journal: Option<&mut Vec<StepRecord>>,
) -> RunOutcome {
    let mut outcome = RunOutcome::default();
    let unset = |k: &String| inputs.settings.get(k).is_none_or(|v| v.trim().is_empty());
    if doc.requires.iter().any(unset) {
        return outcome;
    }
    let tracked = match doc.steps.first() {
        Some(Step::Enumerate(e)) if e.track => Some(e),
        _ => None,
    };
    let now_us = inputs.now.and_utc().timestamp_micros();
    let Some(e) = tracked else {
        let mut exec = Exec {
            doc,
            transport,
            sink,
            sql,
            inputs,
            keep,
            journal,
            fresh_shas: HashMap::new(),
            tokens: HashMap::new(),
            ahead: HashMap::new(),
            item: None,
        };
        match exec.pass(0, HashMap::new()).await {
            Ok(n) => outcome.written = n,
            Err(f) => outcome.failure = Some(f),
        }
        outcome.fresh_shas = exec.fresh_shas;
        return outcome;
    };
    let rows = match enumerate(e, inputs, vec![Row::default()]) {
        Ok(r) => r,
        Err(f) => {
            outcome.failure = Some(f);
            return outcome;
        }
    };
    let keys: Vec<String> = rows.iter().map(|(k, _)| k.clone()).collect();
    let asked = e.cross.iter().any(|a| a.asked.is_some());
    // Rows asked explicitly (a route, a pipeline_ask node) are all taken unless
    // the document caps them: a silent default cap dropped the rest (D999).
    let default_cap = if asked {
        usize::MAX
    } else {
        PER_RUN_DEFAULT as usize
    };
    let budget = max_items.unwrap_or(e.per_run.map_or(default_cap, |n| n as usize));
    // Rows asked by a start are taken whatever they did before, unless a
    // navigation still runs for them (D996).
    let due: Vec<String> = if asked {
        keys.iter()
            .filter(|k| {
                known
                    .get(*k)
                    .is_none_or(|i| i.state != track::State::Waiting)
            })
            .take(budget)
            .cloned()
            .collect()
    } else {
        track::due(&keys, known, now_us, budget)
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    let by_key: HashMap<&str, &Row> = rows.iter().map(|(k, r)| (k.as_str(), r)).collect();
    let settled = e.empty == pnex_core::pipeline::doc::Empty::Settled;
    for key in due {
        let row = by_key[key.as_str()].clone();
        let mut exec = Exec {
            doc,
            transport,
            sink,
            sql,
            inputs,
            keep: keep.as_deref_mut(),
            journal: journal.as_deref_mut(),
            fresh_shas: HashMap::new(),
            tokens: HashMap::new(),
            ahead: HashMap::new(),
            item: Some(key.clone()),
        };
        if let Some(k) = exec.keep.as_deref_mut() {
            k.entry(e.as_.clone())
                .or_default()
                .push(Table::Rows(vec![row.clone()]));
        }
        let cols = row.cols.clone();
        let tables = HashMap::from([(e.as_.clone(), Table::Rows(vec![row]))]);
        let result = exec.pass(1, tables).await;
        outcome.fresh_shas.extend(exec.fresh_shas);
        let (state, failure, rows) = match result {
            Ok(0) => (track::State::Empty, None, 0),
            Ok(n) => (track::State::Done, None, n),
            // The row is kept for the resume: its navigation runs (D995).
            Err(f) if f.code == WAITING => (
                track::State::Waiting,
                Some(fail(WAITING, json!({ "row": cols }))),
                0,
            ),
            // The rest waits for a later run; the row is left unmarked so a
            // source's bad hour never adds to its failure count.
            Err(f) if throttled(&f) => {
                outcome.failure = Some(f);
                break;
            }
            Err(f) => (track::State::Failed, Some(f), 0),
        };
        outcome.written += rows;
        let item = track::after(
            &key,
            known.get(&key),
            state,
            settled,
            e.retry_days,
            e.again_days,
            now_us,
        );
        outcome.items.push((item, failure, rows));
    }
    outcome
}
