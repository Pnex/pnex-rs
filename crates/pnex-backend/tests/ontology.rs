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

/// D191: a device placement (D43) is mirrored as a temporal `placed_at`
/// link; moving the device closes it and opens another.
#[tokio::test]
#[serial]
async fn device_placements_become_placed_at_links() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let dev = device(&server, &env, org, "p1").await;
        env.db
            .execute_unprepared(&format!(
                "INSERT INTO map_pins (id, org_id, mode, label, x, y) VALUES
                   ('00000000-0000-0000-0000-0000000000a1', {org}, 'plan', 'Hall A', 1, 1),
                   ('00000000-0000-0000-0000-0000000000a2', {org}, 'plan', 'Hall B', 2, 2);
                 INSERT INTO device_placements (org_id, device_registry_id, device_id, pin_id)
                   SELECT {org}, id, device_id, '00000000-0000-0000-0000-0000000000a1'
                   FROM device_registries WHERE device_id = 'p1' AND org_id = {org};"
            ))
            .await
            .unwrap();
        let (_, now) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{dev}/links"),
            &env.alice,
            org,
            None,
        )
        .await;
        let placed: Vec<&Value> = now
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["link_type"] == "placed_at")
            .collect();
        assert_eq!(placed.len(), 1, "{now}");
        assert_eq!(placed[0]["target"]["title"], "Hall A");

        env.db
            .execute_unprepared(
                "UPDATE device_placements SET pin_id = '00000000-0000-0000-0000-0000000000a2'",
            )
            .await
            .unwrap();
        let (_, now) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{dev}/links"),
            &env.alice,
            org,
            None,
        )
        .await;
        let titles: Vec<&str> = now
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| l["link_type"] == "placed_at")
            .map(|l| l["target"]["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["Hall B"]);
        let (_, hist) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{dev}/links?history=true"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(
            hist.as_array()
                .unwrap()
                .iter()
                .filter(|l| l["link_type"] == "placed_at")
                .count(),
            2
        );
        // The D42 POI links still list only open placements.
        env.db
            .execute_unprepared("DELETE FROM device_placements")
            .await
            .unwrap();
        let (_, now) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{dev}/links"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert!(
            now.as_array()
                .unwrap()
                .iter()
                .all(|l| l["link_type"] != "placed_at"),
            "{now}"
        );
    })
    .await;
}

/// Case tracks (cases-workspace.md K27): the window is bounded and
/// validated; another org gets 404 (R1); without a readable position
/// history the answer says so instead of failing.
#[tokio::test]
#[serial]
async fn case_tracks_window_isolation_and_unavailable_history() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let case = object(&server, &env.alice, org, "case", "Line 3", json!({"status": "open"})).await;
        let cid = case["id"].as_str().unwrap();
        let d1 = device(&server, &env, org, "t-07").await;
        let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/events"), &env.alice, org, Some(json!({
            "title": "Truck enters the secure area", "occurred": instant("2026-10-10T14:03:00Z"), "attach": [{"object_id": d1}]
        }))).await;
        assert_eq!(st, 201, "{w}");

        let path = format!("/api/v1/cases/{cid}/tracks");
        let (st, e) = call(&server, "POST", &path, &env.alice, org, Some(json!({"from": "2026-10-10T14:00:00Z", "to": "2026-10-10T13:00:00Z"}))).await;
        assert_eq!((st, e["to"].as_str()), (400, Some("invalid")), "{e}");
        let (st, e) = call(&server, "POST", &path, &env.alice, org, Some(json!({"from": "2026-01-01T00:00:00Z", "to": "2026-03-01T00:00:00Z"}))).await;
        assert_eq!((st, e["to"].as_str()), (400, Some("invalid")), "{e}");
        let window = json!({"from": "2026-10-10T13:30:00Z", "to": "2026-10-10T16:00:00Z", "resolution_ms": 10000});
        let (st, t) = call(&server, "POST", &path, &env.alice, org, Some(window.clone())).await;
        assert_eq!(st, 200, "{t}");
        assert_eq!(t["available"], false);
        assert_eq!(t["tracks"], json!([]));

        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "POST", &path, &env.bob, org_b, Some(window)).await;
        assert_eq!(st, 404);
    })
    .await;
}

/// Case detections (cases-workspace.md K30): without a readable position
/// history nothing is proposed; a proposal is retained once (same event on a
/// second retain, members attached as actors, provenance of the detector),
/// a reviewed proposal is never reviewed the other way, a viewer only reads
/// (R2) and another org sees nothing (R1).
#[tokio::test]
#[serial]
async fn case_detections_are_proposed_and_reviewed_by_a_human() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let case = object(&server, &env.alice, org, "case", "Dock 2", json!({"status": "open"})).await;
        let cid = case["id"].as_str().unwrap();
        let (d1, d2) = (device(&server, &env, org, "t-07").await, device(&server, &env, org, "b-14").await);
        for d in [&d1, &d2] {
            let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/attach"), &env.alice, org, Some(json!({"object_id": d}))).await;
            assert_eq!(st, 200, "{w}");
        }
        let window = json!({"from": "2026-10-10T13:00:00Z", "to": "2026-10-10T16:00:00Z"});
        let (st, d) = call(&server, "POST", &format!("/api/v1/cases/{cid}/detect"), &env.alice, org, Some(window.clone())).await;
        assert_eq!(st, 200, "{d}");
        assert_eq!((d["available"].as_bool(), d["inserted"].as_u64()), (Some(false), Some(0)), "{d}");

        // Two proposals as the detectors would write them.
        let mut pids = Vec::new();
        for (kind, refs) in [("co_presence", format!("'{d1}', '{d2}'")), ("stop", format!("'{d1}'"))] {
            let pid = uuid::Uuid::new_v4();
            env.db
                .execute_unprepared(&format!(
                    "INSERT INTO case_proposals (id, org_id, case_id, kind, detector, params, fingerprint, t_start, t_end, refs, payload) \
                     VALUES ('{pid}', {org}, '{cid}', '{kind}', '{kind}@1', '{{\"min_s\": 120}}', '{kind}-fp', \
                             '2026-10-10T14:32:00Z', '2026-10-10T14:40:00Z', ARRAY[{refs}]::uuid[], '{{\"at\": [2.35, 48.85]}}')"
                ))
                .await
                .unwrap();
            pids.push(pid.to_string());
        }
        let (st, list) = call(&server, "GET", &format!("/api/v1/cases/{cid}/proposals"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{list}");
        let row = |list: &Value, pid: &str| list.as_array().unwrap().iter().find(|p| p["id"] == pid).cloned().unwrap();
        assert_eq!(list.as_array().unwrap().len(), 2);
        assert_eq!(row(&list, &pids[0])["refs"].as_array().unwrap().len(), 2, "{list}");

        let retain = format!("/api/v1/cases/{cid}/proposals/{}/retain", pids[0]);
        let (st, w) = call(&server, "POST", &retain, &env.alice, org, Some(json!({"title": "Truck and badge together"}))).await;
        assert_eq!(st, 200, "{w}");
        assert_eq!(w["events"].as_array().unwrap().len(), 1, "{w}");
        let ev = &w["events"][0];
        assert_eq!(ev["event"]["properties"]["origin"], "detection");
        assert_eq!(ev["event"]["properties"]["location"], json!({"lon": 2.35, "lat": 48.85}));
        assert_eq!(ev["event"]["properties"]["occurred"]["precision"], "range");
        assert_eq!(ev["event"]["source_ref"], "detector:co_presence@1");
        assert_eq!(
            attached(&w, "Truck and badge together"),
            vec![("t-07".to_string(), "actor".to_string()), ("b-14".to_string(), "actor".to_string())]
        );
        let (st, again) = call(&server, "POST", &retain, &env.alice, org, Some(json!({"title": "Other title"}))).await;
        assert_eq!(st, 200);
        assert_eq!(again["events"].as_array().unwrap().len(), 1, "retaining twice gives the same event");
        let (st, e) = call(&server, "POST", &format!("/api/v1/cases/{cid}/proposals/{}/reject", pids[0]), &env.alice, org, Some(json!({}))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("case-proposal-stale")), "{e}");

        let reject = format!("/api/v1/cases/{cid}/proposals/{}/reject", pids[1]);
        let (st, list) = call(&server, "POST", &reject, &env.alice, org, Some(json!({"reason": "  parked at the depot  "}))).await;
        assert_eq!(st, 200, "{list}");
        assert_eq!(row(&list, &pids[1])["status"], "rejected");
        assert_eq!(row(&list, &pids[1])["reject_reason"], "parked at the depot");
        let (st, e) = call(&server, "POST", &format!("/api/v1/cases/{cid}/proposals/{}/retain", pids[1]), &env.alice, org, Some(json!({"title": "x"}))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("case-proposal-stale")), "{e}");

        // R1: another org sees nothing; R2: a viewer reads, never reviews.
        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "GET", &format!("/api/v1/cases/{cid}/proposals"), &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org, Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st < 300);
        let (st, _) = call(&server, "GET", &format!("/api/v1/cases/{cid}/proposals"), &env.bob, org, None).await;
        assert_eq!(st, 200);
        for path in [format!("/api/v1/cases/{cid}/detect"), reject] {
            let (st, _) = call(&server, "POST", &path, &env.bob, org, Some(json!({}))).await;
            assert_eq!(st, 403, "{path}");
        }
    })
    .await;
}

