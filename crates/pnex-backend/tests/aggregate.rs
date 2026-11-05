//! Read-only aggregates (object-aggregates.md D579–D582), on an invented
//! ontology: the exact numbers of each metric and split, currencies kept
//! apart, what a role may not read refused, the org boundary.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

const ROUTE: &str = "/api/v1/ontology/aggregate/query";

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, String) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    with_db(|server, alice, bob, _| f(server, alice, bob)).await;
}

async fn with_db<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, String, String, sea_orm::DatabaseConnection) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let base = common::spawn_mock_rauthy().await;
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let config: RequestConfig = RequestConfigBuilder::new().build();
    let alice = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000a",
        "alice",
        "alice@example.com",
    );
    let bob = common::valid_token(
        &base,
        "00000000-0000-0000-0000-00000000000b",
        "bob",
        "bob@example.com",
    );
    loco_rs::testing::request::request_with_config::<App, _, _>(
        config,
        move |server, ctx| async move {
            common::seed_catalogue(&ctx.db).await;
            f(server, alice, bob, ctx.db.clone()).await;
        },
    )
    .await;
}

async fn call(
    server: &axum_test::TestServer,
    path: &str,
    token: &str,
    org: i64,
    body: Value,
) -> (u16, Value) {
    let res = server
        .post(path)
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .json(&body)
        .await;
    let status = res.status_code().as_u16();
    let text = res.text();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn me(server: &axum_test::TestServer, token: &str) -> Value {
    server
        .get("/api/v1/user-info")
        .add_header("Authorization", format!("Bearer {token}"))
        .await
        .json::<Value>()
}

async fn object(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    type_key: &str,
    title: &str,
    p: Value,
) -> String {
    let (st, o) = call(
        server,
        "/api/v1/ontology/objects",
        token,
        org,
        json!({"type_key": type_key, "title": title, "properties": p}),
    )
    .await;
    assert_eq!(st, 201, "{o}");
    o["id"].as_str().unwrap().to_string()
}

async fn link(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    link_type: &str,
    a: &str,
    b: &str,
) {
    let (st, l) = call(
        server,
        "/api/v1/ontology/links",
        token,
        org,
        json!({"link_type": link_type, "source_id": a, "target_id": b}),
    )
    .await;
    assert_eq!(st, 201, "{l}");
}

/// Articles by two authors of one desk; a third article without author.
async fn seed(server: &axum_test::TestServer, alice: &str, org: i64) -> (String, String, String) {
    for t in [
        json!({"def": {"key": "desk", "name": "Desk", "properties": []}}),
        json!({"def": {"key": "writer", "name": "Writer", "title_classification": "personal", "properties": []}}),
        json!({"def": {"key": "article", "name": "Article", "properties": [
            {"key": "state", "name": "State", "kind": "enum", "values": ["draft", "live"]},
            {"key": "words", "name": "Words", "kind": "number"},
            {"key": "price", "name": "Price", "kind": "money"},
            {"key": "published", "name": "Published", "kind": "date"},
            {"key": "note", "name": "Note", "kind": "text", "classification": "personal"}
        ]}}),
    ] {
        let (st, e) = call(server, "/api/v1/ontology/types", alice, org, t).await;
        assert_eq!(st, 201, "{e}");
    }
    for l in [
        json!({"key": "written_by", "name": "written by", "from_types": ["article"], "to_types": ["writer"]}),
        json!({"key": "member_of_desk", "name": "member of", "from_types": ["writer"], "to_types": ["desk"]}),
    ] {
        let (st, e) = call(server, "/api/v1/ontology/link-types", alice, org, l).await;
        assert_eq!(st, 201, "{e}");
    }
    let desk = object(server, alice, org, "desk", "News", json!({})).await;
    let ann = object(server, alice, org, "writer", "Ann", json!({})).await;
    let lee = object(server, alice, org, "writer", "Lee", json!({})).await;
    link(server, alice, org, "member_of_desk", &ann, &desk).await;
    link(server, alice, org, "member_of_desk", &lee, &desk).await;
    let money = |a: f64, c: &str| json!({"amount": a, "currency": c});
    let a1 = object(server, alice, org, "article", "One", json!({
        "state": "live", "words": 100, "price": money(10.0, "EUR"), "published": "2026-01-15", "note": "x"})).await;
    let a2 = object(
        server,
        alice,
        org,
        "article",
        "Two",
        json!({
        "state": "live", "words": 300, "price": money(5.0, "USD"), "published": "2026-01-20"}),
    )
    .await;
    let a3 = object(
        server,
        alice,
        org,
        "article",
        "Three",
        json!({
        "state": "draft", "price": money(1.0, "EUR"), "published": "2026-02-01"}),
    )
    .await;
    object(server, alice, org, "article", "Four", json!({})).await;
    link(server, alice, org, "written_by", &a1, &ann).await;
    link(server, alice, org, "written_by", &a2, &ann).await;
    link(server, alice, org, "written_by", &a2, &lee).await;
    link(server, alice, org, "written_by", &a3, &lee).await;
    (ann, lee, desk)
}

fn articles(extra: Value) -> Value {
    let mut v = json!({"query": {"type_key": "article"}});
    v.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    v
}

/// The group holding `key` as its first key.
fn group<'a>(r: &'a Value, key: Option<&str>) -> &'a Value {
    r["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["key"][0].as_str() == key)
        .unwrap_or_else(|| panic!("no group {key:?} in {r}"))
}

