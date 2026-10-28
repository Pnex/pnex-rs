//! Change feed of the ontology (external-apps.md D435) on an invented
//! ontology (`article`, `author`, `written_by`): creates, updates, closes,
//! one entry per object and page, a transaction that commits late never
//! skipped, compaction to a full copy, expiry, filters, and a service
//! account reading it.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseConnection, TransactionTrait};
use serde_json::{json, Value};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, DatabaseConnection) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, alice, ctx.db.clone()).await;
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
    let mut req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "PUT" => server.put(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {token}"))
    .add_header("X-Org-Id", org.to_string());
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.await;
    (
        res.status_code().as_u16(),
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

const SCOPE: &str = "types=article,author&link_types=written_by";

async fn feed(server: &axum_test::TestServer, token: &str, org: i64, since: &str) -> Value {
    let (st, page) = call(
        server,
        "GET",
        &format!("/api/v1/ontology/changes?since={since}&{SCOPE}"),
        token,
        org,
        None,
    )
    .await;
    assert_eq!(st, 200, "{page}");
    page
}

fn summary(page: &Value) -> Vec<(String, String, String)> {
    page["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let name = c["object"]["title"]
                .as_str()
                .or(c["link"]["link_type"].as_str())
                .unwrap_or_default();
            (
                c["action"].as_str().unwrap().to_string(),
                c["type_key"].as_str().unwrap().to_string(),
                name.to_string(),
            )
        })
        .collect()
}

fn s(a: &str, t: &str, n: &str) -> (String, String, String) {
    (a.into(), t.into(), n.into())
}

#[tokio::test]
#[serial]
async fn the_feed_follows_every_change_without_a_gap() {
    with_app(|server, alice, db| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        for key in ["article", "author"] {
            let (st, t) = call(&server, "POST", "/api/v1/ontology/types", &alice, org, Some(json!({"def": {
                "key": key, "name": key,
                "properties": [{"key": "lang", "name": "Lang", "kind": "text", "max_len": 8}]
            }}))).await;
            assert_eq!(st, 201, "{t}");
        }
        let (st, lt) = call(&server, "POST", "/api/v1/ontology/link-types", &alice, org,
            Some(json!({"key": "written_by", "name": "written by", "from_types": ["article"], "to_types": ["author"]}))).await;
        assert_eq!(st, 201, "{lt}");

        let (st, h) = call(&server, "GET", "/api/v1/ontology/changes/head", &alice, org, None).await;
        assert_eq!(st, 200, "{h}");
        let start = feed(&server, &alice, org, "0").await;
        assert_eq!(start["changes"], json!([]), "nothing in scope yet: {start}");
        let c0 = start["next"].as_str().unwrap().to_string();
        assert_eq!(c0, h["cursor"].as_str().unwrap(), "a short page ends at the head");

        let obj = |t: &str, title: &str| json!({"type_key": t, "title": title, "properties": {"lang": "fr"}});
        let (_, ada) = call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("author", "Ada"))).await;
        let (_, art) = call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("article", "Engines"))).await;
        let (st, link) = call(&server, "POST", "/api/v1/ontology/links", &alice, org,
            Some(json!({"link_type": "written_by", "source_id": art["id"], "target_id": ada["id"]}))).await;
        assert_eq!(st, 201, "{link}");
        let p1 = feed(&server, &alice, org, &c0).await;
        assert_eq!(
            summary(&p1),
            vec![s("create", "author", "Ada"), s("create", "article", "Engines"), s("create", "written_by", "written_by")],
            "{p1}"
        );
        assert_eq!(p1["more"], false);
        let c1 = p1["next"].as_str().unwrap().to_string();
        assert_eq!(feed(&server, &alice, org, &c1).await["changes"], json!([]), "nothing new");

        // Two updates in a page: one entry, the current state.
        let url = format!("/api/v1/ontology/objects/{}", art["id"].as_str().unwrap());
        let (_, v2) = call(&server, "PUT", &url, &alice, org, Some(json!({"title": "Engines 2", "properties": {"lang": "fr"}, "expected_version": art["version"]}))).await;
        let (st, v3) = call(&server, "PUT", &url, &alice, org, Some(json!({"title": "Engines 3", "properties": {"lang": "en"}, "expected_version": v2["version"]}))).await;
        assert_eq!(st, 200, "{v3}");
        let p2 = feed(&server, &alice, org, &c1).await;
        assert_eq!(summary(&p2), vec![s("update", "article", "Engines 3")], "{p2}");
        assert_eq!(p2["changes"][0]["object"]["properties"]["lang"], "en");
        let c2 = p2["next"].as_str().unwrap().to_string();

        // A transaction still running holds back what committed after it
        // began, so the late one is never skipped.
        let late = db.begin().await.unwrap();
        late.execute_unprepared(&format!(
            "INSERT INTO objects (id, org_id, type_key, type_version, title, native_id, properties, version) \
             SELECT u, {org}, 'author', 1, 'Late', u::text, '{{}}'::jsonb, 1 FROM gen_random_uuid() u"
        ))
        .await
        .unwrap();
        let (_, _bea) = call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("author", "Bea"))).await;
        let held = feed(&server, &alice, org, &c2).await;
        assert_eq!(held["changes"], json!([]), "Bea waits for the older transaction: {held}");
        assert_eq!(held["next"], c2.as_str());
        late.commit().await.unwrap();
        let p3 = feed(&server, &alice, org, &c2).await;
        assert_eq!(summary(&p3), vec![s("create", "author", "Late"), s("create", "author", "Bea")], "{p3}");
        let c3 = p3["next"].as_str().unwrap().to_string();

        // A closed link carries its end.
        let (st, _) = call(&server, "POST", &format!("/api/v1/ontology/links/{}/close", link["id"]), &alice, org, Some(json!({}))).await;
        assert_eq!(st, 200);
        let p4 = feed(&server, &alice, org, &c3).await;
        assert_eq!(summary(&p4), vec![s("close", "written_by", "written_by")], "{p4}");
        assert!(p4["changes"][0]["link"]["valid_to"].is_string(), "{p4}");

        // Paging: `more` and a cursor per page.
        let (_, small) = call(&server, "GET", &format!("/api/v1/ontology/changes?since={c0}&limit=2&{SCOPE}"), &alice, org, None).await;
        assert_eq!((small["changes"].as_array().unwrap().len(), &small["more"]), (2, &json!(true)), "{small}");

        // Filters: another type is left out, and the cursor still moves past it.
        let (_, only) = call(&server, "GET", &format!("/api/v1/ontology/changes?since={c3}&types=article&link_types=none"), &alice, org, None).await;
        assert_eq!(only["changes"], json!([]), "{only}");

        // Compaction keeps the last change of each: cursor 0 is a full copy.
        pnex_backend::services::ontology::feed::compact(&db).await.unwrap();
        let full = feed(&server, &alice, org, "0").await;
        let mut got = summary(&full);
        got.sort();
        assert_eq!(
            got,
            vec![s("close", "written_by", "written_by"), s("create", "author", "Ada"), s("create", "author", "Bea"), s("create", "author", "Late"), s("update", "article", "Engines 3")],
            "{full}"
        );

        // A cursor behind a compacted close is expired; 0 never is.
        db.execute_unprepared(&format!(
            "INSERT INTO ontology_change_marks (org_id, txid, seq) \
             SELECT org_id, txid, seq FROM ontology_changes WHERE org_id = {org} ORDER BY txid DESC, seq DESC LIMIT 1"
        ))
        .await
        .unwrap();
        let (st, e) = call(&server, "GET", &format!("/api/v1/ontology/changes?since={c1}"), &alice, org, None).await;
        assert_eq!((st, e["error"].as_str()), (410, Some("changes-cursor-expired")), "{e}");
        assert_eq!(feed(&server, &alice, org, "0").await["changes"].as_array().unwrap().len(), 5);
        let (st, e) = call(&server, "GET", "/api/v1/ontology/changes?since=abc", &alice, org, None).await;
        assert_eq!(st, 400, "{e}");

        // A read-only service account follows the same feed.
        let (_, sa) = call(&server, "POST", "/api/v1/iam/service-accounts", &alice, org, Some(json!({"name": "mirror"}))).await;
        let sa_id = sa["id"].as_str().unwrap().to_string();
        let expires = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
        let (_, key) = call(&server, "POST", &format!("/api/v1/iam/service-accounts/{sa_id}/keys"), &alice, org, Some(json!({"expires_at": expires}))).await;
        let secret = key["secret"].as_str().unwrap().to_string();
        let (st, _) = call(&server, "GET", "/api/v1/ontology/changes", &secret, org, None).await;
        assert_eq!(st, 403, "nothing before its role");
        let (st, a) = call(&server, "POST", "/api/v1/iam/assignments", &alice, org, Some(json!({"service_account_id": sa_id, "managed_role": "viewer"}))).await;
        assert_eq!(st, 201, "{a}");
        assert_eq!(feed(&server, &secret, org, "0").await["changes"].as_array().unwrap().len(), 5);
    })
    .await;
}