/// Imported tracks (cases-workspace.md K26, K29, K30): a GPX previewed then
/// imported onto a member serves its track with no telemetry store, the same
/// file twice is one import, a CSV without offset needs a time zone, the
/// detectors see the import (a zone visit and a stop, nothing new on a
/// second run), a viewer cannot import (R2), another org sees nothing (R1).
#[tokio::test]
#[serial]
async fn imported_tracks_feed_the_tracks_and_the_detectors() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, _) = call(&server, "POST", "/api/v1/ontology/packs/investigation/install", &env.alice, org, Some(json!({}))).await;
        assert!(st < 300);
        let case = object(&server, &env.alice, org, "case", "Depot", json!({"status": "open"})).await;
        let cid = case["id"].as_str().unwrap();
        let d1 = device(&server, &env, org, "t-07").await;
        let gate = object(&server, &env.alice, org, "place", "Gate", json!({"zone": {"center": {"lon": 2.0, "lat": 48.003}, "radius_m": 30.0}})).await;
        for o in [d1.as_str(), gate["id"].as_str().unwrap()] {
            let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/attach"), &env.alice, org, Some(json!({"object_id": o}))).await;
            assert_eq!(st, 200, "{w}");
        }
        let (_, w) = call(&server, "GET", &format!("/api/v1/cases/{cid}/workspace"), &env.alice, org, None).await;
        assert_eq!(w["zones"][0]["object"]["title"], "Gate", "{w}");
        // Located members: the zone's centre stands for the place.
        assert_eq!(w["places"][0]["object"]["title"], "Gate", "{w}");
        assert!((w["places"][0]["lat"].as_f64().unwrap() - 48.003).abs() < 1e-9, "{w}");

        // Walks north for 5 min (through the gate), then waits 20 min there.
        let mut pts = String::new();
        let t0 = chrono::DateTime::parse_from_rfc3339("2026-03-01T10:00:00Z").unwrap();
        for s in 0..=1_500i64 {
            let lat = 48.0 + (s.min(300) as f64) * 0.00001;
            let t = (t0 + chrono::Duration::seconds(s)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            pts.push_str(&format!("<trkpt lat=\"{lat}\" lon=\"2.0\"><time>{t}</time></trkpt>"));
        }
        let gpx = format!("<?xml version=\"1.0\"?><gpx version=\"1.1\"><trk><trkseg>{pts}</trkseg></trk></gpx>");
        let path = format!("/api/v1/cases/{cid}/tracks/import");
        let body = |dry: bool| json!({"object_id": d1, "file_name": "t07.gpx", "format": "gpx", "content": gpx, "dry_run": dry});
        let (st, pv) = call(&server, "POST", &path, &env.alice, org, Some(body(true))).await;
        assert_eq!(st, 200, "{pv}");
        assert_eq!((pv["points"].as_u64(), pv["needs_time_zone"].as_bool()), (Some(1_501), Some(false)));
        assert!(pv["import"].is_null());
        let (st, done) = call(&server, "POST", &path, &env.alice, org, Some(body(false))).await;
        assert_eq!(st, 200, "{done}");
        let import_id = done["import"]["id"].as_str().unwrap().to_string();
        let (_, again) = call(&server, "POST", &path, &env.alice, org, Some(body(false))).await;
        assert_eq!(again["import"]["id"], import_id.as_str(), "same file, same import");

        // Local times: previewed as needing a zone, refused without one.
        let csv = |dry: bool| json!({"object_id": d1, "file_name": "t07.csv", "format": "csv", "content": "time,lat,lon\n2026-03-01 11:00:00,48.0,2.0\n", "dry_run": dry});
        let (st, pv) = call(&server, "POST", &path, &env.alice, org, Some(csv(true))).await;
        assert_eq!((st, pv["needs_time_zone"].as_bool()), (200, Some(true)), "{pv}");
        let (st, e) = call(&server, "POST", &path, &env.alice, org, Some(csv(false))).await;
        assert_eq!((st, e["error"].as_str()), (422, Some("track-import-timezone-required")), "{e}");

        let window = json!({"from": "2026-03-01T09:00:00Z", "to": "2026-03-01T12:00:00Z"});
        let (st, t) = call(&server, "POST", &format!("/api/v1/cases/{cid}/tracks"), &env.alice, org, Some(window.clone())).await;
        assert_eq!(st, 200, "{t}");
        assert_eq!(t["available"], true);
        assert_eq!(t["tracks"][0]["raw_points"], 1_501, "{t}");
        // Thresholds of the case: bounded, then used by the detectors.
        let put_case = |min_s: i64| json!({"title": "Depot", "properties": {"status": "open", "stop_min_s": min_s}, "expected_version": case["version"]});
        let (st, e) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{cid}"), &env.alice, org, Some(put_case(5))).await;
        assert_eq!((st, e["stop_min_s"].as_str()), (400, Some("invalid")), "{e}");
        let (st, e) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{cid}"), &env.alice, org, Some(put_case(600))).await;
        assert_eq!(st, 200, "{e}");
        let (st, d) = call(&server, "POST", &format!("/api/v1/cases/{cid}/detect"), &env.alice, org, Some(window.clone())).await;
        assert_eq!(st, 200, "{d}");
        let (_, list) = call(&server, "GET", &format!("/api/v1/cases/{cid}/proposals"), &env.alice, org, None).await;
        let kinds: Vec<&str> = list.as_array().unwrap().iter().map(|p| p["kind"].as_str().unwrap()).collect();
        assert!(kinds.contains(&"zone") && kinds.contains(&"stop"), "{list}");
        let stop = list.as_array().unwrap().iter().find(|p| p["kind"] == "stop").unwrap();
        assert_eq!(stop["params"]["min_s"], 600, "{stop}");
        let (_, d2) = call(&server, "POST", &format!("/api/v1/cases/{cid}/detect"), &env.alice, org, Some(window.clone())).await;
        assert_eq!(d2["inserted"], 0, "a second run proposes nothing new");

        // K38 log: unavailable without O2 (never invented), private to the org.
        let (st, r) = call(&server, "GET", &format!("/api/v1/cases/{cid}/reads"), &env.alice, org, None).await;
        assert_eq!((st, r), (200, Value::Null));

        // R1 / R2, then the import goes and its track with it.
        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "GET", &format!("/api/v1/cases/{cid}/reads"), &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "GET", &format!("/api/v1/cases/{cid}/tracks/imports"), &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org, Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st < 300);
        let (st, _) = call(&server, "POST", &path, &env.bob, org, Some(body(true))).await;
        assert_eq!(st, 403);
        let (st, list) = call(&server, "GET", &format!("/api/v1/cases/{cid}/tracks/imports"), &env.bob, org, None).await;
        assert_eq!((st, list.as_array().map(Vec::len)), (200, Some(1)));
        let (st, _) = call(&server, "DELETE", &format!("/api/v1/cases/{cid}/tracks/imports/{import_id}"), &env.alice, org, None).await;
        assert_eq!(st, 204);
        let (_, t) = call(&server, "POST", &format!("/api/v1/cases/{cid}/tracks"), &env.alice, org, Some(window)).await;
        assert_eq!(t["tracks"], json!([]));
    })
    .await;
}

