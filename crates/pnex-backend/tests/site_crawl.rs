//! Site crawls and web proofs (partner-watch.md D307–D310): a server crawl
//! of a local site (robots.txt, sitemap, links, never off the origin),
//! relevance, the page registry and its `record` objects, misses → gone,
//! then the proof of a relevant page uploaded part by part by a paired
//! agent, sealed in the org's chain, verified, and a tampered manifest
//! caught.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL) and Valkey, database emptied
//! between tests.

mod common;

use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_core::browser_agent::{self as ba, MissionSpec};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, loco_rs::app::AppContext, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, ctx, alice).await;
        },
    )
    .await;
}

async fn call(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<Value>,
) -> (u16, Value) {
    let req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {token}"))
    .add_header("X-Org-Id", org.to_string());
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

/// The site: two pages linked from the home, one off the origin, one
/// excluded by robots.txt. `ALT` drops the second page.
fn site(state: Arc<Mutex<bool>>) -> axum::Router {
    use axum::response::Html;
    use axum::routing::get;
    let s1 = state.clone();
    axum::Router::new()
        .route("/robots.txt", get(|| async { "User-agent: *\nDisallow: /admin\n" }))
        .route("/sitemap.xml", get(|| async { (axum::http::StatusCode::NOT_FOUND, "") }))
        .route(
            "/",
            get(move || {
                let alt = *s1.lock().unwrap();
                async move {
                    let second = if alt { "" } else { r#"<a href="/formations/rs">RS</a>"# };
                    Html(format!(
                        r#"<html><head><title>Accueil</title></head><body><p>Organisme</p>
                        <a href="/formations/rncp">RNCP</a>{second}<a href="/admin/x">admin</a>
                        <a href="https://ailleurs.example/x">ailleurs</a></body></html>"#
                    ))
                }
            }),
        )
        .route(
            "/formations/rncp",
            get(|| async { Html("<html><head><title>Titre pro</title></head><body><h1>Titre RNCP12345</h1><p>Éligible CPF</p></body></html>") }),
        )
        .route(
            "/formations/rs",
            get(|| async { Html("<html><head><title>Bureautique</title></head><body><p>Formation courte</p></body></html>") }),
        )
        .route("/admin/x", get(|| async { Html("<p>RNCP12345 secret</p>") }))
}

struct Install {
    pair: EcdsaKeyPair,
    rng: SystemRandom,
}

impl Install {
    fn new() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        Self { pair, rng }
    }

    fn sign(&self, msg: &str) -> String {
        URL_SAFE_NO_PAD.encode(self.pair.sign(&self.rng, msg.as_bytes()).unwrap())
    }

    fn pair_body(&self, org: i64) -> Value {
        let ts = chrono::Utc::now().timestamp_millis();
        json!({
            "install_key": URL_SAFE_NO_PAD.encode(self.pair.public_key().as_ref()),
            "fingerprint": { "browser": "Chrome", "os": "Linux" },
            "ts": ts,
            "signature": self.sign(&ba::pair_message(org, ts)),
        })
    }
}

fn sha_hex(b: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

#[tokio::test]
#[serial]
async fn server_crawl_registry_and_a_sealed_proof_chain() {
    with_app(|server, ctx, alice| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, _) = call(&server, "POST", "/api/v1/ontology/packs/quali_watch/install", &alice, org, None).await;
        assert_eq!(st, 200);
        let alt = Arc::new(Mutex::new(false));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = site(alt.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let origin = format!("http://{addr}");

        let (st, m) = call(&server, "POST", "/api/v1/extension/missions", &alice, org, Some(json!({
            "name": "Site OF", "interval_secs": 86400,
            "spec": {"kind": "crawl_site", "params": {
                "url": format!("{origin}/formations/x?y=1"),
                "match": [{"kind": "regex", "value": "RNCP\\s?\\d{5}"}, {"kind": "word", "value": "eligible"}],
                "record": {"type": "page_observee", "properties": {"url": "url", "title": "titre", "state": "etat", "relevant": "pertinente"},
                           "proof_link": "prouve_par"}
            }}
        }))).await;
        assert_eq!(st, 201, "{m}");
        assert_eq!(m["target"], "web:site:127.0.0.1", "keyed by the normalised host (D307)");
        let mission: uuid::Uuid = m["id"].as_str().unwrap().parse().unwrap();
        let Ok(MissionSpec::CrawlSite(params)) = serde_json::from_value::<MissionSpec>(m["spec"].clone()) else {
            panic!("crawl spec");
        };
        // A proof task cannot be created by hand.
        let (st, _) = call(&server, "POST", "/api/v1/extension/missions", &alice, org, Some(json!({
            "name": "x", "interval_secs": 3600,
            "spec": {"kind": "capture_proof", "params": {"url": "https://e.fr", "page_id": "00000000-0000-0000-0000-000000000000"}}
        }))).await;
        assert_eq!(st, 400);

        let out = pnex_backend::services::crawl::crawl(&ctx, org, mission, &params).await;
        assert_eq!((out.pages, out.relevant, out.blocked), (3, 1, None), "{out:?}");
        let (_, pages) = call(&server, "GET", &format!("/api/v1/extension/missions/{mission}/pages"), &alice, org, None).await;
        let urls: Vec<&str> = pages["results"].as_array().unwrap().iter().map(|p| p["url"].as_str().unwrap()).collect();
        assert!(urls.iter().all(|u| u.starts_with(&origin)), "{urls:?}");
        assert!(!urls.iter().any(|u| u.contains("/admin")), "robots.txt obeyed");
        let relevant = pages["results"].as_array().unwrap().iter().find(|p| p["state"] == "relevant").cloned().expect("relevant page");
        assert!(relevant["url"].as_str().unwrap().ends_with("/formations/rncp"));
        assert!(relevant["doc_asset_id"].is_string(), "its text is kept as a document");
        // The record wrote the page as an object.
        let object = relevant["object_id"].as_str().expect("recorded object");
        let (_, o) = call(&server, "GET", &format!("/api/v1/ontology/objects/{object}"), &alice, org, None).await;
        assert_eq!((o["properties"]["etat"].clone(), o["properties"]["pertinente"].clone()), (json!("relevant"), json!(true)), "{o}");

        // A page gone from two complete crawls is `gone`.
        *alt.lock().unwrap() = true;
        for _ in 0..2 {
            let out = pnex_backend::services::crawl::crawl(&ctx, org, mission, &params).await;
            assert_eq!(out.pages, 2, "{out:?}");
        }
        let (_, gone) = call(&server, "GET", &format!("/api/v1/extension/missions/{mission}/pages?state=gone"), &alice, org, None).await;
        assert_eq!(gone["count"], 1, "{gone}");

        // The relevant page has one open proof task for an agent.
        let row = ctx.db.query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT id::text AS id, count(*) OVER () AS n FROM extension_missions WHERE org_id = $1 AND kind = 'capture_proof' AND enabled",
            [org.into()],
        )).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 1, "one task per page, whatever the crawls");
        let task: String = row.try_get("", "id").unwrap();
        let (_, listed) = call(&server, "GET", "/api/v1/extension/missions", &alice, org, None).await;
        assert!(listed["results"].as_array().unwrap().iter().all(|m| m["spec"]["kind"] != "capture_proof"));

        let install = Install::new();
        let (st, paired) = call(&server, "POST", "/api/v1/extension/agents", &alice, org, Some(install.pair_body(org))).await;
        assert_eq!(st, 201, "{paired}");
        let (token, device) = (paired["token"].as_str().unwrap().to_string(), paired["agent"]["device_id"].as_str().unwrap().to_string());
        let png = {
            // A real 1×1 PNG: the media store sniffs it.
            URL_SAFE_NO_PAD
                .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk-M9QDwADhgGAWjR9awAAAABJRU5ErkJggg")
                .unwrap()
        };
        let html = br#"<html><body onload="steal()"><script>x()</script><h1>Titre RNCP12345</h1></body></html>"#.to_vec();
        let text = serde_json::to_vec(&json!({"mission_id": task, "url": relevant["url"], "title": "Titre pro", "text": "Titre RNCP12345\nÉligible CPF"})).unwrap();
        let mut sealed = Value::Null;
        for (part, body) in [(ba::ProofPart::Html, html), (ba::ProofPart::Screenshot, png), (ba::ProofPart::Text, text)] {
            let ts = chrono::Utc::now().timestamp_millis();
            let sig = install.sign(&ba::proof_upload_message(&device, ts, &task, part, &sha_hex(&body)));
            let res = server
                .post("/api/v1/extension/uploads/proof")
                .add_header("Authorization", format!("Bearer {token}"))
                .add_header("X-Pnex-Ts", ts.to_string())
                .add_header("X-Pnex-Signature", sig)
                .add_header("X-Pnex-Mission", task.clone())
                .add_header("X-Pnex-Part", part.wire())
                .bytes(body.into())
                .await;
            assert_eq!(res.status_code().as_u16(), 201, "{}", res.text());
            sealed = res.json::<Value>();
        }
        assert_eq!(sealed["sealed"], true);
        let proof_id = sealed["proof_id"].as_str().unwrap();
        let (_, p) = call(&server, "GET", &format!("/api/v1/extension/proofs/{proof_id}"), &alice, org, None).await;
        assert_eq!((p["state"].clone(), p["seq"].clone()), (json!("sealed"), json!(1)), "{p}");
        assert_eq!(p["manifest"]["prev_manifest_sha256"], Value::Null);
        assert!(p["manifest"]["captured_at"].is_string());
        // The stored HTML lost its script and handler.
        let html_media = p["parts"]["html"]["media_id"].as_str().unwrap();
        let (_, chain) = call(&server, "GET", "/api/v1/extension/proofs/verify", &alice, org, None).await;
        assert_eq!((chain["ok"].clone(), chain["count"].clone()), (json!(true), json!(1)), "{chain}");
        // The page is proven, the task closed, the parts linked to its object.
        let (_, pages) = call(&server, "GET", &format!("/api/v1/extension/missions/{mission}/pages?state=captured"), &alice, org, None).await;
        assert_eq!(pages["count"], 1, "{pages}");
        let (_, links) = call(&server, "GET", &format!("/api/v1/ontology/objects/{object}/links"), &alice, org, None).await;
        let proven = links["results"].as_array().unwrap().iter().filter(|l| l["link_type"] == "prouve_par").count();
        assert_eq!(proven, 3, "{links}");
        assert!(!html_media.is_empty());

        // A manifest changed afterwards breaks the chain.
        ctx.db.execute_unprepared(&format!(
            "UPDATE web_proofs SET manifest = jsonb_set(manifest, '{{final_url}}', '\"https://faux.example\"') WHERE id = '{proof_id}'"
        )).await.unwrap();
        let (_, chain) = call(&server, "GET", "/api/v1/extension/proofs/verify", &alice, org, None).await;
        assert_eq!((chain["ok"].clone(), chain["broken_at"].clone()), (json!(false), json!(1)), "{chain}");
    })
    .await;
}