#[tokio::test]
#[serial]
async fn each_metric_and_split_gives_the_exact_number() {
    with_app(|server, alice, _| async move {
        let org = me(&server, &alice).await["orgs"][0]["id"].as_i64().unwrap();
        let (ann, lee, desk) = seed(&server, &alice, org).await;
        let ask = |body: Value| {
            let (server, alice) = (&server, &alice);
            async move {
                let (st, r) = call(server, ROUTE, alice, org, body).await;
                assert_eq!(st, 200, "{r}");
                r
            }
        };

        // No split: one group; a filter selects, `where` counts apart.
        let r = ask(articles(json!({"metrics": [
            {"key": "all", "op": "count"},
            {"key": "live", "op": "count", "where": {"key": "state", "op": "eq", "value": "live"}},
            {"key": "rate", "ratio": ["live", "all"]},
            {"key": "words", "op": "sum", "property": "words"},
            {"key": "mean", "op": "avg", "property": "words"},
            {"key": "low", "op": "min", "property": "words"},
            {"key": "high", "op": "max", "property": "words"},
            {"key": "states", "op": "count_distinct", "property": "state"}
        ]}))).await;
        assert_eq!(r["groups"].as_array().unwrap().len(), 1, "{r}");
        let v = &r["groups"][0]["values"];
        assert_eq!(v["all"]["value"], 4.0, "{r}");
        assert_eq!(v["live"]["value"], 2.0);
        assert_eq!(v["rate"]["value"], 0.5);
        assert_eq!(v["words"]["value"], 400.0);
        assert_eq!(v["mean"]["value"], 200.0);
        assert_eq!((&v["low"]["value"], &v["high"]["value"]), (&json!(100.0), &json!(300.0)));
        assert_eq!(v["states"]["value"], 2.0);
        assert_eq!(r["truncated"], false);

        // By enum: the largest first, the objects without a value in a null group.
        let r = ask(articles(json!({
            "group_by": [{"property": "state"}],
            "metrics": [{"key": "n", "op": "count"}]
        }))).await;
        assert_eq!(r["groups"][0]["key"][0], "live", "{r}");
        assert_eq!(group(&r, Some("live"))["values"]["n"]["value"], 2.0);
        assert_eq!(group(&r, Some("draft"))["values"]["n"]["value"], 1.0);
        assert_eq!(group(&r, None)["values"]["n"]["value"], 1.0);

        // Money: one value per currency, never a total across them.
        let r = ask(articles(json!({
            "group_by": [{"property": "state"}],
            "metrics": [{"key": "n", "op": "count"}, {"key": "total", "op": "sum", "property": "price"}]
        }))).await;
        let live = &group(&r, Some("live"))["values"]["total"];
        assert_eq!(live["by_currency"], json!({"EUR": 10.0, "USD": 5.0}), "{r}");
        assert!(live.get("value").is_none(), "{r}");

        // By month, in date order.
        let r = ask(articles(json!({
            "group_by": [{"property": "published", "bucket": "month"}],
            "metrics": [{"key": "n", "op": "count"}]
        }))).await;
        let keys: Vec<_> = r["groups"].as_array().unwrap().iter().map(|g| g["key"][0].clone()).collect();
        assert_eq!(keys, vec![json!("2026-01-01"), json!("2026-02-01"), Value::Null], "{r}");
        assert_eq!(group(&r, Some("2026-01-01"))["values"]["n"]["value"], 2.0);

        // By link: an article with two writers counts under both (D581).
        let by_writer = articles(json!({
            "group_by": [{"hops": [{"link_type": "written_by", "direction": "out"}]}],
            "metrics": [{"key": "n", "op": "count"}]
        }));
        let r = ask(by_writer.clone()).await;
        let g = group(&r, Some(ann.as_str()));
        assert_eq!((&g["values"]["n"]["value"], &g["labels"][0]), (&json!(2.0), &json!("Ann")), "{r}");
        assert_eq!(group(&r, Some(lee.as_str()))["values"]["n"]["value"], 2.0);
        assert_eq!(group(&r, None)["values"]["n"]["value"], 1.0, "without a writer");

        // Two hops, and a filter on the objects selected.
        let r = ask(json!({
            "query": {"type_key": "article", "filters": [{"key": "state", "op": "eq", "value": "live"}]},
            "group_by": [{"hops": [{"link_type": "written_by", "direction": "out"}, {"link_type": "member_of_desk", "direction": "out"}]}],
            "metrics": [{"key": "n", "op": "count"}]
        })).await;
        let g = group(&r, Some(desk.as_str()));
        assert_eq!((&g["values"]["n"]["value"], &g["labels"][0]), (&json!(2.0), &json!("News")), "{r}");

        // Two keys.
        let r = ask(articles(json!({
            "group_by": [{"property": "state"}, {"property": "published", "bucket": "year"}],
            "metrics": [{"key": "n", "op": "count"}]
        }))).await;
        assert_eq!(group(&r, Some("live"))["key"][1], "2026-01-01", "{r}");

        // Past the limit: the rest adds up in `others`.
        let r = ask(articles(json!({
            "group_by": [{"property": "state"}], "limit": 1,
            "metrics": [{"key": "n", "op": "count"}]
        }))).await;
        assert_eq!(r["groups"].as_array().unwrap().len(), 1, "{r}");
        assert_eq!(r["others"]["values"]["n"]["value"], 2.0, "{r}");

        // Relative dates, as the flows write them.
        let r = ask(articles(json!({"metrics": [
            {"key": "past", "op": "count", "where": {"key": "published", "op": "lt", "value": "$today-1"}},
            {"key": "future", "op": "count", "where": {"key": "published", "op": "gt", "value": "$today+1"}}
        ]}))).await;
        assert_eq!(r["groups"][0]["values"]["past"]["value"], 3.0, "{r}");
        assert_eq!(r["groups"][0]["values"]["future"]["value"], 0.0, "{r}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_bad_request_is_refused_by_field() {
    with_app(|server, alice, _| async move {
        let org = me(&server, &alice).await["orgs"][0]["id"].as_i64().unwrap();
        seed(&server, &alice, org).await;
        let n = json!([{"key": "n", "op": "count"}]);
        for (body, field) in [
            (articles(json!({"metrics": [{"key": "s", "op": "sum", "property": "state"}]})), "metrics.0.property"),
            (articles(json!({"metrics": [{"key": "s", "op": "sum", "property": "nope"}]})), "metrics.0.property"),
            (articles(json!({"group_by": [{"property": "nope"}], "metrics": n})), "group_by.0"),
            (articles(json!({"group_by": [{"property": "state", "bucket": "day"}], "metrics": n})), "group_by.0"),
            (articles(json!({"group_by": [{"property": "price"}], "metrics": n})), "group_by.0"),
            (articles(json!({"group_by": [{"hops": [{"link_type": "nope", "direction": "out"}]}], "metrics": n})), "group_by.0"),
            (articles(json!({"metrics": [
                {"key": "t", "op": "sum", "property": "price"}, {"key": "a", "op": "count"},
                {"key": "r", "ratio": ["t", "a"]}]})), "metrics.2"),
            (json!({"query": {"type_key": "nope"}, "metrics": n}), "type_key"),
        ] {
            let (st, e) = call(&server, ROUTE, &alice, org, body.clone()).await;
            assert_eq!(st, 400, "{body}: {e}");
            assert!(e["errors"]["fields"].get(field).is_some(), "{field} in {e}");
        }
    })
    .await;
}

/// D582: what a role may not read is refused by name, in a split, a metric
/// or a condition; with the permission the same request answers.
#[tokio::test]
#[serial]
async fn an_aggregate_never_reveals_what_the_caller_may_not_read() {
    with_app(|server, alice, bob| async move {
        let org = me(&server, &alice).await["orgs"][0]["id"].as_i64().unwrap();
        seed(&server, &alice, org).await;
        let bob_id = me(&server, &bob).await["id"].as_i64().unwrap();
        let (st, m) = call(&server, &format!("/api/v1/orgs/{org}/members"), &alice, org,
            json!({"email": "bob@example.com", "role": "member"})).await;
        assert!(st == 200 || st == 201, "{m}");
        let (st, role) = call(&server, "/api/v1/iam/roles", &alice, org, json!({
            "key": "no-personal", "name": "No personal data",
            "statements": [{"effect": "deny", "permissions": ["ontology.personal:read"]}]
        })).await;
        assert_eq!(st, 201, "{role}");

        let n = json!([{"key": "n", "op": "count"}]);
        let hidden = [
            articles(json!({"group_by": [{"property": "note"}], "metrics": n})),
            articles(json!({"metrics": [{"key": "d", "op": "count_distinct", "property": "note"}]})),
            articles(json!({"metrics": [{"key": "x", "op": "count", "where": {"key": "note", "op": "eq", "value": "x"}}]})),
            articles(json!({"group_by": [{"hops": [{"link_type": "written_by", "direction": "out"}]}], "metrics": n})),
        ];
        for body in &hidden {
            let (st, e) = call(&server, ROUTE, &bob, org, body.clone()).await;
            assert_eq!(st, 200, "before the deny: {body}: {e}");
        }
        let (st, a) = call(&server, "/api/v1/iam/assignments", &alice, org,
            json!({"user_id": bob_id, "role_id": role["id"]})).await;
        assert_eq!(st, 201, "{a}");
        for body in &hidden {
            let (st, e) = call(&server, ROUTE, &bob, org, body.clone()).await;
            assert_eq!((st, e["error"].as_str()), (403, Some("access-denied")), "{body}: {e}");
        }
        // A query filter on it matches nothing, as in the plain query: no oracle.
        let (st, r) = call(&server, ROUTE, &bob, org, json!({
            "query": {"type_key": "article", "filters": [{"key": "note", "op": "eq", "value": "x"}]},
            "metrics": n
        })).await;
        assert_eq!((st, &r["groups"][0]["values"]["n"]["value"]), (200, &json!(0.0)), "{r}");
        // What stays readable still answers.
        let (st, r) = call(&server, ROUTE, &bob, org, articles(json!({"metrics": n}))).await;
        assert_eq!((st, &r["groups"][0]["values"]["n"]["value"]), (200, &json!(4.0)), "{r}");

        // Without ontology:read: nothing at all.
        let (st, role) = call(&server, "/api/v1/iam/roles", &alice, org, json!({
            "key": "no-read", "name": "No ontology",
            "statements": [{"effect": "deny", "permissions": ["ontology:read"]}]
        })).await;
        assert_eq!(st, 201, "{role}");
        let (st, a) = call(&server, "/api/v1/iam/assignments", &alice, org,
            json!({"user_id": bob_id, "role_id": role["id"]})).await;
        assert_eq!(st, 201, "{a}");
        let (st, e) = call(&server, ROUTE, &bob, org, articles(json!({"metrics": n}))).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("access-denied")), "{e}");
        let transitions = "/api/v1/ontology/transitions/query";
        let body = json!({"type_key": "article", "property": "state"});
        let (st, e) = call(&server, transitions, &bob, org, body.clone()).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("access-denied")), "{e}");

        // Another org sees neither the type nor its objects (R1).
        let (st, other) = call(&server, "/api/v1/orgs", &bob, org, json!({"name": "Elsewhere"})).await;
        assert!(st == 200 || st == 201, "{other}");
        let (st, e) = call(&server, transitions, &bob, other["id"].as_i64().unwrap(), body).await;
        assert_eq!(st, 400, "the type is not known there: {e}");
        let (st, _) = call(&server, ROUTE, &bob, other["id"].as_i64().unwrap(), articles(json!({"metrics": n}))).await;
        assert_eq!(st, 400, "the type does not exist over there");
    })
    .await;
}