/// `fuzzy_time` value of an exact instant.
fn instant(t: &str) -> Value {
    json!({"from": t, "to": t, "precision": "instant"})
}

/// Titles of the attachments of the workspace event titled `event`.
fn attached(w: &Value, event: &str) -> Vec<(String, String)> {
    w["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["event"]["title"] == event)
        .flat_map(|e| e["attachments"].as_array().unwrap().iter())
        .map(|a| {
            (
                a["object"]["title"].as_str().unwrap().to_string(),
                a["role"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn undated(w: &Value) -> Vec<String> {
    let mut t: Vec<String> = w["undated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["title"].as_str().unwrap().to_string())
        .collect();
    t.sort();
    t
}

/// Case workspace (cases-workspace.md K20–K22): events created with their
/// first attachments in one gesture, roles defaulting by type and closed,
/// undated pieces, detaching keeps a piece in the case, removing an event
/// keeps its pieces; signals leave the workspace's own operations out;
/// another org never sees it (R1); a viewer reads but never writes (R2).
#[tokio::test]
#[serial]
async fn case_workspace_events_attachments_and_signals() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, e) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(json!({"type_key": "case", "title": "P-12", "properties": {}}))).await;
        assert_eq!((st, e["status"].as_str()), (400, Some("required")), "{e}");
        let case = object(&server, &env.alice, org, "case", "Low pressure P-12", json!({"status": "open", "question": "Why?"})).await;
        let cid = case["id"].as_str().unwrap().to_string();
        let d1 = device(&server, &env, org, "c47").await;
        let d2 = device(&server, &env, org, "c88").await;
        let note = object(&server, &env.alice, org, "note", "Valve left closed?", json!({"body": "To check"})).await;
        let url = |p: &str| format!("/api/v1/cases/{cid}/{p}");

        let (st, w) = call(&server, "POST", &url("events"), &env.alice, org, Some(json!({
            "title": "High pressure alarm", "occurred": instant("2026-10-10T14:32:00Z"),
            "attach": [{"object_id": d1}, {"object_id": note["id"]}]
        }))).await;
        assert_eq!(st, 201, "{w}");
        assert_eq!(attached(&w, "High pressure alarm"), vec![("c47".into(), "evidence".into()), ("Valve left closed?".into(), "hypothesis".into())], "{w}");
        let ev = w["events"][0]["event"]["id"].as_str().unwrap().to_string();
        assert_eq!(w["events"][0]["event"]["properties"]["origin"], "manual");
        // An attachment role is a closed list; an event of another case is refused.
        let (st, _) = call(&server, "POST", &url("attach"), &env.alice, org, Some(json!({"event_id": ev, "object_id": d2, "role": "suspect"}))).await;
        assert_eq!(st, 400);
        let (st, _) = call(&server, "POST", &url("attach"), &env.alice, org, Some(json!({"event_id": d1, "object_id": d2}))).await;
        assert_eq!(st, 400);

        // Undated, then dated: it leaves the undated pieces; attaching twice is a no-op.
        let (st, w) = call(&server, "POST", &url("attach"), &env.alice, org, Some(json!({"object_id": d2}))).await;
        assert_eq!((st, undated(&w)), (200, vec!["c88".to_string()]), "{w}");
        for _ in 0..2 {
            let (st, w) = call(&server, "POST", &url("attach"), &env.alice, org, Some(json!({"event_id": ev, "object_id": d2, "role": "context", "detail": "downstream sensor"}))).await;
            assert_eq!(st, 200, "{w}");
            assert!(undated(&w).is_empty(), "{w}");
            assert_eq!(attached(&w, "High pressure alarm").len(), 3, "{w}");
        }
        // Another role: the attachment changes, still one.
        let (st, w) = call(&server, "POST", &url("attach"), &env.alice, org, Some(json!({"event_id": ev, "object_id": d2, "role": "evidence"}))).await;
        assert_eq!(st, 200, "{w}");
        let a = w["events"][0]["attachments"].as_array().unwrap().iter().find(|a| a["object"]["title"] == "c88").unwrap().clone();
        assert_eq!((a["role"].as_str(), a["detail"].as_str()), (Some("evidence"), Some("downstream sensor")), "{w}");
        assert_eq!(attached(&w, "High pressure alarm").len(), 3, "{w}");
        // Detached from the event, it stays in the case, undated.
        let (st, w) = call(&server, "POST", &url("detach"), &env.alice, org, Some(json!({"event_id": ev, "object_id": d2}))).await;
        assert_eq!((st, undated(&w)), (200, vec!["c88".to_string()]), "{w}");

        // Signals: a link among members is one, the workspace's own links are not.
        let (st, lt) = call(&server, "POST", "/api/v1/ontology/link-types", &env.alice, org, Some(json!({"key": "feeds", "name": "feeds", "from_types": ["device"], "to_types": ["device"]}))).await;
        assert_eq!(st, 201, "{lt}");
        let (st, l) = link(&server, &env.alice, org, json!({"link_type": "feeds", "source_id": d1, "target_id": d2})).await;
        assert_eq!(st, 201, "{l}");
        let around_now = json!({"from": (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339(), "to": (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339()});
        let (st, sig) = call(&server, "POST", &url("signals"), &env.alice, org, Some(around_now.clone())).await;
        assert_eq!(st, 200, "{sig}");
        let kinds: Vec<&str> = sig["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "link_open").map(|i| i["link"]["link_type"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["feeds"], "{sig}");
        // Default window: around the events.
        let (st, sig) = call(&server, "POST", &url("signals"), &env.alice, org, Some(json!({}))).await;
        assert_eq!(st, 200, "{sig}");
        assert!(sig["from"].as_str().unwrap().starts_with("2026-10-10T13:32:00"), "{sig}");

        // Removing the event: its pieces stay in the case, undated.
        let (st, w) = call(&server, "POST", &url("detach"), &env.alice, org, Some(json!({"object_id": ev}))).await;
        assert_eq!(st, 200, "{w}");
        assert!(w["events"].as_array().unwrap().is_empty(), "{w}");
        assert_eq!(undated(&w), vec!["Valve left closed?".to_string(), "c47".into(), "c88".into()], "{w}");

        // Bad window, not a case, another org.
        let (st, _) = call(&server, "POST", &url("signals"), &env.alice, org, Some(json!({"from": "2026-10-02T00:00:00Z", "to": "2026-10-01T00:00:00Z"}))).await;
        assert_eq!(st, 400);
        let (st, _) = call(&server, "GET", &format!("/api/v1/cases/{d1}/workspace"), &env.alice, org, None).await;
        assert_eq!(st, 404);
        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "GET", &url("workspace"), &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "POST", &url("attach"), &env.bob, org_b, Some(json!({"object_id": d1}))).await;
        assert_eq!(st, 404);

        // Viewer: reads the workspace, cannot attach nor create an event.
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org, Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st < 300);
        let (st, _) = call(&server, "GET", &url("workspace"), &env.bob, org, None).await;
        assert_eq!(st, 200);
        let (st, _) = call(&server, "POST", &url("attach"), &env.bob, org, Some(json!({"object_id": d1}))).await;
        assert_eq!(st, 403);
        let (st, _) = call(&server, "POST", &url("events"), &env.bob, org, Some(json!({"title": "x", "occurred": instant("2026-10-10T14:00:00Z")}))).await;
        assert_eq!(st, 403);
        let (st, _) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{cid}"), &env.bob, org, Some(json!({"title": "x", "properties": {"status": "concluded"}, "expected_version": 1}))).await;
        assert_eq!(st, 403);
    })
    .await;
}

/// Opening a case from an alert (cases-workspace.md K20): its first event is
/// the alert at its instant, the flow that raised it attached as source,
/// its neighbours then as context; a range scoped on a member is a signal;
/// another org's flow adds nothing (R1); a viewer cannot open a case (R2).
#[tokio::test]
#[serial]
async fn case_opened_from_an_alert() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let flow = |org: i64, name: &str| format!("INSERT INTO flows (name, status, org_id) VALUES ('{name}', 'draft', {org}) RETURNING id");
        let row = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, flow(org, "pressure watch"))).await.unwrap().unwrap();
        let flow_id: i64 = row.try_get("", "id").unwrap();
        let (st, f) = call(&server, "GET", &format!("/api/v1/ontology/resolve/flow/{flow_id}"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{f}");
        let d1 = device(&server, &env, org, "p12-sensor").await;
        let (st, e) = call(&server, "POST", "/api/v1/ontology/link-types", &env.alice, org, Some(json!({"key": "drives", "name": "drives", "from_types": ["flow"], "to_types": ["device"]}))).await;
        assert_eq!(st, 201, "{e}");
        let (st, e) = link(&server, &env.alice, org, json!({"link_type": "drives", "source_id": f["id"], "target_id": d1, "valid_from": "2026-10-01T00:00:00Z"})).await;
        assert_eq!(st, 201, "{e}");

        for bad in [
            json!({"title": "x", "from": {"kind": "rumour", "at": "2026-10-03T10:02:00Z"}}),
            json!({"title": "x", "from": {"kind": "notification", "at": "yesterday"}}),
            json!({"title": "", "from": {"kind": "notification", "at": "2026-10-03T10:02:00Z"}}),
        ] {
            let (st, e) = call(&server, "POST", "/api/v1/cases", &env.alice, org, Some(bad)).await;
            assert_eq!(st, 400, "{e}");
        }

        let (st, c) = call(&server, "POST", "/api/v1/cases", &env.alice, org, Some(json!({
            "title": "Low pressure P-12",
            "from": {"kind": "notification", "at": "2026-10-03T12:02:00+02:00", "flow_id": flow_id}
        }))).await;
        assert_eq!(st, 201, "{c}");
        let cid = c["id"].as_str().unwrap().to_string();
        let (_, w) = call(&server, "GET", &format!("/api/v1/cases/{cid}/workspace"), &env.alice, org, None).await;
        let ev = &w["events"][0]["event"];
        assert_eq!(ev["properties"]["occurred"], instant("2026-10-03T10:02:00Z"), "{w}");
        assert_eq!(ev["properties"]["origin"], "signal");
        assert_eq!(attached(&w, "Low pressure P-12"), vec![("pressure watch".into(), "source".into()), ("p12-sensor".into(), "context".into())], "{w}");

        // A shift on the sensor around the alert: a signal of the case.
        let (st, r) = call(&server, "POST", "/api/v1/time-ranges", &env.alice, org, Some(json!({
            "scope_kind": "object", "scope_id": d1, "label": "Night shift",
            "planned_start": "2026-10-03T06:00:00Z", "planned_end": "2026-10-03T14:00:00Z"
        }))).await;
        assert_eq!(st, 201, "{r}");
        let (st, sig) = call(&server, "POST", &format!("/api/v1/cases/{cid}/signals"), &env.alice, org, Some(json!({}))).await;
        assert_eq!(st, 200, "{sig}");
        let range = sig["items"].as_array().unwrap().iter().find(|i| i["kind"] == "range").expect("range item");
        assert_eq!(range["range"]["label"], "Night shift");
        assert_eq!(range["lane"].as_str(), Some(d1.as_str()));

        // Another org's flow adds nothing; a viewer cannot open a case.
        let org_b = personal_org(&server, &env.bob).await;
        let row = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, flow(org_b, "foreign"))).await.unwrap().unwrap();
        let foreign: i64 = row.try_get("", "id").unwrap();
        let (st, c) = call(&server, "POST", "/api/v1/cases", &env.alice, org, Some(json!({
            "title": "Foreign", "from": {"kind": "event", "at": "2026-10-03T10:02:00Z", "flow_id": foreign}
        }))).await;
        assert_eq!(st, 201, "{c}");
        let (_, w) = call(&server, "GET", &format!("/api/v1/cases/{}/workspace", c["id"].as_str().unwrap()), &env.alice, org, None).await;
        assert!(attached(&w, "Foreign").is_empty(), "{w}");
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org, Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st < 300);
        let (st, _) = call(&server, "POST", "/api/v1/cases", &env.bob, org, Some(json!({"title": "nope"}))).await;
        assert_eq!(st, 403);
    })
    .await;
}

