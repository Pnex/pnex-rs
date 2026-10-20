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
            3
        );
        assert_eq!(count(format!("SELECT count(*)::bigint AS n FROM flows WHERE org_id = {org}")).await, 3);
        // SIRENE, the partner sheet and the five France Compétences imports.
        assert_eq!(count(format!("SELECT count(*)::bigint AS n FROM object_imports WHERE org_id = {org}")).await, 7);

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

// ───────────── open-data ingestion (ingestion.md D317–D322) ─────────────

const FC_STANDARD: &str = "\"Id_Fiche\";\"Numero_Fiche\";\"Intitule\";\"Nomenclature_Europe_Niveau\";\"Date_Decision\";\"Date_Fin_Enregistrement\";\"Date_Effet\";\"Actif\"\n\
    \"1\";\"RS6765\";\"Télépilote de drone\";\"\";\"15/01/2024\";\"15/01/2027\";\"\";\"ACTIVE\"\n\
    \"2\";\"RS6766\";\"Télépilote confirmé\";\"\";\"16/01/2024\";\"16/01/2027\";\"\";\"INACTIVE\"\n\
    \"3\";\"RNCP1\";\"Autre\";\"NIV6\";\"01/01/2020\";\"01/01/2025\";\"01/02/2020\";\"ACTIVE\"\n";
const FC_CERTIFIERS: &str = "\"Numero_Fiche\";\"Siret_Certificateur\";\"Nom_Certificateur\"\n\
    \"RS6765\";\"\t80259488700019\";\"TELEPILOTE\"\n\
    \"RS6766\";\"\t80259488700019\";\"TELEPILOTE\"\n\
    \"RNCP1\";\"\t13002526500013\";\"AUTRE\"\n";

/// Partner rows; `drop` removes those whose SIREN starts with it.
fn fc_partners(drop: &[&str]) -> String {
    let rows = [
        (
            "RS6765",
            "PARTENAIRE UN",
            "11111111800001",
            "HABILITATION_ORGA_FORM",
        ),
        (
            "RS6765",
            "PARTENAIRE UN",
            "11111111800019",
            "HABILITATION_ORGA_FORM",
        ),
        (
            "RS6765",
            "PARTENAIRE DEUX",
            "22222222600003",
            "HABILITATION_FORMER",
        ),
        (
            "RS6765",
            "PARTENAIRE DEUX",
            "22222222600011",
            "HABILITATION_ORGANISER",
        ),
        (
            "RS6766",
            "PARTENAIRE TROIS",
            "33333333400006",
            "HABILITATION_ORGANISER",
        ),
        ("RNCP1", "AILLEURS", "13002526500013", "HABILITATION_FORMER"),
    ];
    let mut out = String::from(
        "\"Numero_Fiche\";\"Nom_Partenaire\";\"Siret_Partenaire\";\"Habilitation_Partenaire\"\n",
    );
    for (fiche, nom, siret, hab) in rows {
        if !drop.iter().any(|d| siret.starts_with(d)) {
            out.push_str(&format!("\"{fiche}\";\"{nom}\";\"\t{siret}\";\"{hab}\"\n"));
        }
    }
    out
}

