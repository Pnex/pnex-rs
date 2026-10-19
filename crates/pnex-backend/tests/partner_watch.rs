//! Partner network watch, core bricks (partner-watch.md): natural keys
//! (D302), pack extensions of required and system types (D306), the
//! `entreprises_fr` and `quali_watch` packs and their hierarchy (D304,
//! D305), declarative imports with preview, application, snapshot
//! closing and roles (D301).
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

async fn install(server: &axum_test::TestServer, token: &str, org: i64, pack: &str) {
    let (st, r) = call(
        server,
        "POST",
        &format!("/api/v1/ontology/packs/{pack}/install"),
        token,
        org,
        None,
    )
    .await;
    assert_eq!(st, 200, "{r}");
}

async fn object(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    type_key: &str,
    title: &str,
    props: Value,
) -> (u16, Value) {
    call(
        server,
        "POST",
        "/api/v1/ontology/objects",
        token,
        org,
        Some(json!({"type_key": type_key, "title": title, "properties": props})),
    )
    .await
}

#[tokio::test]
#[serial]
async fn natural_keys_extensions_and_hierarchy() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        // D306: the dependencies came along, `organization` is extended.
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/organization", &env.alice, org, None).await;
        let siren = t["def"]["properties"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["key"] == "siren")
            .cloned()
            .expect("siren added by entreprises_fr");
        assert_eq!((siren["unique"].clone(), siren["pack"].clone()), (json!(true), json!("entreprises_fr")));
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/map_pin", &env.alice, org, None).await;
        assert!(t["def"]["properties"].as_array().unwrap().iter().any(|p| p["key"] == "siret"), "{t}");
        // Installing again (same version) changes nothing.
        install(&server, &env.alice, org, "entreprises_fr").await;
        // D316: the pack's rules and flow models, flows as drafts only.
        let count = |sql: String| {
            let db = env.db.clone();
            async move {
                db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql))
                    .await
                    .unwrap()
                    .unwrap()
                    .try_get::<i64>("", "n")
                    .unwrap()
            }
        };
        assert_eq!(count(format!("SELECT count(*)::bigint AS n FROM functions WHERE org_id = {org}")).await, 3);
        assert_eq!(
            count(format!("SELECT count(*)::bigint AS n FROM flows WHERE org_id = {org} AND status = 'draft'")).await,
            2
        );
        assert_eq!(count(format!("SELECT count(*)::bigint AS n FROM flows WHERE org_id = {org}")).await, 2);
        assert_eq!(count(format!("SELECT count(*)::bigint AS n FROM object_imports WHERE org_id = {org}")).await, 2);

        // D302: one live organization per SIREN.
        let (st, a) = object(&server, &env.alice, org, "organization", "ACME", json!({"siren": "123456789"})).await;
        assert_eq!(st, 201, "{a}");
        let (st, e) = object(&server, &env.alice, org, "organization", "ACME bis", json!({"siren": "123456789"})).await;
        assert_eq!(st, 400, "{e}");
        assert!(e.to_string().contains("duplicate"), "{e}");
        let (st, b) = object(&server, &env.alice, org, "organization", "Holding", json!({"siren": "987654321"})).await;
        assert_eq!(st, 201);
        // Updating the object itself keeps its own key.
        let aid = a["id"].as_str().unwrap();
        let (st, _) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{aid}"), &env.alice, org,
            Some(json!({"title": "ACME SAS", "properties": {"siren": "123456789"}, "expected_version": a["version"]}))).await;
        assert_eq!(st, 200);

        // Making `nda` unique while two objects share it: refused, listed.
        env.db
            .execute_unprepared(&format!(
                "UPDATE objects SET properties = properties || '{{\"nda\": \"11111111111\"}}' \
                 WHERE org_id = {org} AND type_key = 'organization'"
            ))
            .await
            .unwrap();
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/organization", &env.alice, org, None).await;
        let mut def = t["def"].clone();
        for p in def["properties"].as_array_mut().unwrap() {
            if p["key"] == "nda" {
                p["unique"] = json!(true);
            }
        }
        let (st, e) = call(&server, "PUT", "/api/v1/ontology/types/organization", &env.alice, org,
            Some(json!({"def": def, "expected_version": t["version"]}))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-unique-duplicates")), "{e}");
        assert_eq!(e["errors"]["args"]["values"], "11111111111");

        // D305: the group hierarchy follows `owns`, several parents shown.
        let bid = b["id"].as_str().unwrap();
        let (_, p) = object(&server, &env.alice, org, "person", "Jeanne Martin", json!({})).await;
        let pid = p["id"].as_str().unwrap();
        for body in [
            json!({"link_type": "owns", "source_id": bid, "target_id": aid}),
            json!({"link_type": "dirige", "source_id": pid, "target_id": aid, "attributes": {"role": "president"}}),
            json!({"link_type": "dirige", "source_id": pid, "target_id": bid, "attributes": {"role": "gerant"}}),
        ] {
            let (st, r) = call(&server, "POST", "/api/v1/ontology/links", &env.alice, org, Some(body)).await;
            assert_eq!(st, 201, "{r}");
        }
        let (st, h) = call(&server, "GET", &format!("/api/v1/ontology/objects/{bid}/hierarchy?key=groupe"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{h}");
        let titles: Vec<(String, u64)> = h["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (n["title"].as_str().unwrap().to_string(), n["depth"].as_u64().unwrap()))
            .collect();
        assert_eq!(titles[0], ("Holding".to_string(), 0));
        assert!(titles.contains(&("ACME SAS".to_string(), 1)), "{titles:?}");
        let jeanne: Vec<&Value> = h["nodes"].as_array().unwrap().iter().filter(|n| n["title"] == "Jeanne Martin").collect();
        assert_eq!(jeanne.len(), 2, "under the holding and under ACME: {h}");
        assert!(jeanne.iter().any(|n| n["also_under"] == json!(["ACME SAS"]) || n["also_under"] == json!(["Holding"])));
        assert!(h["hierarchies"].as_array().unwrap().iter().any(|x| x["key"] == "reseau"));
    })
    .await;
}

const MAPPING: &str = r#"
key: reseau
name: Partner network
objects:
  - as: org
    type: organization
    key: siren
    title: raison_sociale
    properties:
      siren: { from: siret, convert: [digits, "left:9"] }
  - as: cert
    type: certification
    key: code
    create: false
    properties:
      code: { from: code_certification, convert: [trim, upper] }
links:
  - type: habilite_pour
    from: org
    to: cert
    valid_from: { from: date_debut_habilitation, convert: ["date:%d/%m/%Y"] }
    attributes:
      perimetre: perimetre
      declaree_fc: { from: declaree_france_competences, convert: [{enum: {oui: "true", non: "false"}}] }
"#;

async fn preview(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: &str,
    csv: &str,
) -> Value {
    let res = server
        .post(&format!(
            "/api/v1/ontology/imports/{id}/dry-run?filename=reseau.csv"
        ))
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(csv.as_bytes().to_vec().into())
        .await;
    assert_eq!(res.status_code().as_u16(), 202, "{}", res.text());
    let run = res.json::<Value>()["id"].as_str().unwrap().to_string();
    settle(server, token, org, id, &run).await
}

/// Polls a run until it leaves `running`.
async fn settle(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: &str,
    run: &str,
) -> Value {
    for _ in 0..100 {
        let (st, r) = call(
            server,
            "GET",
            &format!("/api/v1/ontology/imports/{id}/runs/{run}"),
            token,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{r}");
        if r["run"]["state"] != "running" {
            return r;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("run {run} never settled");
}

async fn apply(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: &str,
    p: &Value,
    version: &Value,
) -> (u16, Value) {
    let run = p["run"]["id"].as_str().unwrap();
    let (st, r) = call(
        server,
        "POST",
        &format!("/api/v1/ontology/imports/{id}/apply"),
        token,
        org,
        Some(json!({"run_id": run, "expected_version": version})),
    )
    .await;
    if st != 202 {
        return (st, r);
    }
    (st, settle(server, token, org, id, run).await)
}

#[tokio::test]
#[serial]
async fn imports_preview_apply_and_close_missing_links() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        for code in ["RNCP12345", "RS6789"] {
            let (st, r) = object(&server, &env.alice, org, "certification", code, json!({"code": code})).await;
            assert_eq!(st, 201, "{r}");
        }
        // A mapping that names an unknown property is refused with its path.
        let bad = MAPPING.replace("siren: { from", "nope: { from");
        let (st, e) = call(&server, "POST", "/api/v1/ontology/imports", &env.alice, org,
            Some(json!({"name": "x", "mapping": bad, "mode": "snapshot"}))).await;
        assert_eq!(st, 400, "{e}");
        let (st, imp) = call(&server, "POST", "/api/v1/ontology/imports", &env.alice, org,
            Some(json!({"name": "Réseau", "mapping": MAPPING, "mode": "snapshot"}))).await;
        assert_eq!(st, 201, "{imp}");
        let id = imp["id"].as_str().unwrap().to_string();
        let v = imp["version"].clone();

        let csv = "siret;raison_sociale;code_certification;perimetre;date_debut_habilitation;declaree_france_competences\n\
                   123 456 789 00012;ACME;rncp12345;les_deux;01/02/2024;oui\n\
                   123456789 00012;ACME;RS6789;evaluation;01/03/2024;non\n\
                   987654321 00011;Beta;RS0000;preparation;01/03/2024;oui\n";
        // A preview with an unknown certification lists the error.
        let p = preview(&server, &env.alice, org, &id, csv).await;
        assert_eq!(p["run"]["state"], "preview", "{p}");
        assert_eq!(p["diff"]["errors"][0]["row"], 4, "{p}");
        assert_eq!(p["diff"]["errors"][0]["column"], "code_certification");
        assert_eq!(p["diff"]["errors"][0]["token"], "unknown_key");
        let (st, e) = apply(&server, &env.alice, org, &id, &p, &v).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("import-has-errors")), "{e}");

        let csv = csv.lines().take(3).collect::<Vec<_>>().join("\n");
        let p = preview(&server, &env.alice, org, &id, &csv).await;
        assert_eq!(p["run"]["counts"]["created"], 1, "{p}");
        assert_eq!(p["run"]["counts"]["links_opened"], 2, "{p}");
        // R2: a viewer previews nothing and applies nothing.
        personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org,
            Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st == 200 || st == 201);
        let (st, _) = apply(&server, &env.bob, org, &id, &p, &v).await;
        assert_eq!(st, 403);
        let (st, _) = call(&server, "DELETE", &format!("/api/v1/ontology/imports/{id}"), &env.bob, org, None).await;
        assert_eq!(st, 403);

        let (st, r) = apply(&server, &env.alice, org, &id, &p, &v).await;
        assert_eq!(st, 202, "{r}");
        assert_eq!(r["run"]["state"], "applied", "{r}");
        // Applying the same preview twice is refused.
        let (st, e) = apply(&server, &env.alice, org, &id, &p, &v).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("import-run-stale")));
        let row = env.db.query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT count(*)::bigint AS n FROM resource_edges WHERE org_id = {org} \
                     AND relation = 'habilite_pour' AND valid_to IS NULL AND source_ref LIKE 'import:{id}@%'"),
        )).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 2);

        // Same file again: nothing to do.
        let p = preview(&server, &env.alice, org, &id, &csv).await;
        let c = &p["run"]["counts"];
        assert_eq!((c["created"].clone(), c["updated"].clone(), c["links_opened"].clone(), c["closed"].clone()), (json!(0), json!(0), json!(0), json!(0)), "{p}");

        // A row gone from a snapshot closes its accreditation, a changed
        // scope updates the link, a new title updates the object.
        let csv2 = "siret;raison_sociale;code_certification;perimetre;date_debut_habilitation;declaree_france_competences\n\
                    123456789 00012;ACME Formation;RNCP12345;preparation;01/02/2024;oui\n";
        let p = preview(&server, &env.alice, org, &id, csv2).await;
        let c = &p["run"]["counts"];
        assert_eq!((c["closed"].clone(), c["links_updated"].clone(), c["updated"].clone()), (json!(1), json!(1), json!(1)), "{p}");
        assert_eq!(p["diff"]["closed"][0]["kind"], "link");
        let (_, r) = apply(&server, &env.alice, org, &id, &p, &v).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let row = env.db.query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT count(*)::bigint AS n FROM resource_edges WHERE org_id = {org} \
                     AND relation = 'habilite_pour' AND valid_to IS NOT NULL"),
        )).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 1, "closed, never deleted");

        // An import edited since the preview cannot apply it.
        let p = preview(&server, &env.alice, org, &id, &csv).await;
        let (st, imp2) = call(&server, "PUT", &format!("/api/v1/ontology/imports/{id}"), &env.alice, org,
            Some(json!({"name": "Réseau", "mapping": MAPPING, "mode": "incremental", "expected_version": v}))).await;
        assert_eq!(st, 200, "{imp2}");
        let (st, _) = apply(&server, &env.alice, org, &id, &p, &imp2["version"]).await;
        assert_eq!(st, 409);
        let (_, runs) = call(&server, "GET", &format!("/api/v1/ontology/imports/{id}/runs"), &env.alice, org, None).await;
        assert!(runs["count"].as_i64().unwrap() >= 5, "{runs}");
    })
    .await;
}

