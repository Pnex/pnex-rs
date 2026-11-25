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
    assert!(
        pnex_core::pipeline::track::due(std::slice::from_ref(&item.key), &known, now_us, 10)
            .is_empty()
    );

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

/// D996: every profile network of the packs has its capture pipeline, on
/// the network's own site and in the member's session.
#[test]
fn every_profile_network_has_a_capture_pipeline() {
    let nets = super::store::networks();
    assert_eq!(nets.len(), 6);
    for n in nets {
        let id = format!("prospecting_b2b/profile_capture_{}", n.key);
        let sp = shipped()
            .iter()
            .find(|p| p.id() == id)
            .unwrap_or_else(|| panic!("{id}"));
        let Some(pnex_core::pipeline::doc::Step::Browse(b)) = sp.doc.steps.get(1) else {
            panic!("{id}: browse second")
        };
        assert_eq!(b.site, n.site, "{id}");
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

/// D333, D779, D951: one person's profile on each network of the packs, in
/// its canonical form; search, relation and suggestion pages never are one
/// (whitelist only: a capture never discovers people).
#[test]
fn profiles_are_canonical_and_discovery_pages_are_refused() {
    use super::store::{people_page_of, profile_of};
    use pnex_core::browser_agent::ProfileRefusal::{NotNetwork, NotProfile};
    let ok = |raw: &str| profile_of(raw).unwrap();
    for (raw, network, canonical) in [
        (
            "https://fr.linkedin.com/in/Jane-Doe-123/?originalSubdomain=fr",
            "linkedin",
            "https://www.linkedin.com/in/jane-doe-123",
        ),
        (
            "https://www.linkedin.com/in/jane%C3%A9",
            "linkedin",
            "https://www.linkedin.com/in/jane%c3%a9",
        ),
        ("https://twitter.com/JaneDoe", "x", "https://x.com/janedoe"),
        (
            "https://x.com/jane_doe_123456",
            "x",
            "https://x.com/jane_doe_123456",
        ),
        (
            "https://m.facebook.com/profile.php?id=1000123",
            "facebook",
            "https://www.facebook.com/profile.php?id=1000123",
        ),
        (
            "https://www.facebook.com/jane.doe",
            "facebook",
            "https://www.facebook.com/jane.doe",
        ),
        (
            "https://instagram.com/jane_doe/",
            "instagram",
            "https://www.instagram.com/jane_doe/",
        ),
        (
            "https://www.tiktok.com/@jane.doe",
            "tiktok",
            "https://www.tiktok.com/@jane.doe",
        ),
        (
            "https://www.youtube.com/@Jane/about",
            "youtube",
            "https://www.youtube.com/@jane",
        ),
    ] {
        assert_eq!(
            ok(raw),
            (network.to_string(), canonical.to_string()),
            "{raw}"
        );
    }
    for raw in [
        "https://x.com/jane_doe_1234567",
        "https://www.facebook.com/profile.php?id=12a",
        "https://www.tiktok.com/jane.doe",
        "https://www.youtube.com/@Jane/videos",
        "https://www.linkedin.com/search/results/people/?keywords=acme",
        "https://www.linkedin.com/in/jane/details/connections/",
        "https://www.linkedin.com/mynetwork/",
        "https://www.linkedin.com/company/acme/people/",
        "https://x.com/jane/followers",
        "https://x.com/search?q=acme",
        "https://x.com/followers",
        "https://www.facebook.com/friends",
        "https://www.facebook.com/jane/friends",
        "https://www.facebook.com/profile.php",
        "https://www.facebook.com/photo.php",
        "https://www.instagram.com/explore/",
        "https://www.instagram.com/jane/followers/",
        "https://www.tiktok.com/search?q=jane",
        "https://www.youtube.com/watch?v=abc",
    ] {
        assert_eq!(profile_of(raw), Err(NotProfile), "{raw}");
    }
    for raw in [
        "https://example.org/jane",
        "https://notlinkedin.com/in/jane",
        "ftp://x.com/jane",
        "not a url",
        "https://user:pw@x.com/jane",
    ] {
        assert_eq!(profile_of(raw), Err(NotNetwork), "{raw}");
    }
    assert_eq!(
        people_page_of("https://fr.linkedin.com/company/Acme-Corp/people/?keywords=x"),
        Ok("https://www.linkedin.com/company/acme-corp/people/".into())
    );
    for raw in [
        "https://www.linkedin.com/company/acme/",
        "https://www.linkedin.com/search/results/people/?keywords=acme",
        "https://www.linkedin.com/in/jane/",
        "https://www.linkedin.com/company/acme/people/extra",
    ] {
        assert_eq!(people_page_of(raw), Err(NotProfile), "{raw}");
    }
    assert_eq!(
        people_page_of("https://x.com/company/acme/people"),
        Err(NotNetwork)
    );
}

/// Every followed platform gets the extension's response hook: without it
/// the agent reads no post there (TikTok shipped without it).
#[test]
fn the_extension_hooks_every_followed_platform() {
    for manifest in [
        include_str!("../../../../pnex-extension/extension/manifest.json"),
        include_str!("../../../../pnex-extension/extension/manifest.firefox.json"),
    ] {
        let m: serde_json::Value = serde_json::from_str(manifest).unwrap();
        for n in super::store::networks() {
            let Some(f) = &n.follow else { continue };
            let host = f.pages[0].split('/').nth(2).unwrap();
            for script in m["content_scripts"].as_array().unwrap() {
                let matches = script["matches"].to_string();
                assert!(
                    matches.contains(&format!("https://{host}/*")),
                    "{host}: {script}"
                );
            }
            assert!(!f.responses.is_empty(), "{host}: no response read");
        }
    }
}

/// `after` names a pipeline of the same pack, and no pipeline waits on
/// itself through others (D946): the scheduler would never run them.
#[test]
fn pack_pipelines_run_after_others_without_a_cycle() {
    use super::store::upstream;
    for sp in shipped() {
        for a in &sp.doc.after {
            assert!(
                shipped()
                    .iter()
                    .any(|o| o.pack == sp.pack && &o.doc.key == a),
                "{}: after {a} is not a pipeline of its pack",
                sp.id()
            );
        }
        let (mut seen, mut next) = (Vec::new(), upstream(sp));
        while let Some(up) = next.pop() {
            assert_ne!(up.id(), sp.id(), "{} waits on itself", sp.id());
            if !seen.contains(&up.id()) {
                seen.push(up.id());
                next.extend(upstream(up));
            }
        }
    }
}

/// The plan of a followed platform, as the server sends it.
fn follow(key: &str) -> pnex_core::browser_agent::FollowPlan {
    let n = super::store::networks()
        .iter()
        .find(|n| n.key == key)
        .unwrap();
    n.follow.as_ref().unwrap().plan("")
}

/// The pack's YouTube reader (D951): videos and shorts of both layouts,
/// playlists skipped; captions prefer a track written by people.
#[test]
fn youtube_reader_reads_videos_shorts_and_captions() {
    use pnex_core::post_reader::{cues_of, is_response, posts_of, track_of};
    let plan = follow("youtube");
    let body = include_str!("../../../../pnex-extension/tests/fixtures/youtube_channel.json");
    let posts: Vec<_> = posts_of(&plan, body, "SomeChannel")
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    let ids: Vec<&str> = posts.iter().map(|p| p.platform_id.as_str()).collect();
    assert_eq!(
        ids,
        ["AbCdEfGhIj1", "KlMnOpQrSt2", "Short_Old-5", "UvWxYz01234"]
    );
    assert_eq!(posts[0].text, "Episode 12: the long talk");
    assert_eq!(posts[0].duration_ms, Some(3_723_000));
    assert_eq!(posts[0].url, "https://www.youtube.com/watch?v=AbCdEfGhIj1");
    assert_eq!(posts[0].author, "SomeChannel");
    assert_eq!(
        posts[3].video_url.as_deref(),
        Some("https://www.youtube.com/shorts/UvWxYz01234")
    );
    assert!(is_response(
        &plan,
        "https://www.youtube.com/youtubei/v1/browse?prettyPrint=false"
    ));
    assert!(!is_response(
        &plan,
        "https://www.youtube.com/youtubei/v1/player"
    ));

    let page = plan.captions.as_ref().unwrap();
    let player = serde_json::json!({"captions":{"playerCaptionsTracklistRenderer":{"captionTracks":[
        {"baseUrl":"https://www.youtube.com/api/timedtext?v=x&lang=fr&kind=asr","languageCode":"fr","kind":"asr"},
        {"baseUrl":"https://www.youtube.com/api/timedtext?v=x&lang=en","languageCode":"en"}]}}});
    let (lang, url) = track_of(&page.track, &player, &plan.sites).unwrap();
    assert_eq!(lang, "en");
    assert_eq!(
        url,
        "https://www.youtube.com/api/timedtext?v=x&lang=en&fmt=json3"
    );
    assert!(track_of(
        &page.track,
        &serde_json::json!({"captions":{}}),
        &plan.sites
    )
    .is_none());
    let evil = serde_json::json!({"captions":{"playerCaptionsTracklistRenderer":{"captionTracks":[{"baseUrl":"https://evil.example/x","languageCode":"en"}]}}});
    assert!(
        track_of(&page.track, &evil, &plan.sites).is_none(),
        "only the platform's own caption addresses"
    );

    let json3 = r#"{"events":[
        {"tStartMs":0,"dDurationMs":1500,"segs":[{"utf8":"hello"},{"utf8":"  world"}]},
        {"tStartMs":1500,"dDurationMs":10,"segs":[{"utf8":"\n"}]},
        {"tStartMs":2000}]}"#;
    let cues = cues_of(&plan.cues, json3);
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "hello world");
    assert_eq!((cues[0].start_ms, cues[0].end_ms), (0, 1500));
}

