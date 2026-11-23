//! A pipeline's `tests:` replayed (pipelines.md D945): each case runs on
//! recorded answers, without network nor the org's tables, and its working
//! tables are compared with what it expects. A pack's answers are files of
//! the pack; an org's are written in its document (D934).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use chrono::NaiveDate;
use pnex_core::pipeline::doc::{Document, TestCase};
use serde_json::{json, Map, Value};

use super::blob::{Batches, Blob};
use super::sql::SqlRunner;
use super::store::fixture;
use super::{fail, run, Failure, Inputs, Req, Row, Sink, Table, Transport};

/// Fixtures stay small: a test shows the shape of a source, not its data.
pub const FIXTURE_MAX: usize = 64 * 1024;

/// Where a case's recorded answers are: the files of a pack, or the
/// document itself (`None`).
#[derive(Clone)]
pub struct Replay {
    pub pack: Option<String>,
    pub given: HashMap<String, String>,
}

impl Replay {
    fn file(&self, name: &str) -> Result<Vec<u8>, Failure> {
        let Some(pack) = &self.pack else {
            return Ok(name.as_bytes().to_vec());
        };
        let bytes =
            fixture(pack, name).ok_or_else(|| fail("fixture_missing", json!({ "file": name })))?;
        if bytes.len() > FIXTURE_MAX {
            return Err(fail("fixture_too_large", json!({ "file": name })));
        }
        Ok(bytes.to_vec())
    }

    fn answer(&self, url: &reqwest::Url) -> Result<Option<Blob>, Failure> {
        let Some(file) = self.given.get(url.as_str()) else {
            // Shown when a test fails: the address its `given` should list.
            tracing::debug!(url = %url, "no recorded answer");
            return Ok(None);
        };
        Ok(Some(Blob::Mem(self.file(file)?)))
    }
}

impl Transport for Replay {
    async fn get(&self, url: &reqwest::Url, _: &Req) -> Result<Option<Blob>, Failure> {
        self.answer(url)
    }

    async fn post_form(
        &self,
        url: &reqwest::Url,
        _: Option<&reqwest::Url>,
        _: &[(String, String)],
        _: &Req,
    ) -> Result<Option<Blob>, Failure> {
        self.answer(url)
    }
}

/// Writes kept in memory; the org's tables a case gives, from its fixtures.
#[derive(Default)]
pub struct Memory(
    pub HashMap<String, Vec<Arc<Batches>>>,
    HashMap<String, Arc<Batches>>,
    /// Extracts of each navigation (`given: browse:<as>`): row → name → text.
    pub HashMap<String, Value>,
    /// The model's answer of each `model` step (`given: model:<as>`).
    HashMap<String, Value>,
    /// The CSV each `connector` step reads (`given: connector:<as>`).
    HashMap<String, Vec<u8>>,
);

impl Memory {
    pub fn with_case(
        replay: &Replay,
        tables: &BTreeMap<String, String>,
        given: &BTreeMap<String, String>,
    ) -> Result<Self, Failure> {
        let mut out = Self::default();
        for (name, f) in tables {
            out.1.insert(name.clone(), csv_text(&replay.file(f)?)?);
        }
        for (k, f) in given {
            let json = |step: &str| -> Result<(String, Value), Failure> {
                let v = serde_json::from_slice(&replay.file(f)?)
                    .map_err(|_| fail("fixture_invalid", json!({ "given": k })))?;
                Ok((step.to_string(), v))
            };
            if let Some(step) = k.strip_prefix("browse:") {
                let (s, v) = json(step)?;
                out.2.insert(s, v);
            } else if let Some(step) = k.strip_prefix("model:") {
                let (s, v) = json(step)?;
                out.3.insert(s, v);
            } else if let Some(step) = k.strip_prefix("connector:") {
                out.4.insert(step.to_string(), replay.file(f)?);
            }
        }
        Ok(out)
    }
}