fn fc_zip(partners: &str) -> Vec<u8> {
    use std::io::Write;
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in [
        (
            "export_fiches_CSV_Ancienne_Nouvelle_Certification_2026_10_20.csv",
            "Numero_Fiche\n",
        ),
        ("export_fiches_CSV_Standard_2026_10_20.csv", FC_STANDARD),
        (
            "export_fiches_CSV_Certificateurs_2026_10_20.csv",
            FC_CERTIFIERS,
        ),
        ("export_fiches_CSV_Partenaires_2026_10_20.csv", partners),
    ] {
        w.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(body.as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// data.gouv.fr stand-in: the dataset (an older and the current export)
/// and the current zip, which the test swaps.
async fn mock_datagouv(zip: std::sync::Arc<std::sync::Mutex<Vec<u8>>>) -> String {
    use axum::routing::get;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let dataset = json!({"title": "Répertoires", "resources": [
        {"title": "export-fiches-csv-2026-10-19.zip", "url": format!("{base}/files/old.zip"),
         "format": "zip", "last_modified": "2026-10-19T03:00:00+00:00"},
        {"title": "export-fiches-csv-2026-10-20.zip", "url": format!("{base}/files/export-fiches-csv-2026-10-20.zip"),
         "format": "zip", "last_modified": "2026-10-20T03:00:00.000000"},
        {"title": "export-fiches-xml-2026-10-21.zip", "url": format!("{base}/files/x.zip"),
         "format": "zip", "last_modified": "2026-10-21T03:00:00+00:00"}
    ]});
    let app = axum::Router::new()
        .route(
            "/api/1/datasets/fc/",
            get(move || async move { axum::Json(dataset) }),
        )
        .route(
            "/files/export-fiches-csv-2026-10-20.zip",
            get(move || async move { zip.lock().unwrap().clone() }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

async fn ingest(server: &axum_test::TestServer, node: Value, msg: &Value) -> Value {
    let res = server
        .post("/internal/flow/ingest")
        .add_header("x-pnex-flow-token", "tok-pw-1")
        .json(&json!({"node": node, "msg": msg}))
        .await;
    assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
    res.json::<Value>()
}

fn count(db_row: Option<sea_orm::QueryResult>) -> i64 {
    db_row.unwrap().try_get::<i64>("", "n").unwrap()
}

#[tokio::test]
#[serial]
async fn open_data_ingestion_chain() {
    with_app(|server, env| async move {
        use sea_orm::Statement;
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        let zip = std::sync::Arc::new(std::sync::Mutex::new(fc_zip(&fc_partners(&[]))));
        let base = mock_datagouv(zip.clone()).await;
        unsafe { std::env::set_var("PNEX_DATAGOUV_API_URL", format!("{base}/api/1")) };
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-pw-1") };
        let q = |sql: String| env.db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql));

        // The pack ships the flow as a draft, its import nodes resolved by name.
        let row = q(format!("SELECT f.id, v.id AS vid, v.graph FROM flows f JOIN flow_versions v ON v.flow_id = f.id \
            WHERE f.org_id = {org} AND f.name = 'Ingestion France Compétences' ORDER BY v.version_number DESC LIMIT 1"))
            .await.unwrap().expect("pack flow");
        let (flow, vid) = (row.try_get::<i64>("", "id").unwrap(), row.try_get::<i64>("", "vid").unwrap());
        let mut graph = row.try_get::<Value>("", "graph").unwrap();
        let nodes = graph["nodes"].as_array_mut().unwrap();
        for n in nodes.iter_mut() {
            if n["kind"] == "ingest" && n["config"]["step"] == "ontology_import" {
                assert!(n["config"]["import_id"].as_str().is_some_and(|s| s.len() == 36), "{n}");
            }
            // Q-I1: the client certifier's SIRET, set by the operator.
            if n["id"] == "filter_c" {
                n["config"]["values"] = json!(["80259488700019"]);
            }
            if n["id"] == "resolve" {
                n["config"]["dataset"] = json!("fc");
            }
        }
        let version = q(format!("SELECT version_number AS n FROM flow_versions WHERE id = {vid}"))
            .await.unwrap().unwrap().try_get::<i64>("", "n").unwrap();
        env.db.execute_raw(Statement::from_sql_and_values(sea_orm::DatabaseBackend::Postgres,
            "UPDATE flow_versions SET graph = $1 WHERE id = $2", vec![graph.into(), vid.into()])).await.unwrap();
        env.db.execute_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
            format!("UPDATE flows SET status = 'deployed', deployed_version_id = {vid} WHERE id = {flow}"))).await.unwrap();
        let node = |id: &str| json!({"org_id": org, "flow_id": flow, "version": version, "node_id": id});

        // One pass of the chain, as the runtime wires it; returns the
        // replies by node.
        let chain = [
            "resolve", "download", "extract", "inspect_c", "filter_c", "import_c",
            "inspect_s", "filter_s", "import_s", "inspect_p", "filter_p", "import_p", "cleanup",
        ];
        let run = |stop_on_port: bool| {
            let server = &server;
            async move {
                let mut msg = json!({});
                let mut replies = std::collections::HashMap::new();
                for id in chain {
                    let r = ingest(server, node(id), &msg).await;
                    msg = r["msg"].clone();
                    let port = r["port"].as_u64().unwrap();
                    replies.insert(id, r);
                    if stop_on_port && port != 0 {
                        break;
                    }
                }
                replies
            }
        };

        let r = run(true).await;
        for id in chain {
            assert_eq!(r[id]["port"], 0, "{id}: {}", r[id]);
        }
        assert!(r["resolve"]["manifest"]["url"].as_str().unwrap().ends_with("export-fiches-csv-2026-10-20.zip"));
        let dl = &r["download"]["manifest"];
        assert_eq!(dl["sha256"].as_str().unwrap().len(), 64, "{dl}");
        assert!(dl["staging_key"].as_str().unwrap().starts_with(&format!("org_{org}/staging/{flow}/")));
        let x = &r["extract"]["manifest"];
        assert_eq!((x["format"].clone(), x["total"].clone(), x["selected"].clone()), (json!("zip"), json!(4), json!(3)), "{x}");
        assert_eq!(r["inspect_c"]["manifest"]["rows"], 3);
        assert_eq!(r["inspect_c"]["manifest"]["columns"][1]["name"], "Siret_Certificateur");
        assert_eq!((r["filter_c"]["manifest"]["rows_in"].clone(), r["filter_c"]["manifest"]["rows_out"].clone()), (json!(3), json!(2)));
        assert_eq!(r["import_c"]["manifest"]["state"], "applied", "{}", r["import_c"]);
        assert_eq!(r["filter_s"]["manifest"]["rows_out"], 2, "only the client's sheets");
        assert_eq!(r["filter_p"]["manifest"]["rows_out"], 5, "{}", r["filter_p"]);
        assert_eq!(r["import_p"]["manifest"]["state"], "applied", "{}", r["import_p"]);
        assert!(r["cleanup"]["manifest"]["deleted_keys"].as_array().unwrap().len() >= 5, "{}", r["cleanup"]);

        // The ontology: certifications of the client with their sheet,
        // its `porte` links, the partners' accreditations and scopes.
        let (_, certs) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org,
            Some(json!({"type_key": "certification", "limit": 10}))).await;
        let by_code = |code: &str| certs["rows"].as_array().unwrap().iter()
            .find(|r| r["object"]["properties"]["code"] == code).cloned().unwrap_or(Value::Null);
        assert_eq!(certs["count"], 2, "{certs}");
        let rs = &by_code("RS6765")["object"]["properties"];
        assert_eq!((rs["repertoire"].clone(), rs["statut"].clone()), (json!("rs"), json!("actif")), "{rs}");
        assert_eq!(rs["intitule"], "Télépilote de drone");
        assert_eq!(rs["date_enregistrement"], "2024-01-15", "Date_Decision when Date_Effet is empty");
        assert_eq!(by_code("RS6766")["object"]["properties"]["statut"], "inactif");
        let links = |relation: &str| format!("SELECT count(*)::bigint AS n FROM resource_edges WHERE org_id = {org} \
            AND relation = '{relation}' AND valid_to IS NULL");
        assert_eq!(count(q(links("porte")).await.unwrap()), 2);
        assert_eq!(count(q(links("habilite_pour")).await.unwrap()), 3, "two SIRET of one SIREN, one link");
        let scope = q(format!("SELECT e.attributes AS a FROM resource_edges e JOIN objects o ON o.id = e.source_object_id \
            WHERE e.org_id = {org} AND e.relation = 'habilite_pour' AND o.properties ->> 'siren' = '333333334'"))
            .await.unwrap().unwrap().try_get::<Value>("", "a").unwrap();
        assert_eq!((scope["perimetre"].clone(), scope["declaree_fc"].clone()), (json!("evaluation"), json!(true)), "{scope}");
        // Two establishments of one SIREN, one preparing, one assessing:
        // one link covering both, whatever the row order (Q-I2).
        let both = q(format!("SELECT e.attributes AS a FROM resource_edges e JOIN objects o ON o.id = e.source_object_id \
            WHERE e.org_id = {org} AND e.relation = 'habilite_pour' AND o.properties ->> 'siren' = '222222226'"))
            .await.unwrap().unwrap().try_get::<Value>("", "a").unwrap();
        assert_eq!(both["perimetre"], "les_deux", "{both}");

        // The node panel reads the journal.
        let (st, steps) = call(&server, "GET", &format!("/api/v1/flows/{flow}/ingest-steps?node_id=extract"), &env.alice, org, None).await;
        assert_eq!((st, steps["count"].clone()), (200, json!(1)), "{steps}");
        assert_eq!(steps["results"][0]["manifest"]["entries"][0]["selected"], false);

        // Same file the next night: unchanged, nothing written.
        let r2 = run(true).await;
        assert_eq!(r2["download"]["port"], 2, "{}", r2["download"]);
        assert!(!r2.contains_key("extract"));

        // A partner gone from the open data: its accreditation closes.
        *zip.lock().unwrap() = fc_zip(&fc_partners(&["333"]));
        let r3 = run(true).await;
        assert_eq!(r3["import_p"]["manifest"]["closed"], 1, "{}", r3["import_p"]);
        assert_eq!(count(q(links("habilite_pour")).await.unwrap()), 2);

        // Most of the network gone at once: refused, nothing closed.
        *zip.lock().unwrap() = fc_zip(&fc_partners(&["111", "222"]));
        let r4 = run(true).await;
        assert_eq!(r4["import_p"]["port"], 1, "{}", r4["import_p"]);
        assert_eq!(r4["import_p"]["code"], "ingest-suspicious-closures");
        assert_eq!(count(q(links("habilite_pour")).await.unwrap()), 2);

        // A changed shape stops the chain before any import.
        *zip.lock().unwrap() = fc_zip(&fc_partners(&[]).replace("Habilitation_Partenaire", "Habilitation"));
        let r5 = run(true).await;
        assert_eq!(r5["inspect_p"]["code"], "import-schema-changed", "{}", r5["inspect_p"]);
        assert!(!r5.contains_key("import_p"));

        // A forged node is refused; a key of another org is never read.
        let res = server.post("/internal/flow/ingest").add_header("x-pnex-flow-token", "tok-pw-1")
            .json(&json!({"node": node("nope"), "msg": {}})).await;
        assert_eq!(res.status_code().as_u16(), 403);
        let r = ingest(&server, node("inspect_c"), &json!({"staging_key": "org_999999/staging/1/x/raw/a.csv"})).await;
        assert_eq!((r["port"].clone(), r["code"].clone()), (json!(1), json!("ingest-entry-missing")), "{r}");

        // The purge leaves live runs alone and empties what is expired.
        env.db.execute_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
            format!("UPDATE ingest_steps SET created_at = now() - interval '8 days', keep_until = NULL WHERE org_id = {org}"))).await.unwrap();
        let store = pnex_backend::services::artifact_store::S3Config::from_config(
            &loco_rs::config::Config::new(&loco_rs::environment::Environment::Test).unwrap()).store().unwrap();
        pnex_backend::services::ingest::purge(&env.db, &store).await.unwrap();
        assert_eq!(count(q(format!("SELECT count(*)::bigint AS n FROM ingest_steps WHERE org_id = {org} \
            AND purged_at IS NULL AND bytes > 0")).await.unwrap()), 0);
        assert!(store.list_prefix(&format!("org_{org}/staging/")).await.unwrap().is_empty());
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
        unsafe { std::env::remove_var("PNEX_DATAGOUV_API_URL") };
    })
    .await;
}

