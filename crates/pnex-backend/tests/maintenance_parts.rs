//! Parts and documentation, lot PD0 (parts-docs.md D960–D961): a parts list
//! imported from CSV gives models, parts and their references stored
//! normalized; a reference typed any way finds its part by prefix, and a
//! supersession leads from the old part to the new one.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, sea_orm::DatabaseConnection) -> Fut,
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

struct Api<'a> {
    server: &'a axum_test::TestServer,
    token: String,
    org: i64,
}

impl Api<'_> {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let req = match method {
            "GET" => self.server.get(path),
            "POST" => self.server.post(path),
            _ => unreachable!(),
        }
        .add_header("Authorization", format!("Bearer {}", self.token))
        .add_header("X-Org-Id", self.org.to_string());
        let res = match body {
            Some(b) => req.json(&b).await,
            None => req.await,
        };
        let st = res.status_code().as_u16();
        let text = res.text();
        (
            st,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    /// Runs the mapping of the pack import `name` on `csv` and applies it.
    async fn import(&self, name: &str, csv: &str) -> Value {
        let (st, import) = self
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(common::pack_import_body(name)),
            )
            .await;
        assert_eq!(st, 201, "{import}");
        let id = import["id"].as_str().unwrap().to_string();
        let res = self
            .server
            .post(&format!(
                "/api/v1/ontology/imports/{id}/dry-run?filename=list.csv"
            ))
            .add_header("Authorization", format!("Bearer {}", self.token))
            .add_header("X-Org-Id", self.org.to_string())
            .add_header("Content-Type", "application/octet-stream")
            .bytes(csv.as_bytes().to_vec().into())
            .await;
        assert_eq!(res.status_code().as_u16(), 202, "{}", res.text());
        let run = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let settled = |run: String| {
            let id = id.clone();
            async move {
                for _ in 0..200 {
                    let (_, r) = self
                        .call(
                            "GET",
                            &format!("/api/v1/ontology/imports/{id}/runs/{run}"),
                            None,
                        )
                        .await;
                    if r["run"]["state"] != "running" {
                        return r;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                panic!("run never settled");
            }
        };
        let p = settled(run.clone()).await;
        assert_eq!(p["run"]["state"], "preview", "{p}");
        let (st, r) = self
            .call(
                "POST",
                &format!("/api/v1/ontology/imports/{id}/apply"),
                Some(json!({ "run_id": run, "expected_version": import["version"] })),
            )
            .await;
        assert_eq!(st, 202, "{r}");
        let done = settled(run).await;
        assert_eq!(done["run"]["state"], "applied", "{done}");
        done
    }

    async fn query(&self, q: Value) -> Value {
        let (st, r) = self.call("POST", "/api/v1/ontology/query", Some(q)).await;
        assert_eq!(st, 200, "{r}");
        r
    }
}

fn titles(r: &Value) -> Vec<String> {
    let mut t: Vec<String> = r["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["object"]["title"].as_str().unwrap().to_string())
        .collect();
    t.sort();
    t
}

#[tokio::test]
#[serial]
async fn a_parts_list_is_found_by_any_spelling_of_a_reference() {
    with_app(|server, token, _db| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token, org };
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/maintenance_partners/install", None).await;
        assert_eq!(st, 200, "{r}");

        let done = api
            .import(
                "Parts lists (CSV)",
                "model,model_name,position,part_number,part_name,reference,quantity,serial_from,serial_to,date_from,date_to,variant\n\
                 CF-200,Cold room unit,12,P-100,Fan motor,ab-12.3/x,1,,,,,\n\
                 CF-200,Cold room unit,13,P-200,Gasket,,2,,,,,230V\n\
                 CF-200,Cold room unit,14,P-300,Fan motor (new),AB 12.4,1,S5000,,2024-01-01,,\n",
            )
            .await;
        assert_eq!(done["run"]["counts"]["errors"], 0, "{done}");

        // The reference is stored normalized, its title as typed.
        let refs = api
            .query(json!({ "type_key": "part_reference", "filters": [{ "key": "reference", "op": "eq", "value": "AB123X" }] }))
            .await;
        assert_eq!(titles(&refs), ["ab-12.3/x"], "{refs}");

        // Any spelling, by prefix, finds both parts through their references.
        let typed = pnex_core::ontology::schema::Normalize::Reference.apply("ab 12");
        let found = api
            .query(json!({
                "type_key": "part_reference",
                "filters": [{ "key": "reference", "op": "starts_with", "value": typed }],
                "traverse": [{ "link_type": "refers_to", "direction": "out" }]
            }))
            .await;
        assert_eq!(titles(&found), ["Fan motor (new) · P-300", "Fan motor · P-100"], "{found}");

        // The parts list line keeps its bounds.
        let model = api.query(json!({ "type_key": "equipment_model" })).await;
        let model_id = model["rows"][0]["object"]["id"].as_str().unwrap().to_string();
        let (_, links) = api.call("GET", &format!("/api/v1/ontology/objects/{model_id}/links"), None).await;
        assert_eq!(links["count"], 3, "{links}");
        let p300 = links["results"].as_array().unwrap().iter().find(|l| l["attributes"]["position"] == "14").unwrap();
        assert_eq!(p300["attributes"]["serial_from"], "S5000");
        assert_eq!(p300["attributes"]["date_from"], "2024-01-01");

        // P-300 replaces P-100: from the old part, the newest one.
        api.import("Part supersessions (CSV)", "old_part_number,new_part_number,since\nP-100,P-300,2024-01-01\n").await;
        let newer = api
            .query(json!({
                "type_key": "spare_part",
                "filters": [{ "key": "part_number", "op": "eq", "value": "P-100" }],
                "traverse": [{ "link_type": "supersedes", "direction": "in" }]
            }))
            .await;
        assert_eq!(titles(&newer), ["Fan motor (new) · P-300"], "{newer}");

        // A reference made of separators only is refused.
        let (st, e) = api
            .call("POST", "/api/v1/ontology/objects", Some(json!({ "type_key": "part_reference", "title": "--", "properties": { "reference": "--" } })))
            .await;
        assert_eq!(st, 400, "{e}");
    })
    .await;
}

