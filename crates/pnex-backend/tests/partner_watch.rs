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
        // quali_watch 3 + presence_web 6 functions; 3 + 1 flows.
        assert_eq!(count(format!("SELECT count(*)::bigint AS n FROM functions WHERE org_id = {org}")).await, 9);
        assert_eq!(
            count(format!("SELECT count(*)::bigint AS n FROM flows WHERE org_id = {org} AND status = 'draft'")).await,
            4
        );
        assert_eq!(count(format!("SELECT count(*)::bigint AS n FROM flows WHERE org_id = {org}")).await, 4);
        // SIRENE, GLEIF ×2, Wikidata, the partner sheet and the five France
        // Compétences imports.
        assert_eq!(count(format!("SELECT count(*)::bigint AS n FROM object_imports WHERE org_id = {org}")).await, 10);

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

/// account-mapping.md D343: an org on `entreprises_fr` 1.0.0 (shared fields
/// and links owned by it) upgrades onto the worldwide `entreprises` pack,
/// which takes those keys over; objects keep their values.
#[tokio::test]
#[serial]
async fn country_pack_hands_shared_keys_to_the_worldwide_pack() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "entreprises_fr").await;
        let (st, a) = object(&server, &env.alice, org, "organization", "ACME", json!({"siren": "123456789", "etat_administratif": "actif"})).await;
        assert_eq!(st, 201, "{a}");
        // Back to the 1.0.0 layout: everything owned by `entreprises_fr`.
        for sql in [
            format!(
                "UPDATE object_type_versions SET definition = replace(definition::text, '\"pack\": \"entreprises\"', '\"pack\": \"entreprises_fr\"')::jsonb \
                 WHERE object_type_id IN (SELECT id FROM object_types WHERE org_id = {org})"
            ),
            format!("UPDATE link_types SET pack_key = 'entreprises_fr' WHERE org_id = {org} AND pack_key = 'entreprises'"),
            format!("DELETE FROM packs WHERE org_id = {org} AND key = 'entreprises'"),
            format!("UPDATE packs SET version = '1.0.0' WHERE org_id = {org} AND key = 'entreprises_fr'"),
        ] {
            env.db.execute_unprepared(&sql).await.unwrap();
        }
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/organization", &env.alice, org, None).await;
        assert!(!t.to_string().contains("\"pack\":\"entreprises\""), "{t}");

        install(&server, &env.alice, org, "entreprises_fr").await;
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/organization", &env.alice, org, None).await;
        let owner = |k: &str| t["def"]["properties"].as_array().unwrap().iter().find(|p| p["key"] == k).map(|p| p["pack"].clone());
        assert_eq!(owner("etat_administratif"), Some(json!("entreprises")));
        assert_eq!(owner("siren"), Some(json!("entreprises_fr")));
        assert_eq!(owner("code_activite"), Some(json!("entreprises")));
        let (_, l) = call(&server, "GET", "/api/v1/ontology/link-types?limit=100", &env.alice, org, None).await;
        let dirige = l["results"].as_array().unwrap().iter().find(|x| x["def"]["key"] == "dirige").cloned();
        assert_eq!(dirige.map(|d| d["pack_key"].clone()), Some(json!("entreprises")), "{l}");
        let aid = a["id"].as_str().unwrap();
        let (_, o) = call(&server, "GET", &format!("/api/v1/ontology/objects/{aid}"), &env.alice, org, None).await;
        assert_eq!(o["properties"]["etat_administratif"], "actif", "{o}");
    })
    .await;
}

/// account-mapping.md D331: group links from GLEIF (LEI, declared parents,
/// confirmed) and Wikidata (parents and subsidiaries with a SIREN, to be
/// validated); per-object imports take the companies in turn.
#[tokio::test]
#[serial]
async fn group_links_from_gleif_and_wikidata() {
    with_app(|server, env| async move {
        use sea_orm::Statement;
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "entreprises_fr").await;
        install(&server, &env.alice, org, "prospection_b2b").await;
        let base = mock_registers().await;
        let sql = |s: String| env.db.execute_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, s));
        for (from, to) in [("https://api.gleif.org/api/v1", format!("{base}/gleif")), ("https://query.wikidata.org", format!("{base}/wd"))] {
            sql(format!("UPDATE object_imports SET source_url = replace(source_url, '{from}', '{to}') WHERE org_id = {org}")).await.unwrap();
        }
        let (_, meca) = object(&server, &env.alice, org, "organization", "MECACHROME FRANCE", json!({"siren": "429853351"})).await;
        let (_, safran) = object(&server, &env.alice, org, "organization", "SAFRAN", json!({"siren": "562082909"})).await;
        let run = |name: &'static str| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                let (id, _) = import_named(server, &token, org, name).await;
                let (st, r) = call(server, "POST", &format!("/api/v1/ontology/imports/{id}/runs"), &token, org, None).await;
                assert_eq!(st, 202, "{r}");
                let r = settle_within(server, &token, org, &id, r["id"].as_str().unwrap(), 300).await;
                assert_eq!(r["run"]["state"], "applied", "{name}: {r}");
                id
            }
        };
        let filiales = |id: &str| {
            let (server, token, id) = (&server, env.alice.clone(), id.to_string());
            async move {
                let (st, l) = call(server, "GET", &format!("/api/v1/ontology/objects/{id}/links"), &token, org, None).await;
                assert_eq!(st, 200, "{l}");
                l.to_string()
            }
        };

        // The LEI first; the parent is not known yet, no link.
        let lei = run("Legal entity identifiers (GLEIF)").await;
        assert_eq!(props_of(&env.db, org, "organization", "siren", "429853351").await["lei"], "969500YMMRBQ5N157D72");
        assert!(!filiales(meca["id"].as_str().unwrap()).await.contains("filiale_de"));
        // The parents GLEIF names are created (French: by SIREN).
        run("Parent companies (GLEIF)").await;
        let mere = props_of(&env.db, org, "organization", "siren", "513149161").await;
        assert_eq!((mere["lei"].as_str(), mere["pays"].as_str()), (Some("969500ST98AQCLGNQS21"), Some("FR")), "{mere}");
        // Next turn: one link, direct and ultimate parent being the same.
        run("Legal entity identifiers (GLEIF)").await;
        let edges = sql(format!("SELECT 1 FROM resource_edges WHERE org_id = {org} AND relation = 'filiale_de' AND valid_to IS NULL")).await.unwrap();
        assert_eq!(edges.rows_affected(), 1);
        let l = filiales(meca["id"].as_str().unwrap()).await;
        assert!(l.contains("consolidation_directe"), "{l}");

        // Turns: the cursor stops at the last company queried, then wraps.
        let cursor = || async {
            env.db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
                format!("SELECT per_object_cursor AS c FROM object_imports WHERE id = '{lei}'")))
                .await.unwrap().unwrap().try_get::<Option<String>>("", "c").unwrap()
        };
        assert_eq!(cursor().await.as_deref(), Some("562082909"));
        sql(format!("UPDATE object_imports SET max_pages = 1 WHERE id = '{lei}'")).await.unwrap();
        run("Legal entity identifiers (GLEIF)").await;
        assert_eq!(cursor().await.as_deref(), Some("429853351"));
        run("Legal entity identifiers (GLEIF)").await;
        assert_eq!(cursor().await.as_deref(), Some("513149161"));

        // Wikidata: subsidiaries and parents with a SIREN, as proposals
        // (proposals.md D375): nothing enters the graph before a human.
        run("Parents and subsidiaries (Wikidata)").await;
        let sae = props_of(&env.db, org, "organization", "siren", "414815217").await;
        assert_eq!(sae["pays"], "FR", "{sae}");
        let sid = safran["id"].as_str().unwrap();
        assert!(!filiales(sid).await.contains("filiale_de"));
        let (st, p) = call(&server, "GET", &format!("/api/v1/proposals?object_id={sid}"), &env.alice, org, None).await;
        assert_eq!((st, p["count"].clone()), (200, json!(2)), "{p}");
        assert!(p.to_string().contains("Agence des participations"), "{p}");
        let to_sae = p["results"].as_array().unwrap().iter()
            .find(|r| r.to_string().contains("Safran Aircraft Engines")).cloned().unwrap();
        let (st, _) = call(&server, "POST", &format!("/api/v1/proposals/{}/decision", to_sae["id"].as_str().unwrap()),
            &env.alice, org, Some(json!({"decision": "accept"}))).await;
        assert_eq!(st, 200);
        // Proposed again by the next run: nothing new.
        run("Parents and subsidiaries (Wikidata)").await;
        let (_, p) = call(&server, "GET", &format!("/api/v1/proposals?object_id={sid}"), &env.alice, org, None).await;
        assert_eq!(p["count"], 1, "{p}");
        let (st, h) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}/hierarchy?key=groupe_commercial", safran["id"].as_str().unwrap()), &env.alice, org, None).await;
        assert_eq!(st, 200, "{h}");
        assert!(h.to_string().contains("Safran Aircraft Engines"), "{h}");
    })
    .await;
}

