//! Pipelines API (data-pipelines.md D571, D573, D574): documents, versions, run
//! queue, org isolation (R1), permissions (R2). The worker has its own tests.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

const DOC: &str =
    "transforms:\n  - output: out\n    inputs: { r: raw }\n    sql: SELECT * FROM r\n";

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
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
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
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test]
#[serial]
async fn documents_versions_and_runs() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (s, p) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            Some(json!({ "name": "fc", "document": DOC })),
        )
        .await;
        assert_eq!(s, 201, "{p}");
        assert_eq!(p["version"], 1);
        let id = p["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/pipelines/{id}");

        let (s, e) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            Some(json!({ "name": "fc", "document": DOC })),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "pipeline-name-taken");

        // A bad document says where, in machine tokens.
        for (document, field, token) in [
            ("transforms: [", "document", "yaml"),
            ("transforms: []", "transforms", "required"),
            (
                "transforms:\n  - output: a\n    inputs: { x: a }\n    sql: SELECT 1\n",
                "transforms.0.inputs.x",
                "cycle",
            ),
        ] {
            let (s, e) = call(
                &server,
                "PUT",
                &path,
                &env.alice,
                org,
                Some(json!({ "document": document, "expected_version": 1 })),
            )
            .await;
            assert_eq!(s, 400, "{document}");
            assert_eq!(e["error"], "pipeline-document-invalid");
            assert_eq!(e["errors"]["violations"][0]["path"], field);
            assert_eq!(e["errors"]["violations"][0]["token"], token);
        }

        // A new version, on the version that was read.
        let two = DOC.replace("output: out", "output: out2");
        let (s, p) = call(
            &server,
            "PUT",
            &path,
            &env.alice,
            org,
            Some(json!({ "document": two, "expected_version": 1 })),
        )
        .await;
        assert_eq!(s, 200, "{p}");
        assert_eq!(p["version"], 2);
        let (s, e) = call(
            &server,
            "PUT",
            &path,
            &env.alice,
            org,
            Some(json!({ "document": DOC, "expected_version": 1 })),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "pipeline-version-conflict");
        assert_eq!(e["errors"]["args"]["current"], 2);
        let (_, v) = call(
            &server,
            "GET",
            &format!("{path}/versions"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(v["count"], 2);
        assert_eq!(v["results"][1]["document"], DOC);
        assert_eq!(v["results"][0]["author"], "alice@example.com", "{v}");

        // A draft is checked without being saved, with the same answers as a save.
        let check = "/api/v1/pipelines/validate";
        let (s, parsed) = call(
            &server,
            "POST",
            check,
            &env.alice,
            org,
            Some(json!({ "document": DOC })),
        )
        .await;
        assert_eq!(s, 200, "{parsed}");
        assert!(parsed["transforms"][0]["output"].is_string(), "{parsed}");
        for (document, token) in [("transforms: []", "required"), ("not: [yaml", "yaml")] {
            let (s, e) = call(
                &server,
                "POST",
                check,
                &env.alice,
                org,
                Some(json!({ "document": document })),
            )
            .await;
            assert_eq!(s, 400, "{e}");
            assert_eq!(e["error"], "pipeline-document-invalid");
            assert_eq!(e["errors"]["violations"][0]["token"], token, "{e}");
        }
        let (_, again) = call(
            &server,
            "GET",
            &format!("{path}/versions"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(again["count"], 2, "a check saves nothing");
        assert_eq!(v["results"][1]["author"], "alice@example.com", "{v}");

        // A run is queued for the worker, once.
        let runs = format!("{path}/runs");
        let (s, r) = call(&server, "POST", &runs, &env.alice, org, None).await;
        assert_eq!(s, 202, "{r}");
        assert_eq!(r["state"], "queued");
        assert_eq!(r["version"], 2);
        let run_id = r["id"].as_str().unwrap().to_string();
        let (s, e) = call(&server, "POST", &runs, &env.alice, org, None).await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "pipeline-run-active");
        let (s, r) = call(
            &server,
            "GET",
            &format!("{runs}/{run_id}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 200);
        assert_eq!(r["state"], "queued");
        let (_, l) = call(&server, "GET", &runs, &env.alice, org, None).await;
        assert_eq!(l["count"], 1);

        // Not deletable while it runs.
        let (s, e) = call(&server, "DELETE", &path, &env.alice, org, None).await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "pipeline-run-active");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn isolation_between_orgs_and_viewer_read_only() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (_, p) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org_a,
            Some(json!({ "name": "private", "document": DOC })),
        )
        .await;
        let id = p["id"].as_str().unwrap().to_string();
        let path = format!("/api/v1/pipelines/{id}");
        let runs = format!("{path}/runs");

        for (m, p, body) in [
            ("GET", path.clone(), None),
            ("GET", format!("{path}/versions"), None),
            ("GET", runs.clone(), None),
            ("POST", runs.clone(), None),
            (
                "PUT",
                path.clone(),
                Some(json!({ "document": DOC, "expected_version": 1 })),
            ),
            ("DELETE", path.clone(), None),
        ] {
            let (s, e) = call(&server, m, &p, &env.bob, org_b, body).await;
            assert_eq!(s, 404, "{m} {p}");
            assert_eq!(e["error"], "pipeline-not-found");
        }

        // A check goes through the org of the caller like everything else (R1).
        let (s, _) = call(
            &server,
            "POST",
            "/api/v1/pipelines/validate",
            &env.bob,
            org_a,
            Some(json!({ "document": DOC })),
        )
        .await;
        assert_eq!(s, 403);

        common::join_org(
            &server,
            &env.alice,
            &env.bob,
            org_a,
            "bob@example.com",
            "viewer",
        )
        .await;
        let (s, _) = call(&server, "GET", &path, &env.bob, org_a, None).await;
        assert_eq!(s, 200);
        for (m, p, body) in [
            (
                "POST",
                "/api/v1/pipelines".to_string(),
                Some(json!({ "name": "v", "document": DOC })),
            ),
            (
                "PUT",
                path.clone(),
                Some(json!({ "document": DOC, "expected_version": 1 })),
            ),
            ("POST", runs.clone(), None),
            ("DELETE", path.clone(), None),
        ] {
            let (s, e) = call(&server, m, &p, &env.bob, org_a, body).await;
            assert_eq!(s, 403, "{m} {p}");
            assert_eq!(e["error"], "access-denied");
        }
    })
    .await;
}