/// The pack's X reader: the account's posts with their best video, a
/// repost of another account skipped, garbage tolerated.
#[test]
fn x_reader_reads_the_account_posts_with_their_best_video() {
    use pnex_core::post_reader::{is_response, posts_of};
    let plan = follow("x");
    let body = include_str!("../../../../pnex-extension/tests/fixtures/x_user_tweets.json");
    let posts: Vec<_> = posts_of(&plan, body, "example_account")
        .into_iter()
        .map(|(p, _)| p)
        .collect();
    let ids: Vec<&str> = posts.iter().map(|p| p.platform_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "1850000000000000001",
            "1850000000000000002",
            "1850000000000000003"
        ]
    );
    let clip = &posts[0];
    assert_eq!(
        clip.video_url.as_deref(),
        Some("https://video.twimg.com/ext_tw_video/1/pu/vid/1280x720/high.mp4")
    );
    assert_eq!(clip.duration_ms, Some(31_500));
    assert_eq!(
        clip.published_at.as_deref(),
        Some("2026-10-17T18:04:05+00:00")
    );
    assert_eq!(
        clip.url,
        "https://x.com/Example_Account/status/1850000000000000001"
    );
    assert!(posts[1].video_url.is_none(), "text only");
    assert!(
        posts[2].video_url.is_none(),
        "the repost wrapper has no media"
    );
    assert!(is_response(
        &plan,
        "https://x.com/i/api/graphql/abc/UserTweets?variables=1"
    ));
    assert!(!is_response(
        &plan,
        "https://x.com/i/api/graphql/abc/HomeTimeline"
    ));
    assert!(posts_of(&plan, "not json", "a").is_empty());
}

/// The pack's TikTok reader: play address and listed captions, another
/// author and a photo post handled.
#[test]
fn tiktok_reader_reads_play_addresses_and_captions() {
    use pnex_core::post_reader::{is_response, posts_of};
    let plan = follow("tiktok");
    let body = include_str!("../../../../pnex-extension/tests/fixtures/tiktok_item_list.json");
    let posts = posts_of(&plan, body, "Some.Account");
    assert_eq!(posts.len(), 2, "the other author is skipped");
    let (clip, captions) = &posts[0];
    assert_eq!(clip.platform_id, "7420000000000000001");
    assert_eq!(
        clip.url,
        "https://www.tiktok.com/@some.account/video/7420000000000000001"
    );
    assert_eq!(clip.duration_ms, Some(42_000));
    assert_eq!(clip.published_at.as_deref(), Some("2025-10-18T10:04:05Z"));
    assert!(clip.video_url.as_deref().unwrap().contains("a.mp4"));
    assert_eq!(captions.as_ref().map(|(l, _)| l.as_str()), Some("fr-FR"));
    assert!(posts[1].0.video_url.is_none(), "photo post");
    assert!(is_response(
        &plan,
        "https://www.tiktok.com/api/post/item_list/?aid=1988"
    ));
}