/// account-mapping.md D337: a saved search of the company register, paged,
/// brings new targets (`cible`) and leaves known accounts' status alone.
#[tokio::test]
#[serial]
async fn target_list_enters_new_accounts_as_targets() {
    with_app(|server, env| async move {
        use axum::extract::Query;
        use axum::routing::get;
        use sea_orm::Statement;
        use std::collections::HashMap;
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "prospection_fr").await;
        let hit = |siren: &str, name: &str| json!({
            "siren": siren, "nom_complet": name, "nature_juridique": "5710",
            "activite_principale": "25.62B", "etat_administratif": "A",
            "date_creation": "2000-01-01", "tranche_effectif_salarie": "42",
            "finances": {"2024": {"ca": 300000000}, "2025": {"ca": 343838577}},
            "siege": {"siret": format!("{siren}00011"), "latitude": "47.41", "longitude": "0.98",
                      "etat_administratif": "A", "tranche_effectif_salarie": "41"}});
        let pages = vec![
            vec![hit("429853351", "MECACHROME FRANCE"), hit("329936793", "MECACHROME TOULOUSE")],
            vec![hit("931711311", "MECACHROME SAINT ANDRE")],
        ];
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let search = move |Query(q): Query<HashMap<String, String>>| {
            let pages = pages.clone();
            async move {
                assert_eq!(q.get("section_activite_principale").map(String::as_str), Some("C"));
                let n: usize = q.get("page").and_then(|p| p.parse().ok()).unwrap_or(1);
                axum::Json(json!({"results": pages.get(n - 1).cloned().unwrap_or_default()}))
            }
        };
        let app = axum::Router::new().route("/search", get(search));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        env.db.execute_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "UPDATE object_imports SET source_url = replace(source_url, 'https://recherche-entreprises.api.gouv.fr', '{base}') WHERE org_id = {org}"))).await.unwrap();
        let (st, c) = object(&server, &env.alice, org, "organization", "Mecachrome France", json!({"siren": "429853351", "statut_compte": "client"})).await;
        assert_eq!(st, 201, "{c}");

        let (id, _) = import_named(&server, &env.alice, org, "Target list (company register search)").await;
        let (st, r) = call(&server, "POST", &format!("/api/v1/ontology/imports/{id}/runs"), &env.alice, org, None).await;
        assert_eq!(st, 202, "{r}");
        let r = settle_within(&server, &env.alice, org, &id, r["id"].as_str().unwrap(), 300).await;
        assert_eq!((r["run"]["state"].as_str(), r["run"]["rows"].as_u64()), (Some("applied"), Some(3)), "{r}");
        let client = props_of(&env.db, org, "organization", "siren", "429853351").await;
        assert_eq!((client["statut_compte"].as_str(), client["effectif_min"].as_f64()), (Some("client"), Some(1000.0)), "{client}");
        assert_eq!(client["ca_dernier"]["amount"].as_f64(), Some(343838577.0), "{client}");
        for siren in ["329936793", "931711311"] {
            let o = props_of(&env.db, org, "organization", "siren", siren).await;
            assert_eq!((o["statut_compte"].as_str(), o["code_activite"].as_str()), (Some("cible"), Some("25.62")), "{o}");
        }
        let pin = props_of(&env.db, org, "map_pin", "siret", "93171131100011").await;
        assert_eq!(pin["effectif_min"].as_f64(), Some(500.0), "{pin}");
    })
    .await;
}