/// Investigation pack (cases.md K5, K7, cases-workspace.md K38): approximate
/// dates are validated; a person joins a case only once its processing
/// basis is declared; a claim becomes corroborated only with two pieces of
/// distinct origins.
#[tokio::test]
#[serial]
async fn investigation_pack_legal_basis_and_claims() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(&server, "POST", "/api/v1/ontology/packs/investigation/install", &env.alice, org, None).await;
        assert!(st < 300, "{p}");
        let (st, e) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(json!({
            "type_key": "event", "title": "Meeting", "properties": {"occurred": {"from": "2019-03-31T00:00:00Z", "to": "2019-03-01T00:00:00Z", "precision": "range"}}
        }))).await;
        assert_eq!((st, e["occurred"].as_str()), (400, Some("invalid")), "{e}");

        let case = object(&server, &env.alice, org, "case", "Port affair", json!({"status": "open"})).await;
        let cid = case["id"].as_str().unwrap();
        let month = json!({"from": "2019-03-01T00:00:00Z", "to": "2019-03-31T23:59:59Z", "precision": "month"});
        let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/events"), &env.alice, org, Some(json!({"title": "Meeting at the port", "occurred": month}))).await;
        assert_eq!(st, 201, "{w}");
        let ev = w["events"][0]["event"]["id"].clone();
        let person = object(&server, &env.alice, org, "person", "X", json!({})).await;
        let attach = json!({"event_id": ev, "object_id": person["id"], "detail": "witness"});
        let (st, e) = call(&server, "POST", &format!("/api/v1/cases/{cid}/attach"), &env.alice, org, Some(attach.clone())).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("case-legal-basis-required")), "{e}");
        let upd = json!({"title": "Port affair", "properties": {"status": "open", "legal_basis": "journalism (art. 85 GDPR)"}, "expected_version": case["version"]});
        let (st, v) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{cid}"), &env.alice, org, Some(upd)).await;
        assert_eq!(st, 200, "{v}");
        let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/attach"), &env.alice, org, Some(attach)).await;
        assert_eq!(st, 200, "{w}");
        assert_eq!(w["events"][0]["attachments"][0]["role"], "actor");
        assert_eq!(w["events"][0]["attachments"][0]["detail"], "witness");

        // K7: corroborated needs two pieces of distinct origins.
        let claim = object(&server, &env.alice, org, "claim", "C1", json!({"statement": "X was at the port", "status": "unverified"})).await;
        let corroborate = |v: i64| json!({"title": "C1", "properties": {"statement": "X was at the port", "status": "corroborated"}, "expected_version": v});
        let (st, e) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{}", claim["id"].as_str().unwrap()), &env.alice, org, Some(corroborate(1))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-claim-unsourced")), "{e}");
        let (st, e) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(json!({"type_key": "claim", "title": "C2", "properties": {"statement": "s", "status": "corroborated"}}))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-claim-unsourced")), "{e}");
        let mut pieces = Vec::new();
        for (t, url) in [("article", "https://a.example/1"), ("copy", "https://a.example/1"), ("court record", "https://b.example/2")] {
            let p = object(&server, &env.alice, org, "piece", t, json!({"source_url": url})).await;
            pieces.push(p["id"].as_str().unwrap().to_string());
        }
        // Two pieces, one origin: still refused.
        for p in &pieces[..2] {
            let (st, l) = link(&server, &env.alice, org, json!({"link_type": "sourced_by", "source_id": claim["id"], "target_id": p, "attributes": {"locator": "p. 3"}})).await;
            assert_eq!(st, 201, "{l}");
        }
        let (st, _) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{}", claim["id"].as_str().unwrap()), &env.alice, org, Some(corroborate(1))).await;
        assert_eq!(st, 409);
        let (st, _) = link(&server, &env.alice, org, json!({"link_type": "sourced_by", "source_id": claim["id"], "target_id": pieces[2]})).await;
        assert_eq!(st, 201);
        let (st, c) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{}", claim["id"].as_str().unwrap()), &env.alice, org, Some(corroborate(1))).await;
        assert_eq!(st, 200, "{c}");
        assert_eq!(c["properties"]["status"], "corroborated");
        // A written justification overrides the rule, an empty one does not.
        let justified = |note: &str| json!({"type_key": "claim", "title": "C3", "properties": {"statement": "s", "status": "corroborated", "corroboration_note": note}});
        let (st, _) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(justified("  "))).await;
        assert_eq!(st, 409);
        let (st, c3) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org, Some(justified("Official statement, single source by nature"))).await;
        assert_eq!(st, 201, "{c3}");
    })
    .await;
}

