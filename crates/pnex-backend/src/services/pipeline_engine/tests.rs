//! The replay tests of every shipped pipeline (pipeline-engine.md D945):
//! each `tests:` case runs on recorded answers, without network nor base,
//! and its working tables are compared with what it expects.

use std::collections::HashMap;

use std::sync::Arc;

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::blob::{Batches, Blob};
use super::sql::SqlRunner;
use super::store::{fixture, shipped};
use super::{fail, run, Failure, Inputs, Req, Sink, Table, Transport};

/// Fixtures stay small: a test shows the shape of a source, not its data.
const FIXTURE_MAX: usize = 64 * 1024;

struct Replay {
    pack: String,
    given: HashMap<String, String>,
}

impl Replay {
    fn answer(&self, url: &reqwest::Url) -> Result<Option<Blob>, Failure> {
        let Some(file) = self.given.get(url.as_str()) else {
            // Shown when a test fails: the address its `given` should list.
            println!("{}: no fixture for {url}", self.pack);
            return Ok(None);
        };
        let bytes = fixture(&self.pack, file)
            .ok_or_else(|| fail("fixture_missing", json!({ "file": file })))?;
        assert!(
            bytes.len() <= FIXTURE_MAX,
            "{}/{file}: fixtures stay under 64 KB",
            self.pack
        );
        Ok(Some(Blob::Mem(bytes.to_vec())))
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

/// The worker's own query function, in this process: same session, same
/// allow list (D945).
struct InProcess;

impl SqlRunner for InProcess {
    async fn call(
        &self,
        code: &str,
        settings: &serde_json::Map<String, serde_json::Value>,
        input: Option<Arc<Batches>>,
    ) -> Result<Option<Arc<Batches>>, Failure> {
        let batches: Vec<_> = match input {
            Some(f) => f.read()?.collect::<Result<_, _>>()?,
            None => Vec::new(),
        };
        let out = pnex_pipeline_worker::step::call_in_memory(code, settings, &batches)
            .map_err(|e| fail("sql_failed", json!({ "reason": e })))?;
        let Some(first) = out.first() else {
            return Ok(None);
        };
        let mut w = super::blob::BatchWriter::new(first.schema())?;
        for b in &out {
            w.write(b)?;
        }
        w.finish().map(Some)
    }

    async fn run(
        &self,
        sql: &str,
        staged: Vec<(String, Arc<Batches>)>,
        tables: &std::collections::BTreeMap<String, String>,
    ) -> Result<Option<Arc<Batches>>, Failure> {
        assert!(tables.is_empty(), "a replay test reads no table of an org");
        let mut inputs = Vec::new();
        for (name, file) in staged {
            let batches: Vec<_> = file.read()?.collect::<Result<_, _>>()?;
            inputs.push((name, file.schema.clone(), batches));
        }
        let limits = pnex_pipeline_worker::engine::Limits {
            memory_bytes: 256 * 1024 * 1024,
            partitions: 1,
            spill_dir: std::env::temp_dir(),
            spill_max_bytes: 1024 * 1024 * 1024,
        };
        let out = pnex_pipeline_worker::step::run_in_memory(&limits, sql, inputs)
            .await
            .map_err(|e| fail("sql_failed", json!({ "reason": e })))?;
        let Some(first) = out.first() else {
            return Ok(None);
        };
        let mut w = super::blob::BatchWriter::new(first.schema())?;
        for b in &out {
            w.write(b)?;
        }
        w.finish().map(Some)
    }
}

#[derive(Default)]
struct Memory(HashMap<String, Vec<Arc<Batches>>>);

impl Sink for Memory {
    async fn append(
        &mut self,
        table: &str,
        _: bool,
        files: Vec<Arc<Batches>>,
    ) -> Result<u64, Failure> {
        let n = files.iter().map(|f| f.rows).sum();
        self.0.entry(table.to_string()).or_default().extend(files);
        Ok(n)
    }

    async fn ranges(&mut self, rows: Vec<serde_json::Map<String, Value>>) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }
}

/// Rows of a working table as text, the way a table sample shows them.
fn rendered(tables: &[Table]) -> (Vec<String>, Vec<Value>) {
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
                    for b in file.read().unwrap() {
                        rows.extend(
                            crate::services::datasets::parquet::records(&b.unwrap()).unwrap(),
                        );
                    }
                }
            }
        }
    }
    (columns, rows)
}

fn matches(row: &Value, want: &serde_json::Map<String, Value>) -> bool {
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

#[tokio::test]
async fn every_shipped_pipeline_passes_its_tests() {
    let all = shipped();
    assert!(
        !all.is_empty(),
        "no shipped pipeline found under packs/*/pipelines"
    );
    for sp in all {
        let id = sp.id();
        let v = sp.doc.violations();
        assert!(v.is_empty(), "{id}: {v:?}");
        assert!(
            !sp.doc.tests.is_empty(),
            "{id}: a shipped pipeline has a test (D945)"
        );
        println!("{id}: {} test(s)", sp.doc.tests.len());
        for (i, case) in sp.doc.tests.iter().enumerate() {
            let today = case
                .today
                .as_deref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .unwrap_or_else(|| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
            // A key goes in a header or a parameter added when sent: never in
            // the address a fixture is found by.
            let secrets = sp
                .doc
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
            let inputs = Inputs {
                settings: case.settings.clone().into_iter().collect(),
                now: today.and_hms_opt(12, 0, 0).unwrap(),
                secrets,
                objects: case.objects.clone().into_iter().collect(),
                functions: super::store::call_functions(&sp.pack, &sp.doc),
                extra: case
                    .extra
                    .iter()
                    .map(|(k, by_value)| {
                        let by_value = by_value
                            .iter()
                            .map(|(v, cols)| {
                                let cols = cols
                                    .iter()
                                    .map(|(c, x)| (c.clone(), serde_json::Value::String(x.clone())))
                                    .collect();
                                (v.clone(), cols)
                            })
                            .collect();
                        (k.clone(), by_value)
                    })
                    .collect(),
                ..Default::default()
            };
            let transport = Replay {
                pack: sp.pack.clone(),
                given: case.given.clone().into_iter().collect(),
            };
            let mut sink = Memory::default();
            let mut kept = HashMap::new();
            let out = run(
                &sp.doc,
                &inputs,
                &transport,
                &mut sink,
                &InProcess,
                &HashMap::new(),
                Some(usize::MAX),
                Some(&mut kept),
                None,
            )
            .await;
            let at = format!("{id} test {i} {}", case.name);
            assert!(out.failure.is_none(), "{at}: {:?}", out.failure);
            for (item, failure, _) in &out.items {
                assert!(
                    failure.is_none(),
                    "{at}: row {} failed: {failure:?}",
                    item.key
                );
            }
            for (table, want) in &case.expect {
                let (columns, rows) = rendered(kept.get(table).map(Vec::as_slice).unwrap_or(&[]));
                if let Some(n) = want.rows {
                    assert_eq!(rows.len() as u64, n, "{at}: rows of {table}: {rows:#?}");
                }
                for c in &want.columns {
                    assert!(
                        columns.contains(c),
                        "{at}: {table} has no column {c}: {columns:?}"
                    );
                }
                for w in &want.contains {
                    let w: serde_json::Map<String, Value> = w.clone().into_iter().collect();
                    assert!(
                        rows.iter().any(|r| matches(r, &w)),
                        "{at}: {table} has no row {w:?} in {rows:#?}"
                    );
                }
            }
        }
    }
}