/// account-mapping.md D330, D339: statuses carry their colour to the
/// hierarchy, the graph and the map; the commercial group sums its users
/// and lists the nodes with neither a contact nor a client status.
#[tokio::test]
#[serial]
async fn group_coverage_and_status_colours() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "prospection_b2b").await;
        let mk = |title: &'static str, props: Value| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                let (st, o) = object(server, &token, org, "organization", title, props).await;
                assert_eq!(st, 201, "{o}");
                o["id"].as_str().unwrap().to_string()
            }
        };
        let mother = mk("Groupe", json!({"statut_compte": "client", "utilisateurs_actuels": 50, "potentiel_utilisateurs": 5000})).await;
        let covered = mk("Filiale suivie", json!({"statut_compte": "prospect"})).await;
        let gap = mk("Filiale nue", json!({"statut_compte": "cible", "potentiel_utilisateurs": 800})).await;
        let (_, p) = object(&server, &env.alice, org, "person", "Contact", json!({})).await;
        let link = |kind: &str, from: &str, to: &str| json!({"link_type": kind, "source_id": from, "target_id": to, "attributes": {}});
        for body in [link("filiale_de", &covered, &mother), link("filiale_de", &gap, &mother), link("employs", &covered, p["id"].as_str().unwrap())] {
            let (st, r) = call(&server, "POST", "/api/v1/ontology/links", &env.alice, org, Some(body)).await;
            assert_eq!(st, 201, "{r}");
        }

        let (st, h) = call(&server, "GET", &format!("/api/v1/ontology/objects/{mother}/hierarchy?key=groupe_commercial"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{h}");
        let node = |id: &str| h["nodes"].as_array().unwrap().iter().find(|n| n["id"] == id).cloned().unwrap();
        assert_eq!(node(&mother)["marks"], json!([{"property": "statut_compte", "value": "client", "color": "green"}]));
        assert_eq!((node(&gap)["white_space"].clone(), node(&covered)["white_space"].clone()), (json!(true), Value::Null), "{h}");
        assert_eq!((h["has_coverage"].clone(), h["white_space"].clone()), (json!(true), json!(1)));
        let totals: Vec<(String, f64)> = h["totals"].as_array().unwrap().iter()
            .map(|t| (t["property"].as_str().unwrap().to_string(), t["total"].as_f64().unwrap())).collect();
        assert_eq!(totals, [("utilisateurs_actuels".to_string(), 50.0), ("potentiel_utilisateurs".to_string(), 5800.0)]);
        assert_eq!(h["counts"].as_array().unwrap().len(), 3, "{h}");

        let (st, g) = call(&server, "GET", &format!("/api/v1/ontology/objects/{mother}/graph"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{g}");
        assert_eq!((g["colors"][&mother].clone(), g["colors"][&gap].clone()), (json!("green"), json!("blue")), "{g}");

        // A site's status colours its pin on the map.
        let res = server.post("/api/v1/pois")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({"label": "Usine", "latitude": 47.4, "longitude": 0.98}))
            .await;
        assert_eq!(res.status_code().as_u16(), 201, "{}", res.text());
        let pin = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let row = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "SELECT id::text AS id, version FROM objects WHERE org_id = {org} AND type_key = 'map_pin' AND native_id = '{pin}'"))).await.unwrap().unwrap();
        let (oid, version) = (row.try_get::<String>("", "id").unwrap(), row.try_get::<i32>("", "version").unwrap());
        let (st, r) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{oid}"), &env.alice, org,
            Some(json!({"properties": {"statut_site": "client"}, "expected_version": version}))).await;
        assert_eq!(st, 200, "{r}");
        let (st, c) = call(&server, "GET", "/api/v1/pois/cluster?bbox=0.0,46.0,2.0,48.0&zoom=10", &env.alice, org, None).await;
        assert_eq!(st, 200, "{c}");
        assert_eq!(c["items"][0]["color"], "green", "{c}");

        // D338: an introduction path follows the links between people only.
        let (_, me) = object(&server, &env.alice, org, "person", "Moi", json!({})).await;
        let me = me["id"].as_str().unwrap().to_string();
        let (st, r) = call(&server, "POST", "/api/v1/ontology/links", &env.alice, org,
            Some(link("knows", &me, p["id"].as_str().unwrap()))).await;
        assert_eq!(st, 201, "{r}");
        let people = pnex_core::ontology::api::INTRODUCTION_LINKS.join(",");
        let path = |to: &str, links: &str| {
            let (server, token, url) = (&server, env.alice.clone(), format!("/api/v1/ontology/path?from={me}&to={to}&links={links}"));
            async move { call(server, "GET", &url, &token, org, None).await }
        };
        let (st, r) = path(&covered, &people).await;
        assert_eq!(st, 200, "{r}");
        let titles: Vec<&str> = r["path"].as_array().unwrap().iter().map(|o| o["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Moi", "Contact", "Filiale suivie"]);
        // The group link is not a link between people: no introduction there.
        assert_eq!(path(&mother, &people).await.0, 404);
        assert_eq!(path(&mother, "").await.0, 200);
        // Several starts: whoever follows an account (here "Moi", on the group).
        let (st, r) = call(&server, "POST", "/api/v1/ontology/links", &env.alice, org,
            Some(link("suit_le_compte", &me, &mother))).await;
        assert_eq!(st, 201, "{r}");
        let url = format!("/api/v1/ontology/path?from_links=suit_le_compte&to={covered}&links={people}");
        let (st, r) = call(&server, "GET", &url, &env.alice, org, None).await;
        assert_eq!(st, 200, "{r}");
        let titles: Vec<&str> = r["path"].as_array().unwrap().iter().map(|o| o["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Moi", "Contact", "Filiale suivie"]);
        let url = format!("/api/v1/ontology/path?from_links=knows_nobody&to={covered}");
        assert_eq!(call(&server, "GET", &url, &env.alice, org, None).await.0, 404);

        // D340: an account plan filled from the account, linked to it.
        let (st, page) = call(&server, "POST", &format!("/api/v1/ontology/objects/{mother}/pages"), &env.alice, org,
            Some(json!({"template": "plan_de_compte"}))).await;
        assert_eq!(st, 201, "{page}");
        assert_eq!(page["title"], "Account plan — Groupe");
        let snap = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "SELECT snapshot FROM docs WHERE id = '{}'", page["id"].as_str().unwrap()))).await.unwrap().unwrap()
            .try_get::<Vec<u8>>("", "snapshot").unwrap();
        let text = String::from_utf8_lossy(&snap);
        for want in ["Plan de compte — Groupe", "Filiale nue", "utilisateurs actuels 50", "potentiel utilisateurs 5800"] {
            assert!(text.contains(want), "{want} missing");
        }
        assert!(!text.contains("{{"), "every placeholder filled");
        let (_, l) = call(&server, "GET", &format!("/api/v1/ontology/objects/{mother}/links"), &env.alice, org, None).await;
        assert!(l.to_string().contains("page_sur"), "{l}");
        // A template of another type is refused.
        let (st, _) = call(&server, "POST", &format!("/api/v1/ontology/objects/{}/pages", p["id"].as_str().unwrap()), &env.alice, org,
            Some(json!({"template": "plan_de_compte"}))).await;
        assert_eq!(st, 400);
    })
    .await;
}

