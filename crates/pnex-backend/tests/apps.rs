//! Applications declared in an org (external-apps.md D433) on an invented
//! ontology: declaration and its checks, one application per service
//! account, `GET /apps/me`, the change feed kept to the scope, and the
//! service account that cannot declare.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, alice).await;
        },
    )
    .await;
}

async fn call(
    server: &axum_test::TestServer,
    method: &str,
    path: &str,
    token: &str,
    org: i64,
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "PUT" => server.put(path),
        "DELETE" => server.delete(path),
        _ => unreachable!(),
    }
    .add_header("Authorization", format!("Bearer {token}"))
    .add_header("X-Org-Id", org.to_string());
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.await;
    (
        res.status_code().as_u16(),
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

#[tokio::test]
#[serial]
async fn an_application_reads_its_scope() {
    with_app(|server, alice| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        for key in ["article", "auteur", "brouillon"] {
            let (st, t) = call(&server, "POST", "/api/v1/ontology/types", &alice, org,
                Some(json!({"def": {"key": key, "name": key, "properties": [{"key": "lang", "name": "Lang", "kind": "text", "max_len": 8}]}}))).await;
            assert_eq!(st, 201, "{t}");
        }
        let (st, _) = call(&server, "POST", "/api/v1/ontology/link-types", &alice, org,
            Some(json!({"key": "ecrit_par", "name": "written by", "from_types": ["article"], "to_types": ["auteur"]}))).await;
        assert_eq!(st, 201);
        let (_, sa) = call(&server, "POST", "/api/v1/iam/service-accounts", &alice, org, Some(json!({"name": "reader"}))).await;
        let sa_id = sa["id"].as_str().unwrap().to_string();

        // Checks of the declaration.
        let (st, e) = call(&server, "POST", "/api/v1/apps", &alice, org, Some(json!({
            "key": "Bad Key", "name": "", "scope": {"types": ["inconnu"], "media_kinds": ["hologram"]}
        }))).await;
        assert_eq!(st, 400, "{e}");
        let fields = &e["errors"]["fields"];
        assert_eq!((fields["key"].as_str(), fields["name"].as_str(), fields["scope.media_kinds"].as_str()),
            (Some("invalid"), Some("required"), Some("invalid")), "{e}");
        let (st, e) = call(&server, "POST", "/api/v1/apps", &alice, org, Some(json!({
            "key": "reader", "name": "Reader", "scope": {"types": ["inconnu"]}
        }))).await;
        assert_eq!((st, e["errors"]["fields"]["scope.types"].as_str()), (400, Some("not-found")), "{e}");

        let decl = json!({
            "key": "reader", "name": "Reader", "service_account_id": sa_id,
            "scope": {"types": ["auteur", "article"], "link_types": ["ecrit_par"], "media_kinds": ["photo"], "media_max_bytes": 5000000}
        });
        let (st, app) = call(&server, "POST", "/api/v1/apps", &alice, org, Some(decl.clone())).await;
        assert_eq!(st, 201, "{app}");
        assert_eq!(app["scope"]["types"], json!(["article", "auteur"]), "sorted: {app}");
        let mut other = decl.clone();
        other["key"] = json!("other");
        let (st, e) = call(&server, "POST", "/api/v1/apps", &alice, org, Some(other)).await;
        assert_eq!((st, e["errors"]["fields"]["service_account_id"].as_str()), (400, Some("unique")), "one application per account: {e}");
        let (st, e) = call(&server, "POST", "/api/v1/apps", &alice, org, Some(json!({"key": "reader", "name": "Again"}))).await;
        assert_eq!((st, e["errors"]["fields"]["key"].as_str()), (400, Some("unique")), "{e}");
        let (st, list) = call(&server, "GET", "/api/v1/apps", &alice, org, None).await;
        assert_eq!((st, &list["count"]), (200, &json!(1)), "{list}");

        // Replace on the version read.
        let url = format!("/api/v1/apps/{}", app["id"].as_str().unwrap());
        let mut out = decl.clone();
        out["expected_version"] = json!(1);
        out["object_sets"] = json!({"drafts": {"query": {"type_key": "brouillon"}}});
        let (st, e) = call(&server, "PUT", &url, &alice, org, Some(out)).await;
        assert_eq!((st, e["errors"]["fields"]["object_sets.drafts.type_key"].as_str()), (400, Some("out-of-scope")), "{e}");
        let mut v2 = decl.clone();
        v2["description"] = json!("Reads articles");
        v2["object_sets"] = json!({
            "by_lang": {"description": "Articles of a language", "query": {"type_key": "article", "filters": [{"key": "lang", "op": "eq", "value": "$lang"}]}},
            "authors": {"query": {"type_key": "article", "traverse": [{"link_type": "ecrit_par"}]}}
        });
        v2["expected_version"] = json!(1);
        let (st, a2) = call(&server, "PUT", &url, &alice, org, Some(v2.clone())).await;
        assert_eq!((st, &a2["version"]), (200, &json!(2)), "{a2}");
        let (st, e) = call(&server, "PUT", &url, &alice, org, Some(v2)).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("app-version-conflict")), "{e}");

        // Objects in and out of the scope.
        let obj = |t: &str, title: &str| json!({"type_key": t, "title": title, "properties": {"lang": "fr"}});
        call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("article", "Moteurs"))).await;
        call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("brouillon", "Secret"))).await;
        call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("auteur", "Ada"))).await;

        // The application's key.
        let expires = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
        let (_, key) = call(&server, "POST", &format!("/api/v1/iam/service-accounts/{sa_id}/keys"), &alice, org, Some(json!({"expires_at": expires}))).await;
        let secret = key["secret"].as_str().unwrap().to_string();
        call(&server, "POST", "/api/v1/iam/assignments", &alice, org, Some(json!({"service_account_id": sa_id, "managed_role": "viewer"}))).await;

        let (st, me) = call(&server, "GET", "/api/v1/apps/me", &secret, org, None).await;
        assert_eq!((st, me["key"].as_str()), (200, Some("reader")), "{me}");
        let (st, e) = call(&server, "GET", "/api/v1/apps/me", &alice, org, None).await;
        assert_eq!((st, e["error"].as_str()), (404, Some("app-not-found")), "a person is no application");

        let titles = |page: &Value| -> Vec<String> {
            let mut t: Vec<String> = page["changes"].as_array().unwrap().iter()
                .filter_map(|c| c["object"]["title"].as_str().map(str::to_string)).collect();
            t.sort();
            t
        };
        let (_, all) = call(&server, "GET", "/api/v1/ontology/changes?since=0&types=article,auteur,brouillon", &alice, org, None).await;
        assert_eq!(titles(&all), vec!["Ada", "Moteurs", "Secret"], "a person reads every type: {all}");
        let (st, scoped) = call(&server, "GET", "/api/v1/ontology/changes?since=0", &secret, org, None).await;
        assert_eq!(st, 200, "{scoped}");
        assert_eq!(titles(&scoped), vec!["Ada", "Moteurs"], "the application, its scope: {scoped}");
        let (_, asked) = call(&server, "GET", "/api/v1/ontology/changes?since=0&types=brouillon,article", &secret, org, None).await;
        assert_eq!(titles(&asked), vec!["Moteurs"], "never past its scope: {asked}");

        // Its object sets, with their parameters.
        let (st, r) = call(&server, "GET", "/api/v1/apps/me/object-sets/by_lang?lang=fr", &secret, org, None).await;
        assert_eq!((st, &r["count"]), (200, &json!(1)), "{r}");
        let (_, r) = call(&server, "GET", "/api/v1/apps/me/object-sets/by_lang?lang=en", &secret, org, None).await;
        assert_eq!(r["count"], 0, "{r}");
        let (st, e) = call(&server, "GET", "/api/v1/apps/me/object-sets/by_lang", &secret, org, None).await;
        assert_eq!((st, e["errors"]["fields"]["lang"].as_str()), (400, Some("required")), "{e}");
        let (st, _) = call(&server, "GET", "/api/v1/apps/me/object-sets/nope", &secret, org, None).await;
        assert_eq!(st, 404);

        // The application cannot declare itself more.
        let (st, _) = call(&server, "POST", "/api/v1/apps", &secret, org, Some(json!({"key": "more", "name": "More"}))).await;
        assert_eq!(st, 403);
        let (st, _) = call(&server, "DELETE", &url, &alice, org, None).await;
        assert_eq!(st, 204);
        let (st, _) = call(&server, "GET", &url, &alice, org, None).await;
        assert_eq!(st, 404);
    })
    .await;
}

/// An unknown API route answers a coded 404 in JSON, never the app's HTML
/// page; known routes keep answering.
#[tokio::test]
#[serial]
async fn an_unknown_api_route_is_a_json_404() {
    with_app(|server, alice| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        for path in [
            "/api/v1/nothing",
            "/api/v1/ontology/objects/00000000-0000-0000-0000-000000000000/nothing",
            "/api/v2/apps",
        ] {
            let (st, e) = call(&server, "GET", path, &alice, org, None).await;
            assert_eq!(
                (st, e["error"].as_str()),
                (404, Some("api-route-not-found")),
                "{path}: {e}"
            );
        }
        let (st, _) = call(&server, "GET", "/api/v1/apps", &alice, org, None).await;
        assert_eq!(st, 200, "a known route still answers");
    })
    .await;
}
