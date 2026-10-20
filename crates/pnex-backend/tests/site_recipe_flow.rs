//! `site_recipe` flow node (site-recipes.md D327): the internal endpoint
//! plays the rule of the deployed node only, answers on the passed /
//! failed / broke ports, and the studio reads when the rule plays next
//! from the deployed flows' cron.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{json, Value};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, loco_rs::app::AppContext) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-sr-1") };
    // Nothing listens there: every run fails at once with `browser-unavailable`.
    unsafe { std::env::set_var("PNEX_BROWSER_URL", "http://127.0.0.1:1") };
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move { f(server, alice, ctx.clone()).await },
    )
    .await;
}

async fn deploy(db: &DatabaseConnection, org: i64, recipe_id: &str) -> i64 {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, Set};
    let flow = flows::ActiveModel {
        name: Set("veille".into()),
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
            {"id": "i", "kind": "inject", "config": {"cron": "0 6 * * *"},
             "outputs": [{"port": 0, "targets": ["s"]}]},
            {"id": "s", "kind": "site_recipe", "config": {"recipe_id": recipe_id}}
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

async fn play(server: &axum_test::TestServer, body: Value) -> (u16, Value) {
    let res = server
        .post("/internal/flow/site-recipe")
        .add_header("x-pnex-flow-token", "tok-sr-1")
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
async fn deployed_node_plays_its_rule_and_reports_when_it_breaks() {
    with_app(|server, token, ctx| async move {
        let db = ctx.db.clone();
        let auth = format!("Bearer {token}");
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", &auth)
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let created = server
            .post("/api/v1/site-recipes")
            .add_header("Authorization", &auth)
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({"yaml": "name: programme\nsite: example.org\nsteps:\n  - open: \"/\"\n"}))
            .await;
        assert_eq!(created.status_code().as_u16(), 201, "{}", created.text());
        let recipe_id = created.json::<Value>()["id"].as_str().unwrap().to_string();

        // Before any flow plays it, the studio says so.
        let sched = |server: &axum_test::TestServer| {
            server
                .get(&format!("/api/v1/site-recipes/{recipe_id}/schedule"))
                .add_header("Authorization", &auth)
                .add_header("X-Org-Id", org.to_string())
        };
        assert_eq!(sched(&server).await.json::<Value>()["flows"], json!([]));

        let flow = deploy(&db, org, &recipe_id).await;
        let s = sched(&server).await.json::<Value>();
        assert_eq!(s["flows"][0]["flow_id"], json!(flow), "{s}");
        assert_eq!(s["flows"][0]["cron"], json!(["0 6 * * *"]));
        assert_eq!(s["next"].as_array().map(Vec::len), Some(5));

        let node = |id: &str| json!({"org_id": org, "flow_id": flow, "version": 1, "node_id": id});
        let (st, e) = play(&server, json!({"node": node("i")})).await;
        assert_eq!((st, e["code"].as_str()), (403, Some("flow-node-not-deployed")));

        // No passing run before: a failure is just a failure.
        let (st, r) = play(&server, json!({"node": node("s")})).await;
        assert_eq!((st, r["port"].as_u64()), (200, Some(1)), "{r}");
        assert_eq!(r["manifest"]["status"], "failed");
        assert_eq!(r["manifest"]["reason"], pnex_core::site_recipe::reason::BROWSER);
        let run = r["manifest"]["run"].as_str().unwrap().to_string();

        // The previous flow run passed: this failure is the break.
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE site_recipe_runs SET status = 'ok', reason = NULL WHERE id = $1::uuid",
            [run.into()],
        ))
        .await
        .unwrap();
        let (_, r) = play(&server, json!({"node": node("s")})).await;
        assert_eq!(r["port"].as_u64(), Some(2), "{r}");
        let (_, r) = play(&server, json!({"node": node("s")})).await;
        assert_eq!(r["port"].as_u64(), Some(1), "only the first failure breaks");

        // The deploy gate refuses an unset or unknown rule.
        let graph: pnex_core::flow::FlowGraph = serde_json::from_value(json!({"nodes": [
            {"id": "a", "kind": "site_recipe"},
            {"id": "b", "kind": "site_recipe", "config": {"recipe_id": "6f1c2b7e-1d2a-4c3b-9e8f-0a1b2c3d4e5f"}},
            {"id": "c", "kind": "site_recipe", "config": {"recipe_id": recipe_id}}
        ]}))
        .unwrap();
        let v = pnex_backend::services::site_recipes::flow_node::deploy_violations(&db, org, &graph)
            .await
            .unwrap();
        let codes: Vec<_> = v.iter().map(|v| (v.node_id.clone(), v.code.clone())).collect();
        assert_eq!(
            codes,
            vec![
                (Some("a".to_string()), "site_recipe_config_unset".to_string()),
                (Some("b".to_string()), "site_recipe_unknown".to_string()),
            ]
        );
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_run_resumes_only_from_a_valid_step_of_the_same_site() {
    with_app(|server, token, ctx| async move {
        let db = ctx.db.clone();
        let auth = format!("Bearer {token}");
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", &auth)
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let yaml = "name: programme\nsite: example.org\nsteps:\n  - open: \"/\"\n  - extract: {all_text: true, as: page}\n  - assert: {not_empty: page}\n";
        let earlier = uuid::Uuid::new_v4();
        let steps = json!([
            {"index": 0, "action": "open", "label": "/", "status": "ok", "url": "https://example.org/", "ms": 10, "screenshot": "step-0.jpg"},
            {"index": 1, "action": "extract", "label": "page", "status": "failed", "reason": "target-not-found", "url": "https://example.org/", "ms": 10}
        ]);
        db.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO site_recipe_runs (id, org_id, site, name, yaml, params, trigger, status, steps, extracts, keys, finished_at) \
             VALUES ($1, $2, 'example.org', 'programme', $3, '{}', 'studio', 'failed', $4, '[]', '{}', now())",
            [earlier.into(), org.into(), yaml.into(), steps.into()],
        ))
        .await
        .unwrap();
        let start = |resume: Value| {
            server
                .post("/api/v1/site-recipes/runs")
                .add_header("Authorization", &auth)
                .add_header("X-Org-Id", org.to_string())
                .json(&json!({"yaml": yaml, "resume": resume}))
        };
        // Step 1 starts on the page left by step 0, which passed.
        let ok = start(json!({"run_id": earlier, "step": 1})).await;
        assert_eq!(ok.status_code().as_u16(), 202, "{}", ok.text());
        // Step 2 would start after the failed step 1; step 0 has no page before.
        for step in [0, 2, 9] {
            let r = start(json!({"run_id": earlier, "step": step})).await;
            assert_eq!(r.status_code().as_u16(), 400, "step {step}: {}", r.text());
        }
        let r = start(json!({"run_id": uuid::Uuid::new_v4(), "step": 1})).await;
        assert_eq!(r.json::<Value>()["error"], "site-recipe-run-not-found");
        // Another site's run is refused.
        let other = yaml.replace("example.org", "example.com");
        let r = server
            .post("/api/v1/site-recipes/runs")
            .add_header("Authorization", &auth)
            .add_header("X-Org-Id", org.to_string())
            .json(&json!({"yaml": other, "resume": {"run_id": earlier, "step": 1}}))
            .await;
        assert_eq!(r.status_code().as_u16(), 400, "{}", r.text());
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_assistant_tries_and_proposes_rules_without_saving() {
    use pnex_backend::services::ai::tools::{execute, ToolDeps};
    with_app(|server, token, ctx| async move {
        let auth = format!("Bearer {token}");
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", &auth)
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let deps = |role: pnex_backend::models::_entities::sea_orm_active_enums::OrgMemberRole| ToolDeps {
            db: &ctx.db,
            org_id: org,
            grants: pnex_backend::services::iam::Grants::managed(&role),
            author: None,
            o2: None,
            config: Some(&ctx.config),
            app: Some(&ctx),
            actor: Some(pnex_backend::services::ontology::Actor::manual(1, role)),
        };
        use pnex_backend::models::_entities::sea_orm_active_enums::OrgMemberRole as R;
        let yaml = "name: programme\nsite: https://www.example.org/x\nsteps:\n  - open: \"/\"\n  - extract: {all_text: true, as: page}\n";
        let r = execute(&deps(R::Member), "try_site_recipe", &json!({"yaml": yaml})).await.unwrap().value;
        assert_eq!(r["status"], "failed", "{r}");
        assert_eq!(r["reason"], pnex_core::site_recipe::reason::BROWSER);
        let run = execute(&deps(R::Member), "get_site_recipe_run", &json!({"run_id": r["run_id"]})).await.unwrap().value;
        assert_eq!(run["run_id"], r["run_id"]);
        let bad = execute(&deps(R::Member), "try_site_recipe", &json!({"yaml": "name: x\nsite: example.org\nsteps: []\n"})).await.unwrap_err();
        assert_eq!(bad.code, Some("site-recipe-invalid"));
        let p = execute(&deps(R::Member), "propose_site_recipe", &json!({"yaml": yaml})).await.unwrap().value;
        assert_eq!((p["executed"].as_bool(), p["site"].as_str()), (Some(false), Some("example.org")), "{p}");
        let list = execute(&deps(R::Member), "list_site_recipes", &json!({})).await.unwrap().value;
        assert_eq!(list["rules"], json!([]), "a proposal saves nothing");
        // A viewer reads but neither plays nor proposes.
        let denied = execute(&deps(R::Viewer), "try_site_recipe", &json!({"yaml": yaml})).await.unwrap_err();
        assert_eq!(denied.code, Some("access-denied"));
        execute(&deps(R::Viewer), "list_site_recipes", &json!({})).await.unwrap();
    })
    .await;
}