/// web-presence.md D363: `presence_web` takes `site_web` and `a_pour_site`
/// over from an org on `quali_watch` 1.2.0, objects and links in place.
#[tokio::test]
#[serial]
async fn presence_web_takes_websites_over() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        let (st, site) = object(&server, &env.alice, org, "site_web", "of.fr", json!({"url_racine": "https://of.fr/", "domaine": "of.fr"})).await;
        assert_eq!(st, 201, "{site}");
        // Back to the 1.2.0 layout: the two keys owned by `quali_watch`.
        for sql in [
            format!("UPDATE object_types SET pack_key = 'quali_watch' WHERE org_id = {org} AND key = 'site_web'"),
            format!("UPDATE link_types SET pack_key = 'quali_watch' WHERE org_id = {org} AND key = 'a_pour_site'"),
            format!("DELETE FROM packs WHERE org_id = {org} AND key = 'presence_web'"),
            format!("UPDATE packs SET version = '1.2.0' WHERE org_id = {org} AND key = 'quali_watch'"),
        ] {
            env.db.execute_unprepared(&sql).await.unwrap();
        }
        install(&server, &env.alice, org, "quali_watch").await;
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/site_web", &env.alice, org, None).await;
        assert_eq!(t["pack_key"], "presence_web", "{t}");
        let (_, l) = call(&server, "GET", "/api/v1/ontology/link-types?limit=100", &env.alice, org, None).await;
        let site_link = l["results"].as_array().unwrap().iter().find(|x| x["def"]["key"] == "a_pour_site").cloned().unwrap();
        assert_eq!(site_link["pack_key"], "presence_web");
        assert!(site_link["def"]["attributes"].to_string().contains("valide_par"), "{site_link}");
        let (st, o) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}", site["id"].as_str().unwrap()), &env.alice, org, None).await;
        assert_eq!((st, o["properties"]["domaine"].as_str()), (200, Some("of.fr")), "{o}");

        // D376: the candidate status left the link: a candidate is a proposal.
        assert!(!site_link["def"]["attributes"].to_string().contains("\"statut\""), "{site_link}");

        // D368: the org's layer over the pack's settings, kept by reinstall.
        let url = "/api/v1/ontology/packs/presence_web/settings";
        let (st, p) = call(&server, "PATCH", url, &env.alice, org, Some(json!({"values": {"seuil_confirme": 70}}))).await;
        assert_eq!(st, 200, "{p}");
        let get = |p: &Value, k: &str| p["settings"].as_array().unwrap().iter().find(|s| s["key"] == k).cloned().unwrap();
        assert_eq!((get(&p, "seuil_confirme")["value"].clone(), get(&p, "seuil_confirme")["overridden"].clone()), (json!(70), json!(true)), "{p}");
        assert_eq!(get(&p, "seuil_propose")["value"], json!(40));
        for bad in [json!({"inconnu": 1}), json!({"seuil_confirme": 500}), json!({"seuil_confirme": "70"})] {
            let (st, e) = call(&server, "PATCH", url, &env.alice, org, Some(json!({"values": bad}))).await;
            assert_eq!(st, 400, "{e}");
        }
        // An upgrade rewrites what the org never edited and keeps the rest:
        // pw_score as an older pack wrote it, pw_lien edited by the org, the
        // first flow still at the pack's own (older) version.
        let fv = |name: &str| format!("SELECT id FROM functions WHERE org_id = {org} AND name = '{name}'");
        let flow_id = format!("SELECT id FROM flows WHERE org_id = {org} AND name = 'Web presence · find websites'");
        for sql in [
            format!("UPDATE function_versions SET code = code || E'\\n# older' WHERE function_id = ({})", fv("pw_score")),
            format!("INSERT INTO function_versions (created_at, updated_at, function_id, org_id, version_number, code, inputs, outputs, note) SELECT now(), now(), function_id, org_id, version_number + 1, code || E'\\n# mine', inputs, outputs, 'my fix' FROM function_versions WHERE function_id = ({}) ORDER BY version_number DESC LIMIT 1", fv("pw_lien")),
            format!("UPDATE flow_versions SET graph = jsonb_set(graph, '{{nodes}}', '[]') WHERE flow_id = ({flow_id})"),
        ] {
            env.db.execute_unprepared(&sql).await.unwrap();
        }
        // D370: the org names and colours a value of a pack's enum.
        let (_, t) = call(&server, "GET", "/api/v1/ontology/types/site_web", &env.alice, org, None).await;
        let mut def = t["def"].clone();
        for p in def["properties"].as_array_mut().unwrap() {
            if p["key"] == "etat_collecte" {
                p["labels"] = json!({"bloque": {"fr": "Bloqué par le site"}});
                p["colors"] = json!({"bloque": "red"});
            }
        }
        let (st, t2) = call(&server, "PUT", "/api/v1/ontology/types/site_web", &env.alice, org, Some(json!({"def": def, "expected_version": t["version"]}))).await;
        assert_eq!(st, 200, "{t2}");
        env.db.execute_unprepared(&format!("UPDATE packs SET version = '1.0.0' WHERE org_id = {org} AND key = 'presence_web'")).await.unwrap();
        let (st, up) = call(&server, "POST", "/api/v1/ontology/packs/presence_web/install", &env.alice, org, None).await;
        assert_eq!((st, up["kept"].clone()), (200, json!(["pw_lien"])), "the org's own version is named");
        let (_, t3) = call(&server, "GET", "/api/v1/ontology/types/site_web", &env.alice, org, None).await;
        let etat = t3["def"]["properties"].as_array().unwrap().iter().find(|p| p["key"] == "etat_collecte").unwrap().clone();
        assert_eq!((etat["labels"]["bloque"]["fr"].clone(), etat["colors"]["bloque"].clone()), (json!("Bloqué par le site"), json!("red")), "kept across the upgrade: {etat}");
        let shipped = pnex_backend::services::ontology::packs::shipped().into_iter().find(|p| p.key == "presence_web").unwrap();
        let code_of = |name: &str| shipped.functions.iter().find(|f| f.name == name).unwrap().code.clone();
        let latest = |sql: String| {
            let db = env.db.clone();
            async move {
                let r = db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql)).await.unwrap().unwrap();
                (r.try_get::<i64>("", "n").unwrap(), r.try_get::<String>("", "t").unwrap())
            }
        };
        let last_fn = |name: &str| format!("SELECT version_number AS n, code AS t FROM function_versions WHERE function_id = ({}) ORDER BY version_number DESC LIMIT 1", fv(name));
        assert_eq!(latest(last_fn("pw_score")).await, (2, code_of("pw_score")), "unedited function upgraded");
        assert!(latest(last_fn("pw_lien")).await.1.ends_with("# mine"), "edited function kept");
        let (n, author) = latest(format!("SELECT version_number AS n, author AS t FROM flow_versions WHERE flow_id = ({flow_id}) ORDER BY version_number DESC LIMIT 1")).await;
        assert_eq!((n, author.as_str()), (2, "pack:presence_web"), "unedited flow upgraded");
        install(&server, &env.alice, org, "presence_web").await;
        assert_eq!(latest(last_fn("pw_score")).await.0, 2, "a same-version reinstall writes nothing");
        let in_force = pnex_backend::services::ontology::packs::org_settings(&env.db, org).await.unwrap();
        assert_eq!(in_force["presence_web.seuil_confirme"], json!(70), "kept across the upgrade");
        let (st, p) = call(&server, "PATCH", url, &env.alice, org, Some(json!({"values": {"seuil_confirme": null}}))).await;
        assert_eq!((st, get(&p, "seuil_confirme")["value"].clone(), get(&p, "seuil_confirme")["overridden"].clone()), (200, json!(80), json!(false)));
        let (st, _) = call(&server, "PATCH", "/api/v1/ontology/packs/maintenance/settings", &env.alice, org, Some(json!({"values": {}}))).await;
        assert_eq!(st, 404, "not installed");
        // The org's edits stay listed, and it can take the pack's version.
        let (_, list) = call(&server, "GET", "/api/v1/ontology/packs", &env.alice, org, None).await;
        let pw = list["results"].as_array().unwrap().iter().find(|p| p["key"] == "presence_web").unwrap().clone();
        assert_eq!(pw["kept"], json!(["pw_lien"]), "{pw}");
        let take = "/api/v1/ontology/packs/presence_web/take";
        let (st, e) = call(&server, "POST", take, &env.alice, org, Some(json!({"names": ["pw_score"]}))).await;
        assert_eq!(st, 400, "not edited: {e}");
        let (st, p) = call(&server, "POST", take, &env.alice, org, Some(json!({"names": ["pw_lien"]}))).await;
        assert_eq!((st, p.get("kept").cloned()), (200, None), "{p}");
        assert_eq!(latest(last_fn("pw_lien")).await.1, code_of("pw_lien"), "the pack's version");
        // A function of the org that only shares a pack function's name is
        // its own: never listed as edited.
        let fid: i64 = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, fv("pw_lien"))).await.unwrap().unwrap().try_get("", "id").unwrap();
        env.db.execute_unprepared(&format!("UPDATE function_versions SET note = 'mine', code = code || 'x' WHERE function_id = {fid}")).await.unwrap();
        let (_, list) = call(&server, "GET", "/api/v1/ontology/packs", &env.alice, org, None).await;
        let pw = list["results"].as_array().unwrap().iter().find(|p| p["key"] == "presence_web").unwrap().clone();
        assert_eq!(pw.get("kept"), None, "{pw}");
    })
    .await;
}

/// account-mapping.md D335: the legal announcements of followed accounts
/// become signals linked to them, decoded, never reset once decided.
#[tokio::test]
#[serial]
async fn legal_announcements_become_signals() {
    with_app(|server, env| async move {
        use axum::extract::Query;
        use axum::routing::get;
        use sea_orm::Statement;
        use std::collections::HashMap;
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "prospection_fr").await;
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = asked.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let records = move |Query(q): Query<HashMap<String, String>>| {
            let seen = seen.clone();
            async move {
                let w = q.get("where").cloned().unwrap_or_default();
                seen.lock().unwrap().push(w.clone());
                let results = if w.contains("\"429853351\"") {
                    vec![
                        json!({"id": "B20260114713", "registre": ["429853351", "429 853 351"], "commercant": "MECACHROME FRANCE",
                               "familleavis": "modification", "familleavis_lib": "Modifications diverses", "typeavis_lib": "Avis initial",
                               "dateparution": "2026-06-18", "url_complete": "https://www.bodacc.fr/a/B20260114713",
                               "modificationsgenerales": "{\"descriptif\": \"modification survenue sur le capital (augmentation)\"}",
                               "jugement": null, "acte": null}),
                        json!({"id": "A2026000001", "registre": ["429853351"], "commercant": "MECACHROME FRANCE",
                               "familleavis": "creation", "familleavis_lib": "Créations", "typeavis_lib": "Avis initial",
                               "dateparution": "2026-09-01", "url_complete": "https://www.bodacc.fr/a/A2026000001",
                               "modificationsgenerales": null, "jugement": null, "acte": null}),
                    ]
                } else {
                    vec![]
                };
                axum::Json(json!({"total_count": results.len(), "results": results}))
            }
        };
        let app = axum::Router::new().route("/records", get(records));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        env.db.execute_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "UPDATE object_imports SET source_url = replace(source_url, \
             'https://bodacc-datadila.opendatasoft.com/api/explore/v2.1/catalog/datasets/annonces-commerciales/records', '{base}/records') \
             WHERE org_id = {org}"))).await.unwrap();
        let (_, meca) = object(&server, &env.alice, org, "organization", "MECACHROME FRANCE", json!({"siren": "429853351", "statut_compte": "client"})).await;
        object(&server, &env.alice, org, "organization", "Inconnue", json!({"siren": "999999999"})).await;

        let (id, _) = import_named(&server, &env.alice, org, "Legal announcements (BODACC)").await;
        let run = || async {
            let (st, r) = call(&server, "POST", &format!("/api/v1/ontology/imports/{id}/runs"), &env.alice, org, None).await;
            assert_eq!(st, 202, "{r}");
            let r = settle_within(&server, &env.alice, org, &id, r["id"].as_str().unwrap(), 300).await;
            assert_eq!(r["run"]["state"], "applied", "{r}");
        };
        run().await;
        // Followed accounts only: the unknown company is not asked about.
        assert_eq!(asked.lock().unwrap().len(), 1, "{:?}", asked.lock().unwrap());
        let modif = props_of(&env.db, org, "signal_commercial", "cle", "B20260114713").await;
        assert_eq!((modif["kind"].as_str(), modif["etat"].as_str()), (Some("annonce_legale"), Some("a_valider")), "{modif}");
        assert_eq!(modif["extrait"], "modification survenue sur le capital (augmentation)");
        assert_eq!(modif["resume"], "Modifications diverses — Avis initial");
        let opening = props_of(&env.db, org, "signal_commercial", "cle", "A2026000001").await;
        assert_eq!(opening["kind"], "nouvel_etablissement", "{opening}");
        let (_, l) = call(&server, "GET", &format!("/api/v1/ontology/objects/{}/links", meca["id"].as_str().unwrap()), &env.alice, org, None).await;
        assert_eq!(l.to_string().matches("signal_sur").count() >= 2, true, "{l}");

        // A human retains it; the next run leaves that alone.
        env.db.execute_unprepared(&format!(
            "UPDATE objects SET properties = properties || '{{\"etat\": \"retenu\"}}' WHERE org_id = {org} AND properties ->> 'cle' = 'B20260114713'")).await.unwrap();
        run().await;
        let modif = props_of(&env.db, org, "signal_commercial", "cle", "B20260114713").await;
        assert_eq!(modif["etat"], "retenu");
    })
    .await;
}

