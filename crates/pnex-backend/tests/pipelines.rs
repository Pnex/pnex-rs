//! Pipelines API (pipelines.md D934, D571, D573, D574): an org's engine
//! documents, their tests, versions, run queue, org isolation (R1),
//! permissions (R2). The worker has its own tests.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL).

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

fn doc() -> String {
    common::sql_pipeline("out", &[("r", "raw")], "SELECT * FROM r")
}

/// A pipeline read from the network, with one test on its inline answer.
fn tested(rows: u64) -> String {
    format!(
        "key: p\nhosts: [data.example.io]\nsteps:\n  - fetch: {{as: page, url: \"https://data.example.io/a.csv\"}}\n  - read: {{as: rows, from: page, format: csv}}\n  - write: {{from: rows, table: {{name: out}}}}\ntests:\n  - given: {{\"https://data.example.io/a.csv\": \"n\\n1\\n2\\n\"}}\n    expect: {{rows: {{rows: {rows}}}}}\n"
    )
}

struct Env {
    alice: String,
    bob: String,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env, loco_rs::app::AppContext) -> Fut,
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
            f(server, env, ctx.clone()).await;
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
    with_app(|server, env, _ctx| async move {
        let org = personal_org(&server, &env.alice).await;
        let (s, p) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            Some(json!({ "name": "fc", "document": doc() })),
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
            Some(json!({ "name": "fc", "document": doc() })),
        )
        .await;
        assert_eq!(s, 409);
        assert_eq!(e["error"], "pipeline-name-taken");

        // A bad document says where, in machine tokens.
        for (document, field, token) in [
            ("steps: [".to_string(), "document", "yaml"),
            ("key: p\nsteps: []".to_string(), "steps", "required"),
            (
                format!("schedule: 1h\n{}", doc()),
                "schedule",
                "org_unsupported",
            ),
            // Its test does not hold: the save is refused where it fails.
            (tested(3), "tests.0.expect.rows", "rows:2"),
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
        let two = doc().replace("name: \"out\"", "name: \"out2\"");
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
            Some(json!({ "document": doc(), "expected_version": 1 })),
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
        assert_eq!(v["results"][1]["document"], doc());
        assert_eq!(v["results"][0]["author"], "alice@example.com", "{v}");

        // A draft is checked without being saved, with the same answers as a save.
        let check = "/api/v1/pipelines/validate";
        let (s, parsed) = call(
            &server,
            "POST",
            check,
            &env.alice,
            org,
            Some(json!({ "document": doc() })),
        )
        .await;
        assert_eq!(s, 200, "{parsed}");
        assert_eq!(parsed["reads"], json!(["raw"]), "{parsed}");
        assert_eq!(parsed["writes"], json!(["out"]), "{parsed}");
        assert_eq!(parsed["steps"], json!(["sql", "write"]), "{parsed}");
        let (s, held) = call(
            &server,
            "POST",
            check,
            &env.alice,
            org,
            Some(json!({ "document": tested(2) })),
        )
        .await;
        assert_eq!((s, &held["tests"]), (200, &json!(1)), "{held}");
        for (document, token) in [("key: p\nsteps: []", "required"), ("not: [yaml", "yaml")] {
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
    with_app(|server, env, _ctx| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let (_, p) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org_a,
            Some(json!({ "name": "private", "document": doc() })),
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
                Some(json!({ "document": doc(), "expected_version": 1 })),
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
            Some(json!({ "document": doc() })),
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
                Some(json!({ "name": "v", "document": doc() })),
            ),
            (
                "PUT",
                path.clone(),
                Some(json!({ "document": doc(), "expected_version": 1 })),
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
    with_app(|server, env, _ctx| async move {
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
    with_app(|server, env, ctx| async move {
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
        let document = common::sql_pipeline(
            "active_partners",
            &[("p", "partners"), ("s", "sheets")],
            "SELECT p.Numero_Fiche AS code, left(digits(p.Siret_Partenaire), 9) AS siren, \
             p.Habilitation_Partenaire AS scope \
             FROM p JOIN s ON p.Numero_Fiche = s.Numero_Fiche WHERE s.Actif = 'ACTIVE'",
        );
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
        // The server takes the run; the worker runs its query.
        assert!(pnex_backend::services::pipelines::run_next(&ctx)
            .await
            .unwrap());
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

/// D934: the server takes a queued run of an org's own pipeline and plays
/// it; its table is written and the run says so. A member who lost a
/// right its writes need sees the run refused when it starts (R2).
#[tokio::test]
#[serial]
async fn an_org_pipeline_runs_on_the_server() {
    with_app(|server, env, ctx| async move {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().route("/a.csv", axum::routing::get(|| async { "n\n1\n2\n" }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        pnex_backend::services::pipeline_engine::store::route_for_tests("data.example.io", &base);

        let org = personal_org(&server, &env.alice).await;
        let (s, p) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            Some(json!({ "name": "numbers", "document": tested(2) })),
        )
        .await;
        assert_eq!(s, 201, "{p}");
        let runs = format!("/api/v1/pipelines/{}/runs", p["id"].as_str().unwrap());
        let (s, r) = call(&server, "POST", &runs, &env.alice, org, None).await;
        assert_eq!(s, 202, "{r}");
        assert!(pnex_backend::services::pipelines::run_next(&ctx)
            .await
            .unwrap());
        let run = format!("{runs}/{}", r["id"].as_str().unwrap());
        let (_, done) = call(&server, "GET", &run, &env.alice, org, None).await;
        assert_eq!(done["state"], "done", "{done}");
        let (_, all) = call(&server, "GET", "/api/v1/datasets", &env.alice, org, None).await;
        let out = all["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "out")
            .expect("its table");
        assert_eq!(out["rows"], 2, "{out}");

        // The starter lost the right to write tables before the run started.
        let (s, r) = call(&server, "POST", &runs, &env.alice, org, None).await;
        assert_eq!(s, 202, "{r}");
        sea_orm::ConnectionTrait::execute_unprepared(
            &ctx.db,
            &format!("UPDATE organization_members SET role = 'viewer' WHERE org_id = {org}"),
        )
        .await
        .unwrap();
        assert!(pnex_backend::services::pipelines::run_next(&ctx)
            .await
            .unwrap());
        let (_, refused) = call(
            &server,
            "GET",
            &format!("{runs}/{}", r["id"].as_str().unwrap()),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(refused["state"], "failed", "{refused}");
        assert_eq!(refused["code"], "access-denied", "{refused}");
    })
    .await;
}

/// Q-PE2: an org's pipeline navigates; its row resumes with the extracts
/// under the rights its starter still holds (R2).
#[tokio::test]
#[serial]
async fn an_org_pipeline_browses_and_resumes_under_its_starters_rights() {
    with_app(|server, env, ctx| async move {
        use sea_orm::ConnectionTrait;
        let org = personal_org(&server, &env.alice).await;
        let document = "key: b\nhosts: [www.example.org]\nsteps:\n  - enumerate: {as: p, track: true, cross: [{as: url, objects: {type: person, property: url}}]}\n  - browse: {as: page, for_each: p, site: www.example.org, steps: [{open: \"{{url}}\"}, {extract: {all_text: true, as: profile}}]}\n  - write: {from: page, table: {name: Pages}}\n";
        let (s, p) = call(
            &server,
            "POST",
            "/api/v1/pipelines",
            &env.alice,
            org,
            Some(json!({ "name": "pages", "document": document })),
        )
        .await;
        assert_eq!(s, 201, "{p}");
        let id = p["id"].as_str().unwrap().to_string();
        let alice: i64 = ctx
            .db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT user_id FROM organization_members WHERE org_id = {org}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "user_id")
            .unwrap();
        let waiting = |item: &str| {
            format!(
                "INSERT INTO pipeline_items (org_id, pipeline, item, state, args, updated_at) \
                 VALUES ({org}, '{id}', '{item}', 'waiting', '{{\"row\": {{\"url\": \"https://www.example.org/{item}\"}}}}', now())"
            )
        };
        let state = |item: &'static str| {
            let (db, id) = (ctx.db.clone(), id.clone());
            async move {
                let r = db
                    .query_one_raw(sea_orm::Statement::from_string(
                        sea_orm::DatabaseBackend::Postgres,
                        format!("SELECT state, code FROM pipeline_items WHERE org_id = {org} AND pipeline = '{id}' AND item = '{item}'"),
                    ))
                    .await
                    .unwrap()
                    .unwrap();
                (
                    r.try_get::<String>("", "state").unwrap(),
                    r.try_get::<Option<String>>("", "code").unwrap(),
                )
            }
        };
        use pnex_backend::services::pipeline_engine::store::resume_row;
        let text = || Some(vec![("profile".to_string(), "Hello".to_string())]);

        ctx.db.execute_unprepared(&waiting("a")).await.unwrap();
        resume_row(&ctx, org, &id, "a", Some(alice), text()).await.unwrap();
        assert_eq!(state("a").await.0, "done");
        let (_, all) = call(&server, "GET", "/api/v1/datasets", &env.alice, org, None).await;
        let pages = all["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "Pages")
            .expect("its table");
        assert_eq!(pages["rows"], 1, "{pages}");

        // The starter lost their rights while the page was read.
        ctx.db.execute_unprepared(&waiting("b")).await.unwrap();
        ctx.db
            .execute_unprepared(&format!(
                "UPDATE organization_members SET role = 'viewer' WHERE org_id = {org}"
            ))
            .await
            .unwrap();
        resume_row(&ctx, org, &id, "b", Some(alice), text()).await.unwrap();
        assert_eq!(state("b").await, ("failed".into(), Some("access-denied".into())));
    })
    .await;
}