/// The shape of the RS XML export (v4.1); partner names and SIRET made up.
const RS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<FICHES>
    <VERSION_FLUX>4.1</VERSION_FLUX>
    <FICHE>
        <NUMERO_FICHE>RS7511</NUMERO_FICHE>
        <INTITULE>Développer son activité avec le référencement naturel</INTITULE>
        <PARTENAIRES>
            <PARTENAIRE>
                <NOM_PARTENAIRE>PARTENAIRE A</NOM_PARTENAIRE>
                <SIRET_PARTENAIRE>11111111100011</SIRET_PARTENAIRE>
                <HABILITATION_PARTENAIRE>HABILITATION_ORGA_FORM</HABILITATION_PARTENAIRE>
                <ETAT_HABILITATION>Inactif</ETAT_HABILITATION>
                <DATE_DERNIERE_MODIFICATION_ETAT>04/08/2026</DATE_DERNIERE_MODIFICATION_ETAT>
            </PARTENAIRE>
            <PARTENAIRE>
                <NOM_PARTENAIRE>PARTENAIRE A</NOM_PARTENAIRE>
                <SIRET_PARTENAIRE>11111111100029</SIRET_PARTENAIRE>
                <HABILITATION_PARTENAIRE>HABILITATION_ORGA_FORM</HABILITATION_PARTENAIRE>
                <ETAT_HABILITATION>Actif</ETAT_HABILITATION>
                <DATE_ACTIF>28/01/2026</DATE_ACTIF>
                <DATE_DERNIERE_MODIFICATION_ETAT>17/03/2026</DATE_DERNIERE_MODIFICATION_ETAT>
            </PARTENAIRE>
            <PARTENAIRE>
                <NOM_PARTENAIRE>PARTENAIRE B</NOM_PARTENAIRE>
                <SIRET_PARTENAIRE>22222222200012</SIRET_PARTENAIRE>
                <HABILITATION_PARTENAIRE>HABILITATION_FORMER</HABILITATION_PARTENAIRE>
                <ETAT_HABILITATION>Inactif</ETAT_HABILITATION>
                <DATE_DERNIERE_MODIFICATION_ETAT>12/05/2026</DATE_DERNIERE_MODIFICATION_ETAT>
            </PARTENAIRE>
            <PARTENAIRE>
                <NOM_PARTENAIRE>PARTENAIRE C</NOM_PARTENAIRE>
                <SIRET_PARTENAIRE>33333333300012</SIRET_PARTENAIRE>
                <HABILITATION_PARTENAIRE>HABILITATION_FORMER</HABILITATION_PARTENAIRE>
                <ETAT_HABILITATION>Actif</ETAT_HABILITATION>
                <DATE_DERNIERE_MODIFICATION_ETAT>12/05/2026</DATE_DERNIERE_MODIFICATION_ETAT>
            </PARTENAIRE>
        </PARTENAIRES>
        <CAPACITES_ATTESTEES>Cadrer le projet de référencement naturel de son site internet (cible, objectifs…), afin de fixer des objectifs marketing.   Réaliser un audit des intentions de recherches de sa cible.    Optimiser les différents contenus web de son site (titles, metas, URL).   Élaborer une stratégie éditoriale orientée référencement.   Mettre en place une stratégie de netlinking.   Auditer les résultats de sa stratégie de référencement naturel.</CAPACITES_ATTESTEES>
        <SI_JURY_FI>
            <ACTIF>Non</ACTIF>
        </SI_JURY_FI>
        <SI_JURY_FC>
            <ACTIF>Oui</ACTIF>
            <COMPOSITION>Le jury de certification est composé de trois personnes.</COMPOSITION>
        </SI_JURY_FC>
        <STATISTIQUES_PROMOTIONS>
            <STATISTIQUES_PROMOTION>
                <ANNEE>2023</ANNEE>
                <NOMBRE_CERTIFIES>4</NOMBRE_CERTIFIES>
            </STATISTIQUES_PROMOTION>
            <STATISTIQUES_PROMOTION>
                <ANNEE>2024</ANNEE>
                <NOMBRE_CERTIFIES>64</NOMBRE_CERTIFIES>
            </STATISTIQUES_PROMOTION>
        </STATISTIQUES_PROMOTIONS>
        <LIEN_URL_DESCRIPTION>▷ Certification « Développer son activité avec le référencement naturel »           </LIEN_URL_DESCRIPTION>
        <OBJECTIFS_CONTEXTE>La certification s’adresse aux dirigeants de petites entreprises.    La certification permet de structurer la démarche.</OBJECTIFS_CONTEXTE>
        <ACTIF>Oui</ACTIF>
        <PREREQUIS_ENTREE_FORMATION>Un site internet ou un projet de site.</PREREQUIS_ENTREE_FORMATION>
    </FICHE>
    <FICHE>
        <NUMERO_FICHE>RS56</NUMERO_FICHE>
        <PARTENAIRES>
            <PARTENAIRE>
                <SIRET_PARTENAIRE>22222222200012</SIRET_PARTENAIRE>
                <ETAT_HABILITATION>Inactif</ETAT_HABILITATION>
            </PARTENAIRE>
        </PARTENAIRES>
        <CAPACITES_ATTESTEES>aptitude au vol aux instruments    connaissances dans les matières énumérées ci-dessus</CAPACITES_ATTESTEES>
        <STATISTIQUES_PROMOTIONS>
            <STATISTIQUES_PROMOTION>
                <ANNEE>2024</ANNEE>
                <NOMBRE_CERTIFIES>12</NOMBRE_CERTIFIES>
            </STATISTIQUES_PROMOTION>
        </STATISTIQUES_PROMOTIONS>
    </FICHE>