/// Meetings: a debrief page filled from the meeting, and the coaching tree
/// of a playbook criterion counting the meetings that covered it.
#[tokio::test]
#[serial]
async fn meeting_debrief_and_coaching() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "prospection_b2b").await;
        let mk = |t: &'static str, title: &'static str, props: Value| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                let (st, o) = object(server, &token, org, t, title, props).await;
                assert_eq!(st, 201, "{o}");
                o["id"].as_str().unwrap().to_string()
            }
        };
        let account = mk("organization", "Usine Nord", json!({"statut_compte": "prospect"})).await;
        let meeting = mk("rendez_vous", "Découverte Usine Nord", json!({"statut": "tenu", "objet": "Découverte", "date": "2026-10-20T09:00:00Z"})).await;
        let director = mk("person", "Directrice d'usine", json!({"persona": "decideur"})).await;
        let criterion = mk("critere_coaching", "Faire parler du turnover", json!({"persona": "sponsor", "actif": true})).await;
        let link = |kind: &str, from: &str, to: &str, attrs: Value| json!({"link_type": kind, "source_id": from, "target_id": to, "attributes": attrs});
        for body in [
            link("rdv_avec", &meeting, &account, json!({})),
            link("participe_a", &director, &meeting, json!({"cote": "client"})),
            link("couvre", &meeting, &criterion, json!({"commentaire": "abordé en fin de rendez-vous"})),
        ] {
            let (st, r) = call(&server, "POST", "/api/v1/ontology/links", &env.alice, org, Some(body)).await;
            assert_eq!(st, 201, "{r}");
        }
        let (st, page) = call(&server, "POST", &format!("/api/v1/ontology/objects/{meeting}/pages"), &env.alice, org,
            Some(json!({"template": "debrief_rdv"}))).await;
        assert_eq!(st, 201, "{page}");
        let snap = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "SELECT snapshot FROM docs WHERE id = '{}'", page["id"].as_str().unwrap()))).await.unwrap().unwrap()
            .try_get::<Vec<u8>>("", "snapshot").unwrap();
        let text = String::from_utf8_lossy(&snap);
        for want in ["Usine Nord", "Directrice d'usine", "cote: client", "Faire parler du turnover", "abordé en fin de rendez-vous"] {
            assert!(text.contains(want), "{want} missing");
        }
        let (st, h) = call(&server, "GET", &format!("/api/v1/ontology/objects/{criterion}/hierarchy?key=coaching"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{h}");
        let titles: Vec<&str> = h["nodes"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Faire parler du turnover", "Découverte Usine Nord", "Usine Nord"]);
        assert!(h["counts"].to_string().contains("\"tenu\""), "{h}");
    })
    .await;
}

