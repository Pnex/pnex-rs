//! Isolation multi-tenant au niveau HTTP : deux utilisateurs (tokens mock
//! signés par un faux Keycloak) ne doivent jamais voir ni modifier les
//! organisations l'un de l'autre ; les règles de rôle s'appliquent.
//!
//! Nécessite PostgreSQL (DATABASE_URL / default pnex_test) — la base de test
//! est créée/supprimée par le framework loco.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boot l'app sur une base de test avec un faux Keycloak, exécute le callback.
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_keycloak().await;
    // Rend visibles les warnings de l'extracteur (rejet JWT, provisioning).
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    // settings.keycloak.base_url lit KEYCLOAK_URL — fixé avant le boot.
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
            // La base de test est vierge : on y met le tier Free (le seed
            // complet est une tâche, hors périmètre ici).
            use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
            use pnex_backend::models::_entities::subscription_tiers as tiers;
            if tiers::Entity::find()
                .filter(tiers::Column::Name.eq("Free"))
                .one(&ctx.db)
                .await
                .expect("lookup tier Free")
                .is_none()
            {
                tiers::ActiveModel {
                    name: Set("Free".into()),
                    max_sensor_devices: Set(3),
                    max_actuator_devices: Set(1),
                    max_mixed_devices: Set(0),
                    min_build_interval_secs: Set(300),
                    data_retention_secs: Set(Some(86_400)),
                    ..Default::default()
                }
                .insert(&ctx.db)
                .await
                .expect("insert tier Free");
            }
            f(server, env).await;
        },
    )
    .await;
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
#[serial]
async fn sans_token_toute_l_api_est_refusee() {
    with_app(|server, _env| async move {
        let res = server.get("/api/v1/user-info").await;
        assert_eq!(res.status_code(), 401);
        let res = server.get("/api/v1/orgs").await;
        assert_eq!(res.status_code(), 401);
        let res = server.post("/api/v1/orgs").await;
        assert_eq!(res.status_code(), 401);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn jit_provisioning_cree_user_profil_et_org_owner() {
    with_app(|server, env| async move {
        let res = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 200);
        let body: serde_json::Value = res.json();
        let orgs = body["orgs"].as_array().expect("orgs");
        assert_eq!(orgs.len(), 1, "org personnelle créée : {orgs:?}");
        assert_eq!(orgs[0]["role"], "owner");
        assert_eq!(
            orgs[0]["subscription_tier"]["name"], "Free",
            "org personnelle sur le tier Free"
        );
        let profile = body["profile"].as_object().expect("profil créé");
        assert_eq!(profile["language"], "en");
        assert_eq!(profile["timezone"], "UTC");

        // Idempotent : deuxième appel = même user, pas de doublon d'org.
        let res2 = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        let body2: serde_json::Value = res2.json();
        assert_eq!(body2["id"], body["id"]);
        assert_eq!(body2["orgs"].as_array().unwrap().len(), 1);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn un_tenant_ne_voit_pas_les_orgs_de_l_autre() {
    with_app(|server, env| async move {
        // Provisionne alice puis bob.
        for token in [&env.alice, &env.bob] {
            let res = server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
            assert_eq!(res.status_code(), 200);
        }

        // Org perso d'alice.
        let alice_orgs: serde_json::Value = server
            .get("/api/v1/orgs")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json();
        let alice_org_id = alice_orgs[0]["id"].as_i64().expect("id org alice");

        // Bob ne voit pas l'org d'alice, ni en lecture ni en écriture.
        let res = server
            .get(&format!("/api/v1/orgs/{alice_org_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .await;
        assert_eq!(res.status_code(), 404, "org d'alice invisible pour bob");
        let res = server
            .patch(&format!("/api/v1/orgs/{alice_org_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "name": "Hack" }))
            .await;
        assert_eq!(res.status_code(), 404, "org d'alice non modifiable par bob");
        let res = server
            .delete(&format!("/api/v1/orgs/{alice_org_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .await;
        assert_eq!(res.status_code(), 404);

        // Bob ne peut pas non plus s'ajouter dans l'org d'alice.
        let res = server
            .post(&format!("/api/v1/orgs/{alice_org_id}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "email": "bob@example.com" }))
            .await;
        assert_eq!(res.status_code(), 404);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn roles_viewer_owner_et_garde_fous() {
    with_app(|server, env| async move {
        for token in [&env.alice, &env.bob] {
            server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
        }

        // Alice crée une org partagée et y ajoute bob viewer.
        let created: serde_json::Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Co" }))
            .await
            .json();
        let org_id = created["id"].as_i64().expect("id Atelier Co");

        let added: serde_json::Value = server
            .post(&format!("/api/v1/orgs/{org_id}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await
            .json();
        assert_eq!(added["role"], "viewer", "rôle lowercase en entrée/sortie");

        // Bob (viewer) lit, mais ne peut pas écrire.
        let res = server
            .get(&format!("/api/v1/orgs/{org_id}"))
            .add_header("Authorization", bearer(&env.bob))
            .await;
        assert_eq!(res.status_code(), 200);
        let detail: serde_json::Value = res.json();
        assert_eq!(detail["role"], "viewer");
        assert_eq!(detail["members"].as_array().unwrap().len(), 2);

        let res = server
            .patch(&format!("/api/v1/orgs/{org_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "name": "Hack Co" }))
            .await;
        assert_eq!(res.status_code(), 403, "viewer ne renomme pas");
        let res = server
            .post(&format!("/api/v1/orgs/{org_id}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "email": "alice@example.com" }))
            .await;
        assert_eq!(res.status_code(), 403, "viewer n'ajoute pas de membre");

        // Alice ne peut pas quitter son rôle de dernier owner.
        let alice_user_id: i64 = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.alice))
            .await
            .json::<serde_json::Value>()["id"]
            .as_i64()
            .expect("id alice");
        let res = server
            .delete(&format!("/api/v1/orgs/{org_id}/members/{alice_user_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 409, "dernier owner inamovible");

        // Bob viewer tente de se promouvoir : refusé (owner requis).
        let bob_user_id: i64 = server
            .get("/api/v1/user-info")
            .add_header("Authorization", bearer(&env.bob))
            .await
            .json::<serde_json::Value>()["id"]
            .as_i64()
            .expect("id bob");
        let res = server
            .patch(&format!("/api/v1/orgs/{org_id}/members/{bob_user_id}"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.bob))
            .json(&serde_json::json!({ "role": "owner" }))
            .await;
        assert_eq!(res.status_code(), 403, "viewer ne se promeut pas");
    })
    .await;
}
