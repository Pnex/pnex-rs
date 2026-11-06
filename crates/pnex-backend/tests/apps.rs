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
        for key in ["article", "author", "draft"] {
            let (st, t) = call(&server, "POST", "/api/v1/ontology/types", &alice, org,
                Some(json!({"def": {"key": key, "name": key, "properties": [{"key": "lang", "name": "Lang", "kind": "text", "max_len": 8}]}}))).await;
            assert_eq!(st, 201, "{t}");
        }
        let (st, _) = call(&server, "POST", "/api/v1/ontology/link-types", &alice, org,
            Some(json!({"key": "written_by", "name": "written by", "from_types": ["article"], "to_types": ["author"]}))).await;
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
            "scope": {"types": ["author", "article"], "link_types": ["written_by"], "media_kinds": ["photo"], "media_max_bytes": 5000000}
        });
        let (st, app) = call(&server, "POST", "/api/v1/apps", &alice, org, Some(decl.clone())).await;
        assert_eq!(st, 201, "{app}");
        assert_eq!(app["scope"]["types"], json!(["article", "author"]), "sorted: {app}");
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
        out["object_sets"] = json!({"drafts": {"query": {"type_key": "draft"}}});
        let (st, e) = call(&server, "PUT", &url, &alice, org, Some(out)).await;
        assert_eq!((st, e["errors"]["fields"]["object_sets.drafts.type_key"].as_str()), (400, Some("out-of-scope")), "{e}");
        let mut v2 = decl.clone();
        v2["description"] = json!("Reads articles");
        v2["object_sets"] = json!({
            "by_lang": {"description": "Articles of a language", "query": {"type_key": "article", "filters": [{"key": "lang", "op": "eq", "value": "$lang"}]}},
            "authors": {"query": {"type_key": "article", "traverse": [{"link_type": "written_by"}]}}
        });
        v2["expected_version"] = json!(1);
        let (st, a2) = call(&server, "PUT", &url, &alice, org, Some(v2.clone())).await;
        assert_eq!((st, &a2["version"]), (200, &json!(2)), "{a2}");
        let (st, e) = call(&server, "PUT", &url, &alice, org, Some(v2)).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("app-version-conflict")), "{e}");

        // Objects in and out of the scope.
        let obj = |t: &str, title: &str| json!({"type_key": t, "title": title, "properties": {"lang": "fr"}});
        call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("article", "Moteurs"))).await;
        call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("draft", "Secret"))).await;
        call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(obj("author", "Ada"))).await;

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
        let (_, all) = call(&server, "GET", "/api/v1/ontology/changes?since=0&types=article,author,draft", &alice, org, None).await;
        assert_eq!(titles(&all), vec!["Ada", "Moteurs", "Secret"], "a person reads every type: {all}");
        let (st, scoped) = call(&server, "GET", "/api/v1/ontology/changes?since=0", &secret, org, None).await;
        assert_eq!(st, 200, "{scoped}");
        assert_eq!(titles(&scoped), vec!["Ada", "Moteurs"], "the application, its scope: {scoped}");
        let (_, asked) = call(&server, "GET", "/api/v1/ontology/changes?since=0&types=draft,article", &secret, org, None).await;
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