/// A CSV fixture as text columns.
fn csv_text(bytes: &[u8]) -> Result<Arc<Batches>, Failure> {
    use arrow_array::{ArrayRef, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    let invalid = || fail("fixture_invalid", json!({ "format": "csv" }));
    let mut rd = csv::Reader::from_reader(bytes);
    let names: Vec<String> = rd
        .headers()
        .map_err(|_| invalid())?
        .iter()
        .map(str::to_string)
        .collect();
    let rows: Vec<csv::StringRecord> = rd
        .records()
        .collect::<Result<_, _>>()
        .map_err(|_| invalid())?;
    let schema = Arc::new(Schema::new(
        names
            .iter()
            .map(|n| Field::new(n, DataType::Utf8, true))
            .collect::<Vec<_>>(),
    ));
    let cols: Vec<ArrayRef> = (0..names.len())
        .map(|i| {
            let v: Vec<Option<&str>> = rows
                .iter()
                .map(|r| r.get(i).filter(|x| !x.is_empty()))
                .collect();
            Arc::new(StringArray::from(v)) as ArrayRef
        })
        .collect();
    let mut w = super::blob::BatchWriter::new(schema.clone())?;
    w.write(&RecordBatch::try_new(schema, cols).map_err(|_| invalid())?)?;
    w.finish()
}

impl Sink for Memory {
    async fn connector(&mut self, c: &pnex_core::pipeline::doc::Connector) -> Result<Row, Failure> {
        let csv = self.4.get(&c.as_).cloned().ok_or_else(|| {
            fail(
                "fixture_missing",
                json!({ "given": format!("connector:{}", c.as_) }),
            )
        })?;
        let mut cols = Map::new();
        cols.insert("name".into(), Value::String(c.name.clone()));
        Ok(Row {
            cols,
            file: Some(Arc::new(Blob::Mem(csv))),
        })
    }

    // A replay reads no table of the worker: a key only dedupes within the
    // run, and a table is the case's fixture or missing.
    async fn has_table(&mut self, _: &str) -> Result<bool, Failure> {
        Ok(false)
    }

    async fn org_table(&mut self, table: &str) -> Result<Option<Arc<Batches>>, Failure> {
        Ok(self.1.get(table).cloned())
    }

    async fn append(
        &mut self,
        t: &pnex_core::pipeline::doc::TableTarget,
        files: Vec<Arc<Batches>>,
        _: &BTreeSet<String>,
    ) -> Result<u64, Failure> {
        let n = files.iter().map(|f| f.rows).sum();
        self.0.entry(t.name.clone()).or_default().extend(files);
        Ok(n)
    }

    async fn ranges(&mut self, rows: Vec<Map<String, Value>>) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn media(
        &mut self,
        _: &pnex_core::pipeline::doc::MediaTarget,
        rows: Vec<Map<String, Value>>,
    ) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn proposals(
        &mut self,
        _: &pnex_core::pipeline::doc::ProposalsTarget,
        rows: Vec<Map<String, Value>>,
    ) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn objects(
        &mut self,
        _: &pnex_core::pipeline::doc::ObjectsTarget,
        rows: Vec<Map<String, Value>>,
    ) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn ask(&mut self, rows: Vec<Map<String, Value>>) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn model(
        &mut self,
        step: &pnex_core::pipeline::doc::Model,
        ex: &pnex_core::extractor::Extractor,
        rows: Vec<Map<String, Value>>,
    ) -> Result<Vec<Row>, Failure> {
        // The recorded answer, checked against each row's own text.
        let answer = self.3.get(&step.as_).cloned().unwrap_or(Value::Null);
        Ok(rows
            .iter()
            .flat_map(|r| {
                let text = r
                    .get(&step.text)
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let checked = ex.check(text, &answer);
                super::item_rows(r, &step.text, &checked, 0)
            })
            .collect())
    }

    async fn browse(
        &mut self,
        b: &pnex_core::pipeline::doc::Browse,
        _: &[String],
        _: &Row,
        item: &str,
    ) -> Result<Option<Vec<Row>>, Failure> {
        // No extracts given: the navigation is started, the row waits.
        let Some(found) = self.2.get(&b.as_) else {
            return Ok(None);
        };
        let texts = found
            .get(item)
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()));
        Ok(Some(super::extract_rows(texts)))
    }
}

/// Rows of a working table as text, the way a table sample shows them.
pub fn rendered(tables: &[Table]) -> Result<(Vec<String>, Vec<Value>), Failure> {
    let (mut columns, mut rows) = (Vec::new(), Vec::new());
    for t in tables {
        match t {
            Table::Rows(rs) => {
                for r in rs {
                    for k in r.cols.keys() {
                        if !columns.contains(k) {
                            columns.push(k.clone());
                        }
                    }
                    rows.push(Value::Object(r.cols.clone()));
                }
            }
            Table::Batches(files) => {
                for file in files {
                    for f in file.schema.fields() {
                        if !columns.contains(f.name()) {
                            columns.push(f.name().clone());
                        }
                    }
                    for b in file.read()? {
                        let records = crate::services::datasets::parquet::records(&b?)
                            .map_err(|_| fail("storage", json!({ "what": "test" })))?;
                        rows.extend(records);
                    }
                }
            }
        }
    }
    Ok((columns, rows))
}