/// GLEIF and Wikidata, answering for the reference companies only.
async fn mock_registers() -> String {
    use axum::extract::Query;
    use axum::routing::get;
    use std::collections::HashMap;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let none =
        |rel: &str| json!({"data": {"type": "reporting-exceptions", "id": format!("x|{rel}")}});
    let parent = |lei: &str| json!({"data": {"type": "lei-records", "id": lei}});
    let record = |lei: &str, name: &str, siren: &str, rels: Value| {
        json!({
        "type": "lei-records", "id": lei,
        "attributes": {"lei": lei, "entity": {"legalName": {"name": name}, "registeredAs": siren,
            "jurisdiction": "FR", "legalAddress": {"country": "FR"}}},
        "relationships": rels})
    };
    let meca_dev = record(
        "969500ST98AQCLGNQS21",
        "MECA DEV",
        "513 149 161",
        json!({"direct-parent": none("d"), "ultimate-parent": none("u")}),
    );
    let meca = record(
        "969500YMMRBQ5N157D72",
        "MECACHROME FRANCE",
        "429853351",
        json!({"direct-parent": parent("969500ST98AQCLGNQS21"), "ultimate-parent": parent("969500ST98AQCLGNQS21")}),
    );
    let gleif = move |Query(q): Query<HashMap<String, String>>| {
        let (meca, meca_dev) = (meca.clone(), meca_dev.clone());
        async move {
            let data = match (
                q.get("filter[entity.registeredAs]").map(String::as_str),
                q.get("filter[owns]").map(String::as_str),
            ) {
                (Some("429853351"), _) => vec![meca],
                (Some("513149161"), _) => vec![meca_dev.clone()],
                (_, Some("969500YMMRBQ5N157D72")) => vec![meca_dev],
                _ => vec![],
            };
            axum::Json(json!({"data": data}))
        }
    };
    let b = |dir: &str, siren: &str, label: &str| {
        json!({
        "dir": {"value": dir}, "self_siren": {"value": "562082909"},
        "other_siren": {"value": siren}, "otherLabel": {"value": label}})
    };
    let wd = move |Query(q): Query<HashMap<String, String>>| async move {
        let rows = if q.get("query").is_some_and(|t| t.contains("\"562082909\"")) {
            vec![
                b("child", "414815217", "Safran Aircraft Engines"),
                b("parent", "130004989", "Agence des participations de l'État"),
            ]
        } else {
            vec![]
        };
        axum::Json(json!({"results": {"bindings": rows}}))
    };
    let app = axum::Router::new()
        .route("/gleif/lei-records", get(gleif))
        .route("/wd/sparql", get(wd));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// account-mapping.md D329: the B2B pack extends the company types and its
/// commercial group follows subsidiaries, under each of their parents.
#[tokio::test]
#[serial]
async fn prospection_pack_maps_a_commercial_group() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "prospection_b2b").await;
        let (st, e) = object(&server, &env.alice, org, "organization", "Bad", json!({"statut_compte": "lead"})).await;
        assert_eq!(st, 400, "{e}");
        let (st, m) = object(&server, &env.alice, org, "organization", "Mother", json!({"lei": "529900T8BM49AURSDO55", "pays": "DE", "statut_compte": "client"})).await;
        assert_eq!(st, 201, "{m}");
        let (_, f) = object(&server, &env.alice, org, "organization", "Filiale FR", json!({"pays": "FR", "code_activite": "28.41", "statut_compte": "prospect"})).await;
        let (_, g) = object(&server, &env.alice, org, "organization", "Usine Lyon SAS", json!({"pays": "FR", "effectif_min": 250, "effectif_max": 499})).await;
        let (mid, fid, gid) = (m["id"].as_str().unwrap(), f["id"].as_str().unwrap(), g["id"].as_str().unwrap());
        // D331: a direct and an ultimate parent coexist, each with its nature.
        for (from, to, nature) in [(fid, mid, "consolidation_directe"), (gid, fid, "consolidation_directe"), (gid, mid, "consolidation_ultime")] {
            let body = json!({"link_type": "filiale_de", "source_id": from, "target_id": to, "attributes": {"nature": nature, "source": "gleif.org"}});
            let (st, r) = call(&server, "POST", "/api/v1/ontology/links", &env.alice, org, Some(body)).await;
            assert_eq!(st, 201, "{r}");
        }
        let (st, h) = call(&server, "GET", &format!("/api/v1/ontology/objects/{mid}/hierarchy?key=groupe_commercial"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{h}");
        let nodes: Vec<(&str, u64)> = h["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (n["title"].as_str().unwrap(), n["depth"].as_u64().unwrap()))
            .collect();
        assert_eq!(nodes[0], ("Mother", 0));
        for n in [("Filiale FR", 1), ("Usine Lyon SAS", 2), ("Usine Lyon SAS", 1)] {
            assert!(nodes.contains(&n), "{n:?} in {nodes:?}");
        }
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
    settle_within(server, token, org, id, &run, 600).await
}

/// Polls a run until it leaves `running`.
async fn settle(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: &str,
    run: &str,
) -> Value {
    settle_within(server, token, org, id, run, 100).await
}

/// `settle` with a budget of `polls` × 100 ms.
async fn settle_within(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    id: &str,
    run: &str,
    polls: u32,
) -> Value {
    for _ in 0..polls {
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

/// A few thousand records apply in seconds: the write loads the schema
/// once, not per object and per link (an N+1 that made a 42k-row file
/// outlast its flow step), and in bulk (D347): above the old 50,000-row
/// cap, creations then updates.
#[tokio::test]
#[serial]
async fn imports_apply_thousands_of_records_quickly() {
    with_app(|server, env| async move {
        const ROWS: usize = 60_000;
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        for i in 0..20 {
            let code = format!("RNCP{i:05}");
            let (st, r) = object(&server, &env.alice, org, "certification", &code, json!({"code": code})).await;
            assert_eq!(st, 201, "{r}");
        }
        let (st, imp) = call(&server, "POST", "/api/v1/ontology/imports", &env.alice, org,
            Some(json!({"name": "Bulk", "mapping": MAPPING, "mode": "incremental"}))).await;
        assert_eq!(st, 201, "{imp}");
        let id = imp["id"].as_str().unwrap().to_string();
        let csv_of = |name: &str| {
            let mut csv = String::from("siret;raison_sociale;code_certification;perimetre;date_debut_habilitation;declaree_france_competences\n");
            for i in 0..ROWS {
                csv.push_str(&format!("{}00012;{name} {i};RNCP{:05};les_deux;01/02/2024;oui\n", 100_000_000 + i, i % 20));
            }
            csv
        };
        let csv = csv_of("Org");
        let res = server
            .post(&format!("/api/v1/ontology/imports/{id}/dry-run?filename=bulk.csv"))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/octet-stream")
            .bytes(csv.into_bytes().into())
            .await;
        assert_eq!(res.status_code().as_u16(), 202, "{}", res.text());
        let run = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let p = settle_within(&server, &env.alice, org, &id, &run, 600).await;
        assert_eq!(p["run"]["counts"]["created"], ROWS, "{p}");
        assert_eq!(p["run"]["counts"]["links_opened"], ROWS, "{p}");

        let started = std::time::Instant::now();
        let (st, r) = call(&server, "POST", &format!("/api/v1/ontology/imports/{id}/apply"), &env.alice, org,
            Some(json!({"run_id": run, "expected_version": imp["version"]}))).await;
        assert_eq!(st, 202, "{r}");
        // Generous on purpose: about 7 s here, 38 s with a scan per object.
        let r = settle_within(&server, &env.alice, org, &id, &run, 600).await;
        let took = started.elapsed();
        assert_eq!(r["run"]["state"], "applied", "{r}");
        eprintln!("applied {ROWS} records + {ROWS} links in {took:?}");
        // Debug build on a shared machine: ~60 s; one write per row took 340 s.
        assert!(took < std::time::Duration::from_secs(120), "apply took {took:?}");

        // Every title changes: one bulk update.
        let p = preview(&server, &env.alice, org, &id, &csv_of("Firm")).await;
        assert_eq!(p["run"]["counts"]["updated"], ROWS, "{}", p["run"]);
        let started = std::time::Instant::now();
        let run = p["run"]["id"].as_str().unwrap();
        let (st, r) = call(&server, "POST", &format!("/api/v1/ontology/imports/{id}/apply"), &env.alice, org,
            Some(json!({"run_id": run, "expected_version": imp["version"]}))).await;
        assert_eq!(st, 202, "{r}");
        let r = settle_within(&server, &env.alice, org, &id, run, 600).await;
        let took = started.elapsed();
        assert_eq!(r["run"]["state"], "applied", "{}", r["run"]);
        eprintln!("updated {ROWS} records in {took:?}");
        assert!(took < std::time::Duration::from_secs(120), "update took {took:?}");
        let row = env.db.query_one_raw(sea_orm::Statement::from_string(
            sea_orm::DatabaseBackend::Postgres,
            format!("SELECT count(*)::bigint AS n FROM objects WHERE org_id = {org} \
                     AND title LIKE 'Firm %' AND version = 2 AND valid_to IS NULL"),
        )).await.unwrap().unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), ROWS as i64);
    })
    .await;
}

/// An import with its own steps runs them on "Run now" (data-pipelines.md
/// D344): no flow, the zip fetched, extracted, filtered, then applied.
#[tokio::test]
#[serial]
async fn import_runs_its_own_steps() {
    with_app(|server, env| async move {
        use sea_orm::Statement;
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        let zip = std::sync::Arc::new(std::sync::Mutex::new(fc_zip(&fc_partners(&[]))));
        let base = mock_datagouv(zip.clone()).await;
        unsafe { std::env::set_var("PNEX_DATAGOUV_API_URL", format!("{base}/api/1")) };
        let q = |sql: String| env.db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql));

        // The steps of the pack's flow, moved onto the certifications import.
        let graph = q(format!("SELECT v.graph FROM flows f JOIN flow_versions v ON v.flow_id = f.id \
            WHERE f.org_id = {org} AND f.name = 'Ingestion France Compétences' ORDER BY v.version_number DESC LIMIT 1"))
            .await.unwrap().expect("pack flow").try_get::<Value>("", "graph").unwrap();
        let config = |id: &str| graph["nodes"].as_array().unwrap().iter()
            .find(|n| n["id"] == id).map(|n| n["config"].clone()).unwrap();
        let mut steps = vec![config("resolve"), config("download"), config("extract"), config("inspect_c"), config("filter_c")];
        steps[0]["dataset"] = json!("fc");
        steps[4]["values"] = json!(["80259488700019"]);
        let import_id = config("import_c")["import_id"].as_str().unwrap().to_string();
        let (st, imp) = call(&server, "GET", &format!("/api/v1/ontology/imports/{import_id}"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{imp}");

        // Nothing to fetch yet: a file import runs from a preview.
        let (st, e) = call(&server, "POST", &format!("/api/v1/ontology/imports/{import_id}/runs"), &env.alice, org, None).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ingest-entry-missing")), "{e}");

        // YAML takes JSON: one step per line.
        let mut mapping = imp["mapping"].as_str().unwrap().trim_end().to_string();
        mapping.push_str("\nsteps:\n");
        for s in &steps {
            mapping.push_str(&format!("  - {s}\n"));
        }
        let (st, imp) = call(&server, "PUT", &format!("/api/v1/ontology/imports/{import_id}"), &env.alice, org,
            Some(json!({"name": imp["name"], "mapping": mapping, "source": "file", "mode": imp["mode"],
                        "expected_version": imp["version"]}))).await;
        assert_eq!(st, 200, "{imp}");

        // R2: a viewer starts nothing.
        personal_org(&server, &env.bob).await;
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org,
            Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st == 200 || st == 201);
        let (st, _) = call(&server, "POST", &format!("/api/v1/ontology/imports/{import_id}/runs"), &env.bob, org, None).await;
        assert_eq!(st, 403);

        let (st, run) = call(&server, "POST", &format!("/api/v1/ontology/imports/{import_id}/runs"), &env.alice, org, None).await;
        assert_eq!((st, run["state"].as_str(), run["trigger"].as_str()), (202, Some("running"), Some("manual")), "{run}");
        let r = settle_within(&server, &env.alice, org, &import_id, run["id"].as_str().unwrap(), 600).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(r["run"]["rows"], 2, "the client's certifications only: {r}");
        let links = format!("SELECT count(*)::bigint AS n FROM resource_edges WHERE org_id = {org} \
            AND relation = 'porte' AND valid_to IS NULL");
        assert_eq!(count(q(links).await.unwrap()), 2);
        // Journaled under the run, the staging emptied at its end.
        let steps = format!("SELECT count(*)::bigint AS n FROM ingest_steps WHERE org_id = {org} \
            AND node_id = 'import:{import_id}' AND run_id = '{}'", run["id"].as_str().unwrap());
        assert_eq!(count(q(steps).await.unwrap()), 5);
        let live = format!("SELECT count(*)::bigint AS n FROM ingest_steps WHERE org_id = {org} \
            AND node_id = 'import:{import_id}' AND purged_at IS NULL AND keys <> '[]'::jsonb \
            AND (keep_until IS NULL OR keep_until <= now())");
        assert_eq!(count(q(live).await.unwrap()), 0);
        // The raw download stays its retention; expired here so the next
        // test reusing this org id finds an empty staging.
        env.db.execute_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres,
            format!("UPDATE ingest_steps SET created_at = now() - interval '8 days', keep_until = NULL WHERE org_id = {org}"))).await.unwrap();
        let store = pnex_backend::services::artifact_store::S3Config::from_config(
            &loco_rs::config::Config::new(&loco_rs::environment::Environment::Test).unwrap()).store().unwrap();
        pnex_backend::services::ingest::purge(&env.db, &store).await.unwrap();
        let left = store.list_prefix(&format!("org_{org}/staging/")).await.unwrap();
        assert!(left.is_empty(), "{left:?}");
        unsafe { std::env::remove_var("PNEX_DATAGOUV_API_URL") };
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
            {"id": "l", "kind": "link_upsert", "config": {"link_type": "habilite_pour"}},
            {"id": "s", "kind": "link_upsert", "config": {"link_type": "a_pour_site", "skip_closed": true}},
            {"id": "p", "kind": "link_upsert", "config": {"link_type": "a_pour_site", "skip_closed": true, "propose": true}},
            {"id": "c", "kind": "link_upsert", "config": {"link_type": "a_pour_site", "close": true}}
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

        // web-presence.md D362: a rejected candidate is closed with its
        // reason, and a skip_closed node never opens it again.
        let (st, site) = object(&server, &env.alice, org, "site_web", "acme.fr", json!({"url_racine": "https://acme.fr/", "domaine": "acme.fr"})).await;
        assert_eq!(st, 201, "{site}");
        let pair = |id: &str, attrs: Value| json!({"node": node(id), "source_id": a["object"]["id"], "target_id": site["id"], "attributes": attrs});
        let (st, open) = internal(&server, "link", pair("s", json!({"score": 55}))).await;
        assert_eq!((st, open["valid_to"].clone()), (200, Value::Null), "{open}");
        let (st, closed) = internal(&server, "link", pair("c", json!({"valide_par": "Alice"}))).await;
        assert_eq!(st, 200, "{closed}");
        assert!(closed["valid_to"].is_string(), "{closed}");
        assert_eq!((closed["attributes"]["valide_par"].as_str(), closed["attributes"]["score"].as_f64()), (Some("Alice"), Some(55.0)));
        let (st, again) = internal(&server, "link", pair("s", json!({"score": 90}))).await;
        assert_eq!((st, again["id"].clone(), again["attributes"]["score"].clone()), (200, closed["id"].clone(), json!(55)), "{again}");
        let open_links = env.db.execute_unprepared(&format!(
            "SELECT 1 FROM resource_edges WHERE org_id = {org} AND relation = 'a_pour_site' AND valid_to IS NULL")).await.unwrap();
        assert_eq!(open_links.rows_affected(), 0);
        // D375: a proposing node writes no link, it proposes one.
        let (_, site2) = object(&server, &env.alice, org, "site_web", "acme2.fr", json!({"url_racine": "https://acme2.fr/", "domaine": "acme2.fr"})).await;
        let req = json!({"node": node("p"), "source_id": a["object"]["id"], "target_id": site2["id"], "attributes": {"score": 60}, "confidence": 60});
        let (st, r) = internal(&server, "link", req.clone()).await;
        assert_eq!((st, r["proposed"].clone()), (200, json!(true)), "{r}");
        assert!(r["proposal_id"].is_string(), "{r}");
        // The model's opinion arrives after: same values, the open proposal takes it.
        let mut req2 = req.clone();
        req2["confidence"] = json!(70);
        req2["evidence"] = json!({"avis_llm": "OUI, mentions légales"});
        let (st, r2) = internal(&server, "link", req2).await;
        assert_eq!((st, r2["proposal_id"].clone()), (200, Value::Null), "{r2}");
        let (_, got) = call(&server, "GET", &format!("/api/v1/proposals/{}", r["proposal_id"].as_str().unwrap()), &env.alice, org, None).await;
        assert_eq!((got["evidence"]["avis_llm"].clone(), got["confidence"].clone(), got["status"].clone()),
            (json!("OUI, mentions légales"), json!(70.0), json!("proposed")), "{got}");
        let open_links = env.db.execute_unprepared(&format!(
            "SELECT 1 FROM resource_edges WHERE org_id = {org} AND relation = 'a_pour_site' AND valid_to IS NULL")).await.unwrap();
        assert_eq!(open_links.rows_affected(), 0);
        // D377: the same open proposal with an org rule is accepted without
        // a human, the rule kept as the reason.
        let mut req3 = req.clone();
        req3["accept"] = json!("presence_web.seuil_confirme >= 80 (score 90)");
        let (st, r3) = internal(&server, "link", req3).await;
        assert_eq!((st, r3["proposal_id"].clone()), (200, r["proposal_id"].clone()), "{r3}");
        let (_, got) = call(&server, "GET", &format!("/api/v1/proposals/{}", r["proposal_id"].as_str().unwrap()), &env.alice, org, None).await;
        assert_eq!((got["status"].clone(), got["decided_by"].clone(), got["reason"].clone()),
            (json!("accepted"), Value::Null, json!("presence_web.seuil_confirme >= 80 (score 90)")), "{got}");
        assert!(got["decided_at"].is_string() && got["applied"].is_string(), "{got}");
        let open_links = env.db.execute_unprepared(&format!(
            "SELECT 1 FROM resource_edges WHERE org_id = {org} AND relation = 'a_pour_site' AND valid_to IS NULL")).await.unwrap();
        assert_eq!(open_links.rows_affected(), 1);

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
        let left = store.list_prefix(&format!("org_{org}/staging/")).await.unwrap();
        assert!(left.is_empty(), "{left:?}");
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
        unsafe { std::env::remove_var("PNEX_DATAGOUV_API_URL") };
    })
    .await;
}