</FICHES>
"#;

const BLOCS_CSV: &str =
    "\"Numero_Fiche\";\"Bloc_Competences_Code\";\"Bloc_Competences_Libelle\"\r\n\
    \"RNCP183\";\"RNCP183BC01\";\"Monter et mettre en service des installations frigorifiques\"\r\n\
    \"RNCP183\";\"RNCP183BC02\";\"Assurer la maintenance des installations frigorifiques\"\r\n";

/// The pack import of this name.
async fn import_named(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    name: &str,
) -> (String, Value) {
    let (st, list) = call(
        server,
        "GET",
        "/api/v1/ontology/imports?limit=50",
        token,
        org,
        None,
    )
    .await;
    assert_eq!(st, 200, "{list}");
    let imp = list["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == name)
        .unwrap_or_else(|| panic!("{name}: {list}"));
    (
        imp["id"].as_str().unwrap().to_string(),
        imp["version"].clone(),
    )
}

async fn props_of(
    db: &DatabaseConnection,
    org: i64,
    type_key: &str,
    key: &str,
    value: &str,
) -> Value {
    db.query_one_raw(sea_orm::Statement::from_string(
        sea_orm::DatabaseBackend::Postgres,
        format!(
            "SELECT properties FROM objects WHERE org_id = {org} AND type_key = '{type_key}' \
                 AND valid_to IS NULL AND properties ->> '{key}' = '{value}'"
        ),
    ))
    .await
    .unwrap()
    .map(|r| r.try_get::<Value>("", "properties").unwrap())
    .unwrap_or(Value::Null)
}

#[tokio::test]
#[serial]
async fn france_competences_sheet_content() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        let mut ids = std::collections::HashMap::new();
        for (code, props) in [("RS7511", json!({"code": "RS7511"})), ("RNCP183", json!({"code": "RNCP183"}))] {
            let (st, r) = object(&server, &env.alice, org, "certification", code, props).await;
            assert_eq!(st, 201, "{r}");
            ids.insert(code, r["id"].as_str().unwrap().to_string());
        }
        for (name, siren) in [("A", "111111111"), ("B", "222222222"), ("C", "333333333")] {
            let (st, r) = object(&server, &env.alice, org, "organization", name, json!({"siren": siren})).await;
            assert_eq!(st, 201, "{r}");
            ids.insert(siren, r["id"].as_str().unwrap().to_string());
        }
        // Accreditations opened by fc_partenaires: A and B, not C.
        for siren in ["111111111", "222222222"] {
            let (st, r) = call(&server, "POST", "/api/v1/ontology/links", &env.alice, org,
                Some(json!({"link_type": "habilite_pour", "source_id": ids[siren], "target_id": ids["RS7511"]}))).await;
            assert_eq!(st, 201, "{r}");
        }

        // RNCP skill blocks.
        let (id, v) = import_named(&server, &env.alice, org, "France Compétences · skill blocks").await;
        let p = preview(&server, &env.alice, org, &id, BLOCS_CSV).await;
        assert_eq!((p["run"]["counts"]["created"].clone(), p["run"]["counts"]["links_opened"].clone()), (json!(2), json!(2)), "{p}");
        let (_, r) = apply(&server, &env.alice, org, &id, &p, &v).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let b = props_of(&env.db, org, "competence", "cle", "RNCP183BC02").await;
        assert_eq!((b["rang"].as_f64(), b["nature"].clone()), (Some(2.0), json!("bloc")), "{b}");

        // RS sheets: RS56 is not one of the org's, nothing of it is written.
        let (id, v) = import_named(&server, &env.alice, org, "France Compétences · RS sheets (XML)").await;
        let p = preview(&server, &env.alice, org, &id, RS_XML).await;
        assert_eq!(p["run"]["state"], "preview", "{p}");
        assert!(p["diff"]["errors"].as_array().unwrap().is_empty(), "{p}");
        let c = &p["run"]["counts"];
        // Six competencies and their links, two accreditations completed
        // (C has none: none is opened), RS7511 enriched.
        assert_eq!((c["created"].clone(), c["links_opened"].clone(), c["links_updated"].clone(), c["updated"].clone()),
            (json!(6), json!(6), json!(2), json!(1)), "{p}");
        let (_, r) = apply(&server, &env.alice, org, &id, &p, &v).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let c3 = props_of(&env.db, org, "competence", "cle", "RS7511-C3").await;
        assert_eq!(c3["rang"].as_f64(), Some(3.0), "{c3}");
        assert!(c3["libelle"].as_str().unwrap().starts_with("Optimiser les différents contenus"), "{c3}");
        assert_eq!(props_of(&env.db, org, "competence", "cle", "RS56-C1").await, Value::Null);
        let cert = props_of(&env.db, org, "certification", "code", "RS7511").await;
        assert_eq!(cert["nb_certifies"].as_f64(), Some(64.0), "{cert}");
        assert_eq!(cert["jury"], json!("Le jury de certification est composé de trois personnes."), "{cert}");
        assert!(cert["objectifs"].as_str().unwrap().starts_with("La certification s’adresse"), "{cert}");
        let edges = env.db.query_all_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT source_id, attributes FROM resource_edges WHERE org_id = {org} \
                     AND relation = 'habilite_pour' AND valid_to IS NULL ORDER BY source_id"),
        )).await.unwrap();
        let states: Vec<(String, Value, Value)> = edges.iter().map(|r| {
            let a = r.try_get::<Value>("", "attributes").unwrap();
            (r.try_get::<String>("", "source_id").unwrap(), a["etat_fc"].clone(), a["date_etat"].clone())
        }).collect();
        assert_eq!(states.len(), 2, "no accreditation opened for C: {states:?}");
        // A: one SIRET still active wins over the inactive one.
        assert_eq!((states[0].1.clone(), states[1].1.clone()), (json!("actif"), json!("inactif")), "{states:?}");
        assert_eq!(states[1].2, json!("2026-05-12"));

        // Same file again: nothing to do.
        let p = preview(&server, &env.alice, org, &id, RS_XML).await;
        let c = &p["run"]["counts"];
        assert_eq!((c["created"].clone(), c["updated"].clone(), c["links_opened"].clone(), c["links_updated"].clone(), c["closed"].clone()),
            (json!(0), json!(0), json!(0), json!(0), json!(0)), "{p}");
    })
    .await;
}
