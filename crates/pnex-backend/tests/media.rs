//! Media domain tests (D21): upload/versioning/purge cycle, org isolation,
//! roles, pagination — D2 scoping, D14 envelope, storage purge checked.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL) — database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
}

/// Boots the app on the test object store (env set BEFORE the boot).
async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
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

/// The user's personal org (created by JIT provisioning).
async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<serde_json::Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

/// Minimal JPEG with an APP1 XMP segment (same encoding as the GPano sniff).
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

/// Minimal JPEG without GPano (plain photo).
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
    with_app(|server, env, _ctx| async move {
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

        // List + kind/search filters + D14 envelope.
        let listed: serde_json::Value = server
            .get("/api/v1/media?kind=panorama")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 1);
        assert_eq!(listed["results"][0]["id"], asset_id);

        // Multi-kind list (studio plan picker: floorplan + photo) —
        // a value missing from the list doesn't break the filter.
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
        assert_eq!(listed["count"], 2, "kind=a,b → both kinds: {}", listed);
        let listed: serde_json::Value = server
            .get("/api/v1/media?kind=panorama,splat")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(
            listed["count"], 1,
            "kind absent from the list → excluded: {}",
            listed
        );

        let listed: serde_json::Value = server
            .get("/api/v1/media?search=introuvable")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(listed["count"], 0, "search without a match → count 0");
        // Detail + versions.
        let detail: serde_json::Value = server
            .get(&format!("/api/v1/media/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(detail["current_version_number"], 1);

        // Version 2 = plain photo, becomes current.
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

        // Versions are a D14 page (D240).
        let page: serde_json::Value = server
            .get(&format!("/api/v1/media/{asset_id}/versions?limit=1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(page["count"], 2, "{page}");
        assert_eq!(page["results"][0]["version_number"], 1, "{page}");
        assert!(page["next"].is_string(), "{page}");

        // Field errors are machine tokens under `errors.fields` (D239).
        let res = upload(&server, &env.alice, org, "?filename=x.jpg", Vec::new()).await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "field-invalid");
        assert_eq!(body["errors"]["fields"]["filename"], "required");
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=x.jpg&kind=nope",
            plain_jpeg(),
        )
        .await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(
            res.json::<serde_json::Value>()["errors"]["fields"]["kind"],
            "invalid"
        );
        let res = upload(
            &server,
            &env.alice,
            org,
            "?filename=x.jpg&kind=mesh",
            plain_jpeg(),
        )
        .await;
        assert_eq!(
            res.status_code(),
            400,
            "a JPEG is not a mesh: {}",
            res.text()
        );
        assert_eq!(
            res.json::<serde_json::Value>()["error"],
            "media-format-unsupported"
        );

        // Restore v1.
        let res = server
            .post(&format!("/api/v1/media/{asset_id}/versions/1/restore"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let restored: serde_json::Value = res.json();
        assert_eq!(restored["current_version_number"], 1);

        // GET content (current version) = exact bytes + content-type.
        let res = server
            .get(&format!("/api/v1/media/{asset_id}/content"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        assert_eq!(
            res.header("content-type").to_str().unwrap(),
            "image/jpeg",
            "actual content-type of the version"
        );
        assert_eq!(res.into_bytes(), gpano_jpeg(), "octets exacts de v1");

        // Version n.
        let res = server
            .get(&format!("/api/v1/media/{asset_id}/versions/2/content"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.into_bytes(), plain_jpeg(), "octets exacts de v2");

        // DELETE asset purges both store keys (checked directly).
        let store = common::media_store();
        let res = server
            .delete(&format!("/api/v1/media/{asset_id}"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 204);
        for blob in [
            format!("org_{org}/media/{asset_id}/1_sphere.jpg"),
            format!("org_{org}/media/{asset_id}/2_fix.jpg"),
        ] {
            assert!(
                !store.exists(&blob).await.unwrap(),
                "blob purged from the store: {blob}"
            );
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_isolation_and_latest_version() {
    with_app(|server, env, _ctx| async move {
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

        // Cross-org: GET/DELETE/restore → masked 404.
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
            assert_eq!(res.status_code(), 404, "{method} cross-org masked");
        }

        // Last version: 409 last_version.
        let res = server
            .delete(&format!("/api/v1/media/{asset_id}/versions/1"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .await;
        assert_eq!(res.status_code(), 409);
        let body: serde_json::Value = res.json();
        assert_eq!(body["error"], "media-last-version");

        // Without X-Org-Id → 400.
        let res = server
            .get("/api/v1/media")
            .add_header("Authorization", bearer(&env.alice))
            .await;
        assert_eq!(res.status_code(), 400);
    })
    .await;
}

/// Browser media cache: content carries a per-version ETag; a matching
/// `If-None-Match` gets a bodiless 304, but only after the org scoping
/// (another org never learns the asset exists) and never across versions.
#[tokio::test]
#[serial]
async fn content_etag_revalidation() {
    with_app(|server, env, _ctx| async move {
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
        let path = format!("/api/v1/media/{asset_id}/content");

        let res = server
            .get(&path)
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .await;
        assert_eq!(res.status_code(), 200);
        let etag = res.header("etag").to_str().unwrap().to_string();
        assert!(
            etag.starts_with('"') && etag.ends_with('"'),
            "strong etag: {etag}"
        );
        assert_eq!(
            res.header("cache-control").to_str().unwrap(),
            "private, no-cache"
        );

        // Same version → 304 without body.
        let res = server
            .get(&path)
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .add_header("If-None-Match", etag.clone())
            .await;
        assert_eq!(res.status_code(), 304);
        assert!(res.into_bytes().is_empty());

        // Other org with the validator → still the masked 404.
        let res = server
            .get(&path)
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", org2.to_string())
            .add_header("If-None-Match", etag.clone())
            .await;
        assert_eq!(res.status_code(), 404);

        // New version → the old validator gets the new bytes.
        let res = server
            .post(&format!(
                "/api/v1/media/{asset_id}/versions?filename=b.jpg&content_type=image%2Fjpeg"
            ))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .add_header("Content-Type", "application/octet-stream")
            .bytes(gpano_jpeg().into())
            .await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let res = server
            .get(&path)
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org1.to_string())
            .add_header("If-None-Match", etag.clone())
            .await;
        assert_eq!(res.status_code(), 200);
        assert_ne!(res.header("etag").to_str().unwrap(), etag);
        assert_eq!(res.into_bytes(), gpano_jpeg());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn roles_and_pagination() {
    with_app(|server, env, _ctx| async move {
        // JIT provisioning of both users (as in tenant_isolation:
        // a user must exist before being added as a member).
        for token in [&env.alice, &env.bob] {
            server
                .get("/api/v1/user-info")
                .add_header("Authorization", bearer(token))
                .await;
        }

        // Shared org: alice owner, bob viewer (as in tenant_isolation).
        let created: serde_json::Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Media" }))
            .await
            .json();
        let org = created["id"].as_i64().unwrap();
        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org,
            "bob@example.com",
            "viewer",
        )
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

        // Viewer: writes forbidden (upload + delete).
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
        assert_eq!(res.status_code(), 403, "viewer delete → 403 before 404");

        // Pagination limit=2 → count 3, next present.
        let page: serde_json::Value = server
            .get("/api/v1/media?limit=2")
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await
            .json();
        assert_eq!(page["count"], 3);
        assert_eq!(page["results"].as_array().unwrap().len(), 2);
        assert!(page["next"].is_string(), "next link present");
    })
    .await;
}

async fn call(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let req = match method {
        "GET" => server.get(path),
        _ => server.post(path),
    };
    let req = req
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string());
    let res = match body {
        Some(b) => req.json(&b).await,
        None => req.await,
    };
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or_default(),
    )
}

async fn put_part(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    path: &str,
    bytes: Vec<u8>,
) -> (u16, serde_json::Value) {
    let res = server
        .put(path)
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .bytes(bytes.into())
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or_default(),
    )
}

/// Sends `parts` as one upload, closes it and waits for the join.
async fn send_in_parts(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    input: serde_json::Value,
    parts: &[Vec<u8>],
) -> serde_json::Value {
    let (s, u) = call(
        server,
        "POST",
        "/api/v1/media/uploads",
        token,
        org,
        Some(input),
    )
    .await;
    assert_eq!(s, 201, "{u}");
    let path = format!("/api/v1/media/uploads/{}", u["id"].as_str().unwrap());
    // Last part first: the order of arrival does not matter.
    for (n, bytes) in parts.iter().enumerate().rev() {
        let (s, u) = put_part(
            server,
            token,
            org,
            &format!("{path}/parts/{}", n + 1),
            bytes.clone(),
        )
        .await;
        assert_eq!(s, 200, "{u}");
    }
    let body = serde_json::json!({ "parts": parts.len() });
    let (s, u) = call(
        server,
        "POST",
        &format!("{path}/complete"),
        token,
        org,
        Some(body),
    )
    .await;
    assert_eq!(s, 202, "{u}");
    for _ in 0..120 {
        let (s, u) = call(server, "GET", &path, token, org, None).await;
        assert_eq!(s, 200, "{u}");
        if u["state"] == "done" || u["state"] == "failed" {
            return u;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!("upload never ended");
}

/// A file too large for one request behind a proxy (data-pipelines.md §9.9): parts
/// joined in the background into a new asset, then into a new version of it. The
/// format is judged on the whole, another org reaches nothing.
#[tokio::test]
#[serial]
async fn a_file_sent_in_parts_becomes_one_version() {
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        let mut file = gpano_jpeg();
        file.extend((0..300_000u32).map(|i| (i % 251) as u8));
        let parts = vec![file[..200_000].to_vec(), file[200_000..].to_vec()];

        let input = serde_json::json!({ "filename": "sphere.jpg", "name": "Sphere" });
        let end = send_in_parts(&server, &env.alice, org, input, &parts).await;
        assert_eq!(end["state"], "done", "{end}");
        assert_eq!(end["version"], 1, "{end}");
        assert_eq!(end["bytes"], file.len(), "{end}");
        let asset = end["asset_id"].as_str().unwrap().to_string();
        let (_, detail) = call(
            &server,
            "GET",
            &format!("/api/v1/media/{asset}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(
            detail["kind"], "panorama",
            "sniffed on the first part: {detail}"
        );
        assert_eq!(detail["name"], "Sphere", "{detail}");
        let content = server
            .get(&format!("/api/v1/media/{asset}/content"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(
            content.as_bytes().to_vec(),
            file,
            "joined in order, nothing lost"
        );

        // A new version of the same asset.
        let input =
            serde_json::json!({ "filename": "sphere2.jpg", "asset_id": asset, "note": "v2" });
        let end = send_in_parts(&server, &env.alice, org, input, &parts).await;
        assert_eq!(end["state"], "done", "{end}");
        assert_eq!(end["version"], 2, "{end}");
        assert_eq!(end["asset_id"], asset.as_str(), "{end}");

        // A format refused once whole: the upload fails with its code, no asset.
        let input = serde_json::json!({ "filename": "fake.jpg" });
        let end = send_in_parts(
            &server,
            &env.alice,
            org,
            input,
            &[b"not a picture".to_vec()],
        )
        .await;
        assert_eq!(end["state"], "failed", "{end}");
        assert_eq!(end["code"], "media-format-unsupported", "{end}");
        assert!(end["asset_id"].is_null(), "{end}");

        // Another org: its asset is refused as a target, its upload is not found (R1).
        let input = serde_json::json!({ "filename": "x.jpg", "asset_id": asset });
        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/media/uploads",
            &env.bob,
            bob_org,
            Some(input),
        )
        .await;
        assert_eq!(s, 400, "{e}");
        let (_, u) = call(
            &server,
            "POST",
            "/api/v1/media/uploads",
            &env.alice,
            org,
            Some(serde_json::json!({ "filename": "y.jpg" })),
        )
        .await;
        let path = format!("/api/v1/media/uploads/{}", u["id"].as_str().unwrap());
        let (s, e) = call(&server, "GET", &path, &env.bob, bob_org, None).await;
        assert_eq!(s, 404, "{e}");
        assert_eq!(e["error"], "media-upload-not-found", "{e}");
        let (s, _) = put_part(
            &server,
            &env.bob,
            bob_org,
            &format!("{path}/parts/1"),
            b"x".to_vec(),
        )
        .await;
        assert_eq!(s, 404);

        // A part over the limit, a missing part.
        let (s, e) = put_part(
            &server,
            &env.alice,
            org,
            &format!("{path}/parts/1"),
            vec![0; 64 * 1024 * 1024 + 1],
        )
        .await;
        assert_eq!(s, 413, "{e}");
        assert_eq!(e["error"], "media-upload-part-too-large", "{e}");
        let (s, _) = put_part(
            &server,
            &env.alice,
            org,
            &format!("{path}/parts/2"),
            plain_jpeg(),
        )
        .await;
        assert_eq!(s, 200);
        let body = serde_json::json!({ "parts": 2 });
        let (s, e) = call(
            &server,
            "POST",
            &format!("{path}/complete"),
            &env.alice,
            org,
            Some(body),
        )
        .await;
        assert_eq!(s, 409, "{e}");
        assert_eq!(e["error"], "media-upload-parts-missing", "{e}");
    })
    .await;
}

async fn sql(db: &sea_orm::DatabaseConnection, text: &str) {
    use sea_orm::ConnectionTrait;
    db.execute_unprepared(text).await.unwrap();
}

/// A join lost with its server starts again, a few times only; an upload nobody
/// finished is closed after a day. Neither leaves its parts on the store.
#[tokio::test]
#[serial]
async fn a_lost_join_starts_again_and_a_left_upload_is_swept() {
    use pnex_backend::services::{media::MediaSettings, media_uploads};
    with_app(|server, env, ctx| async move {
        let store = MediaSettings::from_config(&ctx.config).storage.store().unwrap();
        let org = personal_org(&server, &env.alice).await;
        // The parts of one upload: an earlier test may leave others in the same org.
        let staged = |path: &str| format!("org_{org}/media/uploads/{}/", path.rsplit('/').next().unwrap());
        let start = |close: bool| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                let input = serde_json::json!({ "filename": "p.jpg" });
                let (_, u) = call(server, "POST", "/api/v1/media/uploads", &token, org, Some(input)).await;
                let path = format!("/api/v1/media/uploads/{}", u["id"].as_str().unwrap());
                let (s, _) = put_part(server, &token, org, &format!("{path}/parts/1"), plain_jpeg()).await;
                assert_eq!(s, 200);
                if close {
                    let body = serde_json::json!({ "parts": 1 });
                    let (s, u) = call(server, "POST", &format!("{path}/complete"), &token, org, Some(body)).await;
                    assert_eq!(s, 202, "{u}");
                }
                path
            }
        };
        let end = |path: String| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                for _ in 0..120 {
                    let (_, u) = call(server, "GET", &path, &token, org, None).await;
                    if u["state"] == "done" || u["state"] == "failed" {
                        return u;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                panic!("upload never ended");
            }
        };
        let lost = |attempts: i32| {
            format!(
                "UPDATE media_uploads SET state = 'joining', attempts = {attempts}, \
                 heartbeat_at = now() - interval '11 minutes' WHERE state IN ('ready', 'joining')"
            )
        };

        let path = start(true).await;
        sql(&ctx.db, &lost(1)).await;
        let u = end(path).await;
        assert_eq!(u["state"], "done", "{u}");

        let path = start(true).await;
        sql(&ctx.db, &lost(3)).await;
        let u = end(path.clone()).await;
        assert_eq!(u["state"], "failed", "{u}");
        assert_eq!(u["code"], "media-upload-lost", "{u}");
        // The state is written first, the parts go right after it.
        let mut left = Vec::new();
        for _ in 0..40 {
            left = store.list_prefix(&staged(&path)).await.unwrap();
            if left.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert!(left.is_empty(), "parts freed: {left:?}");

        let path = start(false).await;
        assert_eq!(store.list_prefix(&staged(&path)).await.unwrap().len(), 1);
        media_uploads::sweep(&ctx.db, &store).await.unwrap();
        let (_, u) = call(&server, "GET", &path, &env.alice, org, None).await;
        assert_eq!(u["state"], "open", "a fresh upload stays: {u}");
        sql(&ctx.db, "UPDATE media_uploads SET updated_at = now() - interval '25 hours' WHERE state = 'open'").await;
        media_uploads::sweep(&ctx.db, &store).await.unwrap();
        let (_, u) = call(&server, "GET", &path, &env.alice, org, None).await;
        assert_eq!(u["state"], "failed", "{u}");
        assert_eq!(u["code"], "media-upload-expired", "{u}");
        let left = store.list_prefix(&staged(&path)).await.unwrap();
        assert!(left.is_empty(), "parts freed: {left:?} {}", staged(&path));
    })
    .await;
}
