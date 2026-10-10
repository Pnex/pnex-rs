//! Ontology core (ontology.md D176–D191): pack install, typed objects and
//! their validation, optimistic concurrency, temporal links and `as_of`
//! queries, sensor replacement on a series (D181), system identities and
//! their extension, graph reads, YAML round trip, org isolation (R1) and
//! roles (R2, D188).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{json, Value};
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
    db: DatabaseConnection,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let bob = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000b",
        "bob",
        "bob@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(
                server,
                Env {
                    alice,
                    bob,
                    db: ctx.db.clone(),
                },
            )
            .await;
        },
    )
    .await;
}

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
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
        "PUT" => server.put(path),
        "PATCH" => server.patch(path),
        "DELETE" => server.delete(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {token}"))
    .add_header("X-Org-Id", org.to_string());
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = res.status_code().as_u16();
    let text = res.text();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// A device of the org, inserted directly (the identity comes from the
/// trigger); returns its object id.
async fn device(server: &axum_test::TestServer, env: &Env, org: i64, name: &str) -> String {
    env.db
        .execute_unprepared(&format!(
            "INSERT INTO device_registries (device_id, org_id, predefined_device_id) \
             SELECT '{name}', {org}, id FROM predefined_devices ORDER BY id LIMIT 1"
        ))
        .await
        .unwrap();
    let row = env
        .db
        .query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT id::text AS id FROM device_registries WHERE device_id = '{name}' AND org_id = {org}"),
        ))
        .await
        .unwrap()
        .unwrap();
    let native: String = row.try_get("", "id").unwrap();
    let (st, o) = call(
        server,
        "GET",
        &format!("/api/v1/ontology/resolve/device/{native}"),
        &env.alice,
        org,
        None,
    )
    .await;
    assert_eq!(st, 200, "{o}");
    assert_eq!(o["title"], name);
    o["id"].as_str().unwrap().to_string()
}

async fn object(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    type_key: &str,
    title: &str,
    props: Value,
) -> Value {
    let (st, o) = call(
        server,
        "POST",
        "/api/v1/ontology/objects",
        token,
        org,
        Some(json!({"type_key": type_key, "title": title, "properties": props})),
    )
    .await;
    assert_eq!(st, 201, "{o}");
    o
}

async fn link(server: &axum_test::TestServer, token: &str, org: i64, body: Value) -> (u16, Value) {
    call(
        server,
        "POST",
        "/api/v1/ontology/links",
        token,
        org,
        Some(body),
    )
    .await
}