/// Evidence of an object (D437) on an invented ontology: link types
/// declared `evidence` are followed two hops to media, others never.
#[tokio::test]
#[serial]
async fn evidence_follows_evidence_links_only() {
    with_app(|server, alice, db| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        for key in ["article", "read_page"] {
            call(&server, "POST", "/api/v1/ontology/types", &alice, org, Some(json!({"def": {"key": key, "name": key, "properties": []}}))).await;
        }
        for (key, from, to, evidence) in [
            ("seen_in", "article", "read_page", true),
            ("proves", "read_page", "media_asset", true),
            ("illustrates", "article", "media_asset", false),
        ] {
            let (st, lt) = call(&server, "POST", "/api/v1/ontology/link-types", &alice, org, Some(json!({
                "key": key, "name": key, "from_types": [from], "to_types": [to], "evidence": evidence
            }))).await;
            assert_eq!(st, 201, "{lt}");
        }
        let mut media = Vec::new();
        for name in ["capture", "illustration"] {
            let row = db.query_one_raw(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "INSERT INTO media_assets (id, org_id, kind, name) VALUES (gen_random_uuid(), $1, 'photo', $2) RETURNING object_id",
                [org.into(), name.into()],
            )).await.unwrap().unwrap();
            media.push(row.try_get::<uuid::Uuid>("", "object_id").unwrap().to_string());
        }
        let obj = |t: &str| json!({"type_key": t, "title": t, "properties": {}});
        let (_, art) = call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("article"))).await;
        let (_, page) = call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("read_page"))).await;
        for (lt, s, t) in [("seen_in", &art["id"], &page["id"]), ("proves", &page["id"], &json!(media[0])), ("illustrates", &art["id"], &json!(media[1]))] {
            let (st, l) = call(&server, "POST", "/api/v1/ontology/links", &alice, org, Some(json!({"link_type": lt, "source_id": s, "target_id": t}))).await;
            assert_eq!(st, 201, "{l}");
        }
        let (st, ev) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}/evidence", art["id"].as_str().unwrap()), &alice, org, None).await;
        assert_eq!(st, 200, "{ev}");
        assert_eq!(ev["count"], 1, "the illustration is no evidence: {ev}");
        assert_eq!(ev["results"][0]["media"]["id"], media[0].as_str());
        assert_eq!(ev["results"][0]["via"], json!(["seen_in", "proves"]));
        assert!(ev["results"][0]["proof"].is_null(), "no web proof behind it");
    })
    .await;
}