/// PD0 acceptance (parts-docs.md §4): with 100,000 parts, an old reference
/// typed with separators leads to the current part in less than 300 ms.
/// Run by hand: `cargo test --test maintenance_parts -- --ignored`.
#[tokio::test]
#[serial]
#[ignore]
async fn an_old_reference_finds_the_current_part_among_100000() {
    use sea_orm::ConnectionTrait;
    with_app(|server, token, db| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {token}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token, org };
        let (st, r) = api.call("POST", "/api/v1/ontology/packs/maintenance/install", None).await;
        assert_eq!(st, 200, "{r}");
        let n = 100_000;
        let t = std::time::Instant::now();
        db.execute_unprepared(&format!(
            // Triggers off for the load only (change feed, versions): the
            // reads measured below are the ones a member does.
            "SET session_replication_role = replica; \
             CREATE TEMP TABLE bulk AS SELECT g, gen_random_uuid() AS pid, gen_random_uuid() AS rid \
               FROM generate_series(1, {n}) g; \
             INSERT INTO objects (id, org_id, type_key, title, native_id, properties) \
             SELECT pid, {org}, 'spare_part', 'Part ' || g, pid::text, jsonb_build_object('part_number', 'PN-' || g) FROM bulk; \
             INSERT INTO objects (id, org_id, type_key, title, native_id, properties) \
             SELECT rid, {org}, 'part_reference', 'ref-' || g, rid::text, jsonb_build_object('reference', 'REF' || g, 'nature', 'origin') FROM bulk; \
             INSERT INTO resource_edges (org_id, relation, source_kind, source_id, target_kind, target_id, source_object_id, target_object_id) \
             SELECT {org}, 'refers_to', 'part_reference', rid::text, 'spare_part', pid::text, rid, pid FROM bulk; \
             INSERT INTO resource_edges (org_id, relation, source_kind, source_id, target_kind, target_id, source_object_id, target_object_id) \
             SELECT {org}, 'supersedes', 'spare_part', n.pid::text, 'spare_part', o.pid::text, n.pid, o.pid \
             FROM bulk n, bulk o WHERE n.g = {n} AND o.g = 4242; \
             DROP TABLE bulk; SET session_replication_role = DEFAULT; ANALYZE objects; ANALYZE resource_edges;"
        ))
        .await
        .unwrap();
        println!("loaded {n} parts and references in {:?}", t.elapsed());
        // Typed with separators and in lower case, by prefix: the part, then
        // what supersedes it; each read measured, best of three.
        let typed = pnex_core::ontology::schema::Normalize::Reference.apply("ref-4242");
        let mut best = std::time::Duration::MAX;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let part = api
                .query(json!({
                    "type_key": "part_reference",
                    "filters": [{ "key": "reference", "op": "starts_with", "value": typed }],
                    "traverse": [{ "link_type": "refers_to", "direction": "out" }],
                    "limit": 20
                }))
                .await;
            let id = part["rows"].as_array().unwrap().iter().find(|r| r["object"]["title"] == "Part 4242").unwrap()["object"]["id"].clone();
            let newer = api
                .query(json!({
                    "type_key": "spare_part",
                    "ids": [id],
                    "traverse": [{ "link_type": "supersedes", "direction": "in" }]
                }))
                .await;
            assert_eq!(titles(&newer), [format!("Part {n}")], "{newer}");
            best = best.min(t.elapsed());
        }
        println!("old reference -> current part: {best:?}");
        assert!(best < std::time::Duration::from_millis(300), "{best:?}");
    })
    .await;
}
