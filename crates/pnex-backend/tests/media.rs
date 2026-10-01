//! Tests du domaine média (D21) : cycle upload/versioning/purge, isolation
//! org, rôles, pagination — scoping D2, envelope D14, purge storage vérifiée.
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

/// Boot l'app avec le MediaStore fs pointé sur un répertoire temporaire
/// jetable par test (env posée AVANT le boot — école RAUTHY_URL).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe {
        std::env::set_var(
            "PNEX_MEDIA_DIR",
            format!(
                "{}/pnex-media-tests-{}",
                std::env::temp_dir().display(),
                std::process::id()
            ),
        )
    };
    unsafe { std::env::set_var("MEDIA_BACKEND", "fs") };
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
            f(server, env).await;
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

/// JPEG minimal avec segment APP1 XMP (même encodage que le sniff GPano).
fn gpano_jpeg() -> Vec<u8> {
    let prefix = b"http://ns.adobe.com/xap/1.0/\x00";
    let xmp = r#"<rdf:Description GPano:ProjectionType="equirectangular"/>"#;
    let content = [prefix.as_slice(), xmp.as_bytes()].concat();
    let len = (content.len() + 2) as u16;
    let mut out = vec![0xFF, 0xD8];
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&content);
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

/// JPEG minimal sans GPano (photo simple).
fn plain_jpeg() -> Vec<u8> {
    vec![0xFF, 0xD8, 0xFF, 0xD9]
}

/// Upload octet-stream : POST /api/v1/media?name=&filename=&kind=&content_type=
async fn upload(
    server: &axum_test::TestServer,
    token: &str,
    org_id: i64,
    query: &str,
    body: Vec<u8>,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/media{query}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org_id.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(body.into())
        .await
}

