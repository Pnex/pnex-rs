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
use super::{fail, run, Failure, Inputs, Req, Row, Sink, Table, Transport};

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

/// Writes kept in memory; the org's tables a case gives, from its fixtures.
#[derive(Default)]
struct Memory(
    HashMap<String, Vec<Arc<Batches>>>,
    HashMap<String, Arc<Batches>>,
    /// Extracts of each navigation (`given: browse:<as>`): row → name → text.
    HashMap<String, Value>,
    /// The model's answer of each `model` step (`given: model:<as>`).
    HashMap<String, Value>,
);

impl Memory {
    fn with_case(
        pack: &str,
        tables: &std::collections::BTreeMap<String, String>,
        given: &std::collections::BTreeMap<String, String>,
    ) -> Self {
        let file =
            |name: &str| fixture(pack, name).unwrap_or_else(|| panic!("{pack}/{name}: no fixture"));
        let tables = tables
            .iter()
            .map(|(name, f)| (name.clone(), csv_text(file(f))))
            .collect();
        let browsed = given
            .iter()
            .filter_map(|(k, f)| {
                let step = k.strip_prefix("browse:")?;
                let v = serde_json::from_slice(file(f)).expect("browse fixture is JSON");
                Some((step.to_string(), v))
            })
            .collect();
        let answers = given
            .iter()
            .filter_map(|(k, f)| {
                let step = k.strip_prefix("model:")?;
                let v = serde_json::from_slice(file(f)).expect("model fixture is JSON");
                Some((step.to_string(), v))
            })
            .collect();
        Self(HashMap::new(), tables, browsed, answers)
    }
}

