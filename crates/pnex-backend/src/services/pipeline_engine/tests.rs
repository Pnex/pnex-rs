//! The replay tests of every shipped pipeline (pipelines.md D945):
//! each `tests:` case runs on recorded answers, without network nor base,
//! and its working tables are compared with what it expects.

use std::collections::HashMap;

use std::sync::Arc;

use chrono::NaiveDate;
use serde_json::json;

use super::blob::{Batches, Blob};
use super::replay::{rendered, Memory, Replay};
use super::sql::SqlRunner;
use super::store::shipped;
use super::{fail, run, Failure, Inputs, Req, Transport};

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
        let replay = Replay {
            pack: Some(sp.pack.clone()),
            given: HashMap::new(),
        };
        for (i, c) in sp.doc.tests.iter().enumerate() {
            let problems = super::replay::case(
                &sp.doc,
                i,
                c,
                &replay,
                super::store::call_functions(&sp.pack, &sp.doc),
                super::store::model_extractors(&sp.pack, &sp.doc),
                &InProcess,
            )
            .await;
            assert!(
                problems.is_empty(),
                "{id} test {i} {}: {problems:?}",
                c.name
            );
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
            pack: Some("watch".into()),
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
        pack: Some("watch".into()),
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
        pack: Some("watch".into()),
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
    let (_, rows) = rendered(&kept["kept"]).unwrap();
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

/// D976: the step the server reads as "list hidden" is the connections
/// check, and the click targets the full list (the profile also links the
/// mutual connections, network F only).
#[test]
fn contact_connections_hidden_step_is_the_connections_check() {
    let sp = shipped()
        .iter()
        .find(|p| p.id() == "prospecting_b2b/connections_capture_linkedin")
        .expect("shipped");
    let Some(pnex_core::pipeline::doc::Step::Browse(b)) = sp.doc.steps.get(1) else {
        panic!("browse second")
    };
    assert!(!b.keep, "the raw cards never stay");
    let hidden = &b.steps[crate::services::network_connections::HIDDEN_STEP as usize];
    assert_eq!(hidden.action(), "assert");
    assert!(format!("{hidden:?}").contains("connectionOf"), "{hidden:?}");
    let click = format!("{:?}", b.steps[3]);
    assert!(
        click.contains(r#"connectionOf'][href*='\"S\"']"#),
        "{click}"
    );
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

/// D576 on a table write: a failing check writes nothing and says which
/// check refused what; passing ones write the rows.
#[tokio::test]
async fn table_checks_refuse_or_let_the_write_through() {
    let attempt = |checks: &str| {
        let doc: pnex_core::pipeline::doc::Document = serde_saphyr::from_str(&format!(
            "key: t\nsteps:\n\
             - sql: {{ as: rows, sql: \"SELECT 'a' AS k, 'x' AS v UNION ALL SELECT 'a' AS k, 'y' AS v\" }}\n\
             - write: {{ from: rows, table: {{ name: t, replace: true, checks: {checks} }} }}\n"
        ))
        .unwrap();
        assert!(doc.violations().is_empty(), "{:?}", doc.violations());
        async move {
            let mut sink = Memory::default();
            let transport = Replay {
                pack: Some("watch".into()),
                given: HashMap::new(),
            };
            let out = run(
                &doc,
                &Inputs::default(),
                &transport,
                &mut sink,
                &InProcess,
                &HashMap::new(),
                None,
                None,
                None,
            )
            .await;
            (out, sink.0.contains_key("t"))
        }
    };
    let refused = |out: &super::RunOutcome| {
        let f = out.failure.as_ref().expect("a check refused the rows");
        assert_eq!(f.code, pnex_core::err_codes::PIPELINE_CHECK_FAILED);
        f.args.clone()
    };

    let (out, wrote) = attempt("{ min_rows: 3 }").await;
    assert!(!wrote);
    assert_eq!(
        refused(&out),
        json!({ "output": "t", "check": "min_rows", "column": null, "value": "2", "limit": "3" })
    );

    let (out, wrote) = attempt("{ unique: [k] }").await;
    assert!(!wrote);
    assert_eq!(
        refused(&out),
        json!({ "output": "t", "check": "unique", "column": "k", "value": "1", "limit": "0" })
    );

    let (out, wrote) = attempt(
        "{ min_rows: 2, max_rows: 2, unique: [k, v], max_empty_pct: { v: 0 }, max_drop_pct: 10, same_columns: true }",
    )
    .await;
    assert!(out.failure.is_none(), "{:?}", out.failure);
    assert!(wrote);
    assert_eq!(out.written, 2);
}