/// A deployed flow of `org` with the three ontology nodes; returns its id.
async fn deploy_ontology_nodes(db: &DatabaseConnection, org: i64) -> i64 {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, Set};
    let flow = flows::ActiveModel {
        name: Set("watch".into()),
        status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
        org_id: Set(org),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let version = flow_versions::ActiveModel {
        flow_id: Set(flow.id),
        version_number: Set(1),
        graph: Set(json!({ "nodes": [
            {"id": "q", "kind": "ontology_query", "config": {"object_type": "certification",
              "filters": [{"key": "code", "op": "in", "value": "$msg.payload.codes"}]}},
            {"id": "u", "kind": "object_upsert", "config": {"object_type": "organization", "key": "siren"}},
            {"id": "l", "kind": "link_upsert", "config": {"link_type": "habilite_pour"}}
        ] })),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    let id = flow.id;
    let mut am: flows::ActiveModel = flow.into();
    am.deployed_version_id = Set(Some(version.id));
    am.update(db).await.unwrap();
    id
}

async fn internal(server: &axum_test::TestServer, path: &str, body: Value) -> (u16, Value) {
    let res = server
        .post(&format!("/internal/flow/ontology/{path}"))
        .add_header("x-pnex-flow-token", "tok-pw-1")
        .json(&body)
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

#[tokio::test]
#[serial]
async fn flow_nodes_write_as_their_deployed_node() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        let (_, cert) = object(&server, &env.alice, org, "certification", "RNCP12345", json!({"code": "RNCP12345"})).await;
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-pw-1") };
        let flow = deploy_ontology_nodes(&env.db, org).await;
        let node = |id: &str| json!({"org_id": org, "flow_id": flow, "version": 1, "node_id": id});

        // The node's config comes from the deployed version: a node absent
        // from it is refused, whatever the request says.
        let (st, e) = internal(&server, "upsert", json!({"node": node("nope"), "properties": {"siren": "1"}})).await;
        assert_eq!((st, e["code"].as_str()), (403, Some("flow-node-not-deployed")));
        let (st, _) = internal(&server, "upsert", json!({"node": node("q"), "properties": {"siren": "1"}})).await;
        assert_eq!(st, 403, "a query node cannot write");

        let (st, a) = internal(&server, "upsert", json!({"node": node("u"), "title": "ACME", "properties": {"siren": "123456789"}})).await;
        assert_eq!((st, a["created"].clone()), (200, json!(true)), "{a}");
        assert_eq!(a["object"]["source_ref"], format!("flow:{flow}@1"));
        let (st, b) = internal(&server, "upsert", json!({"node": node("u"), "properties": {"siren": "123456789", "nda": "11111111111"}})).await;
        assert_eq!((st, b["created"].clone()), (200, json!(false)), "{b}");
        assert_eq!(b["object"]["id"], a["object"]["id"]);
        assert_eq!(b["object"]["title"], "ACME");
        assert_eq!(b["object"]["properties"]["nda"], "11111111111");
        // Same validations as the UI: an unknown property is refused.
        let (st, _) = internal(&server, "upsert", json!({"node": node("u"), "properties": {"siren": "123456789", "nope": 1}})).await;
        assert_eq!(st, 400);

        let link = json!({"node": node("l"), "source_id": a["object"]["id"], "target_id": cert["id"],
                          "attributes": {"perimetre": "evaluation"}});
        let (st, l1) = internal(&server, "link", link.clone()).await;
        assert_eq!(st, 200, "{l1}");
        let mut again = link.clone();
        again["attributes"] = json!({"perimetre": "les_deux"});
        let (st, l2) = internal(&server, "link", again).await;
        assert_eq!((st, l2["id"].clone()), (200, l1["id"].clone()), "an open link is updated, not doubled");
        assert_eq!(l2["attributes"]["perimetre"], "les_deux");

        let (st, r) = internal(&server, "query", json!({"node": node("q"), "values": [["RNCP12345", "RS1"]]})).await;
        assert_eq!((st, r["count"].clone()), (200, json!(1)), "{r}");
        assert_eq!(r["rows"][0]["object"]["title"], "RNCP12345");
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}

async fn smtp_channel(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
    dynamic: bool,
) -> (u16, Value) {
    call(
        server,
        "POST",
        "/api/v1/notify/channels",
        token,
        org,
        Some(
            json!({"kind": "smtp", "name": name, "enabled": true, "config": {
                "host": "smtp.example.com", "from": "Certif <conformite@example.com>",
                "to": "reseau@example.com", "dynamic_recipients": dynamic
            }}),
        ),
    )
    .await
}

#[tokio::test]
#[serial]
async fn message_recipients_need_an_admin_flag_and_respect_the_caps() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        // A member cannot turn recipients from messages on.
        personal_org(&server, &env.bob).await;
        call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org}/members"),
            &env.alice,
            org,
            Some(json!({"email": "bob@example.com", "role": "member"})),
        )
        .await;
        let (st, e) = smtp_channel(&server, &env.bob, org, "bob", true).await;
        assert_eq!(
            (st, e["error"].as_str()),
            (403, Some("notify-dynamic-recipients-admin")),
            "{e}"
        );
        let (st, dynamic) = smtp_channel(&server, &env.alice, org, "relances", true).await;
        assert_eq!(st, 201, "{dynamic}");
        let (_, fixed) = smtp_channel(&server, &env.alice, org, "fixe", false).await;

        let (dyn_id, fixed_id) = (
            dynamic["id"].as_str().unwrap().to_string(),
            fixed["id"].as_str().unwrap().to_string(),
        );
        use pnex_backend::models::_entities::{flow_versions, flows};
        use sea_orm::{ActiveModelTrait, Set};
        let flow = flows::ActiveModel {
            name: Set("relances".into()),
            status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
            org_id: Set(org),
            ..Default::default()
        }
        .insert(&env.db)
        .await
        .unwrap();
        let version = flow_versions::ActiveModel {
            flow_id: Set(flow.id),
            version_number: Set(1),
            graph: Set(json!({ "nodes": [
                {"id": "n", "kind": "pnex_notify", "config": {"channel_ids": [dyn_id, fixed_id],
                  "template_id": "00000000-0000-0000-0000-000000000001", "to_field": "contact"}}
            ] })),
            ..Default::default()
        }
        .insert(&env.db)
        .await
        .unwrap();
        let mut am: flows::ActiveModel = flow.clone().into();
        am.deployed_version_id = Set(Some(version.id));
        am.update(&env.db).await.unwrap();

        unsafe { std::env::set_var("PNEX_NOTIFY_INTERNAL_TOKEN", "tok-notify-pw") };
        let ask = |channel: &str, to: &str| {
            json!({"org_id": org, "flow_id": flow.id, "version": 1, "node_id": "n",
                   "channel_id": channel, "to": to})
        };
        let post = |body: Value| {
            let server = &server;
            async move {
                let res = server
                    .post("/internal/notify/recipient")
                    .add_header("x-pnex-internal-token", "tok-notify-pw")
                    .json(&body)
                    .await;
                let st = res.status_code().as_u16();
                (
                    st,
                    serde_json::from_str::<Value>(&res.text()).unwrap_or(Value::Null),
                )
            }
        };
        for _ in 0..3 {
            let (st, _) = post(ask(&dyn_id, "Contact@Partenaire.fr")).await;
            assert_eq!(st, 204);
        }
        let (st, e) = post(ask(&dyn_id, "contact@partenaire.fr")).await;
        assert_eq!(
            (st, e["code"].as_str()),
            (429, Some("notify-recipient-daily-cap"))
        );
        let (st, e) = post(ask(&dyn_id, "a@b.fr, c@d.fr")).await;
        assert_eq!(
            (st, e["code"].as_str()),
            (400, Some("notify-recipient-invalid"))
        );
        let (st, e) = post(ask(&fixed_id, "other@partenaire.fr")).await;
        assert_eq!(
            (st, e["code"].as_str()),
            (403, Some("notify-dynamic-recipients-off"))
        );
        let mut forged = ask(&dyn_id, "x@partenaire.fr");
        forged["node_id"] = json!("other");
        let (st, _) = post(forged).await;
        assert_eq!(st, 403);
        // Only hashes are stored.
        let row = env
            .db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT recipient_hash FROM notify_dynamic_sends LIMIT 1".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        let h: String = row.try_get("", "recipient_hash").unwrap();
        assert_eq!(h, pnex_notify::recipient_hash("contact@partenaire.fr"));
        unsafe { std::env::remove_var("PNEX_NOTIFY_INTERNAL_TOKEN") };
    })
    .await;
}