/// Board and export (cases.md K8, cases-workspace.md K33): the board puts
/// claims for and against each hypothesis and flags unsourced and
/// contradicted claims; the export (`pnex.case/1`) holds the workspace,
/// counts persons and lists pieces; another org reads nothing (R1).
#[tokio::test]
#[serial]
async fn case_board_and_export() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/investigation/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert!(st < 300, "{p}");
        let case = object(
            &server,
            &env.alice,
            org,
            "case",
            "Leak",
            json!({"status": "open", "legal_basis": "legitimate interest"}),
        )
        .await;
        let cid = case["id"].as_str().unwrap().to_string();
        let h = object(
            &server,
            &env.alice,
            org,
            "hypothesis",
            "Seal worn",
            json!({"statement": "The seal wore out", "status": "open"}),
        )
        .await;
        let c1 = object(
            &server,
            &env.alice,
            org,
            "claim",
            "Seal changed late",
            json!({"statement": "s1", "status": "unverified"}),
        )
        .await;
        let c2 = object(
            &server,
            &env.alice,
            org,
            "claim",
            "Seal changed on time",
            json!({"statement": "s2", "status": "unverified"}),
        )
        .await;
        let piece = object(
            &server,
            &env.alice,
            org,
            "piece",
            "Work order",
            json!({"sha256": "ab".repeat(32)}),
        )
        .await;
        let person = object(&server, &env.alice, org, "person", "Technician", json!({})).await;
        for o in [&h, &c1, &c2, &piece, &person] {
            let (st, w) = call(
                &server,
                "POST",
                &format!("/api/v1/cases/{cid}/attach"),
                &env.alice,
                org,
                Some(json!({"object_id": o["id"]})),
            )
            .await;
            assert_eq!(st, 200, "{w}");
        }
        for (t, s, d) in [
            ("supports", &c1, &h),
            ("contradicts", &c2, &c1),
            ("sourced_by", &c2, &piece),
        ] {
            let (st, l) = link(
                &server,
                &env.alice,
                org,
                json!({"link_type": t, "source_id": s["id"], "target_id": d["id"]}),
            )
            .await;
            assert_eq!(st, 201, "{l}");
        }

        let (st, b) = call(
            &server,
            "GET",
            &format!("/api/v1/cases/{cid}/board"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{b}");
        assert_eq!(
            b["hypotheses"][0]["supports"][0]["claim"]["title"], "Seal changed late",
            "{b}"
        );
        assert_eq!(b["hypotheses"][0]["status"], "open");
        let titles = |k: &str| {
            b[k].as_array()
                .unwrap()
                .iter()
                .map(|c| c["claim"]["title"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(titles("unsourced"), vec!["Seal changed late"]);
        assert_eq!(titles("contradicted"), vec!["Seal changed late"]);

        let (st, x) = call(
            &server,
            "GET",
            &format!("/api/v1/cases/{cid}/export"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{x}");
        assert_eq!(x["schema"], "pnex.case/1");
        assert_eq!(
            (
                x["persons"].as_u64(),
                x["workspace"]["undated"].as_array().unwrap().len()
            ),
            (Some(1), 5),
            "{x}"
        );
        assert_eq!(x["pieces"][0]["sha256"], "ab".repeat(32));
        assert_eq!(
            x["workspace"]["case"]["properties"]["legal_basis"],
            "legitimate interest"
        );
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/cases/{cid}/detach"),
            &env.alice,
            org,
            Some(json!({"object_id": person["id"]})),
        )
        .await;
        assert_eq!(st, 200);
        let (_, x) = call(
            &server,
            "GET",
            &format!("/api/v1/cases/{cid}/export"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(x["persons"].as_u64(), Some(0), "{x}");

        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(
            &server,
            "GET",
            &format!("/api/v1/cases/{cid}/board"),
            &env.bob,
            org_b,
            None,
        )
        .await;
        assert_eq!(st, 404);
        let (st, _) = call(
            &server,
            "GET",
            &format!("/api/v1/cases/{cid}/export"),
            &env.bob,
            org_b,
            None,
        )
        .await;
        assert_eq!(st, 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn case_pages_from_templates_and_conclusion() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        // Templates come with the pack.
        let (_, t) = call(&server, "GET", "/api/v1/cases/templates", &env.alice, org, None).await;
        assert_eq!(t.as_array().unwrap().len(), 0, "{t}");
        let (st, p) = call(&server, "POST", "/api/v1/ontology/packs/investigation/install", &env.alice, org, None).await;
        assert!(st < 300, "{p}");
        let (_, t) = call(&server, "GET", "/api/v1/cases/templates", &env.alice, org, None).await;
        assert!(t.as_array().unwrap().iter().any(|t| t["key"] == "five_whys"), "{t}");

        let case = object(&server, &env.alice, org, "case", "Pump trip", json!({"status": "open"})).await;
        let cid = case["id"].as_str().unwrap().to_string();
        let url = format!("/api/v1/cases/{cid}/pages");
        let (st, page) = call(&server, "POST", &url, &env.alice, org, Some(json!({"title": "Why it tripped", "template": "five_whys"}))).await;
        assert_eq!(st, 201, "{page}");
        let doc = page["id"].as_str().unwrap().to_string();
        let row = env.db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT length(snapshot) AS n FROM docs WHERE id = '{doc}'"),
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(row.try_get::<i32>("", "n").unwrap() > 100);
        let (st, blank) = call(&server, "POST", &url, &env.alice, org, Some(json!({"title": "Notes"}))).await;
        assert_eq!(st, 201, "{blank}");
        let (st, e) = call(&server, "POST", &url, &env.alice, org, Some(json!({"title": "x", "template": "nope"}))).await;
        assert_eq!((st, e["template"].as_str()), (400, Some("invalid")), "{e}");
        let (st, e) = call(&server, "POST", &url, &env.alice, org, Some(json!({"title": "  "}))).await;
        assert_eq!((st, e["title"].as_str()), (400, Some("required")));

        // Both pages joined the case as undated pieces.
        let (_, w) = call(&server, "GET", &format!("/api/v1/cases/{cid}/workspace"), &env.alice, org, None).await;
        assert_eq!(undated(&w), vec!["Notes".to_string(), "Why it tripped".into()], "{w}");
        let page_obj = w["undated"].as_array().unwrap().iter().find(|o| o["native_id"] == doc.as_str()).unwrap()["id"].clone();

        // Concluding versions the designated conclusion page.
        let upd = json!({"title": "Pump trip", "properties": {"status": "concluded", "conclusion_page": page_obj}, "expected_version": case["version"]});
        let (st, v) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{cid}"), &env.alice, org, Some(upd)).await;
        assert_eq!(st, 200, "{v}");
        let (_, versions) = call(&server, "GET", &format!("/api/v1/docs/{doc}/versions"), &env.alice, org, None).await;
        assert!(versions.to_string().contains("\"Conclusion\""), "{versions}");

        // R1 / R2: another org sees nothing; a viewer cannot add a page.
        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "POST", &url, &env.bob, org_b, Some(json!({"title": "x"}))).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org, Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st == 200 || st == 201);
        let (st, _) = call(&server, "POST", &url, &env.bob, org, Some(json!({"title": "x"}))).await;
        assert_eq!(st, 403);
    })
    .await;
}

/// Two installs of the same pack at once both succeed: the second waits,
/// then finds the pack installed (they used to race into a 409).
#[tokio::test]
#[serial]
async fn concurrent_pack_installs_both_succeed() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let url = "/api/v1/ontology/packs/investigation/install";
        let (a, b) = tokio::join!(
            call(&server, "POST", url, &env.alice, org, None),
            call(&server, "POST", url, &env.alice, org, None),
        );
        assert!(a.0 < 300 && b.0 < 300, "{a:?} {b:?}");
    })
    .await;
}

/// The case graph (K31): relations among members with their history, and a
/// node expanded in two steps (counted facets, then the picked neighbours).
#[tokio::test]
#[serial]
async fn case_graph_relations_and_expansion() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(&server, "POST", "/api/v1/ontology/packs/investigation/install", &env.alice, org, None).await;
        assert!(st < 300, "{p}");
        let case = object(&server, &env.alice, org, "case", "Ring", json!({"status": "open", "legal_basis": "contract"})).await;
        let cid = case["id"].as_str().unwrap().to_string();
        let ann = object(&server, &env.alice, org, "person", "Ann", json!({})).await;
        let bob = object(&server, &env.alice, org, "person", "Bob", json!({})).await;
        let carl = object(&server, &env.alice, org, "person", "Carl", json!({})).await;
        let phone = object(&server, &env.alice, org, "identifier", "Ann phone", json!({"kind": "phone", "value": "+33600000001"})).await;
        for o in [&ann, &bob] {
            let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/attach"), &env.alice, org, Some(json!({"object_id": o["id"]}))).await;
            assert_eq!(st, 200, "{w}");
        }
        for body in [
            json!({"link_type": "knows", "source_id": ann["id"], "target_id": bob["id"]}),
            json!({"link_type": "knows", "source_id": ann["id"], "target_id": carl["id"]}),
            json!({"link_type": "uses_identifier", "source_id": ann["id"], "target_id": phone["id"]}),
        ] {
            let (st, l) = link(&server, &env.alice, org, body).await;
            assert_eq!(st, 201, "{l}");
        }
        let (st, rel) = call(&server, "GET", &format!("/api/v1/cases/{cid}/relations"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{rel}");
        let rel = rel.as_array().unwrap();
        assert_eq!(rel.len(), 1, "only Ann–Bob joins two members: {rel:?}");
        assert_eq!(rel[0]["link_type"], "knows");

        let url = format!("/api/v1/cases/{cid}/expand");
        let shown = json!([ann["id"], bob["id"]]);
        let (st, f) = call(&server, "POST", &url, &env.alice, org, Some(json!({"object_id": ann["id"], "exclude": shown}))).await;
        assert_eq!(st, 200, "{f}");
        let facets: Vec<(String, String, u64)> = f["facets"].as_array().unwrap().iter()
            .map(|x| (x["link_type"].as_str().unwrap().into(), x["type_key"].as_str().unwrap().into(), x["count"].as_u64().unwrap()))
            .collect();
        assert_eq!(facets, [("knows".into(), "person".into(), 1), ("uses_identifier".into(), "identifier".into(), 1)]);
        assert_eq!(f["nodes"], json!([]));
        let pick = json!({"object_id": ann["id"], "exclude": shown, "pick": [{"link_type": "knows", "type_key": "person"}]});
        let (st, r) = call(&server, "POST", &url, &env.alice, org, Some(pick.clone())).await;
        assert_eq!(st, 200, "{r}");
        assert_eq!(r["nodes"][0]["title"], "Carl", "{r}");
        assert_eq!(r["nodes"].as_array().map(Vec::len), Some(1));

        // Positions arranged by hand are kept; none = the automatic layout.
        let lay = format!("/api/v1/cases/{cid}/layout");
        let (st, l) = call(&server, "GET", &lay, &env.alice, org, None).await;
        assert_eq!((st, l), (200, json!({"positions": {}})));
        let mine = json!({"positions": {ann["id"].as_str().unwrap(): [120.5, 80.0]}});
        let (st, _) = call(&server, "PUT", &lay, &env.alice, org, Some(mine.clone())).await;
        assert_eq!(st, 200);
        let (_, l) = call(&server, "GET", &lay, &env.alice, org, None).await;
        assert_eq!(l, mine);
        let (st, e) = call(&server, "PUT", &lay, &env.alice, org, Some(json!({"positions": {"x": [1e9, 0.0]}}))).await;
        assert_eq!((st, e["positions"].as_str()), (400, Some("invalid")), "{e}");

        // R1: another org sees neither.
        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "GET", &format!("/api/v1/cases/{cid}/relations"), &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "POST", &url, &env.bob, org_b, Some(pick)).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "GET", &lay, &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        // R2: a viewer reads the arrangement, never saves one.
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org, Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st < 300);
        let (st, _) = call(&server, "PUT", &lay, &env.bob, org, Some(json!({"positions": {}}))).await;
        assert_eq!(st, 403);
        let (st, l) = call(&server, "GET", &lay, &env.bob, org, None).await;
        assert_eq!((st, l), (200, mine));
        // Re-layout: empty positions clear the arrangement.
        let (st, _) = call(&server, "PUT", &lay, &env.alice, org, Some(json!({"positions": {}}))).await;
        assert_eq!(st, 200);
        let (_, l) = call(&server, "GET", &lay, &env.alice, org, None).await;
        assert_eq!(l, json!({"positions": {}}));
    })
    .await;
}