#[tokio::test]
#[serial]
async fn pack_objects_validation_and_versions() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(&server, "POST", "/api/v1/ontology/packs/maintenance/install", &env.alice, org, None).await;
        assert_eq!(st, 200, "{p}");
        // Re-install is an upgrade: idempotent, no conflict.
        let (st, _) = call(&server, "POST", "/api/v1/ontology/packs/maintenance/install", &env.alice, org, None).await;
        assert_eq!(st, 200);
        let (_, types) = call(&server, "GET", "/api/v1/ontology/types", &env.alice, org, None).await;
        let keys: Vec<&str> = types.as_array().unwrap().iter().map(|t| t["def"]["key"].as_str().unwrap()).collect();
        for k in ["device", "folder", "site", "line", "machine", "pump"] {
            assert!(keys.contains(&k), "{k} in {keys:?}");
        }

        // Validation tokens.
        let (st, e) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(json!({"type_key": "pump", "title": "P12", "properties": {}}))).await;
        assert_eq!((st, e["serial"].as_str()), (400, Some("required")));
        let (st, e) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(json!({"type_key": "pump", "title": "P12", "properties": {"serial": "S1", "status": "flying"}}))).await;
        assert_eq!((st, e["status"].as_str()), (400, Some("invalid")));
        let (st, _) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(json!({"type_key": "device", "title": "x"}))).await;
        assert_eq!(st, 409, "system objects are created by their own pages");

        let pump = object(&server, &env.alice, org, "pump", "P12", json!({"serial": "S-12", "rated_flow": 40, "status": "running"})).await;
        let id = pump["id"].as_str().unwrap();
        assert_eq!(pump["version"], 1);
        assert_eq!(pump["source_ref"].as_str().unwrap().split(':').next(), Some("manual"));

        // Optimistic concurrency.
        let upd = json!({"title": "P12 bis", "properties": {"serial": "S-12", "status": "maintenance"}, "expected_version": 1});
        let (st, v) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{id}"), &env.alice, org, Some(upd.clone())).await;
        assert_eq!((st, v["version"].as_i64()), (200, Some(2)), "{v}");
        let (st, e) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{id}"), &env.alice, org, Some(upd)).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-version-conflict")));

        // Property query (GIN path) and title search.
        let (st, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"type_key": "pump", "filters": [{"key": "status", "op": "eq", "value": "maintenance"}]}))).await;
        assert_eq!((st, r["count"].as_i64()), (200, Some(1)), "{r}");
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"filters": [{"key": "rated_flow", "op": "gt", "value": 50}]}))).await;
        assert_eq!(r["count"], 0);
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"text": "bis"}))).await;
        assert_eq!(r["count"], 1);

        // Type versions: a new version of an org type.
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/pump", &env.alice, org, None).await;
        let mut def = t["def"].clone();
        def["name"] = json!("Centrifugal pump");
        let (st, t2) = call(&server, "PUT", "/api/v1/ontology/types/pump", &env.alice, org, Some(json!({"def": def, "expected_version": t["version"]}))).await;
        assert_eq!(st, 200, "{t2}");
        assert_eq!(t2["version"].as_i64(), t["version"].as_i64().map(|v| v + 1));
        let (_, vs) = call(&server, "GET", "/api/v1/ontology/types/pump/versions", &env.alice, org, None).await;
        assert!(vs.as_array().unwrap().len() >= 2);

        // Type in use, archive, then delete.
        let (st, e) = call(&server, "DELETE", "/api/v1/ontology/types/pump", &env.alice, org, None).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-type-in-use")));
        let (st, _) = call(&server, "DELETE", &format!("/api/v1/ontology/objects/{id}"), &env.alice, org, None).await;
        assert_eq!(st, 204);
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"type_key": "pump"}))).await;
        assert_eq!(r["count"], 0, "archived objects leave the default view");
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"type_key": "pump", "include_archived": true}))).await;
        assert_eq!(r["count"], 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn temporal_links_containment_and_graph() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        call(&server, "POST", "/api/v1/ontology/packs/maintenance/install", &env.alice, org, None).await;
        let site = object(&server, &env.alice, org, "site", "Plant A", json!({})).await;
        let line = object(&server, &env.alice, org, "line", "L2", json!({"code": "L2"})).await;
        let p1 = object(&server, &env.alice, org, "pump", "P1", json!({"serial": "1"})).await;
        let p2 = object(&server, &env.alice, org, "pump", "P2", json!({"serial": "2"})).await;
        let id = |o: &Value| o["id"].as_str().unwrap().to_string();

        // Containment through the D42 layer, now type-aware.
        let (st, e) = call(&server, "PUT", &format!("/api/v1/resources/line/{}/containment", id(&line)), &env.alice, org, Some(json!({"parent": {"kind": "site", "id": id(&site)}}))).await;
        assert!(st == 200 || st == 204, "{st} {e}");
        let (st, _) = call(&server, "PUT", &format!("/api/v1/resources/pump/{}/containment", id(&p1)), &env.alice, org, Some(json!({"parent": {"kind": "line", "id": id(&line)}}))).await;
        assert!(st == 200 || st == 204);
        let (st, _) = call(&server, "PUT", &format!("/api/v1/resources/site/{}/containment", id(&site)), &env.alice, org, Some(json!({"parent": {"kind": "pump", "id": id(&p1)}}))).await;
        assert_eq!(st, 400, "a pump contains nothing");
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"within": id(&site), "type_key": "pump"}))).await;
        assert_eq!(r["count"], 1, "{r}");

        // Typed links.
        let (st, e) = link(&server, &env.alice, org, json!({"link_type": "feeds", "source_id": id(&site), "target_id": id(&p1)})).await;
        assert_eq!((st, e["error"].as_str()), (400, Some("ontology-link-not-allowed")));
        let (st, l1) = link(&server, &env.alice, org, json!({"link_type": "feeds", "source_id": id(&p1), "target_id": id(&p2), "valid_from": "2026-01-01T00:00:00Z"})).await;
        assert_eq!(st, 201, "{l1}");
        let (st, _) = link(&server, &env.alice, org, json!({"link_type": "feeds", "source_id": id(&p2), "target_id": id(&line)})).await;
        assert_eq!(st, 201);
        let (st, e) = link(&server, &env.alice, org, json!({"link_type": "feeds", "source_id": id(&p1), "target_id": id(&p2)})).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-already-exists")));
        // backup_of: one target per source.
        link(&server, &env.alice, org, json!({"link_type": "backup_of", "source_id": id(&p2), "target_id": id(&p1)})).await;
        let p3 = object(&server, &env.alice, org, "pump", "P3", json!({"serial": "3"})).await;
        let (st, e) = link(&server, &env.alice, org, json!({"link_type": "backup_of", "source_id": id(&p2), "target_id": id(&p3)})).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-link-cardinality")));

        // Traversal: P1 →feeds→ P2 →feeds→ L2.
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"ids": [id(&p1)], "traverse": [{"link_type": "feeds"}, {"link_type": "feeds"}]}))).await;
        assert_eq!(r["rows"][0]["object"]["title"], "L2", "{r}");
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"ids": [id(&line)], "traverse": [{"link_type": "feeds", "direction": "in"}]}))).await;
        assert_eq!(r["rows"][0]["object"]["title"], "P2");
        let (st, _) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"traverse": [{"link_type": "feeds"}, {"link_type": "feeds"}, {"link_type": "feeds"}, {"link_type": "feeds"}, {"link_type": "feeds"}]}))).await;
        assert_eq!(st, 400, "bounded to 4 hops");

        // Close P1→P2: the link stays as history, `as_of` still sees it.
        let lid = l1["id"].as_i64().unwrap();
        let (st, c) = call(&server, "POST", &format!("/api/v1/ontology/links/{lid}/close"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{c}");
        assert!(c["valid_to"].is_string());
        let (_, now) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}/links", id(&p1)), &env.alice, org, None).await;
        assert!(now.as_array().unwrap().iter().all(|l| l["link_type"] != "feeds"), "{now}");
        let (_, then) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}/links?as_of=2026-06-01T00:00:00Z", id(&p1)), &env.alice, org, None).await;
        assert!(then.as_array().unwrap().iter().any(|l| l["link_type"] == "feeds"), "{then}");
        let (_, hist) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}/links?history=true", id(&p1)), &env.alice, org, None).await;
        assert!(hist.as_array().unwrap().iter().any(|l| l["id"] == lid));
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org, Some(json!({"ids": [id(&p1)], "traverse": [{"link_type": "feeds"}], "as_of": "2026-06-01T00:00:00Z"}))).await;
        assert_eq!(r["count"], 1, "as_of traversal sees the closed link: {r}");

        // Graph: neighbourhood, impact, path.
        let (st, g) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}/graph?depth=2", id(&p2)), &env.alice, org, None).await;
        assert_eq!(st, 200, "{g}");
        assert!(g["nodes"].as_array().unwrap().len() >= 3);
        let (_, imp) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}/impact", id(&p2)), &env.alice, org, None).await;
        let titles: Vec<&str> = imp.as_array().unwrap().iter().map(|o| o["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"L2"), "{titles:?}");
        let (st, path) = call(&server, "GET", &format!("/api/v1/ontology/path?from={}&to={}", id(&p3), id(&line)), &env.alice, org, None).await;
        assert_eq!(st, 404, "P3 is isolated: {path}");
        let (st, path) = call(&server, "GET", &format!("/api/v1/ontology/path?from={}&to={}", id(&p1), id(&line)), &env.alice, org, None).await;
        assert_eq!(st, 200, "{path}");
        assert_eq!(path.as_array().unwrap().last().unwrap()["title"], "L2");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn sensor_replacement_keeps_the_series_and_system_identities() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        call(&server, "POST", "/api/v1/ontology/packs/maintenance/install", &env.alice, org, None).await;
        let pump = object(&server, &env.alice, org, "pump", "P12", json!({"serial": "12"})).await;
        let pid = pump["id"].as_str().unwrap();
        let c47 = device(&server, &env, org, "c47").await;
        let c88 = device(&server, &env, org, "c88").await;

        let measures = |dev: &str, prop: &str, from: &str| json!({"link_type": "measures", "source_id": dev, "target_id": pid, "attributes": {"metric": "temperature", "property": prop}, "valid_from": from});
        let (st, e) = link(&server, &env.alice, org, measures(&c47, "rated_flow", "2026-03-01T00:00:00Z")).await;
        assert_eq!((st, e["attributes.property"].as_str()), (400, Some("invalid")), "not a series");
        let (st, l) = link(&server, &env.alice, org, measures(&c47, "temperature", "2026-03-01T00:00:00Z")).await;
        assert_eq!(st, 201, "{l}");
        let (st, e) = link(&server, &env.alice, org, measures(&c88, "temperature", "2026-06-12T00:00:00Z")).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-link-cardinality")), "one feeder at a time");
        let (st, _) = call(&server, "POST", &format!("/api/v1/ontology/links/{}/close", l["id"]), &env.alice, org, Some(json!({"valid_to": "2026-06-12T00:00:00Z"}))).await;
        assert_eq!(st, 200);
        let (st, _) = link(&server, &env.alice, org, measures(&c88, "temperature", "2026-06-12T00:00:00Z")).await;
        assert_eq!(st, 201);
        // Without O2 the points are empty but the bindings are listed.
        let (st, s) = call(&server, "GET", &format!("/api/v1/ontology/objects/{pid}/series/temperature?window=30d"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{s}");
        assert_eq!(s["unit"], "°C");
        let (st, s) = call(&server, "GET", &format!("/api/v1/ontology/objects/{pid}/series/temperature?window=bogus"), &env.alice, org, None).await;
        assert_eq!(st, 400, "{s}");

        // A system type gains org properties; its objects carry them.
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/device", &env.alice, org, None).await;
        let mut def = t["def"].clone();
        def["properties"] = json!([{"key": "asset_tag", "name": "Asset tag", "kind": "text", "max_len": 32}]);
        let (st, t2) = call(&server, "PUT", "/api/v1/ontology/types/device", &env.alice, org, Some(json!({"def": def, "expected_version": 0}))).await;
        assert_eq!(st, 200, "{t2}");
        assert!(t2["def"]["system"].as_bool().unwrap());
        let (st, d) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{c47}"), &env.alice, org, Some(json!({"title": "ignored", "properties": {"asset_tag": "AT-47"}, "expected_version": 1}))).await;
        assert_eq!(st, 200, "{d}");
        assert_eq!((d["title"].as_str(), d["properties"]["asset_tag"].as_str()), (Some("c47"), Some("AT-47")));
        let (st, _) = call(&server, "DELETE", &format!("/api/v1/ontology/objects/{c47}"), &env.alice, org, None).await;
        assert_eq!(st, 409, "system objects are deleted from their own page");

        // Deleting the native row closes the identity; history survives.
        env.db.execute_unprepared(&format!("DELETE FROM device_registries WHERE device_id = 'c47' AND org_id = {org}")).await.unwrap();
        let (_, o) = call(&server, "GET", &format!("/api/v1/ontology/objects/{c47}"), &env.alice, org, None).await;
        assert!(o["valid_to"].is_string(), "{o}");
        let (_, hist) = call(&server, "GET", &format!("/api/v1/ontology/objects/{pid}/links?history=true"), &env.alice, org, None).await;
        assert_eq!(hist.as_array().unwrap().iter().filter(|l| l["link_type"] == "measures").count(), 2);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_roles_and_yaml_round_trip() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        call(&server, "POST", "/api/v1/ontology/packs/maintenance/install", &env.alice, org_a, None).await;
        let p = object(&server, &env.alice, org_a, "pump", "P1", json!({"serial": "1"})).await;
        let pid = p["id"].as_str().unwrap();

        // R1: another org sees nothing.
        let (st, _) = call(&server, "GET", &format!("/api/v1/ontology/objects/{pid}"), &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "POST", "/api/v1/ontology/objects", &env.bob, org_b, Some(json!({"type_key": "pump", "title": "x", "properties": {"serial": "1"}}))).await;
        assert_eq!(st, 400, "the type does not exist in org B");
        let (_, r) = call(&server, "POST", "/api/v1/ontology/query", &env.bob, org_b, Some(json!({"ids": [pid]}))).await;
        assert_eq!(r["count"], 0);

        // Export from A, import into B.
        let res = server
            .get("/api/v1/ontology/export")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org_a.to_string())
            .await;
        assert_eq!(res.status_code().as_u16(), 200);
        let yaml = res.text();
        assert!(yaml.contains("key: pump"), "{yaml}");
        let res = server
            .post("/api/v1/ontology/import")
            .add_header("Authorization", format!("Bearer {}", env.bob))
            .add_header("X-Org-Id", org_b.to_string())
            .bytes(yaml.into_bytes().into())
            .await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        object(&server, &env.bob, org_b, "pump", "B-P1", json!({"serial": "1"})).await;

        // R2: a viewer reads but never writes; a member writes objects but
        // not the schema (D188: org admin for types).
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org_a}/members"), &env.alice, org_a, Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st == 200 || st == 201);
        let (st, _) = call(&server, "GET", &format!("/api/v1/ontology/objects/{pid}"), &env.bob, org_a, None).await;
        assert_eq!(st, 200);
        let (st, e) = call(&server, "POST", "/api/v1/ontology/objects", &env.bob, org_a, Some(json!({"type_key": "pump", "title": "x", "properties": {"serial": "1"}}))).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("ontology-write-forbidden")));
        let (st, _) = call(&server, "POST", "/api/v1/ontology/links", &env.bob, org_a, Some(json!({"link_type": "feeds", "source_id": pid, "target_id": pid}))).await;
        assert_eq!(st, 403);
        let (st, e) = call(&server, "POST", "/api/v1/ontology/packs/maintenance/install", &env.bob, org_a, None).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("ontology-admin-required")));

        // A type locked to admins refuses a member.
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/site", &env.alice, org_a, None).await;
        let mut def = t["def"].clone();
        def["write_role"] = json!("admin");
        call(&server, "PUT", "/api/v1/ontology/types/site", &env.alice, org_a, Some(json!({"def": def, "expected_version": t["version"]}))).await;
        env.db
            .execute_unprepared(&format!(
                "UPDATE organization_members SET role = 'member' WHERE org_id = {org_a} AND role = 'viewer'"
            ))
            .await
            .unwrap();
        let (st, e) = call(&server, "POST", "/api/v1/ontology/objects", &env.bob, org_a, Some(json!({"type_key": "site", "title": "S"}))).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("ontology-write-forbidden")), "{e}");
        let (st, _) = call(&server, "POST", "/api/v1/ontology/objects", &env.bob, org_a, Some(json!({"type_key": "pump", "title": "P9", "properties": {"serial": "9"}}))).await;
        assert_eq!(st, 201);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn ranges_search_and_type_dashboards_reach_objects() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        call(&server, "POST", "/api/v1/ontology/packs/maintenance/install", &env.alice, org, None).await;
        let line = object(&server, &env.alice, org, "line", "Ligne Nord", json!({"code": "N"})).await;
        let lid = line["id"].as_str().unwrap();

        // D182: a shift of a line.
        let (st, r) = call(&server, "POST", "/api/v1/time-ranges", &env.alice, org, Some(json!({
            "scope_kind": "object", "scope_id": lid, "label": "Morning shift",
            "planned_start": "2026-10-01T06:00:00Z", "planned_end": "2026-10-01T14:00:00Z"
        }))).await;
        assert_eq!(st, 201, "{r}");
        let (st, _) = call(&server, "POST", "/api/v1/time-ranges", &env.alice, org, Some(json!({
            "scope_kind": "object", "scope_id": "00000000-0000-0000-0000-000000000000", "label": "x",
            "planned_start": "2026-10-01T06:00:00Z", "planned_end": "2026-10-01T14:00:00Z"
        }))).await;
        assert_eq!(st, 400, "unknown object scope");

        // D69: global search finds org-type objects.
        let (st, s) = call(&server, "GET", "/api/v1/search?q=nord", &env.alice, org, None).await;
        assert_eq!(st, 200);
        let group = s["groups"].as_array().unwrap().iter().find(|g| g["entity_type"] == "object").cloned();
        assert_eq!(group.unwrap()["results"][0]["id"], lid, "{s}");

        // D187: a type dashboard reads an object property; the binding
        // follows the open `measures` link.
        let pump = object(&server, &env.alice, org, "pump", "P1", json!({"serial": "1"})).await;
        let pid = pump["id"].as_str().unwrap();
        let dev = device(&server, &env, org, "c1").await;
        link(&server, &env.alice, org, json!({"link_type": "measures", "source_id": dev, "target_id": pid,
            "attributes": {"metric": "temperature", "property": "temperature"}})).await;
        let (st, b) = call(&server, "GET", &format!("/api/v1/ontology/objects/{pid}/bindings"), &env.alice, org, None).await;
        assert_eq!(st, 200);
        assert_eq!(b, json!([{"property": "temperature", "device_id": "c1", "metric": "temperature"}]));
        let layout = |object_type: Value| json!({
            "canvas": {"width": 800, "height": 600}, "object_type": object_type,
            "widgets": [{"id": "w1", "type": "gauge", "x": 0, "y": 0, "w": 100, "h": 100,
                "source": [{"metric": "", "device_id": "", "window": "1h", "object_property": "temperature"}]}]
        });
        let (st, d) = call(&server, "POST", "/api/v1/dashboards", &env.alice, org, Some(json!({"name": "Pump"}))).await;
        assert!(st == 200 || st == 201, "{st} {d}");
        let path = format!("/api/v1/dashboards/{}", d["id"].as_str().unwrap());
        let (st, e) = call(&server, "PATCH", &path, &env.alice, org, Some(json!({"expected_version_number": 1, "layout": layout(Value::Null)}))).await;
        assert_eq!(st, 400, "object source without object type: {e}");
        let (st, d) = call(&server, "PATCH", &path, &env.alice, org, Some(json!({"expected_version_number": 1, "layout": layout(json!("pump"))}))).await;
        assert_eq!(st, 200, "{d}");
        assert_eq!(d["layout"]["object_type"], "pump");
    })
    .await;
}