pub fn matches(row: &Value, want: &Map<String, Value>) -> bool {
    want.iter().all(|(k, v)| {
        let got = match row.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => return v.is_null(),
        };
        let v = v.as_str().map_or_else(|| v.to_string(), str::to_string);
        got == v || got.starts_with(&v)
    })
}

/// A case that did not hold: which, and a machine token with its argument
/// (`rows:12`, `column:price`, `row:3`, a failure code).
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub case: usize,
    pub table: Option<String>,
    pub token: String,
}

/// Plays one case of `doc`; empty when it holds. `functions` and
/// `extractors` are what its `call` and `model` steps name.
pub async fn case(
    doc: &Document,
    index: usize,
    case: &TestCase,
    replay: &Replay,
    functions: HashMap<String, String>,
    extractors: HashMap<String, pnex_core::extractor::Extractor>,
    sql: &impl SqlRunner,
) -> Vec<Problem> {
    let problem = |table: Option<&str>, token: String| Problem {
        case: index,
        table: table.map(str::to_string),
        token,
    };
    let today = case
        .today
        .as_deref()
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 1, 1).expect("a date"));
    // A key goes in a header or a parameter added when sent: never in
    // the address a fixture is found by.
    let secrets = doc
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
                .flat_map(|t| [t.user.clone(), t.password.clone()]);
            std::iter::once(s.name.clone()).chain(grant)
        })
        .map(|n| (n, "test-key".to_string()))
        .collect();
    let text = |m: &BTreeMap<String, String>| -> Map<String, Value> {
        m.iter()
            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
            .collect()
    };
    let inputs = Inputs {
        settings: case.settings.clone().into_iter().collect(),
        now: today.and_hms_opt(12, 0, 0).expect("noon"),
        secrets,
        objects: case.objects.clone().into_iter().collect(),
        functions,
        extractors,
        asked: case.asked.iter().map(text).collect(),
        extra: case
            .extra
            .iter()
            .map(|(k, by_value)| {
                let by_value = by_value
                    .iter()
                    .map(|(v, cols)| (v.clone(), text(cols)))
                    .collect();
                (k.clone(), by_value)
            })
            .collect(),
        ..Default::default()
    };
    let replay = Replay {
        given: case.given.clone().into_iter().collect(),
        ..replay.clone()
    };
    let mut sink = match Memory::with_case(&replay, &case.tables, &case.given) {
        Ok(m) => m,
        Err(f) => return vec![problem(None, f.code.to_string())],
    };
    let mut kept = HashMap::new();
    let out = run(
        doc,
        &inputs,
        &replay,
        &mut sink,
        sql,
        &HashMap::new(),
        Some(usize::MAX),
        Some(&mut kept),
        None,
    )
    .await;
    if let Some(f) = out.failure {
        return vec![problem(None, f.code.to_string())];
    }
    if let Some((_, Some(f), _)) = out.items.iter().find(|(_, f, _)| f.is_some()) {
        return vec![problem(None, f.code.to_string())];
    }
    let mut problems = Vec::new();
    for (table, want) in &case.expect {
        let (columns, rows) = match rendered(kept.get(table).map(Vec::as_slice).unwrap_or(&[])) {
            Ok(r) => r,
            Err(f) => {
                problems.push(problem(Some(table), f.code.to_string()));
                continue;
            }
        };
        if let Some(n) = want.rows.filter(|n| *n != rows.len() as u64) {
            tracing::debug!(table, want = n, "rows of a test case differ");
            problems.push(problem(Some(table), format!("rows:{}", rows.len())));
        }
        for c in want.columns.iter().filter(|c| !columns.contains(c)) {
            problems.push(problem(Some(table), format!("column:{c}")));
        }
        for (i, w) in want.contains.iter().enumerate() {
            let w: Map<String, Value> = w.clone().into_iter().collect();
            if !rows.iter().any(|r| matches(r, &w)) {
                problems.push(problem(Some(table), format!("row:{i}")));
            }
        }
    }
    problems
}