/// Frozen states (K33) read back byte for byte with their fingerprint, are
/// numbered, and an export imports back into an identical new case.
#[tokio::test]
#[serial]
async fn case_snapshots_and_export_round_trip() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(&server, "POST", "/api/v1/ontology/packs/investigation/install", &env.alice, org, None).await;
        assert!(st < 300, "{p}");
        let case = object(&server, &env.alice, org, "case", "Leak", json!({"status": "concluded", "question": "Why?", "legal_basis": "contract", "stop_min_s": 600})).await;
        let cid = case["id"].as_str().unwrap().to_string();
        let ann = object(&server, &env.alice, org, "person", "Ann", json!({})).await;
        let pump = object(&server, &env.alice, org, "place", "Pump room", json!({})).await;
        let gone = object(&server, &env.alice, org, "place", "Old shed", json!({})).await;
        let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/events"), &env.alice, org, Some(json!({
            "title": "Leak seen", "occurred": instant("2025-05-02T09:00:00Z"),
            "attach": [{"object_id": ann["id"], "detail": "witness"}, {"object_id": pump["id"]}, {"object_id": gone["id"]}]
        }))).await;
        assert_eq!(st, 201, "{w}");
        let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/events"), &env.alice, org, Some(json!({
            "title": "Valve closed", "occurred": instant("2025-05-02T09:30:00Z"), "origin": "signal"
        }))).await;
        assert_eq!(st, 201, "{w}");
        let note = object(&server, &env.alice, org, "note", "Pressure log", json!({"body": "to check"})).await;
        let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/attach"), &env.alice, org, Some(json!({"object_id": note["id"]}))).await;
        assert_eq!(st, 200, "{w}");

        // Freeze twice: numbered, fingerprinted, read back as fingerprinted.
        let url = format!("/api/v1/cases/{cid}/snapshots");
        let (st, s1) = call(&server, "POST", &url, &env.alice, org, Some(json!({"label": "Before the report"}))).await;
        assert_eq!((st, s1["seq"].as_i64()), (201, Some(1)), "{s1}");
        let (_, s2) = call(&server, "POST", &url, &env.alice, org, Some(json!({}))).await;
        assert_eq!(s2["seq"], 2);
        let (st, list) = call(&server, "GET", &url, &env.alice, org, None).await;
        assert_eq!(st, 200);
        let seqs: Vec<i64> = list.as_array().unwrap().iter().map(|s| s["seq"].as_i64().unwrap()).collect();
        assert_eq!(seqs, [2, 1]);
        assert_eq!(list[1]["label"], "Before the report");
        let (st, full) = call(&server, "GET", &format!("{url}/1"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{full}");
        let content = full["content"].as_str().unwrap();
        use sha2::Digest;
        assert_eq!(format!("{:x}", sha2::Sha256::digest(content.as_bytes())), full["snapshot"]["sha256"].as_str().unwrap());
        let frozen: Value = serde_json::from_str(content).unwrap();
        assert_eq!(frozen["export"]["workspace"]["events"].as_array().map(Vec::len), Some(2));
        // Never modified: no route writes a frozen state again.
        let (st, _) = call(&server, "PUT", &format!("{url}/1"), &env.alice, org, Some(json!({"label": "x"}))).await;
        assert_eq!(st, 405);
        let (st, _) = call(&server, "GET", &format!("{url}/9"), &env.alice, org, None).await;
        assert_eq!(st, 404);

        // Round trip: the export, one object gone, imports into a new open case.
        let (st, before) = call(&server, "GET", &format!("/api/v1/cases/{cid}/export"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{before}");
        let (st, _) = call(&server, "DELETE", &format!("/api/v1/ontology/objects/{}", gone["id"].as_str().unwrap()), &env.alice, org, None).await;
        assert!(st < 300, "{st}");
        let (st, imp) = call(&server, "POST", "/api/v1/cases/import", &env.alice, org, Some(before.clone())).await;
        assert_eq!(st, 201, "{imp}");
        assert_eq!(imp["events"], 2);
        assert_eq!(imp["missing"][0]["title"], "Old shed", "{imp}");
        let nid = imp["case"]["id"].as_str().unwrap();
        assert_ne!(nid, cid);
        let (_, after) = call(&server, "GET", &format!("/api/v1/cases/{nid}/export"), &env.alice, org, None).await;
        let (wb, wa) = (&before["workspace"], &after["workspace"]);
        assert_eq!(wa["case"]["properties"]["status"], "open");
        for k in ["question", "legal_basis", "stop_min_s"] {
            assert_eq!(wa["case"]["properties"][k], wb["case"]["properties"][k], "{k}");
        }
        let shape = |w: &Value| -> Vec<(Value, Value, Vec<(Value, Value, Value)>)> {
            w["events"].as_array().unwrap().iter().map(|e| {
                let p = e["event"]["properties"].clone();
                let atts = e["attachments"].as_array().unwrap().iter()
                    .filter(|a| a["object"]["title"] != "Old shed")
                    .map(|a| (a["object"]["id"].clone(), a["role"].clone(), a["detail"].clone())).collect();
                (e["event"]["title"].clone(), p, atts)
            }).collect()
        };
        assert_eq!(shape(wa), shape(wb));
        assert_eq!(wa["undated"], wb["undated"]);
        let mut other = before.clone();
        other["schema"] = json!("other/1");
        let (st, e) = call(&server, "POST", "/api/v1/cases/import", &env.alice, org, Some(other)).await;
        assert_eq!((st, e["schema"].as_str()), (400, Some("invalid")), "{e}");

        // R1 / R2.
        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "GET", &url, &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "GET", &format!("{url}/1"), &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org, Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st < 300);
        let (st, _) = call(&server, "POST", &url, &env.bob, org, Some(json!({}))).await;
        assert_eq!(st, 403);
        let (st, _) = call(&server, "POST", "/api/v1/cases/import", &env.bob, org, Some(before)).await;
        assert_eq!(st, 403);
        let (st, l) = call(&server, "GET", &url, &env.bob, org, None).await;
        assert_eq!((st, l.as_array().map(Vec::len)), (200, Some(2)));
    })
    .await;
}

