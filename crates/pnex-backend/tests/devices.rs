//! Tests de parité du domaine devices (Phase 4) : CRUD scopé org, filtres,
//! réactivation implicite, quotas tier, update metadata-only, catalogue
//! global partagé.
//!
//! Nécessite PostgreSQL (TEST_DATABASE_URL) — base vidée entre tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boot l'app + seed du catalogue minimal (tier Free 3/1/0, types sensor/
/// actuator/mixed, capabilities, board, predefined devices).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    // Tier quotas only apply to a SaaS deployment (O37).
    unsafe { std::env::set_var("PNEX_DEPLOYMENT_MODE", "saas") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let env = Env {
        alice: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000a",
            "alice",
            "alice@example.com",
        ),
        bob: common::valid_token(
            &base,
            "00000000-0000-0000-0000-00000000000b",
            "bob",
            "bob@example.com",
        ),
    };
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, env, ctx).await;
        },
    )
    .await;
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Org personnelle de l'utilisateur (créée par JIT provisioning).
async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("org personnelle")
}

async fn create_device(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    device_id: &str,
    predefined: &str,
) -> axum_test::TestResponse {
    server
        .post("/api/v1/devices")
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "device_id": device_id,
            "predefined_device_name": predefined,
        }))
        .await
}

async fn list_devices(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    query: &str,
) -> serde_json::Value {
    server
        .get(&format!("/api/v1/devices{query}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .await
        .json()
}

async fn patch_device(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    id: i64,
    body: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .patch(&format!("/api/v1/devices/{id}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/json")
        .json(&body)
        .await
}

#[tokio::test]
#[serial]
async fn sans_token_devices_et_catalogue_refuses() {
    with_app(|server, _env, _ctx| async move {
        for (method, path) in [
            ("GET", "/api/v1/devices"),
            ("POST", "/api/v1/devices"),
            ("GET", "/api/v1/device-capabilities"),
            ("GET", "/api/v1/predefined-devices"),
        ] {
            let res = match method {
                "GET" => server.get(path).await,
                _ => server.post(path).await,
            };
            assert_eq!(res.status_code(), 401, "{method} {path} sans token");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn cycle_creation_reactivation_et_refus_device_actif() {
    with_app(|server, env, ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Création : inactive + token + clé de chiffrement (base64 44 chars).
        let res = create_device(&server, &env.alice, org, "esp-001", "soil_sensor").await;
        assert_eq!(res.status_code(), 201, "création → 201");
        let body: serde_json::Value = res.json();
        let device_pk = body["id"].as_i64().expect("id");
        assert_eq!(body["active"], false, "created inactive (legacy parity)");
        assert_eq!(body["org_id"], org);
        assert_eq!(body["device_type"], "sensor");
        assert_eq!(body["predefined_device_name"], "soil_sensor");
        assert_eq!(body["allow_dynamic_measurements"], false);
        assert_eq!(body["discovered_measurements"], serde_json::json!([]));
        let caps = body["capabilities"].as_array().expect("capabilities");
        assert_eq!(caps[0]["name"], "read_temperature");
        assert_eq!(caps[0]["mode"], "input", "mode minuscule sur le wire");
        let token = body["device_token"].as_object().expect("token auto");
        assert!(token["token"].as_str().is_some_and(|t| t.len() >= 40));
        assert_eq!(token["encryption_key"].as_str().unwrap().len(), 44);
        assert_eq!(token["is_active"], true);

        // Edge agent: the only family with free-form measurements.
        let res = create_device(&server, &env.alice, org, "agent-1", "edge_agent").await;
        let agent: serde_json::Value = res.json();
        assert_eq!(agent["allow_dynamic_measurements"], true);

        // Device inactif connu → réactivation 200 (pas de nouvelle création).
        let res = create_device(&server, &env.alice, org, "esp-001", "soil_sensor").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.json::<serde_json::Value>()["reactivated"], true);
        let list: serde_json::Value = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            list["results"].as_array().unwrap().len(),
            2,
            "pas de doublon : {list:?}"
        );

        // Device inactif + token désactivé → réactivation réactive le token.
        use pnex_backend::models::_entities::{device_registries, device_tokens};
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
        let dev = device_registries::Entity::find_by_id(device_pk)
            .one(&ctx.db)
            .await
            .expect("find")
            .expect("device");
        let tok_row = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(device_pk))
            .one(&ctx.db)
            .await
            .expect("token")
            .expect("token");
        let mut t: device_tokens::ActiveModel = tok_row.into();
        t.is_active = Set(false);
        t.update(&ctx.db).await.expect("désactive token");
        let mut d: device_registries::ActiveModel = dev.into();
        d.active = Set(false);
        d.update(&ctx.db).await.expect("désactive device");
        let res = create_device(&server, &env.alice, org, "esp-001", "soil_sensor").await;
        assert_eq!(res.status_code(), 200, "réactivation device inactif");
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/devices/{device_pk}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["active"], true);
        assert_eq!(detail["device_token"]["is_active"], true, "token réactivé");

        // Device actif → 400 exact.
        let res = create_device(&server, &env.alice, org, "esp-001", "soil_sensor").await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "device-already-active"
        );

        // Predefined inconnu → 400 champ-par-champ.
        let res = create_device(&server, &env.alice, org, "esp-x", "inconnu").await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["predefined_device_name"],
            "PredefinedDevice with name inconnu does not exist."
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn filtres_de_liste() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        for (id, predefined) in [("esp-s1", "soil_sensor"), ("esp-a1", "relay_1ch")] {
            create_device(&server, &env.alice, org, id, predefined).await;
        }
        let get = |query: &'static str| list_devices(&server, &env.alice, org, query);

        let all = get("").await;
        assert_eq!(all["results"].as_array().unwrap().len(), 2);
        assert_eq!(all["count"], 2);

        let sensors = get("?device_type=sensor").await;
        assert_eq!(sensors["results"].as_array().unwrap().len(), 1);
        assert_eq!(sensors["results"][0]["device_id"], "esp-s1");

        // "all" = no-op (legacy parity).
        assert_eq!(
            get("?device_type=all").await["results"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let by_cap = get("?capability=relay").await;
        assert_eq!(by_cap["results"].as_array().unwrap().len(), 1);
        assert_eq!(by_cap["results"][0]["device_id"], "esp-a1");

        let by_id = get("?device_id=esp-s1").await;
        assert_eq!(by_id["results"].as_array().unwrap().len(), 1);

        // Recherche multi-champs : par identifiant, puis par capacité.
        let by_search = get("?search=S1").await;
        assert_eq!(by_search["results"].as_array().unwrap().len(), 1);
        assert_eq!(by_search["results"][0]["device_id"], "esp-s1");
        let by_search_cap = get("?search=relay").await;
        assert_eq!(by_search_cap["results"].as_array().unwrap().len(), 1);
        assert_eq!(by_search_cap["results"][0]["device_id"], "esp-a1");
        // Casse ignorée, terme introuvable → vide.
        assert_eq!(
            get("?search=ESP-A1").await["results"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            get("?search=zzz").await["results"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        // Aucun actif : le filtre active=true vide la liste.
        assert_eq!(
            get("?active=true").await["results"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            get("?active=false").await["results"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    })
    .await;
}

/// No device update route: a device's identity and model are fixed at
/// registration (board and screen have their own routes).
#[tokio::test]
#[serial]
async fn device_has_no_generic_update_route() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created: serde_json::Value =
            create_device(&server, &env.alice, org, "esp-001", "soil_sensor")
                .await
                .json();
        let id = created["id"].as_i64().expect("id");
        assert!(created.get("metadata").is_none(), "{created}");
        let res = patch_device(
            &server,
            &env.alice,
            org,
            id,
            serde_json::json!({ "metadata": { "location": "serre" } }),
        )
        .await;
        assert_eq!(res.status_code(), 405, "{}", res.text());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn quotas_tier_par_type() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;

        // Tier Free : 3 sensors, 1 actuator, 0 mixed.
        for n in 1..=3 {
            let res = create_device(
                &server,
                &env.alice,
                org,
                &format!("esp-s{n}"),
                "soil_sensor",
            )
            .await;
            assert_eq!(res.status_code(), 201, "capteur {n}");
        }
        let res = create_device(&server, &env.alice, org, "esp-s4", "soil_sensor").await;
        assert_eq!(res.status_code(), 400, "4e capteur refusé");
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["error"], "device-quota-reached");
        assert_eq!(body["errors"]["args"]["type"], "sensor");

        // Inactive devices count toward the quota (legacy parity):
        // un seul actuator créé inactif → le 2e est déjà au-dessus du quota.
        let res = create_device(&server, &env.alice, org, "esp-a1", "relay_1ch").await;
        assert_eq!(res.status_code(), 201);
        let res = create_device(&server, &env.alice, org, "esp-a2", "relay_1ch").await;
        assert_eq!(res.status_code(), 400);
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["error"], "device-quota-reached");
        assert_eq!(body["errors"]["args"]["type"], "actuator");

        // mixed : 1 autorisé (Brick 0 — device générique prototypable en Free),
        // le 2e est refusé.
        let res = create_device(&server, &env.alice, org, "esp-m1", "mixed_hub_v1").await;
        assert_eq!(res.status_code(), 201);
        let res = create_device(&server, &env.alice, org, "esp-m2", "mixed_hub_v1").await;
        assert_eq!(res.status_code(), 400);
        let body = res.json::<serde_json::Value>();
        assert_eq!(body["error"], "device-quota-reached");
        assert_eq!(body["errors"]["args"]["type"], "mixed");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_tenant_et_roles() {
    with_app(|server, env, ctx| async move {
        let alice_org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;

        // Même device_id dans deux orgs : deux devices distincts.
        let created: serde_json::Value =
            create_device(&server, &env.alice, alice_org, "esp-001", "soil_sensor")
                .await
                .json();
        let alice_device = created["id"].as_i64().expect("id");
        let created_bob: serde_json::Value =
            create_device(&server, &env.bob, bob_org, "esp-001", "soil_sensor")
                .await
                .json();
        assert_ne!(
            created_bob["id"].as_i64().unwrap(),
            alice_device,
            "devices distincts par org"
        );

        // Chaque org ne voit que ses devices.
        for (token, org_id, expected) in [(&env.alice, alice_org, 1), (&env.bob, bob_org, 1)] {
            let list: serde_json::Value = server
                .get("/api/v1/devices")
                .add_header("Authorization", bearer(token))
                .add_header("X-Org-Id", org_id.to_string())
                .await
                .json();
            assert_eq!(list["results"].as_array().unwrap().len(), expected);
        }

        // Bob n'atteint pas le device d'alice (ni lecture, ni écriture).
        let res = server
            .get(&format!("/api/v1/devices/{alice_device}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);
        let res = server
            .delete(&format!("/api/v1/devices/{alice_device}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", bob_org.to_string())
            .await;
        assert_eq!(res.status_code(), 404);

        // Sans X-Org-Id : rejet (le scoping est explicite).
        let res = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);

        // Viewer : lecture OK, écriture refusée.
        server
            .post(&format!("/api/v1/orgs/{alice_org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let res = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await;
        assert_eq!(res.status_code(), 200, "viewer lit les devices de l'org");
        let res = create_device(&server, &env.bob, alice_org, "esp-v", "soil_sensor").await;
        assert_eq!(res.status_code(), 403, "viewer ne crée pas");

        // SEC-8: device credentials never reach a viewer (list + detail),
        // nor the firmware image that embeds them; the owner keeps them.
        let list: serde_json::Value = server
            .get("/api/v1/devices")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await
            .json();
        assert!(list["results"][0]["device_token"].is_null(), "{list}");
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/devices/{alice_device}"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await
            .json();
        assert!(detail["device_token"].is_null(), "{detail}");
        let owner: serde_json::Value = server
            .get(&format!("/api/v1/devices/{alice_device}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", alice_org.to_string())
            .await
            .json();
        assert!(owner["device_token"]["token"].is_string(), "{owner}");
        let res = server
            .get("/api/v1/download/firmware/esp-001")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await;
        assert_eq!(
            res.status_code(),
            403,
            "viewer ne télécharge pas le firmware"
        );

        // SEC-7: a viewer neither deploys nor cancels an OTA rollout.
        let res = server
            .post(&format!("/api/v1/devices/{alice_device}/ota"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .add_header("Content-Type", "application/json")
            .json(&serde_json::json!({ "force": true }))
            .await;
        assert_eq!(res.status_code(), 403, "viewer ne déploie pas d'OTA");
        let res = server
            .delete(&format!("/api/v1/devices/{alice_device}/ota"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await;
        assert_eq!(res.status_code(), 403, "viewer n'annule pas d'OTA");
        let _ = ctx;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn suppression_nettoie_token_et_build_records() {
    with_app(|server, env, ctx| async move {
        use pnex_backend::models::_entities::{build_records, device_registries, device_tokens};
        use sea_orm::{
            ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set,
        };

        let org = personal_org(&server, &env.alice).await;
        let created: serde_json::Value =
            create_device(&server, &env.alice, org, "esp-001", "soil_sensor")
                .await
                .json();
        let id = created["id"].as_i64().expect("id");

        // Deux enregistrements firmware à nettoyer.
        for phase in ["compile", "link"] {
            build_records::ActiveModel {
                device_id: Set(Some("esp-001".into())),
                build_phase: Set(phase.into()),
                org_id: Set(org),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await
            .expect("build record");
        }

        let res = server
            .delete(&format!("/api/v1/devices/{id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);

        let remaining_dev = device_registries::Entity::find()
            .filter(device_registries::Column::Id.eq(id))
            .one(&ctx.db)
            .await
            .unwrap();
        assert!(remaining_dev.is_none(), "device supprimé");
        let remaining_tok = device_tokens::Entity::find()
            .filter(device_tokens::Column::DeviceRegistryId.eq(id))
            .one(&ctx.db)
            .await
            .unwrap();
        assert!(remaining_tok.is_none(), "token supprimé");
        let remaining_builds = build_records::Entity::find()
            .filter(build_records::Column::OrgId.eq(org))
            .count(&ctx.db)
            .await
            .unwrap();
        assert_eq!(remaining_builds, 0, "build records nettoyés");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn latest_build_hydrate_liste_et_detail() {
    with_app(|server, env, ctx| async move {
        use pnex_backend::models::_entities::build_records;
        use sea_orm::{ActiveModelTrait, Set};

        let org = personal_org(&server, &env.alice).await;
        let s1: serde_json::Value =
            create_device(&server, &env.alice, org, "esp-s1", "soil_sensor")
                .await
                .json();
        create_device(&server, &env.alice, org, "esp-a1", "soil_sensor").await;

        // Record de build succeeded pour esp-s1 uniquement (insertion directe —
        // un record par (org, device_id), upsert côté contrôleur builds).
        build_records::ActiveModel {
            device_id: Set(Some("esp-s1".into())),
            build_phase: Set("succeeded".into()),
            org_id: Set(org),
            ..Default::default()
        }
        .insert(&ctx.db)
        .await
        .expect("build record");

        // Liste : hydratation par device de la page (colonne Firmware).
        let list = list_devices(&server, &env.alice, org, "").await;
        let results = list["results"].as_array().expect("results");
        let s1_row = results
            .iter()
            .find(|d| d["device_id"] == "esp-s1")
            .expect("esp-s1 en liste");
        let build = s1_row["latest_build"].as_object().expect("latest_build");
        assert!(build.get("success").is_none());
        assert_eq!(build["build_phase"], "succeeded");
        assert!(build["updated_at"].as_str().is_some(), "RFC 3339");
        let a1_row = results
            .iter()
            .find(|d| d["device_id"] == "esp-a1")
            .expect("esp-a1 en liste");
        assert!(a1_row["latest_build"].is_null(), "sans build → null");

        // Détail : même hydratation via device_full.
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/devices/{}", s1["id"].as_i64().unwrap()))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["latest_build"]["build_phase"], "succeeded");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn catalogue_global_partage() {
    with_app(|server, env, _ctx| async move {
        // Capabilities : formes exactes + filtre mode (+ valeur inconnue vide).
        let caps: serde_json::Value = server
            .get("/api/v1/device-capabilities")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        let caps = caps["results"].as_array().expect("enveloppe");
        assert!(caps
            .iter()
            .any(|c| c["name"] == "relay" && c["mode"] == "output"));
        assert!(caps.iter().any(|c| c["mode"] == "input"));

        let outputs: serde_json::Value = server
            .get("/api/v1/device-capabilities?mode=output")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(outputs["results"].as_array().unwrap().len(), 1);

        let none: serde_json::Value = server
            .get("/api/v1/device-capabilities?mode=bidon")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(none["results"].as_array().unwrap().len(), 0);

        // Predefined devices : capabilities = noms, filtres combinables.
        let pds: serde_json::Value = server
            .get("/api/v1/predefined-devices")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        let pds = pds["results"].as_array().expect("enveloppe");
        assert_eq!(
            pds.len(),
            6,
            "4 predefined + generic_esp8266 (Brick 0) + edge_agent (D95)"
        );
        let relay_pd = pds
            .iter()
            .find(|p| p["name"] == "relay_1ch")
            .expect("relay_1ch");
        assert_eq!(relay_pd["device_type"], "actuator");
        assert_eq!(relay_pd["board"], "esp32");
        assert_eq!(relay_pd["capabilities"], serde_json::json!(["relay"]));
        assert!(
            relay_pd.get("id").is_none(),
            "pas d'id dans le contrat catalogue"
        );

        // Filtres : device_type, capabilities (OU), name icontains.
        let sensors: serde_json::Value = server
            .get("/api/v1/predefined-devices?device_type=sensor")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        // soil_sensor is the only sensor model of the test catalogue.
        assert_eq!(sensors["results"].as_array().unwrap().len(), 1);

        let by_caps: serde_json::Value = server
            .get("/api/v1/predefined-devices?capabilities=relay&capabilities=read_temperature")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            by_caps["results"].as_array().unwrap().len(),
            3,
            "OU sur capabilities : soil_sensor, relay_1ch et mixed_hub_v1"
        );

        let icontains: serde_json::Value = server
            .get("/api/v1/predefined-devices?name=SOIL")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(icontains["results"].as_array().unwrap().len(), 1);

        // Recherche multi-champs (D14) : board, capacité, type — en SQL.
        let by_board: serde_json::Value = server
            .get("/api/v1/predefined-devices?search=ESP32")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            by_board["count"], 5,
            "esp32 board only (edge_agent included) — generic_esp8266 is on esp8266, filtered out"
        );
        let by_cap_search: serde_json::Value = server
            .get("/api/v1/predefined-devices?search=RELAY")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            by_cap_search["count"], 2,
            "relay_1ch (nom et cap) + mixed_hub_v1 (cap relay)"
        );
        let by_type_search: serde_json::Value = server
            .get("/api/v1/predefined-devices?search=actuator")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(by_type_search["count"], 1);

        // Bob voit le même catalogue (global, pas scopé org).
        let bob_pds: serde_json::Value = server
            .get("/api/v1/predefined-devices")
            .add_header("Authorization", bearer(&env.bob))
            .await
            .json();
        assert_eq!(bob_pds["results"].as_array().unwrap().len(), 6);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn pagination_des_listes() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        // Tier Free : 3 capteurs max — parfait pour 3 pages de 2.
        for n in 1..=3 {
            create_device(
                &server,
                &env.alice,
                org,
                &format!("esp-s{n}"),
                "soil_sensor",
            )
            .await;
        }

        // Registre : page 1 → next explicite, previous absent.
        let page1 = list_devices(&server, &env.alice, org, "?limit=2").await;
        assert_eq!(page1["count"], 3);
        assert_eq!(page1["results"].as_array().unwrap().len(), 2);
        assert_eq!(
            page1["next"].as_str().unwrap(),
            "/api/v1/devices?limit=2&offset=2"
        );
        assert!(page1["previous"].is_null());

        // Dernière page incomplète : next absent, previous pointe en 0.
        let page2 = list_devices(&server, &env.alice, org, "?limit=2&offset=2").await;
        assert_eq!(page2["results"].as_array().unwrap().len(), 1);
        assert!(page2["next"].is_null());
        assert_eq!(
            page2["previous"].as_str().unwrap(),
            "/api/v1/devices?limit=2&offset=0"
        );

        // Offset au-delà de la fin : page vide cohérente.
        let beyond = list_devices(&server, &env.alice, org, "?limit=2&offset=9").await;
        assert_eq!(beyond["count"], 3);
        assert_eq!(beyond["results"].as_array().unwrap().len(), 0);
        assert!(beyond["next"].is_null());

        // Les liens conservent les filtres actifs.
        let filtered = list_devices(&server, &env.alice, org, "?device_type=sensor&limit=2").await;
        assert_eq!(
            filtered["next"].as_str().unwrap(),
            "/api/v1/devices?device_type=sensor&limit=2&offset=2"
        );

        // Défaut : 10 par page (var d'env PAGINATION_DEFAULT_LIMIT).
        let def = list_devices(&server, &env.alice, org, "").await;
        assert_eq!(def["count"], 3);
        assert_eq!(def["results"].as_array().unwrap().len(), 3, "3 < défaut 10");

        // Catalogue : pagination SQL (count exact, pages).
        let cat1: serde_json::Value = server
            .get("/api/v1/predefined-devices?limit=2")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            cat1["count"], 6,
            "4 + generic_esp8266 (Brick 0) + edge_agent (D95)"
        );
        assert_eq!(cat1["results"].as_array().unwrap().len(), 2);
        assert_eq!(
            cat1["next"].as_str().unwrap(),
            "/api/v1/predefined-devices?limit=2&offset=2"
        );
        let cat3: serde_json::Value = server
            .get("/api/v1/predefined-devices?limit=2&offset=2")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(cat3["results"].as_array().unwrap().len(), 2);
        // 5th and 6th predefined (generic_esp8266, edge_agent) → a last full page of 2.
        assert_eq!(
            cat3["next"].as_str().unwrap(),
            "/api/v1/predefined-devices?limit=2&offset=4"
        );
        let cat4: serde_json::Value = server
            .get("/api/v1/predefined-devices?limit=2&offset=4")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(cat4["results"].as_array().unwrap().len(), 2);
        assert!(cat4["next"].is_null(), "fin de catalogue");

        // Capabilities : même enveloppe.
        let caps: serde_json::Value = server
            .get("/api/v1/device-capabilities?limit=1")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(caps["count"], 2);
        assert_eq!(caps["results"].as_array().unwrap().len(), 1);
        assert_eq!(
            caps["next"].as_str().unwrap(),
            "/api/v1/device-capabilities?limit=1&offset=1"
        );

        // Orgs de l'utilisateur : enveloppe également (une org perso ici).
        let orgs: serde_json::Value = server
            .get("/api/v1/orgs")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(orgs["count"], 1);
        assert_eq!(orgs["results"].as_array().unwrap().len(), 1);
    })
    .await;
}

/// `GET /devices/{id}/pinout` expose `connected` (état WS, pas DB) : un
/// device jamais connecté répond `connected: false` avec ses pins overlay —
/// l'éditeur de flows s'en sert pour dire « hors ligne » au lieu d'un faux
/// « introuvable » quand le serveur a redémarré (retour utilisateur).
#[tokio::test]
#[serial]
async fn pinout_exposes_connected_flag() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        create_device(&server, &env.alice, org, "esp-off", "soil_sensor").await;
        let list = list_devices(&server, &env.alice, org, "?device_id=esp-off").await;
        let pk = list["results"][0]["id"].as_i64().expect("device créé");

        let body: serde_json::Value = server
            .get(&format!("/api/v1/devices/{pk}/pinout"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(body["connected"], false, "jamais connecté → hors ligne");
        assert_eq!(body["device_id"], "esp-off");
        assert!(body["pins"].is_array());
    })
    .await;
}