/// Screens built in PneX (app-builder.md AB0–AB4), on an invented ontology:
/// a builder lays out what the application permits, readers see what was
/// published, and a screen narrows the caller's rights without adding any.
#[tokio::test]
#[serial]
async fn a_screen_reads_and_writes_inside_its_application() {
    with_app(|server, alice| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        for key in ["article", "draft"] {
            let (st, t) = call(&server, "POST", "/api/v1/ontology/types", &alice, org,
                Some(json!({"def": {"key": key, "name": key,
                    "properties": [{"key": "lang", "name": "Lang", "kind": "text", "max_len": 8}],
                    "actions": [{"key": "review", "name": "Review"}]}}))).await;
            assert_eq!(st, 201, "{t}");
        }
        let a = |title: &str, lang: &str| json!({"type_key": "article", "title": title, "properties": {"lang": lang}});
        let (_, fr) = call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(a("Moteur", "fr"))).await;
        call(&server, "POST", "/api/v1/ontology/objects", &alice, org, Some(a("Engine", "en"))).await;
        let (st, draft) = call(&server, "POST", "/api/v1/ontology/objects", &alice, org,
            Some(json!({"type_key": "draft", "title": "Secret"}))).await;
        assert_eq!(st, 201, "{draft}");

        // Actions are checked at the declaration.
        let decl = json!({"key": "library", "name": "Library", "scope": {"types": ["article"]}, "actions": ["draft.review"]});
        let (st, e) = call(&server, "POST", "/api/v1/apps", &alice, org, Some(decl)).await;
        assert_eq!((st, e["errors"]["fields"]["actions"].as_str()), (400, Some("out-of-scope")), "{e}");
        let (st, app) = call(&server, "POST", "/api/v1/apps", &alice, org, Some(json!({
            "key": "library", "name": "Library", "scope": {"types": ["article"]}, "actions": ["article.review"]
        }))).await;
        assert_eq!(st, 201, "{app}");
        let id = app["id"].as_str().unwrap().to_string();
        let url = format!("/api/v1/apps/{id}");

        // Build: an unknown home, an out-of-scope set, then the real thing.
        let (st, e) = call(&server, "PUT", &format!("{url}/build"), &alice, org, Some(json!({
            "screens": [{"key": "list", "name": "List"}], "home": "nope", "expected_version": 1
        }))).await;
        assert_eq!((st, e["errors"]["fields"]["home"].as_str()), (400, Some("invalid")), "{e}");
        let (st, e) = call(&server, "PUT", &format!("{url}/build"), &alice, org, Some(json!({
            "object_sets": {"drafts": {"query": {"type_key": "draft"}}}, "expected_version": 1
        }))).await;
        assert_eq!((st, e["errors"]["fields"]["object_sets.drafts.type_key"].as_str()), (400, Some("out-of-scope")), "{e}");
        let build = json!({
            "screens": [{"key": "list", "name": "List"}, {"key": "sheet", "name": "Sheet"}],
            "home": "list",
            "variables": [{"name": "lang"}, {"name": "picked", "kind": "object", "type_key": "article"}],
            "object_sets": {"by_lang": {"query": {"type_key": "article",
                "filters": [{"key": "lang", "op": "eq", "value": "$lang"}]}}},
            "expected_version": 1
        });
        let (st, built) = call(&server, "PUT", &format!("{url}/build"), &alice, org, Some(build.clone())).await;
        assert_eq!(st, 200, "{built}");
        assert_eq!(built["screens"][1]["key"], "sheet");
        let (st, _) = call(&server, "PUT", &format!("{url}/build"), &alice, org, Some(build)).await;
        assert_eq!(st, 409, "a build keeps to the version it read");
        let dash = built["screens"][0]["dashboard_id"].as_str().unwrap().to_string();

        // A screen is not a dashboard of the org, and cannot be deleted as one.
        let (_, list) = call(&server, "GET", "/api/v1/dashboards", &alice, org, None).await;
        assert_eq!(list["count"], 0, "{list}");
        let (st, _) = call(&server, "DELETE", &format!("/api/v1/dashboards/{dash}"), &alice, org, None).await;
        assert_eq!(st, 404);

        // Readers see nothing before the first publication.
        let (st, _) = call(&server, "GET", &format!("{url}/screens/list"), &alice, org, None).await;
        assert_eq!(st, 404);

        let widget = |ty: &str, options: Value| json!({"id": "w1", "type": ty, "x": 0, "y": 0, "w": 400, "h": 300, "source": [], "options": options});
        let save = |version: i64, w: Value| json!({"expected_version_number": version, "layout": {
            "canvas": {"width": 1600, "height": 900}, "widgets": [w], "wires": []}});
        let put = |body: Value| {
            let (server, alice, dash) = (&server, alice.clone(), dash.clone());
            async move {
                let mut r = server.patch(&format!("/api/v1/dashboards/{dash}"))
                    .add_header("Authorization", format!("Bearer {alice}"))
                    .add_header("X-Org-Id", org.to_string());
                r = r.json(&body);
                let res = r.await;
                (res.status_code().as_u16(), serde_json::from_str::<Value>(&res.text()).unwrap_or(Value::Null))
            }
        };
        // What a widget names exists in its application.
        let table = |o: Value| widget("object_table", o);
        for (options, code) in [
            (json!({"object_set": "nope"}), "app_set_unknown"),
            (json!({"object_set": "by_lang"}), "app_param_unset"),
            (json!({"object_set": "by_lang", "params": {"lang": "ghost"}}), "app_variable_unknown"),
            (json!({"object_set": "by_lang", "params": {"lang": "lang"}, "columns": ["ghost"]}), "app_column_unknown"),
            (json!({"object_set": "by_lang", "params": {"lang": "lang"}, "variable": "lang"}), "app_variable_kind"),
            (json!({"object_set": "by_lang", "params": {"lang": "lang"}, "screen": "ghost"}), "app_screen_unknown"),
        ] {
            let (st, e) = put(save(1, table(json!({"app": options})))).await;
            assert_eq!(st, 400, "{e}");
            assert_eq!(e["errors"]["violations"][0]["code"], code, "{e}");
        }
        // A bar click writes the enum variable of the property the bars group by (D726).
        let bars = json!({"id": "w1", "type": "range_bars", "x": 0, "y": 0, "w": 400, "h": 300,
            "options": {"app": {"variable": "lang"}},
            "source": [{"metric": "", "device_id": "", "window": "1h", "objects": {"object_set": "by_lang",
                "params": {"lang": "lang"}, "group_by": [{"property": "lang"}], "metrics": [{"key": "n", "op": "count"}], "show": "n"}}]});
        let (st, e) = put(save(1, bars.clone())).await;
        assert_eq!((st, e["errors"]["violations"][0]["code"].as_str()), (400, Some("app_variable_kind")), "{e}");
        let (st, e) = call(&server, "POST", "/api/v1/dashboards", &alice, org, Some(json!({"name": "D", "layout": {
            "canvas": {"width": 1600, "height": 900}, "widgets": [bars]}}))).await;
        assert_eq!((st, e["errors"]["violations"][0]["code"].as_str()), (400, Some("app_widget_outside_app")), "{e}");
        let (st, e) = put(save(1, widget("action_button", json!({"app": {"variable": "picked", "action": "draft.review"}})))).await;
        assert_eq!((st, e["errors"]["violations"][0]["code"].as_str()), (400, Some("app_action_not_permitted")), "{e}");
        let (st, e) = put(save(1, widget("object_form", json!({"app": {"object_type": "draft"}})))).await;
        assert_eq!((st, e["errors"]["violations"][0]["code"].as_str()), (400, Some("app_type_out_of_scope")), "{e}");
        let good = table(json!({"app": {"object_set": "by_lang", "params": {"lang": "lang"},
            "columns": ["title", "lang"], "variable": "picked", "screen": "sheet"}}));
        let (st, saved) = put(save(1, good)).await;
        assert_eq!(st, 200, "{saved}");
        assert_eq!(saved["app_id"].as_str(), Some(id.as_str()));

        // A dashboard of the org refuses these widgets.
        let (st, e) = call(&server, "POST", "/api/v1/dashboards", &alice, org, Some(json!({"name": "D", "layout": {
            "canvas": {"width": 1600, "height": 900},
            "widgets": [widget("object_table", json!({"app": {"object_set": "by_lang"}}))]}}))).await;
        assert_eq!((st, e["errors"]["violations"][0]["code"].as_str()), (400, Some("app_widget_outside_app")), "{e}");

        // Publishing pins what readers see.
        let current = built["version"].as_i64().unwrap();
        let (st, _) = call(&server, "POST", &format!("{url}/publish"), &alice, org, Some(json!({"expected_version": 1}))).await;
        assert_eq!(st, 409);
        let (st, p) = call(&server, "POST", &format!("{url}/publish"), &alice, org, Some(json!({"expected_version": current}))).await;
        assert_eq!(st, 200, "{p}");
        assert_eq!(p["screens"][0]["published_version"], 2, "{p}");
        let (st, shown) = call(&server, "GET", &format!("{url}/screens/list"), &alice, org, None).await;
        assert_eq!((st, shown["version"].clone()), (200, json!(2)), "{shown}");
        let (st, _) = put(save(2, table(json!({"app": {"object_set": "by_lang", "params": {"lang": "lang"}}})))).await;
        assert_eq!(st, 200);
        let (_, shown) = call(&server, "GET", &format!("{url}/screens/list"), &alice, org, None).await;
        assert_eq!(shown["version"], 2, "a saved draft does not reach readers");

        // Object sets, as a person, with parameters as values only.
        let (st, e) = call(&server, "GET", &format!("{url}/object-sets/by_lang"), &alice, org, None).await;
        assert_eq!((st, e["errors"]["fields"]["lang"].as_str()), (400, Some("required")), "{e}");
        let (st, r) = call(&server, "GET", &format!("{url}/object-sets/by_lang?lang=fr"), &alice, org, None).await;
        assert_eq!((st, r["count"].clone()), (200, json!(1)), "{r}");
        assert_eq!(r["rows"][0]["object"]["title"], "Moteur");
        let (st, r) = call(&server, "GET", &format!("{url}/object-sets/by_lang?lang=%25"), &alice, org, None).await;
        assert_eq!((st, r["count"].clone()), (200, json!(0)), "a value, never a pattern: {r}");

        // Writes through a screen keep to the application.
        let (st, e) = call(&server, "POST", &format!("/api/v1/ontology/objects?app={id}"), &alice, org,
            Some(json!({"type_key": "draft", "title": "Out"}))).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("app-out-of-scope")), "{e}");
        let (st, o) = call(&server, "POST", &format!("/api/v1/ontology/objects?app={id}"), &alice, org, Some(a("In", "fr"))).await;
        assert_eq!(st, 201, "{o}");
        let draft_id = draft["id"].as_str().unwrap();
        let (st, e) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{draft_id}?app={id}"), &alice, org,
            Some(json!({"type_key": "draft", "title": "Changed", "expected_version": 1}))).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("app-out-of-scope")), "{e}");
        let (st, e) = call(&server, "POST", &format!("/api/v1/ontology/objects/{draft_id}/actions/review?app={id}"), &alice, org,
            Some(json!({"params": {}}))).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("app-out-of-scope")), "{e}");
        let fr_id = fr["id"].as_str().unwrap();
        let (st, e) = call(&server, "POST", &format!("/api/v1/ontology/objects/{fr_id}/actions/review?app={id}"), &alice, org,
            Some(json!({"params": {}}))).await;
        assert!(matches!(st, 409 | 503), "permitted, no deployed flow: {st} {e}");

        // Removing a screen removes its versions; deleting the application the rest.
        let sheet = built["screens"][1]["dashboard_id"].as_str().unwrap().to_string();
        let (st, b2) = call(&server, "PUT", &format!("{url}/build"), &alice, org, Some(json!({
            "screens": [{"key": "list", "name": "Articles"}], "home": "list",
            "variables": [{"name": "lang"}, {"name": "picked", "kind": "object", "type_key": "article"}],
            "object_sets": {"by_lang": {"query": {"type_key": "article",
                "filters": [{"key": "lang", "op": "eq", "value": "$lang"}]}}},
            "expected_version": p["version"]
        }))).await;
        assert_eq!(st, 200, "{b2}");
        assert_eq!(b2["screens"][0]["published_version"], 2, "a kept screen stays published");
        let (st, _) = call(&server, "GET", &format!("/api/v1/dashboards/{sheet}"), &alice, org, None).await;
        assert_eq!(st, 404);
        let (st, _) = call(&server, "DELETE", &url, &alice, org, None).await;
        assert_eq!(st, 204);
        let (st, _) = call(&server, "GET", &format!("/api/v1/dashboards/{dash}"), &alice, org, None).await;
        assert_eq!(st, 404);
    })
    .await;
}