/// Retention (cases.md K16): an archived case purges the persons and
/// identifiers only it holds, whether attached to one of its events or
/// undated; another case or an open link outside keeps them.
#[tokio::test]
#[serial]
async fn archived_case_purges_its_own_persons_only() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (st, p) = call(&server, "POST", "/api/v1/ontology/packs/investigation/install", &env.alice, org, None).await;
        assert!(st < 300, "{p}");
        let props = json!({"status": "open", "legal_basis": "legitimate interest"});
        let case = object(&server, &env.alice, org, "case", "Fraud", props.clone()).await;
        let cid = case["id"].as_str().unwrap().to_string();
        let other = object(&server, &env.alice, org, "case", "Other", props).await;
        let ann = object(&server, &env.alice, org, "person", "Ann", json!({})).await;
        let phone = object(&server, &env.alice, org, "identifier", "Ann phone", json!({"kind": "phone", "value": "+33600000000"})).await;
        let bob = object(&server, &env.alice, org, "person", "Bob", json!({})).await;
        let carl = object(&server, &env.alice, org, "person", "Carl", json!({})).await;
        let dan = object(&server, &env.alice, org, "person", "Dan", json!({})).await;
        // Ann on an event of the case, the others undated.
        let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/events"), &env.alice, org, Some(json!({
            "title": "Transfer", "occurred": instant("2025-05-02T09:00:00Z"), "attach": [{"object_id": ann["id"]}]
        }))).await;
        assert_eq!(st, 201, "{w}");
        for o in [&phone, &bob, &dan] {
            let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{cid}/attach"), &env.alice, org, Some(json!({"object_id": o["id"]}))).await;
            assert_eq!(st, 200, "{w}");
        }
        // Links among held objects do not hold; a link outside or another case does.
        for body in [
            json!({"link_type": "uses_identifier", "source_id": ann["id"], "target_id": phone["id"]}),
            json!({"link_type": "knows", "source_id": bob["id"], "target_id": carl["id"]}),
        ] {
            let (st, l) = link(&server, &env.alice, org, body).await;
            assert_eq!(st, 201, "{l}");
        }
        let (st, w) = call(&server, "POST", &format!("/api/v1/cases/{}/attach", other["id"].as_str().unwrap()), &env.alice, org, Some(json!({"object_id": dan["id"]}))).await;
        assert_eq!(st, 200, "{w}");
        let url = format!("/api/v1/cases/{cid}/purge");
        let (st, cands) = call(&server, "GET", &url, &env.alice, org, None).await;
        assert_eq!(st, 200, "{cands}");
        let mut titles: Vec<&str> = cands.as_array().unwrap().iter().map(|c| c["title"].as_str().unwrap()).collect();
        titles.sort();
        assert_eq!(titles, ["Ann", "Ann phone"], "{cands}");
        let ids: Vec<Value> = cands.as_array().unwrap().iter().map(|c| c["id"].clone()).chain([bob["id"].clone()]).collect();

        // Frozen states: the one that names Ann goes with her, the other stays.
        let oid = other["id"].as_str().unwrap();
        for c in [cid.as_str(), oid] {
            let (st, s) = call(&server, "POST", &format!("/api/v1/cases/{c}/snapshots"), &env.alice, org, Some(json!({}))).await;
            assert_eq!(st, 201, "{s}");
        }

        // Only an archived case purges.
        let (st, e) = call(&server, "POST", &url, &env.alice, org, Some(json!({"ids": ids}))).await;
        assert_eq!((st, e["status"].as_str()), (400, Some("invalid")), "{e}");
        let upd = json!({"title": "Fraud", "properties": {"status": "archived", "legal_basis": "legitimate interest"}, "expected_version": case["version"]});
        let (st, v) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{cid}"), &env.alice, org, Some(upd)).await;
        assert_eq!(st, 200, "{v}");
        // Bob is not a candidate: asking for him purges nothing of his.
        let (st, r) = call(&server, "POST", &url, &env.alice, org, Some(json!({"ids": ids}))).await;
        assert_eq!((st, r["purged"].as_u64()), (200, Some(2)), "{r}");
        let (st, _) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}", ann["id"].as_str().unwrap()), &env.alice, org, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}", bob["id"].as_str().unwrap()), &env.alice, org, None).await;
        assert_eq!(st, 200);
        let (_, l) = call(&server, "GET", &format!("/api/v1/cases/{cid}/snapshots"), &env.alice, org, None).await;
        assert_eq!(l, json!([]), "the frozen state naming Ann is gone");
        let (_, l) = call(&server, "GET", &format!("/api/v1/cases/{oid}/snapshots"), &env.alice, org, None).await;
        assert_eq!(l.as_array().map(Vec::len), Some(1), "{l}");

        // R1: another org neither lists nor purges.
        let org_b = personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "GET", &url, &env.bob, org_b, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "POST", &url, &env.bob, org_b, Some(json!({"ids": []}))).await;
        assert_eq!(st, 404);
    })
    .await;
}
