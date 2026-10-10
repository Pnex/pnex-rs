//! Document search P1 (doc-search.md): upload → index (worker inline in
//! tests) → lexical search, chunk read, reindex, org isolation and roles.
//!
//! Requires PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::Value;
use serial_test::serial;
use std::io::Write;

struct Env {
    alice: String,
    bob: String,
}

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
                "{}/pnex-docsearch-tests-{}",
                std::env::temp_dir().display(),
                std::process::id()
            ),
        )
    };
    unsafe { std::env::set_var("PNEX_MEDIA_BACKEND", "fs") };
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

async fn personal_org(server: &axum_test::TestServer, token: &str) -> i64 {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", bearer(token))
        .await
        .json::<Value>()["orgs"][0]["id"]
        .as_i64()
        .expect("personal org")
}

async fn upload(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    filename: &str,
    body: Vec<u8>,
) -> axum_test::TestResponse {
    server
        .post(&format!("/api/v1/media?filename={filename}"))
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .add_header("Content-Type", "application/octet-stream")
        .bytes(body.into())
        .await
}

async fn get(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    path: &str,
) -> axum_test::TestResponse {
    server
        .get(path)
        .add_header("Authorization", bearer(token))
        .add_header("X-Org-Id", org.to_string())
        .await
}

fn docx(paragraphs: &[(&str, &str)]) -> Vec<u8> {
    let body: String = paragraphs
        .iter()
        .map(|(style, text)| {
            let ppr = if style.is_empty() {
                String::new()
            } else {
                format!(r#"<w:pPr><w:pStyle w:val="{style}"/></w:pPr>"#)
            };
            format!("<w:p>{ppr}<w:r><w:t>{text}</w:t></w:r></w:p>")
        })
        .collect();
    let xml = format!(r#"<w:document xmlns:w="w"><w:body>{body}</w:body></w:document>"#);
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file(
        "word/document.xml",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    w.write_all(xml.as_bytes()).unwrap();
    w.finish().unwrap().into_inner()
}

#[tokio::test]
#[serial]
async fn upload_index_search_and_read() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let manual = docx(&[
            ("Titre1", "Pompe P2"),
            ("", "Le défaut E-0457 signale une vibration au démarrage."),
            ("Heading1", "Câblage"),
            ("", "Fil rouge sur la borne 24V."),
        ]);
        let res = upload(&server, &env.alice, org, "manuel.docx", manual).await;
        assert_eq!(res.status_code(), 201, "{}", res.text());
        let asset: Value = res.json();
        assert_eq!(asset["kind"], "document", "{asset}");
        let id = asset["id"].as_str().unwrap().to_string();

        let state = get(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/media/{id}/index"),
        )
        .await;
        let state: Value = state.json();
        assert_eq!(state["index"]["status"], "indexed", "{state}");
        assert!(
            state["index"]["chunk_count"].as_i64().unwrap() >= 2,
            "{state}"
        );

        // Exact code (simple config) and stemmed prose (french config).
        for q in ["E-0457", "vibrations", "démarrage"] {
            let res = get(
                &server,
                &env.alice,
                org,
                &format!("/api/v1/media/search?q={q}"),
            )
            .await;
            let hits: Value = res.json();
            assert_eq!(hits["hits"][0]["asset_id"], id.as_str(), "{q}: {hits}");
            assert_eq!(hits["hits"][0]["heading"], "Pompe P2", "{q}: {hits}");
        }
        let hits: Value = get(&server, &env.alice, org, "/api/v1/media/search?q=borne")
            .await
            .json();
        assert_eq!(hits["hits"][0]["heading"], "Câblage", "{hits}");
        assert!(
            hits["hits"][0]["snippet"]
                .as_str()
                .unwrap()
                .contains("⟦borne⟧"),
            "{hits}"
        );

        // Markdown + CSV (table kind) are indexed too.
        let res = upload(
            &server,
            &env.alice,
            org,
            "notes.md",
            b"# Maintenance\nGraisser le roulement R-12.".to_vec(),
        )
        .await;
        assert_eq!(res.json::<Value>()["kind"], "document");
        let res = upload(
            &server,
            &env.alice,
            org,
            "mesures.csv",
            b"capteur,valeur\nT-88,42\n".to_vec(),
        )
        .await;
        assert_eq!(res.json::<Value>()["kind"], "table");
        let hits: Value = get(
            &server,
            &env.alice,
            org,
            "/api/v1/media/search?q=T-88&kind=table",
        )
        .await
        .json();
        assert_eq!(hits["hits"].as_array().unwrap().len(), 1, "{hits}");

        // Chunk read with its neighbours.
        let chunk = hits["hits"][0]["chunk_id"].as_str().unwrap();
        let res = get(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/media/chunks/{chunk}?context=1"),
        )
        .await;
        assert_eq!(res.status_code(), 200);
        assert!(res.json::<Value>()["chunks"][0]["content"]
            .as_str()
            .unwrap()
            .contains("T-88"));

        // A new version replaces what the search sees.
        let res = server
            .post(&format!("/api/v1/media/{id}/versions?filename=manuel.docx"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/octet-stream")
            .bytes(docx(&[("", "Nouvelle révision sans code.")]).into())
            .await;
        assert!(res.status_code().is_success(), "{}", res.text());
        let hits: Value = get(&server, &env.alice, org, "/api/v1/media/search?q=E-0457")
            .await
            .json();
        assert!(hits["hits"].as_array().unwrap().is_empty(), "{hits}");

        // A photo is not indexed and cannot be reindexed.
        let photo = upload(
            &server,
            &env.alice,
            org,
            "p.jpg",
            vec![0xFF, 0xD8, 0xFF, 0xD9],
        )
        .await;
        let pid = photo.json::<Value>()["id"].as_str().unwrap().to_string();
        let res = server
            .post(&format!("/api/v1/media/{pid}/index"))
            .add_header("Authorization", bearer(&env.alice))
            .add_header("X-Org-Id", org.to_string())
            .await;
        assert_eq!(res.status_code(), 400);
        assert_eq!(res.json::<Value>()["error"], "media-index-not-a-document");

        // Strict allowlist: fake extensions and bare zips are refused.
        for (name, bytes) in [
            ("evil.pdf", b"MZ\x90\x00".to_vec()),
            ("photos.docx", b"PK\x03\x04photos/a.jpg".to_vec()),
            ("photo.png", vec![0xFF, 0xD8, 0xFF, 0xD9]),
            ("page.html", b"<script>alert(1)</script>".to_vec()),
        ] {
            let res = upload(&server, &env.alice, org, name, bytes).await;
            assert_eq!(res.status_code(), 400, "{name}");
            assert_eq!(
                res.json::<Value>()["error"],
                "media-format-unsupported",
                "{name}"
            );
        }

        // A corrupt docx ends in a coded error, the upload itself stands.
        let res = upload(
            &server,
            &env.alice,
            org,
            "broken.docx",
            b"PK\x03\x04word/document.xml junk".to_vec(),
        )
        .await;
        assert_eq!(res.status_code(), 201);
        let bid = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let state: Value = get(
            &server,
            &env.alice,
            org,
            &format!("/api/v1/media/{bid}/index"),
        )
        .await
        .json();
        assert_eq!(state["index"]["status"], "error", "{state}");
        assert_eq!(
            state["index"]["error_code"], "media-index-malformed",
            "{state}"
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_and_roles() {
    with_app(|server, env| async move {
        personal_org(&server, &env.alice).await;
        let bob_org = personal_org(&server, &env.bob).await;
        // Shared org: alice owner, bob added as viewer below.
        let created: Value = server
            .post("/api/v1/orgs")
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "name": "Atelier Docs" }))
            .await
            .json();
        let alice_org = created["id"].as_i64().unwrap();
        let res = upload(
            &server,
            &env.alice,
            alice_org,
            "secret.md",
            b"Code confidentiel Z-9001".to_vec(),
        )
        .await;
        let id = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let hits: Value = get(
            &server,
            &env.alice,
            alice_org,
            "/api/v1/media/search?q=Z-9001",
        )
        .await
        .json();
        let chunk = hits["hits"][0]["chunk_id"].as_str().unwrap().to_string();

        // Another org sees nothing (R1): search, chunk, index state.
        let hits: Value = get(&server, &env.bob, bob_org, "/api/v1/media/search?q=Z-9001")
            .await
            .json();
        assert!(hits["hits"].as_array().unwrap().is_empty(), "{hits}");
        let res = get(
            &server,
            &env.bob,
            bob_org,
            &format!("/api/v1/media/chunks/{chunk}"),
        )
        .await;
        assert_eq!(res.status_code(), 404);
        let res = get(
            &server,
            &env.bob,
            bob_org,
            &format!("/api/v1/media/{id}/index"),
        )
        .await;
        assert_eq!(res.status_code(), 404);

        // Viewer of the org: reads, cannot reindex (R2).
        server
            .post(&format!("/api/v1/orgs/{alice_org}/members"))
            .add_header("Content-Type", "application/json")
            .add_header("Authorization", bearer(&env.alice))
            .json(&serde_json::json!({ "email": "bob@example.com", "role": "viewer" }))
            .await;
        let hits: Value = get(
            &server,
            &env.bob,
            alice_org,
            "/api/v1/media/search?q=Z-9001",
        )
        .await
        .json();
        assert_eq!(hits["hits"].as_array().unwrap().len(), 1, "{hits}");
        let res = server
            .post(&format!("/api/v1/media/{id}/index"))
            .add_header("Authorization", bearer(&env.bob))
            .add_header("X-Org-Id", alice_org.to_string())
            .await;
        assert_eq!(res.status_code(), 403);
    })
    .await;
}