/// What a type dashboard reads: the shown object, then the link (D583).
#[tokio::test]
#[serial]
async fn a_type_dashboard_reads_what_its_object_links_to() {
    with_app(|server, alice, _| async move {
        let org = me(&server, &alice).await["orgs"][0]["id"].as_i64().unwrap();
        let (ann, _, _) = seed(&server, &alice, org).await;
        let metrics = json!([
            {"key": "n", "op": "count"},
            {"key": "words", "op": "sum", "property": "words"}
        ]);
        let query = |id: &str| {
            json!({"query": {"type_key": "writer", "ids": [id],
                "traverse": [{"link_type": "written_by", "direction": "in"}]},
              "metrics": metrics})
        };
        let (st, r) = call(&server, ROUTE, &alice, org, query(&ann)).await;
        assert_eq!(st, 200, "{r}");
        assert_eq!(r["groups"][0]["values"]["n"]["value"], json!(2.0));
        assert_eq!(r["groups"][0]["values"]["words"]["value"], json!(400.0));

        // `filters` select the start, `reached` what the link leads to.
        let mut live = query(&ann);
        live["query"]["reached"] = json!([{"key": "state", "op": "eq", "value": "draft"}]);
        let (_, r) = call(&server, ROUTE, &alice, org, live.clone()).await;
        assert_eq!(r["groups"][0]["values"]["n"]["value"], json!(0.0), "{r}");
        live["query"]["reached"] = json!([{"key": "words", "op": "gt", "value": 200}]);
        let (_, r) = call(&server, ROUTE, &alice, org, live).await;
        assert_eq!(r["groups"][0]["values"]["n"]["value"], json!(1.0), "{r}");

        // The token never reaches SQL: the page fills it, or reads nothing.
        let (st, r) = call(&server, ROUTE, &alice, org, query("$object")).await;
        assert_eq!(st, 400, "{r}");

        // Saved as such, on a dashboard of that type only.
        let widget = json!({"id": "w", "type": "stat", "title": "Words",
            "x": 0, "y": 0, "w": 200, "h": 120,
            "source": [{"role": "primary", "metric": "", "device_id": "", "window": "1h",
                "objects": {"query": {"type_key": "writer", "ids": ["$object"],
                    "traverse": [{"link_type": "written_by", "direction": "in"}]},
                  "metrics": [{"key": "n", "op": "count"}], "show": "n"}}]});
        let save = |layout: Value| json!({"name": "Writer", "layout": layout});
        let (st, r) = call(
            &server,
            "/api/v1/dashboards",
            &alice,
            org,
            save(json!({"canvas": {"width": 800, "height": 600}, "widgets": [widget], "object_type": "writer"})),
        )
        .await;
        assert_eq!(st, 201, "{r}");
        let (st, r) = call(
            &server,
            "/api/v1/dashboards",
            &alice,
            org,
            save(json!({"canvas": {"width": 800, "height": 600}, "widgets": [widget]})),
        )
        .await;
        assert_eq!(st, 400, "{r}");
        assert_eq!(r["errors"]["violations"][0]["code"], "object_source_without_type");
    })
    .await;
}