#[tokio::test]
#[serial]
async fn cycle_upload_list_detail_versions() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=sphere.jpg&content_type=image%2Fjpeg",
            gpano_jpeg(),
        )
        .await;
        assert_eq!(
            res.status_code(),
            201,
            "upload GPano → 201 : {}",
            res.text()
        );
        let created: serde_json::Value = res.json();
        let asset_id = created["id"].as_str().expect("id uuid");
        assert_eq!(created["kind"], "panorama", "sniff GPano → panorama");
        assert_eq!(created["current_version_number"], 1);
        assert_eq!(created["versions_count"], 1);

        // Liste + filtres kind/search + envelope D14.
        let listed: serde_json::Value = server
            .get("/api/v1/media?kind=panorama")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1);
        assert_eq!(listed["results"][0]["id"], asset_id);

        // Liste multi-kinds (picker plan du studio : floorplan + photo) —
        // une valeur absente de la liste ne casse pas le filtre.
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=plan.jpg&kind=floorplan&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await;
        assert_eq!(res.status_code(), 201);
        let listed: serde_json::Value = server
            .get("/api/v1/media?kind=panorama,floorplan")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 2, "kind=a,b → les deux kinds : {}", listed);
        let listed: serde_json::Value = server
            .get("/api/v1/media?kind=panorama,splat")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            listed["count"], 1,
            "kind absent de la liste → exclu : {}",
            listed
        );

        let listed: serde_json::Value = server
            .get("/api/v1/media?search=introuvable")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0, "search sans match → count 0");
        // Détail + versions.
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/media/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["current_version_number"], 1);

        // Version 2 = photo simple, devient courante.
        let res = server
            .post(&format!(
                "/api/v1/media/{asset_id}/versions?filename=fix.jpg&content_type=image%2Fjpeg"
            ))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/octet-stream")
            .bytes(plain_jpeg().into())
            .await;
        assert_eq!(res.status_code(), 201);
        let updated: serde_json::Value = res.json();
        assert_eq!(updated["current_version_number"], 2);
        assert_eq!(updated["versions_count"], 2);

        // Restore v1.
        let res = server
            .post(&format!("/api/v1/media/{asset_id}/versions/1/restore"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let restored: serde_json::Value = res.json();
        assert_eq!(restored["current_version_number"], 1);

        // GET content (version courante) = octets exacts + content-type.
        let res = server
            .get(&format!("/api/v1/media/{asset_id}/content"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            res.header("content-type").to_str().unwrap(),
            "image/jpeg",
            "content-type réel de la version"
        );
        assert_eq!(res.into_bytes(), gpano_jpeg(), "octets exacts de v1");

        // Version n.
        let res = server
            .get(&format!("/api/v1/media/{asset_id}/versions/2/content"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.into_bytes(), plain_jpeg(), "octets exacts de v2");

        // DELETE asset → purge des deux clés du store (vérification directe).
        let dir = std::env::var("PNEX_MEDIA_DIR").unwrap();
        let res = server
            .delete(&format!("/api/v1/media/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        for blob in [
            format!("{dir}/org_{org}/media/{asset_id}/1_sphere.jpg"),
            format!("{dir}/org_{org}/media/{asset_id}/2_fix.jpg"),
        ] {
            assert!(
                !std::path::Path::new(&blob).exists(),
                "blob purgé du store fs : {blob}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_org_et_derniere_version() {
    with_app(|server, env| async move {
        let org1 = personal_org(&server, &env.alice).await;
        let org2 = personal_org(&server, &env.bob).await;
        let res = upload(
            &server,
            &env.alice,
            org1,
            "?filename=a.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await;
        assert_eq!(res.status_code(), 201);
        let asset_id = res.json::<serde_json::Value>()["id"]
            .as_str()
            .unwrap()
            .to_string();

        // Cross-org : GET/DELETE/restore → 404 masqué.
        for (method, path) in [
            ("GET", format!("/api/v1/media/{asset_id}")),
            ("DELETE", format!("/api/v1/media/{asset_id}")),
            (
                "POST",
                format!("/api/v1/media/{asset_id}/versions/1/restore"),
            ),
        ] {
            let req = match method {
                "GET" => server.get(&path),
                "DELETE" => server.delete(&path),
                _ => server.post(&path),
            };
            let res = req
                .add_header("Authorization", bearer(&env.bob))
                .add_header("X-Org-Id", org2.to_string())
                .await;
            assert_eq!(res.status_code(), 404, "{method} cross-org masqué");
        }

        // Dernière version : 409 last_version.
        let res = server
            .delete(&format!("/api/v1/media/{asset_id}/versions/1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .await;
        assert_eq!(res.status_code(), 409);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "media-last-version");

        // Sans X-Org-Id → 400.
        let res = server
            .get("/api/v1/media")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn roles_et_pagination() {
    with_app(|server, env| async move {
        // Provisionnement JIT des deux users (école tenant_isolation :
        // un user doit exister avant l'ajout comme member).
        for token in [&env.alice, &env.bob] {
            server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
        }

        // Org partagée : alice owner, bob viewer (école tenant_isolation).
        let created: serde_json::Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Media" }))
            .await
            .json();
        let org = created["id"].as_i64().unwrap();
        server
            .post(&format!("/api/v1/orgs/{org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({
                "email": "bob@example.com",
                "role": "viewer"
            }))
            .await;

        for i in 0..3 {
            let res = upload(
                &server,
                &env.alice,
                org,
                &format!("?filename=p{i}.jpg&content_type=image%2Fjpeg"),
                plain_jpeg(),
            )
            .await;
            assert_eq!(res.status_code(), 201);
        }

        // Viewer : lecture OK.
        let res = server
            .get("/api/v1/media")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);

        // Viewer : écriture interdite (upload + delete).
        let res = upload(
            &server,
            &env.bob,
            org,
            "?filename=x.jpg&content_type=image%2Fjpeg",
            plain_jpeg(),
        )
        .await;
        assert_eq!(res.status_code(), 403);
        let res = server
            .delete("/api/v1/media/00000000-0000-0000-0000-00000000000f")
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 403, "viewer delete → 403 avant 404");

        // Pagination limit=2 → count 3, next présent.
        let page: serde_json::Value = server
            .get("/api/v1/media?limit=2")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(page["count"], 3);
        assert_eq!(page["results"].as_array().unwrap().len(), 2);
        assert!(page["next"].is_string(), "lien next présent");
    })
    .await;
}