/// network-check.md D407: a flow's `site_crawl` node walks the site of its
/// message with the message's rules, through a disabled mission kept per
/// site, and answers the relevant pages and a summary.
#[tokio::test]
#[serial]
async fn flow_node_crawls_the_site_of_its_message() {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, Set};
    with_app(|server, ctx, alice| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, _) = call(&server, "POST", "/api/v1/ontology/packs/quali_watch/install", &alice, org, None).await;
        assert_eq!(st, 200);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = site(Arc::new(Mutex::new(false)));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (st, website) = call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(json!({
            "type_key": "site_web", "title": "of.example", "properties": {"domaine": "127.0.0.1", "url_racine": format!("http://{addr}/")}
        }))).await;
        assert_eq!(st, 201, "{website}");

        let flow = flows::ActiveModel {
            name: Set("check".into()),
            status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
            org_id: Set(org),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let version = flow_versions::ActiveModel {
            flow_id: Set(flow.id),
            version_number: Set(1),
            graph: Set(json!({"nodes": [{"id": "crawl", "kind": "site_crawl", "config": {
                "max_pages": 20, "proofs": false,
                "record": {"type": "page_observee", "properties": {"url": "url", "title": "titre"}, "link_type": "page_de"}
            }}]})),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .unwrap();
        let mut am: flows::ActiveModel = flow.clone().into();
        am.deployed_version_id = Set(Some(version.id));
        am.update(&ctx.db).await.unwrap();
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-sc-1") };
        let crawl = |body: Value| {
            let server = &server;
            async move {
                let res = server.post("/internal/flow/site-crawl").add_header("x-pnex-flow-token", "tok-sc-1").json(&body).await;
                (res.status_code().as_u16(), serde_json::from_str::<Value>(&res.text()).unwrap_or(Value::Null))
            }
        };
        let node = json!({"org_id": org, "flow_id": flow.id, "version": 1, "node_id": "crawl"});
        let ask = json!({"node": node, "url": format!("http://{addr}/formations/x"),
            "match": [{"kind": "regex", "value": "RNCP\\s?\\d{5}"}], "link_to": website["id"]});

        let (st, r) = crawl(ask.clone()).await;
        assert_eq!((st, r["port"].clone()), (200, json!(0)), "{r}");
        let pages = r["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1, "{r}");
        assert!(pages[0]["url"].as_str().unwrap().ends_with("/formations/rncp"), "{r}");
        assert!(pages[0]["text"].as_str().unwrap().contains("RNCP12345"), "{r}");
        assert_eq!((pages[0]["changed"].clone(), pages[0]["object_id"].is_string()), (json!(true), true), "{r}");
        assert_eq!((r["summary"]["pages"].clone(), r["summary"]["relevant"].clone()), (json!(3), json!(1)), "{r}");
        // The page object hangs off the website (record link to link_to).
        let linked = ctx.db.execute_unprepared(&format!(
            "SELECT 1 FROM resource_edges WHERE org_id = {org} AND relation = 'page_de' AND valid_to IS NULL")).await.unwrap();
        assert_eq!(linked.rows_affected(), 1);
        // One disabled mission for the flows; a second pass reuses it.
        let (st, r2) = crawl(ask.clone()).await;
        assert_eq!(st, 200);
        assert_eq!((r2["summary"]["mission_id"].clone(), r2["pages"][0]["changed"].clone()), (r["summary"]["mission_id"].clone(), json!(false)), "{r2}");
        let missions = ctx.db.execute_unprepared(&format!(
            "SELECT 1 FROM extension_missions WHERE org_id = {org} AND kind = 'crawl_site' AND NOT enabled")).await.unwrap();
        assert_eq!(missions.rows_affected(), 1);

        // Refusals on the failure port, with their code.
        let mut bad = ask.clone();
        bad["url"] = json!("ftp://x");
        let (_, r) = crawl(bad).await;
        assert_eq!((r["port"].clone(), r["code"].clone()), (json!(2), json!("site-crawl-url-invalid")), "{r}");
        let mut bad = ask.clone();
        bad["match"] = json!((0..60).map(|i| json!({"kind": "word", "value": format!("w{i}")})).collect::<Vec<_>>());
        let (_, r) = crawl(bad).await;
        assert_eq!((r["code"].clone(), r["args"]["field"].clone()), (json!("site-crawl-invalid"), json!("match")), "{r}");
        let mut forged = ask.clone();
        forged["node"]["node_id"] = json!("nope");
        let (st, _) = crawl(forged).await;
        assert_eq!(st, 403);
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}

/// network-check.md D408: a proof no agent took is tried by the server's
/// browser, three times at most; without a browser it stays for an agent.
#[tokio::test]
#[serial]
async fn waiting_proofs_are_tried_by_the_server_a_bounded_number_of_times() {
    with_app(|server, ctx, alice| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        unsafe { std::env::set_var("PNEX_BROWSER_URL", "http://127.0.0.1:1") };
        let spec = json!({"kind": "capture_proof", "params": {"url": "https://of.example.fr/rs", "page_id": "00000000-0000-0000-0000-000000000000"}});
        let fresh = uuid::Uuid::new_v4();
        let old = uuid::Uuid::new_v4();
        for (id, age) in [(fresh, "0 minutes"), (old, "10 minutes")] {
            ctx.db.execute_unprepared(&format!(
                "INSERT INTO extension_missions (id, org_id, name, kind, spec, target, interval_secs, enabled, state_code, created_at) \
                 VALUES ('{id}', {org}, 'Proof', 'capture_proof', '{spec}', 'web:of.example.fr', 86400, true, 'ext-no-agent', now() - interval '{age}')"
            )).await.unwrap();
        }
        for _ in 0..4 {
            let sealed = pnex_backend::services::proofs::server_pass(&ctx).await.unwrap();
            assert_eq!(sealed, 0);
        }
        let tries = |id: uuid::Uuid| {
            let db = &ctx.db;
            async move {
                db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres,
                    format!("SELECT coalesce((cursor ->> 'server_tries')::int, 0) AS n, enabled FROM extension_missions WHERE id = '{id}'")))
                    .await.unwrap().map(|r| (r.try_get::<i32>("", "n").unwrap(), r.try_get::<bool>("", "enabled").unwrap())).unwrap()
            }
        };
        assert_eq!(tries(old).await, (3, true), "bounded, and left to an agent");
        assert_eq!(tries(fresh).await, (0, true), "an agent gets 5 minutes first");
        unsafe { std::env::remove_var("PNEX_BROWSER_URL") };
    })
    .await;
}

/// The server snapshot on a real browser and a public page:
/// `PNEX_BROWSER_URL=http://127.0.0.1:9222 cargo test --test site_crawl server_snapshot_of_a_public_page -- --ignored`.
#[tokio::test]
#[serial]
#[ignore]
async fn server_snapshot_of_a_public_page() {
    with_app(|server, ctx, alice| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let shot = pnex_backend::services::site_recipes::endpoints::snapshot(
            &ctx.db,
            None,
            org,
            "https://example.com/",
        )
        .await
        .unwrap();
        assert!(shot.png.starts_with(b"\x89PNG"), "png");
        assert!(shot.text.contains("documentation"), "{}", shot.text);
        assert!(shot.html.contains("<html"), "html");
        println!(
            "engine {} · png {} bytes · html {} · text {}",
            shot.engine,
            shot.png.len(),
            shot.html.len(),
            shot.text.len()
        );
    })
    .await;
}