#[tokio::test]
#[serial]
async fn lineage_of_a_dataset_nothing_computed_is_empty() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let (_, d) = call(
            &server,
            "POST",
            "/api/v1/datasets",
            &env.alice,
            org,
            Some(json!({ "name": "raw" })),
        )
        .await;
        let id = d["id"].as_str().unwrap();
        let path = format!("/api/v1/datasets/{id}/lineage");
        let (s, l) = call(&server, "GET", &path, &env.alice, org, None).await;
        assert_eq!(s, 200, "{l}");
        assert_eq!(
            l,
            json!({ "upstream": [], "downstream": [], "stale": false })
        );
        let org_b = personal_org(&server, &env.bob).await;
        let (s, e) = call(&server, "GET", &path, &env.bob, org_b, None).await;
        assert_eq!(s, 404);
        assert_eq!(e["error"], "dataset-not-found");
    })
    .await;
}

/// The OP1.2 acceptance, end to end: two real France Compétences files through
/// the API, joined by the real worker binary. Needs `OP1_FC_DIR` (the unzipped
/// export), `OP1_FC_ACTIVE_ROWS` (the count to reach, computed apart) and
/// `PNEX_WORKER_BIN`.
#[tokio::test]
#[serial]
#[ignore]
async fn france_competences_join_through_the_worker() {
    fn file(dir: &str, part: &str) -> Vec<u8> {
        let path = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().contains(part))
            .expect(part);
        std::fs::read(path).unwrap()
    }
    let dir = std::env::var("OP1_FC_DIR").expect("OP1_FC_DIR");
    let want: i64 = std::env::var("OP1_FC_ACTIVE_ROWS")
        .unwrap()
        .parse()
        .unwrap();
    let bin = std::env::var("PNEX_WORKER_BIN").expect("PNEX_WORKER_BIN");
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        for (name, part) in [("partners", "_Partenaires_"), ("sheets", "_Standard_")] {
            let (_, d) = call(
                &server,
                "POST",
                "/api/v1/datasets",
                &env.alice,
                org,
                Some(json!({ "name": name })),
            )
            .await;
            let res = server
                .post(&format!(
                    "/api/v1/datasets/{}/transactions/csv",
                    d["id"].as_str().unwrap()
                ))
                .add_header("Authorization", format!("Bearer {}", env.alice))
                .add_header("X-Org-Id", org.to_string())
                .bytes(file(&dir, part).into())
                .await;
            assert_eq!(res.status_code().as_u16(), 201, "{}", res.text());
        }
        let document = r#"
