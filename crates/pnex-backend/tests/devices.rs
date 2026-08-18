//! Tests de parité du domaine devices (Phase 4) : CRUD scopé org, filtres,
//! réactivation implicite, quotas tier, update metadata-only, catalogue
//! global partagé — cf. `docs/phase0/api-rest.md` §4.
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
    let base = common::spawn_mock_keycloak().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("KEYCLOAK_URL", &base) };
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
        assert_eq!(body["active"], false, "créé inactif (parité Django)");
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

        // Custom sensor : dynamic measurements autorisées.
        let res = create_device(&server, &env.alice, org, "esp-custom", "custom_sensor").await;
        let custom: serde_json::Value = res.json();
        assert_eq!(custom["allow_dynamic_measurements"], true);

        // Device inactif connu → réactivation 200 (pas de nouvelle création).
        let res = create_device(&server, &env.alice, org, "esp-001", "soil_sensor").await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            res.json::<serde_json::Value>()["detail"],
            "Device reactivated successfully."
        );
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
            res.json::<serde_json::Value>()["detail"],
            "This device is already registered and active."
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
        for (id, predefined) in [
            ("esp-s1", "soil_sensor"),
            ("esp-a1", "4_chan_relay"),
        ] {
            create_device(&server, &env.alice, org, id, predefined).await;
        }
        let get = |query: &'static str| list_devices(&server, &env.alice, org, query);

        let all = get("").await;
        assert_eq!(all["results"].as_array().unwrap().len(), 2);
        assert_eq!(all["count"], 2);

        let sensors = get("?device_type=sensor").await;
        assert_eq!(sensors["results"].as_array().unwrap().len(), 1);
        assert_eq!(sensors["results"][0]["device_id"], "esp-s1");

        // « all » = no-op (parité Django).
        assert_eq!(get("?device_type=all").await["results"].as_array().unwrap().len(), 2);

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
        assert_eq!(get("?search=ESP-A1").await["results"].as_array().unwrap().len(), 1);
        assert_eq!(get("?search=zzz").await["results"].as_array().unwrap().len(), 0);

        // Aucun actif : le filtre active=true vide la liste.
        assert_eq!(get("?active=true").await["results"].as_array().unwrap().len(), 0);
        assert_eq!(get("?active=false").await["results"].as_array().unwrap().len(), 2);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn update_metadata_uniquement() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let created: serde_json::Value =
            create_device(&server, &env.alice, org, "esp-001", "soil_sensor")
                .await
                .json();
        let id = created["id"].as_i64().expect("id");

        // metadata seul : OK, renvoyé tel quel.
        let res = patch_device(
            &server,
            &env.alice,
            org,
            id,
            serde_json::json!({ "metadata": { "location": "serre", "row": 3 } }),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        assert_eq!(body["metadata"]["location"], "serre");

        // Toute autre clé → 400 exact (contrat Django).
        for bad in [
            serde_json::json!({ "active": true }),
            serde_json::json!({ "metadata": {}, "device_id": "hack" }),
            serde_json::json!({}),
        ] {
            let res = patch_device(&server, &env.alice, org, id, bad.clone()).await;
            assert_eq!(res.status_code(), 400, "payload {bad:?}");
            assert_eq!(
                res.json::<serde_json::Value>()["detail"],
                "Only metadata updates are allowed."
            );
        }
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
            let res =
                create_device(&server, &env.alice, org, &format!("esp-s{n}"), "soil_sensor")
                    .await;
            assert_eq!(res.status_code(), 201, "capteur {n}");
        }
        let res = create_device(&server, &env.alice, org, "esp-s4", "soil_sensor").await;
        assert_eq!(res.status_code(), 400, "4e capteur refusé");
        assert_eq!(
            res.json::<serde_json::Value>()["detail"],
            "Device limit reached for sensor devices in your subscription tier."
        );

        // Les devices inactifs comptent dans le quota (parité Django) :
        // un seul actuator créé inactif → le 2e est déjà au-dessus du quota.
        let res = create_device(&server, &env.alice, org, "esp-a1", "4_chan_relay").await;
        assert_eq!(res.status_code(), 201);
        let res = create_device(&server, &env.alice, org, "esp-a2", "4_chan_relay").await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["detail"],
            "Device limit reached for actuator devices in your subscription tier."
        );

        // mixed = 0 : refus immédiat.
        let res = create_device(&server, &env.alice, org, "esp-m1", "mixed_hub_v1").await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["detail"],
            "Device limit reached for mixed devices in your subscription tier."
        );
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
        for (token, org_id, expected) in [
            (&env.alice, alice_org, 1),
            (&env.bob, bob_org, 1),
        ] {
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
        let _ = ctx;
    })
    .await;
}

#[tokio::test]
#[serial]
async fn suppression_nettoie_token_et_build_records() {
    with_app(|server, env, ctx| async move {
        use pnex_backend::models::_entities::{build_records, device_registries, device_tokens};
        use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};

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
                success: Set(true),
                build_phase: Set(Some(phase.into())),
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
            success: Set(true),
            build_phase: Set(Some("succeeded".into())),
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
        assert_eq!(build["success"], true);
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
        assert!(caps.iter().any(|c| c["name"] == "relay" && c["mode"] == "output"));
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
        assert_eq!(pds.len(), 4);
        let relay_pd = pds
            .iter()
            .find(|p| p["name"] == "4_chan_relay")
            .expect("4_chan_relay");
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
        assert_eq!(sensors["results"].as_array().unwrap().len(), 2);

        let by_caps: serde_json::Value = server
            .get("/api/v1/predefined-devices?capabilities=relay&capabilities=read_temperature")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            by_caps["results"].as_array().unwrap().len(),
            3,
            "OU sur capabilities : soil_sensor, 4_chan_relay et mixed_hub_v1"
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
        assert_eq!(by_board["count"], 4, "tous sur board esp32 (casse ignorée)");
        let by_cap_search: serde_json::Value = server
            .get("/api/v1/predefined-devices?search=RELAY")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        assert_eq!(
            by_cap_search["count"], 2,
            "4_chan_relay (nom et cap) + mixed_hub_v1 (cap relay)"
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
        assert_eq!(bob_pds["results"].as_array().unwrap().len(), 4);
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
            create_device(&server, &env.alice, org, &format!("esp-s{n}"), "soil_sensor").await;
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
        assert_eq!(cat1["count"], 4);
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
        assert!(cat3["next"].is_null());

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