/// The flow node reads as its deployed self (D587).
#[tokio::test]
#[serial]
async fn the_flow_node_computes_what_it_declares() {
    use pnex_backend::models::_entities::{flow_versions, flows};
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel};
    with_db(|server, alice, _, db| async move {
        let org = me(&server, &alice).await["orgs"][0]["id"].as_i64().unwrap();
        seed(&server, &alice, org).await;
        unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", "tok-ag-1") };
        let graph = json!({"nodes": [
            {"id": "a", "kind": "ontology_aggregate", "config": {"object_type": "article",
              "filters": [{"key": "state", "op": "eq", "value": "$msg.payload.state"}],
              "metrics": [{"key": "words", "op": "sum", "property": "words"}]}},
            {"id": "q", "kind": "ontology_query", "config": {"object_type": "article"}}
        ]});
        let flow = flows::ActiveModel {
            name: Set("figures".into()),
            status: Set(pnex_core::FLOW_STATUS_DEPLOYED.into()),
            org_id: Set(org),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        let version = flow_versions::ActiveModel {
            flow_id: Set(flow.id),
            version_number: Set(1),
            graph: Set(graph),
            ..Default::default()
        }
        .insert(&db)
        .await
        .unwrap();
        let flow_id = flow.id;
        let mut am = flow.into_active_model();
        am.deployed_version_id = Set(Some(version.id));
        sea_orm::ActiveModelTrait::update(am, &db).await.unwrap();

        let ask = |node: &str, value: &str| {
            json!({"node": {"org_id": org, "flow_id": flow_id, "version": 1, "node_id": node},
                   "values": [value]})
        };
        let post = |body: Value| {
            server
                .post("/internal/flow/ontology/aggregate")
                .add_header("x-pnex-flow-token", "tok-ag-1")
                .json(&body)
        };
        let res = post(ask("a", "live")).await;
        assert_eq!(res.status_code().as_u16(), 200, "{}", res.text());
        let r: Value = res.json();
        assert_eq!(r["groups"][0]["values"]["words"]["value"], json!(400.0));

        // Another kind of node, or none: refused, whatever the request says.
        for node in ["q", "nope"] {
            let res = post(ask(node, "live")).await;
            assert_eq!(res.status_code().as_u16(), 403, "{}", res.text());
        }
        let res = server
            .post("/internal/flow/ontology/aggregate")
            .json(&ask("a", "live"))
            .await;
        assert_eq!(res.status_code().as_u16(), 401);

        // The runtime starts a flow before its deploy marks it deployed: a node
        // called in between gets its answer once the deploy holding the lock ends.
        use pnex_backend::services::db_lock::{ns, TenantLock};
        use pnex_backend::services::ontology::flow_nodes;
        let set_status = |status: &'static str| {
            let db = db.clone();
            async move {
                let mut am = flows::Entity::find_by_id(flow_id)
                    .one(&db)
                    .await
                    .unwrap()
                    .unwrap()
                    .into_active_model();
                am.status = Set(status.into());
                sea_orm::ActiveModelTrait::update(am, &db).await.unwrap();
            }
        };
        set_status(pnex_core::FLOW_STATUS_DRAFT).await;
        let request: pnex_core::ontology::flow::FlowQueryRequest =
            serde_json::from_value(ask("a", "live")).unwrap();
        // No deploy in progress: refused at once, as before.
        assert!(matches!(
            flow_nodes::aggregate(&db, &request).await,
            Err(flow_nodes::NodeError::NotDeployed)
        ));
        let lock =
            TenantLock::acquire(&db, ns::FLOW_DEPLOY, org, std::time::Duration::from_secs(5))
                .await
                .unwrap();
        let waiting = tokio::spawn({
            let db = db.clone();
            let request = request.clone();
            async move { flow_nodes::aggregate(&db, &request).await.is_ok() }
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!waiting.is_finished(), "the node waits for the deploy");
        set_status(pnex_core::FLOW_STATUS_DEPLOYED).await;
        // Without the lock's release: a stop holds it while it waits for this node to end.
        let answer = tokio::time::timeout(std::time::Duration::from_secs(2), waiting).await;
        assert!(
            answer.expect("no wait for the lock").unwrap(),
            "the node runs once marked"
        );
        lock.release().await;

        // A property the type does not have stops the deploy.
        let bad: pnex_core::flow::FlowGraph = serde_json::from_value(json!({"nodes": [
            {"id": "a", "kind": "ontology_aggregate", "config": {"object_type": "article",
              "metrics": [{"key": "x", "op": "sum", "property": "nope"}]}}
        ]}))
        .unwrap();
        let v = pnex_backend::services::ontology::flow_nodes::deploy_violations(&db, org, &bad)
            .await
            .unwrap();
        assert_eq!(v.len(), 1, "{v:?}");
        unsafe { std::env::remove_var("PNEX_FLOW_RUNTIME_TOKEN") };
    })
    .await;
}