/// A CSV fixture as text columns.
fn csv_text(bytes: &[u8]) -> Arc<Batches> {
    use arrow_array::{ArrayRef, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    let mut rd = csv::Reader::from_reader(bytes);
    let names: Vec<String> = rd.headers().unwrap().iter().map(str::to_string).collect();
    let rows: Vec<csv::StringRecord> = rd.records().map(Result::unwrap).collect();
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
    let mut w = super::blob::BatchWriter::new(schema.clone()).unwrap();
    w.write(&RecordBatch::try_new(schema, cols).unwrap())
        .unwrap();
    w.finish().unwrap()
}

impl Sink for Memory {
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

    async fn media(
        &mut self,
        _: &pnex_core::pipeline::doc::MediaTarget,
        rows: Vec<serde_json::Map<String, Value>>,
    ) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn proposals(
        &mut self,
        _: &pnex_core::pipeline::doc::ProposalsTarget,
        rows: Vec<serde_json::Map<String, Value>>,
    ) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn objects(
        &mut self,
        _: &pnex_core::pipeline::doc::ObjectsTarget,
        rows: Vec<serde_json::Map<String, Value>>,
    ) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn ask(&mut self, rows: Vec<serde_json::Map<String, Value>>) -> Result<u64, Failure> {
        Ok(rows.len() as u64)
    }

    async fn model(
        &mut self,
        step: &pnex_core::pipeline::doc::Model,
        ex: &pnex_core::extractor::Extractor,
        rows: Vec<serde_json::Map<String, Value>>,
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
                extractors: super::store::model_extractors(&sp.pack, &sp.doc),
                asked: case
                    .asked
                    .iter()
                    .map(|r| {
                        r.iter()
                            .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                            .collect()
                    })
                    .collect(),
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
            let mut sink = Memory::with_case(&sp.pack, &case.tables, &case.given);
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

/// D975: a fetch reaches the host of an address the org's objects hold,
/// but never with a secret.
#[tokio::test]
async fn object_hosts_open_their_hosts_but_never_to_a_secret() {
    let doc = |secret: &str| -> pnex_core::pipeline::doc::Document {
        serde_saphyr::from_str(&format!(
            "key: t\nobject_hosts: [{{ type: news_source, property: url }}]\nsteps:\n\
             - enumerate: {{ as: s, cross: [{{ as: url, objects: {{ type: news_source, property: url }} }}] }}\n\
             - fetch: {{ as: f, for_each: s, url: \"{{{{url}}}}\"{secret} }}\n\
             - read: {{ as: r, from: f, format: xml, records: rss/channel/item, columns: [{{ name: link }}] }}\n\
             - write: {{ from: r, table: {{ name: t, replace: true }} }}\n"
        ))
        .unwrap()
    };
    let attempt = |doc: pnex_core::pipeline::doc::Document, url: &'static str| async move {
        assert!(doc.violations().is_empty(), "{:?}", doc.violations());
        let inputs = Inputs {
            objects: [("news_source.url".to_string(), vec![url.to_string()])].into(),
            secrets: [("key".to_string(), "k".to_string())].into(),
            ..Default::default()
        };
        let transport = Replay {
            pack: "watch".into(),
            given: [(url.to_string(), "rss.xml".to_string())].into(),
        };
        let (mut sink, mut kept) = (Memory::default(), HashMap::new());
        let out = run(
            &doc,
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
        out.failure.map(|f| f.code.to_string())
    };
    let feed = "https://news.example.org/rss.xml";
    assert_eq!(attempt(doc(""), feed).await, None);
    let with_secret = ", secret: { name: key, header: Authorization }";
    assert_eq!(
        attempt(doc(with_secret), feed).await.as_deref(),
        Some("host_not_declared")
    );
    // A literal address skips the guarded resolver: checked here (R8).
    assert_eq!(
        attempt(doc(""), "http://169.254.169.254/latest")
            .await
            .as_deref(),
        Some("egress_address_refused")
    );
}

/// D995: a navigation pauses its row with the row kept; its extracts
/// resume the steps after it, as `{name, text}` rows.
#[tokio::test]
async fn browse_pauses_a_row_and_its_extracts_resume_it() {
    let doc: pnex_core::pipeline::doc::Document = serde_saphyr::from_str(
        "key: t\nhosts: [www.example.org]\nsteps:\n\
         - enumerate: { as: p, track: true, cross: [{ as: url, objects: { type: person, property: url } }] }\n\
         - browse: { as: page, for_each: p, site: www.example.org, keep: false, steps: [{ open: \"{{url}}\" }, { extract: { all_text: true, as: profile } }] }\n\
         - sql: { as: kept, sql: \"SELECT p.url, page.text FROM p, page WHERE page.name = 'profile'\" }\n\
         - write: { from: kept, table: { name: t, replace: false } }\n",
    )
    .unwrap();
    assert!(doc.violations().is_empty(), "{:?}", doc.violations());
    let url = "https://www.example.org/in/jane";
    let inputs = Inputs {
        objects: [("person.url".to_string(), vec![url.to_string()])].into(),
        ..Default::default()
    };
    let transport = Replay {
        pack: "watch".into(),
        given: HashMap::new(),
    };
    let mut sink = Memory::default();
    let out = run(
        &doc,
        &inputs,
        &transport,
        &mut sink,
        &InProcess,
        &HashMap::new(),
        Some(usize::MAX),
        None,
        None,
    )
    .await;
    let (item, failure, rows) = &out.items[0];
    assert_eq!(item.state, pnex_core::pipeline::track::State::Waiting);
    assert_eq!(*rows, 0);
    let failure = failure.as_ref().expect("waiting carries the row");
    assert_eq!(failure.code, super::WAITING);
    let row = failure.args["row"].as_object().unwrap().clone();
    assert_eq!(row["url"], url);
    // Waiting rows are not taken again before their wait is over.
    let known = HashMap::from([(item.key.clone(), item.clone())]);
    let now_us = inputs.now.and_utc().timestamp_micros();
    assert!(pnex_core::pipeline::track::due(&[item.key.clone()], &known, now_us, 10).is_empty());

    let extracts = super::extract_rows([("profile".to_string(), "Jane, engineer".to_string())]);
    let (done, failure, rows) = super::resume(
        &doc,
        &inputs,
        &transport,
        &mut sink,
        &InProcess,
        Some(item),
        &item.key,
        row.clone(),
        Some(extracts),
    )
    .await;
    assert!(failure.is_none(), "{failure:?}");
    assert_eq!(
        (done.state, rows),
        (pnex_core::pipeline::track::State::Done, 1)
    );
    // A failed navigation fails the row, never the run.
    let (failed, failure, _) = super::resume(
        &doc,
        &inputs,
        &transport,
        &mut sink,
        &InProcess,
        Some(item),
        &item.key,
        row,
        None,
    )
    .await;
    assert_eq!(failed.state, pnex_core::pipeline::track::State::Failed);
    assert_eq!(failure.map(|f| f.code), Some("browse_failed"));
}

/// D996: an `asked` axis enumerates the rows its start gives, with their
/// columns, and takes them again even once done.
#[tokio::test]
async fn asked_rows_are_taken_with_their_columns_every_time() {
    let doc: pnex_core::pipeline::doc::Document = serde_saphyr::from_str(
        "key: t\nhosts: [www.example.org]\nsteps:\n\
         - enumerate: { as: p, track: true, cross: [{ as: person, asked: [url] }] }\n\
         - browse: { as: page, for_each: p, site: www.example.org, steps: [{ open: \"{{url}}\" }, { extract: { all_text: true, as: profile } }] }\n\
         - sql: { as: kept, sql: \"SELECT p.person, p.url, page.text FROM p, page\" }\n\
         - write: { from: kept, table: { name: t, replace: false } }\n",
    )
    .unwrap();
    assert!(doc.violations().is_empty(), "{:?}", doc.violations());
    let inputs = Inputs {
        asked: vec![serde_json::Map::from_iter([
            ("person".to_string(), json!("p1")),
            ("url".to_string(), json!("https://www.example.org/in/jane")),
        ])],
        ..Default::default()
    };
    let transport = Replay {
        pack: "watch".into(),
        given: HashMap::new(),
    };
    let mut sink = Memory::default();
    sink.2
        .insert("page".into(), json!({ "p1": { "profile": "Jane" } }));
    let mut kept = HashMap::new();
    let mut known = HashMap::new();
    for _ in 0..2 {
        let out = run(
            &doc,
            &inputs,
            &transport,
            &mut sink,
            &InProcess,
            &known,
            None,
            Some(&mut kept),
            None,
        )
        .await;
        assert_eq!(out.items.len(), 1, "taken again although done");
        let (item, failure, rows) = &out.items[0];
        assert!(failure.is_none(), "{failure:?}");
        assert_eq!((item.key.as_str(), *rows), ("p1", 1));
        known.insert(item.key.clone(), item.clone());
    }
    let (_, rows) = rendered(&kept["kept"]);
    assert!(
        rows[0]
            .to_string()
            .contains("https://www.example.org/in/jane"),
        "{rows:?}"
    );
}

/// D996: every profile network has its capture pipeline, on its own site
/// and in the member's session.
#[test]
fn every_profile_network_has_a_capture_pipeline() {
    for n in pnex_core::browser_agent::PROFILE_NETWORKS {
        let id = format!("prospecting_b2b/profile_capture_{n}");
        let sp = shipped()
            .iter()
            .find(|p| p.id() == id)
            .unwrap_or_else(|| panic!("{id}"));
        let Some(pnex_core::pipeline::doc::Step::Browse(b)) = sp.doc.steps.get(1) else {
            panic!("{id}: browse second")
        };
        assert_eq!(
            Some(b.site.as_str()),
            pnex_core::default_rules::network_site(n),
            "{id}"
        );
        assert_eq!(b.session.as_deref(), Some("required"), "{id}");
    }
}

/// D995: `browse` only right after a tracked enumeration, on its rows.
#[test]
fn browse_only_follows_a_tracked_enumeration() {
    let doc = |track: &str, from: &str| -> pnex_core::pipeline::doc::Document {
        serde_saphyr::from_str(&format!(
            "key: t\nhosts: [www.example.org]\nsteps:\n\
             - enumerate: {{ as: p, {track} cross: [{{ as: url, objects: {{ type: person, property: url }} }}] }}\n\
             - browse: {{ as: page, for_each: {from}, site: www.example.org, steps: [{{ open: \"{{{{url}}}}\" }}] }}\n\
             - write: {{ from: page, table: {{ name: t, replace: false }} }}\n"
        ))
        .unwrap()
    };
    let position = |d: pnex_core::pipeline::doc::Document| {
        d.violations().iter().any(|v| v.token == "browse_position")
    };
    assert!(!position(doc("track: true,", "p")));
    assert!(position(doc("", "p")));
}

/// A source answering 503 is asked once per run: the other rows wait,
/// none of them counted as failed; its pause doubles up to an hour.
#[tokio::test]
async fn a_source_pushing_back_ends_the_run() {
    struct Busy(std::sync::atomic::AtomicUsize);
    impl Transport for Busy {
        async fn get(&self, url: &reqwest::Url, _: &Req) -> Result<Option<Blob>, Failure> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(fail(
                "http_status",
                json!({ "url": url.as_str(), "status": 503 }),
            ))
        }

        async fn post_form(
            &self,
            url: &reqwest::Url,
            _: Option<&reqwest::Url>,
            _: &[(String, String)],
            r: &Req,
        ) -> Result<Option<Blob>, Failure> {
            self.get(url, r).await
        }
    }
    let sp = shipped()
        .iter()
        .find(|sp| sp.id() == "markets/dukascopy_m1")
        .expect("dukascopy_m1 is shipped");
    let inputs = Inputs {
        settings: [
            ("markets.dukascopy_m1_symbols", "eurusd"),
            ("markets.dukascopy_m1_from", "2024-03-01"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .into(),
        now: NaiveDate::from_ymd_opt(2024, 3, 20)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap(),
        ..Default::default()
    };
    let busy = Busy(Default::default());
    let out = run(
        &sp.doc,
        &inputs,
        &busy,
        &mut Memory::default(),
        &InProcess,
        &HashMap::new(),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(busy.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(out.items.is_empty(), "{:?}", out.items);
    assert_eq!(out.failure.map(|f| f.code), Some("http_status"));
    let pauses: Vec<u64> = (1..=8)
        .map(|n| super::store::cooldown(n).as_secs())
        .collect();
    assert_eq!(pauses, [60, 120, 240, 480, 960, 1920, 3600, 3600]);
}