/// An import step outlives its HTTP call (no staged file here: the import
/// fetches its own source, like an API enrichment): `pending` until done,
/// then the journaled answer, never run twice; a run whose server is gone
/// fails with `import-run-interrupted`.
#[tokio::test]
#[serial]
async fn ingest_import_outlives_its_call() {
    with_app(|server, env| async move {
        use axum::routing::get;
        use sea_orm::Statement;
        let org = personal_org(&server, &env.alice).await;
        install(&server, &env.alice, org, "quali_watch").await;
        let (st, r) = object(&server, &env.alice, org, "certification", "RNCP12345", json!({"code": "RNCP12345"})).await;
        assert_eq!(st, 201, "{r}");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().route("/slow/reseau.csv", get(|| async {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            "siret;raison_sociale;code_certification;perimetre;date_debut_habilitation;declaree_france_competences\n\
             123456789 00012;ACME;RNCP12345;les_deux;01/02/2024;oui\n"
        }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (st, imp) = call(&server, "POST", "/api/v1/ontology/imports", &env.alice, org,
            Some(json!({"name": "Slow", "mapping": MAPPING, "mode": "incremental",
                        "source": "url", "source_url": format!("{base}/slow/reseau.csv")}))).await;
        assert_eq!(st, 201, "{imp}");
        let import_id = imp["id"].as_str().unwrap().to_string();
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-pw-1") };
        unsafe { std::env::set_var("PNEX_INGEST_WAIT_MS", "100") };

        use pnex_backend::models::_entities::{flow_versions, flows};
        use sea_orm::{ActiveModelTrait, Set};
        let flow = flows::ActiveModel {
            name: Set("ingest".into()),
            status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
            org_id: Set(org),
            ..Default::default()
        }.insert(&env.db).await.unwrap();
        let version = flow_versions::ActiveModel {
            flow_id: Set(flow.id),
            version_number: Set(1),
            graph: Set(json!({ "nodes": [{"id": "imp", "kind": "ingest", "config":
                {"step": "ontology_import", "import_id": import_id, "action": "apply"}}] })),
            ..Default::default()
        }.insert(&env.db).await.unwrap();
        let mut am: flows::ActiveModel = flow.clone().into();
        am.deployed_version_id = Set(Some(version.id));
        am.update(&env.db).await.unwrap();
        let node = json!({"org_id": org, "flow_id": flow.id, "version": 1, "node_id": "imp"});
        let q = |sql: String| env.db.query_one_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql));

        // Slower than the wait: pending, the pass id kept in the message.
        let pass = uuid::Uuid::new_v4().to_string();
        let msg = json!({"run_id": pass});
        let mut r = ingest(&server, node.clone(), &msg).await;
        assert_eq!(r["pending"], true, "{r}");
        assert_eq!(r["msg"]["run_id"], pass);
        for _ in 0..50 {
            if r["pending"] != true {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            r = ingest(&server, node.clone(), &r["msg"]).await;
        }
        assert_eq!((r["port"].clone(), r["manifest"]["state"].clone()), (json!(0), json!("applied")), "{r}");
        assert_eq!(r["manifest"]["created"], 1, "{r}");
        let steps = format!("SELECT count(*)::bigint AS n FROM ingest_steps WHERE org_id = {org} AND node_id = 'imp'");
        let runs = format!("SELECT count(*)::bigint AS n FROM object_import_runs WHERE import_id = '{import_id}'");
        assert_eq!((count(q(steps.clone()).await.unwrap()), count(q(runs.clone()).await.unwrap())), (1, 1));

        // The same call again: the journaled answer, nothing run twice.
        let again = ingest(&server, node.clone(), &msg).await;
        assert_eq!((again["port"].clone(), again["manifest"].clone()), (r["port"].clone(), r["manifest"].clone()), "{again}");
        assert_eq!((count(q(steps.clone()).await.unwrap()), count(q(runs.clone()).await.unwrap())), (1, 1));

        // A run whose server stopped beating: failed, then journaled.
        let dead = uuid::Uuid::new_v4();
        env.db.execute_raw(Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "INSERT INTO object_import_runs (id, org_id, import_id, number, state, trigger, import_version, \
               heartbeat_at, ingest_run_id, ingest_node_id) \
             SELECT gen_random_uuid(), {org}, '{import_id}', max(number) + 1, 'running', 'flow', 1, \
               now() - interval '11 minutes', '{dead}', 'imp' FROM object_import_runs WHERE import_id = '{import_id}'"))).await.unwrap();
        let r = ingest(&server, node.clone(), &json!({"run_id": dead})).await;
        assert_eq!((r["port"].clone(), r["code"].clone()), (json!(1), json!("import-run-interrupted")), "{r}");
        let state = q(format!("SELECT state FROM object_import_runs WHERE ingest_run_id = '{dead}'"))
            .await.unwrap().unwrap().try_get::<String>("", "state").unwrap();
        assert_eq!(state, "failed");
        assert_eq!(count(q(steps).await.unwrap()), 2);
        unsafe { std::env::remove_var("PNEX_INGEST_WAIT_MS") };
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}

/// The shape of the RS XML export (v4.1); partner names and SIRET made up.
const RS_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<FICHES>
    <VERSION_FLUX>4.1</VERSION_FLUX>
    <FICHE>
        <NUMERO_FICHE>RS0101</NUMERO_FICHE>
        <INTITULE>Piloter le référencement d'un site</INTITULE>
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
        <LIEN_URL_DESCRIPTION>▷ Certification « Piloter le référencement d'un site »           </LIEN_URL_DESCRIPTION>
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
        for (code, props) in [("RS0101", json!({"code": "RS0101"})), ("RNCP183", json!({"code": "RNCP183"}))] {
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
                Some(json!({"link_type": "habilite_pour", "source_id": ids[siren], "target_id": ids["RS0101"]}))).await;
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
        // (C has none: none is opened), RS0101 enriched.
        assert_eq!((c["created"].clone(), c["links_opened"].clone(), c["links_updated"].clone(), c["updated"].clone()),
            (json!(6), json!(6), json!(2), json!(1)), "{p}");
        let (_, r) = apply(&server, &env.alice, org, &id, &p, &v).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let c3 = props_of(&env.db, org, "competence", "cle", "RS0101-C3").await;
        assert_eq!(c3["rang"].as_f64(), Some(3.0), "{c3}");
        assert!(c3["libelle"].as_str().unwrap().starts_with("Optimiser les différents contenus"), "{c3}");
        assert_eq!(props_of(&env.db, org, "competence", "cle", "RS56-C1").await, Value::Null);
        let cert = props_of(&env.db, org, "certification", "code", "RS0101").await;
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
        // Edges are matched by organization: their ids are random UUIDs.
        let of = |siren: &str| states.iter().find(|s| s.0 == ids[siren]).cloned().unwrap();
        // A: one SIRET still active wins over the inactive one.
        assert_eq!((of("111111111").1, of("222222222").1), (json!("actif"), json!("inactif")), "{states:?}");
        assert_eq!(of("222222222").2, json!("2026-05-12"));

        // Same file again: nothing to do.
        let p = preview(&server, &env.alice, org, &id, RS_XML).await;
        let c = &p["run"]["counts"];
        assert_eq!((c["created"].clone(), c["updated"].clone(), c["links_opened"].clone(), c["links_updated"].clone(), c["closed"].clone()),
            (json!(0), json!(0), json!(0), json!(0), json!(0)), "{p}");
    })
    .await;
}