transforms:
  - output: active_partners
    inputs: { p: partners, s: sheets }
    sql: |
      SELECT p.Numero_Fiche AS code, left(digits(p.Siret_Partenaire), 9) AS siren,
             p.Habilitation_Partenaire AS scope
      FROM p JOIN s ON p.Numero_Fiche = s.Numero_Fiche
      WHERE s.Actif = 'ACTIVE'
"#;
        let (s, p) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            Some(json!({ "name": "france competences", "document": document })),
        )
        .await;
        assert_eq!(s, 201, "{p}");
        let runs = format!("/api/v1/pipelines/{}/runs", p["id"].as_str().unwrap());
        let (s, r) = call(&server, "POST", &runs, &env.alice, org, None).await;
        assert_eq!(s, 202, "{r}");
        let run = format!("{runs}/{}", r["id"].as_str().unwrap());

        let db = std::env::var("TEST_DATABASE_URL").unwrap();
        let mut worker = std::process::Command::new(bin)
            .env("PNEX_PIPELINE_DATABASE_URL", db)
            .env("PNEX_PIPELINE_DATABASE_ROLE", "pnex_pipeline_worker")
            .env("PNEX_S3_ENDPOINT", "http://localhost:9000")
            .env("PNEX_S3_BUCKET", "pnex-test")
            .spawn()
            .unwrap();
        let mut state = Value::Null;
        for _ in 0..120 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            state = call(&server, "GET", &run, &env.alice, org, None).await.1;
            if state["state"] == "done" || state["state"] == "failed" {
                break;
            }
        }
        worker.kill().unwrap();
        worker.wait().unwrap();
        assert_eq!(state["state"], "done", "{state}");

        let (_, all) = call(&server, "GET", "/api/v1/datasets", &env.alice, org, None).await;
        let out = all["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "active_partners")
            .expect("output dataset");
        assert_eq!(out["rows"], want, "{out}");
        let id = out["id"].as_str().unwrap();
        // The server reads what the worker wrote.
        let (s, sample) = call(
            &server,
            "GET",
            &format!("/api/v1/datasets/{id}/sample?limit=3"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(s, 200, "{sample}");
        assert_eq!(sample["columns"], json!(["code", "siren", "scope"]));
        assert_eq!(sample["rows"].as_array().unwrap().len(), 3);
        assert_eq!(sample["rows"][0][1].as_str().unwrap().len(), 9);
        let (_, l) = call(
            &server,
            "GET",
            &format!("/api/v1/datasets/{id}/lineage"),
            &env.alice,
            org,
            None,
        )
        .await;
        let names: Vec<&str> = l["upstream"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["dataset_name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["partners", "sheets"], "{l}");
        assert_eq!(l["stale"], false);
    })
    .await;
}
