//! The sales follow-up pack (crm.md D448–D452): installed with what it
//! builds on, its flows as drafts, opportunities with their account, people
//! and qualification, the opportunity sheet and the account's tree.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde_json::{json, Value};
use serial_test::serial;

struct Env {
    alice: String,
    bob: String,
    db: DatabaseConnection,
}

async fn with_app<F, Fut>(f: F)
where
    F: FnOnce(axum_test::TestServer, Env) -> Fut,
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
            f(
                server,
                Env {
                    alice,
                    bob,
                    db: ctx.db.clone(),
                },
            )
            .await;
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
    let req = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "PUT" => server.put(path),
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
    props: Value,
) -> String {
    let (st, o) = call(
        server,
        "POST",
        "/api/v1/ontology/objects",
        token,
        org,
        Some(json!({"type_key": type_key, "title": title, "properties": props})),
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
    from: &str,
    to: &str,
    attributes: Value,
) -> (u16, Value) {
    call(
        server,
        "POST",
        "/api/v1/ontology/links",
        token,
        org,
        Some(json!({"link_type": link_type, "source_id": from, "target_id": to, "attributes": attributes})),
    )
    .await
}

#[tokio::test]
#[serial]
async fn an_opportunity_lives_in_the_account_graph() {
    with_app(|server, env| async move {
        let _ = &env.bob;
        let org = me(&server, &env.alice).await["orgs"][0]["id"].as_i64().unwrap();
        let (st, e) = call(&server, "POST", "/api/v1/ontology/packs/crm/install", &env.alice, org, None).await;
        assert_eq!(st, 200, "{e}");

        // What it builds on is installed first; its flows wait as drafts.
        let count = |sql: String| {
            let db = env.db.clone();
            async move {
                db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql))
                    .await
                    .unwrap()
                    .unwrap()
                    .try_get::<i64>("", "n")
                    .unwrap()
            }
        };
        assert_eq!(
            count(format!("SELECT count(*)::bigint AS n FROM packs WHERE org_id = {org} AND key IN ('prospecting_b2b', 'crm')")).await,
            2
        );
        assert_eq!(
            count(format!("SELECT count(*)::bigint AS n FROM flows WHERE org_id = {org} AND name LIKE 'CRM · %' AND status = 'draft'")).await,
            15
        );

        let acme = object(&server, &env.alice, org, "organization", "Acme", json!({"account_status": "prospect"})).await;
        let ann = object(&server, &env.alice, org, "person", "Ann Smith", json!({})).await;
        let budget = object(&server, &env.alice, org, "qualification_criterion", "Budget confirmed", json!({"stage": "qualification", "active": true})).await;
        let deal = object(&server, &env.alice, org, "opportunity", "Acme 2027", json!({
            "stage": "qualification", "source": "trade_show", "source_detail": "Global Industrie",
            "amount": {"amount": 48000, "currency": "EUR"}
        })).await;
        // A stage outside the list is refused.
        let (st, _) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org,
            Some(json!({"type_key": "opportunity", "title": "X", "properties": {"stage": "signed"}}))).await;
        assert_eq!(st, 400);

        let (st, e) = link(&server, &env.alice, org, "opportunity_with", &deal, &acme, json!({})).await;
        assert_eq!(st, 201, "{e}");
        let beta = object(&server, &env.alice, org, "organization", "Beta", json!({})).await;
        let (st, _) = link(&server, &env.alice, org, "opportunity_with", &deal, &beta, json!({})).await;
        assert_eq!(st, 409, "one account per opportunity");
        let (st, e) = link(&server, &env.alice, org, "involved_in", &ann, &deal, json!({"role": "decision_maker"})).await;
        assert_eq!(st, 201, "{e}");
        let (st, e) = link(&server, &env.alice, org, "qualified_on", &deal, &budget, json!({"status": "partial", "note": "being decided"})).await;
        assert_eq!(st, 201, "{e}");

        let (st, h) = call(&server, "GET", &format!("/api/v1/ontology/objects/{acme}/hierarchy?key=opportunities"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{h}");
        let titles: Vec<&str> = h["nodes"].as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["Acme", "Acme 2027"]);
        assert!(h["counts"].to_string().contains("\"qualification\""), "{h}");

        let (st, page) = call(&server, "POST", &format!("/api/v1/ontology/objects/{deal}/pages"), &env.alice, org,
            Some(json!({"template": "opportunity_sheet"}))).await;
        assert_eq!(st, 201, "{page}");
        let snap = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "SELECT snapshot FROM docs WHERE id = '{}'", page["id"].as_str().unwrap()))).await.unwrap().unwrap()
            .try_get::<Vec<u8>>("", "snapshot").unwrap();
        let text = String::from_utf8_lossy(&snap);
        for want in ["Acme 2027", "Acme", "Ann Smith", "Budget confirmed", "Global Industrie"] {
            assert!(text.contains(want), "{want} missing");
        }

        // A lead (D514): its status from a closed list, one account.
        let lead = object(&server, &env.alice, org, "lead", "Gamma · Lee", json!({
            "status": "new", "source": "trade_show", "company": "Gamma", "email": "lee@gamma.test"
        })).await;
        let (st, _) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org,
            Some(json!({"type_key": "lead", "title": "X", "properties": {"status": "won"}}))).await;
        assert_eq!(st, 400);
        let (st, e) = link(&server, &env.alice, org, "lead_of", &lead, &acme, json!({})).await;
        assert_eq!(st, 201, "{e}");
        let (st, _) = link(&server, &env.alice, org, "lead_of", &lead, &beta, json!({})).await;
        assert_eq!(st, 409, "one account per lead");
        let (st, e) = link(&server, &env.alice, org, "comes_from", &deal, &lead, json!({})).await;
        assert_eq!(st, 201, "{e}");
        // Convert needs its flow deployed.
        let (st, _) = call(&server, "POST", &format!("/api/v1/ontology/objects/{lead}/actions/convert"), &env.alice, org,
            Some(json!({"params": {"organization": acme}}))).await;
        assert!(matches!(st, 409 | 503), "no deployed flow, refused: {st}");

        // Quotes and invoices (D516): numbers are unique, one opportunity per quote.
        let quote = object(&server, &env.alice, org, "quote", "Acme 2027 — phase 1", json!({
            "ref": "D-2026-012", "state": "sent", "amount": {"amount": 40000, "currency": "EUR"},
            "issued_on": "2026-10-20", "valid_until": "2026-11-20", "content": "Audit and pilot"
        })).await;
        let (st, _) = call(&server, "POST", "/api/v1/ontology/objects", &env.alice, org,
            Some(json!({"type_key": "quote", "title": "Y", "properties": {"ref": "D-2026-012", "state": "draft"}}))).await;
        assert_eq!(st, 400, "a quote number is unique");
        let (st, e) = link(&server, &env.alice, org, "quote_for", &quote, &deal, json!({})).await;
        assert_eq!(st, 201, "{e}");
        let invoice = object(&server, &env.alice, org, "invoice", "F-2026-031", json!({
            "ref": "F-2026-031", "state": "issued", "amount": {"amount": 48000, "currency": "EUR"}, "due_on": "2026-11-30"
        })).await;
        for (lt, to) in [("invoice_for", &deal), ("invoice_for", &acme), ("invoices_quote", &quote)] {
            let (st, e) = link(&server, &env.alice, org, lt, &invoice, to, json!({})).await;
            assert_eq!(st, 201, "{lt}: {e}");
        }
        let (st, page) = call(&server, "POST", &format!("/api/v1/ontology/objects/{quote}/pages"), &env.alice, org,
            Some(json!({"template": "quote_sheet"}))).await;
        assert_eq!(st, 201, "{page}");
        let snap = env.db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, format!(
            "SELECT snapshot FROM docs WHERE id = '{}'", page["id"].as_str().unwrap()))).await.unwrap().unwrap()
            .try_get::<Vec<u8>>("", "snapshot").unwrap();
        let text = String::from_utf8_lossy(&snap);
        for want in ["D-2026-012", "Acme 2027", "Audit and pilot", "2026-11-20"] {
            assert!(text.contains(want), "{want} missing from the quote");
        }
    })
    .await;
}

/// The pack ships its application, ready to use (app-builder.md D513).
#[tokio::test]
#[serial]
async fn the_pack_ships_its_screens() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, e) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/crm/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{e}");
        let (_, apps) = call(&server, "GET", "/api/v1/apps", &env.alice, org, None).await;
        let app = apps["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["key"] == "sales_follow_up")
            .unwrap();
        assert_eq!(app["names"]["fr"], "Suivi commercial");
        let id = app["id"].as_str().unwrap();
        let screens: Vec<&str> = app["screens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            screens,
            [
                "pipeline",
                "figures",
                "results",
                "forecast",
                "stages",
                "prospecting",
                "shows",
                "opportunity",
                "leads",
                "activities",
                "billing"
            ]
        );
        for key in screens {
            let (st, s) = call(
                &server,
                "GET",
                &format!("/api/v1/apps/{id}/screens/{key}"),
                &env.alice,
                org,
                None,
            )
            .await;
            assert_eq!(st, 200, "{s}");
        }
        object(
            &server,
            &env.alice,
            org,
            "opportunity",
            "Acme 2027",
            json!({"stage": "proposal"}),
        )
        .await;
        for (set, count) in [
            ("open_opportunities", 1),
            ("all_opportunities", 1),
            ("won_opportunities", 0),
            ("new_leads", 0),
            // Filled by a click on a bar of the Prospecting screen (D726).
            ("leads_with_status?status=new", 0),
        ] {
            let (st, r) = call(
                &server,
                "GET",
                &format!("/api/v1/apps/{id}/object-sets/{set}"),
                &env.alice,
                org,
                None,
            )
            .await;
            assert_eq!((st, r["count"].clone()), (200, json!(count)), "{set}: {r}");
        }
        // Every number the screens show is a request the server accepts, and
        // the bars of the pipeline count what is there.
        let (mut sources, mut funnels) = (0, 0);
        for key in ["pipeline", "figures", "results", "forecast", "prospecting"] {
            let (_, screen) = call(&server, "GET", &format!("/api/v1/apps/{id}/screens/{key}"), &env.alice, org, None).await;
            for w in screen["layout"]["widgets"].as_array().unwrap() {
                let Some(o) = w["source"][0].get("objects") else { continue };
                if let Some(t) = o.get("transitions") {
                    // 503 without O2: the property must still have a funnel (400 otherwise).
                    let (st, r) = call(&server, "POST", "/api/v1/ontology/transitions/query", &env.alice, org, Some(t.clone())).await;
                    assert!(st == 200 || st == 503, "{key}/{}: {r}", w["id"]);
                    if st == 200 {
                        let keys: Vec<_> = r["groups"].as_array().unwrap().iter().map(|g| g["key"][0].clone()).collect();
                        assert_eq!(keys, ["new", "contacted", "qualified", "converted"], "the funnel's order: {r}");
                    }
                    funnels += 1;
                    continue;
                }
                sources += 1;
                let set = o["object_set"].as_str().unwrap();
                let body = json!({"group_by": o.get("group_by").cloned().unwrap_or(json!([])), "metrics": o["metrics"],
                                  "params": {"from": "2020-01-01", "to": "2030-01-01"}});
                let (st, r) = call(&server, "POST", &format!("/api/v1/apps/{id}/object-sets/{set}/aggregate/query"), &env.alice, org, Some(body)).await;
                assert_eq!(st, 200, "{key}/{}: {r}", w["id"]);
                if w["id"] == "b1" {
                    assert_eq!(r["groups"][0]["key"][0], "proposal", "{r}");
                    assert_eq!(r["groups"][0]["values"]["n"]["value"], 1.0, "{r}");
                }
            }
        }
        assert_eq!((sources, funnels), (23, 2));
        // Not a dashboard of the org.
        let (_, list) = call(&server, "GET", "/api/v1/dashboards", &env.alice, org, None).await;
        assert_eq!(list["count"], 0, "{list}");
    })
    .await;
}

/// The figures of the Results and Prospecting screens and the stalled list
/// (sales-dashboards.md D726): open opportunities stay out of the win rate.
#[tokio::test]
#[serial]
async fn the_results_screens_count_what_is_there() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, e) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/crm/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{e}");
        let (_, apps) = call(&server, "GET", "/api/v1/apps", &env.alice, org, None).await;
        let id = apps["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["key"] == "sales_follow_up")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let today = chrono::Utc::now();
        let days_ago = |n: i64| (today - chrono::Duration::days(n)).to_rfc3339();
        for (title, props) in [
            ("won 1", json!({"stage": "won", "source": "inbound"})),
            ("won 2", json!({"stage": "won", "source": "referral"})),
            ("lost", json!({"stage": "lost", "source": "inbound"})),
            (
                "stalled",
                json!({"stage": "proposal", "source": "inbound", "last_activity": days_ago(31)}),
            ),
            (
                "alive",
                json!({"stage": "proposal", "last_activity": days_ago(29)}),
            ),
            ("new", json!({"stage": "discovery"})),
        ] {
            object(&server, &env.alice, org, "opportunity", title, props).await;
        }
        for (title, status, source) in [
            ("a", "converted", "inbound"),
            ("b", "new", "inbound"),
            ("c", "new", "referral"),
        ] {
            object(
                &server,
                &env.alice,
                org,
                "lead",
                title,
                json!({"status": status, "source": source}),
            )
            .await;
        }
        let source_of = |screen: Value, widget: &str| {
            screen["layout"]["widgets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|w| w["id"] == widget)
                .unwrap()["source"][0]["objects"]
                .clone()
        };
        let aggregate = |key: &'static str, widget: &'static str| {
            let (server, token, id) = (&server, env.alice.clone(), id.clone());
            async move {
                let (st, screen) = call(
                    server,
                    "GET",
                    &format!("/api/v1/apps/{id}/screens/{key}"),
                    &token,
                    org,
                    None,
                )
                .await;
                assert_eq!(st, 200, "{screen}");
                let o = source_of(screen, widget);
                let set = o["object_set"].as_str().unwrap().to_string();
                // The default period of the Results screen, around today.
                let today = chrono::Utc::now().date_naive();
                let params = match o.get("params") {
                    Some(_) => json!({"from": (today - chrono::Days::new(365)).to_string(),
                                      "to": today.to_string()}),
                    None => json!({}),
                };
                let body = json!({"group_by": o.get("group_by").cloned().unwrap_or(json!([])),
                                  "metrics": o["metrics"], "params": params});
                let (st, r) = call(
                    server,
                    "POST",
                    &format!("/api/v1/apps/{id}/object-sets/{set}/aggregate/query"),
                    &token,
                    org,
                    Some(body),
                )
                .await;
                assert_eq!(st, 200, "{r}");
                r
            }
        };
        let close = |v: &Value, expected: f64| {
            let got = v.as_f64().unwrap_or_else(|| panic!("no number: {v}"));
            assert!((got - expected).abs() < 0.01, "{got} != {expected}");
        };
        let r = aggregate("results", "r1").await;
        close(&r["groups"][0]["values"]["rate"]["value"], 200.0 / 3.0);
        let r = aggregate("results", "r4").await;
        let rate_of = |r: &Value, key: &str| {
            r["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["key"][0] == key)
                .unwrap_or_else(|| panic!("{key}: {r}"))["values"]["rate"]["value"]
                .clone()
        };
        close(&rate_of(&r, "inbound"), 50.0);
        close(&rate_of(&r, "referral"), 100.0);
        let r = aggregate("prospecting", "p3").await;
        close(&rate_of(&r, "inbound"), 50.0);
        close(&rate_of(&r, "referral"), 0.0);

        let titles = |set: &'static str| {
            let (server, token, id) = (&server, env.alice.clone(), id.clone());
            async move {
                let (st, r) = call(
                    server,
                    "GET",
                    &format!("/api/v1/apps/{id}/object-sets/{set}"),
                    &token,
                    org,
                    None,
                )
                .await;
                assert_eq!(st, 200, "{r}");
                r["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| o["object"]["title"].as_str().unwrap().to_string())
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(titles("stalled_opportunities").await, ["stalled"]);
        assert_eq!(titles("opportunities_without_activity").await, ["new"]);
        assert_eq!(titles("lost_opportunities").await, ["lost"]);

        // The dates are the server's, and a property without a history is refused.
        let moved = object(
            &server,
            &env.alice,
            org,
            "opportunity",
            "moved",
            json!({"stage": "discovery", "closed_on": "2000-01-01"}),
        )
        .await;
        let read = |id: String| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                let (st, o) = call(
                    server,
                    "GET",
                    &format!("/api/v1/ontology/objects/{id}"),
                    &token,
                    org,
                    None,
                )
                .await;
                assert_eq!(st, 200, "{o}");
                o
            }
        };
        let o = read(moved.clone()).await;
        let day = today.format("%Y-%m-%d").to_string();
        assert_eq!(o["properties"]["stage_entered_on"], day.as_str(), "{o}");
        assert!(o["properties"].get("closed_on").is_none(), "{o}");
        let (st, u) = call(
            &server,
            "PUT",
            &format!("/api/v1/ontology/objects/{moved}"),
            &env.alice,
            org,
            Some(json!({"type_key": "opportunity", "title": "moved",
                        "properties": {"stage": "won", "stage_entered_on": "2000-01-01"},
                        "expected_version": o["version"]})),
        )
        .await;
        assert_eq!(st, 200, "{u}");
        let o = read(moved.clone()).await;
        assert_eq!(o["properties"]["closed_on"], day.as_str(), "{o}");
        assert_eq!(o["properties"]["stage_entered_on"], day.as_str(), "{o}");

        // By closing date: 3 wins and 1 loss closed today.
        let r = aggregate("results", "r6").await;
        close(&r["groups"][0]["values"]["rate"]["value"], 75.0);
        // The reader's period decides: nothing closed in an older one.
        let (st, past) = call(
            &server,
            "POST",
            &format!("/api/v1/apps/{id}/object-sets/closed_in_period/aggregate/query"),
            &env.alice,
            org,
            Some(json!({"metrics": [{"key": "n", "op": "count"}],
                        "params": {"from": "2020-01-01", "to": "2020-12-31"}})),
        )
        .await;
        assert_eq!(st, 200, "{past}");
        close(&past["groups"][0]["values"]["n"]["value"], 0.0);

        // Weighted: an amount without a probability is left out, not assumed.
        let eur = |a: f64| json!({"amount": a, "currency": "EUR"});
        for (title, props) in [
            (
                "a",
                json!({"stage": "proposal", "amount": eur(1000.0), "probability": 50}),
            ),
            ("b", json!({"stage": "proposal", "amount": eur(9000.0)})),
        ] {
            object(&server, &env.alice, org, "opportunity", title, props).await;
        }
        let r = aggregate("forecast", "f1").await;
        close(
            &r["groups"][0]["values"]["total"]["by_currency"]["EUR"],
            500.0,
        );

        let (st, r) = call(
            &server,
            "POST",
            "/api/v1/ontology/transitions/query",
            &env.alice,
            org,
            Some(json!({"type_key": "opportunity", "property": "amount"})),
        )
        .await;
        assert_eq!(st, 400, "{r}");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn a_stale_lead_is_proposed_for_erasure() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, _) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/crm/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200);
        let lead = object(
            &server,
            &env.alice,
            org,
            "lead",
            "Gamma · Lee",
            json!({"status": "new", "email": "lee@gamma.test"}),
        )
        .await;
        env.db
            .execute_unprepared(&format!(
                "UPDATE objects SET updated_at = now() - interval '40 months' WHERE id = '{lead}'"
            ))
            .await
            .unwrap();
        assert_eq!(
            pnex_backend::services::profiles::propose_erasures_in(&env.db, org, 36)
                .await
                .unwrap(),
            1
        );
        let (_, l) = call(&server, "GET", "/api/v1/proposals", &env.alice, org, None).await;
        assert_eq!(l["results"][0]["refs"][0]["id"], lead.as_str(), "{l}");
        let (st, p) = call(
            &server,
            "POST",
            &format!(
                "/api/v1/proposals/{}/decision",
                l["results"][0]["id"].as_str().unwrap()
            ),
            &env.alice,
            org,
            Some(json!({"decision": "accept"})),
        )
        .await;
        assert_eq!(st, 200, "{p}");
        let (_, o) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{lead}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert!(o["erased_at"].is_string(), "{o}");
        assert!(o["properties"].get("email").is_none(), "{o}");

        // A contact from a list names a person too (D610).
        let contact = object(
            &server,
            &env.alice,
            org,
            "list_contact",
            "Lee",
            json!({"ref": "fair|lee", "email": "lee2@gamma.test"}),
        )
        .await;
        env.db
            .execute_unprepared(&format!(
                "UPDATE objects SET updated_at = now() - interval '40 months' WHERE id = '{contact}'"
            ))
            .await
            .unwrap();
        assert_eq!(
            pnex_backend::services::profiles::propose_erasures_in(&env.db, org, 36)
                .await
                .unwrap(),
            1
        );
    })
    .await;
}

/// Every live object of the org holding `needle`, as `type_key:id`: what an
/// erasure left behind, read from the table rather than through the API mask.
async fn objects_holding(db: &DatabaseConnection, org: i64, needle: &str) -> Vec<String> {
    use sea_orm::{DatabaseBackend, Statement};
    db.query_all_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT type_key || ':' || id::text AS at FROM objects
         WHERE org_id = $1 AND (title || ' ' || properties::text) ILIKE '%' || $2 || '%'",
        vec![org.into(), needle.into()],
    ))
    .await
    .unwrap()
    .iter()
    .map(|r| r.try_get::<String>("", "at").unwrap())
    .collect()
}

/// A lead from a mailbox carries the address in its key (crm.md D591), and
/// the follow-up of a person carries the text of their mails (D450): an
/// erasure must take both.
#[tokio::test]
#[serial]
async fn an_erasure_leaves_no_address_and_no_mail_text() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, _) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/crm/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200);

        let lead = object(
            &server,
            &env.alice,
            org,
            "lead",
            "lee@gamma.test",
            json!({"ref": "mail:lee@gamma.test", "status": "new", "email": "lee@gamma.test"}),
        )
        .await;
        // A second holder of the address: proposed, not erased at once.
        let other = object(
            &server,
            &env.alice,
            org,
            "list_contact",
            "Lee from a list",
            json!({"ref": "fair|lee", "email": "LEE@gamma.test"}),
        )
        .await;
        let (st, o) = call(
            &server,
            "POST",
            &format!("/api/v1/ontology/objects/{lead}/erase"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{o}");
        assert_eq!(
            objects_holding(&env.db, org, "lee@gamma.test").await,
            vec![format!("list_contact:{other}")],
            "compared without case, the other holder stays until a person decides"
        );
        let (_, l) = call(&server, "GET", "/api/v1/proposals", &env.alice, org, None).await;
        assert_eq!(l["results"][0]["kind"], "object_erase", "{l}");
        assert_eq!(l["results"][0]["refs"][0]["id"], other.as_str(), "{l}");
        assert!(
            !l.to_string().contains("gamma.test"),
            "no address in it: {l}"
        );
        // The list keeps a hash of the address, never the address.
        let key = pnex_backend::services::ontology::opposition::contact_key(org, "Lee@Gamma.test");
        use sea_orm::{DatabaseBackend, Statement};
        let rows = env
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT fingerprint FROM prospecting_oppositions WHERE org_id = $1",
                vec![org.into()],
            ))
            .await
            .unwrap();
        let list: Vec<String> = rows
            .iter()
            .map(|r| r.try_get::<String>("", "fingerprint").unwrap())
            .collect();
        assert!(list.contains(&key), "{list:?}");
        assert!(!list.iter().any(|k| k.contains("gamma.test")), "{list:?}");

        let ann = object(&server, &env.alice, org, "person", "Ann Vale", json!({})).await;
        let address = object(
            &server,
            &env.alice,
            org,
            "identifier",
            "ann@vale.test",
            json!({"kind": "email", "value": "ann@vale.test"}),
        )
        .await;
        let (st, l) = link(
            &server,
            &env.alice,
            org,
            "uses_identifier",
            &ann,
            &address,
            json!({}),
        )
        .await;
        assert_eq!(st, 201, "{l}");
        let mail = object(
            &server,
            &env.alice,
            org,
            "activity",
            "Mail from Ann",
            json!({"ref": "mail:one@vale.test", "kind": "mail", "state": "done",
                   "direction": "inbound", "content": "my private phrase",
                   "context": "my private phrase", "draft": "my private phrase"}),
        )
        .await;
        let (st, l) = link(
            &server,
            &env.alice,
            org,
            "activity_on",
            &mail,
            &ann,
            json!({}),
        )
        .await;
        assert_eq!(st, 201, "{l}");
        let (st, o) = call(
            &server,
            "POST",
            &format!("/api/v1/ontology/objects/{ann}/erase"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{o}");
        for needle in ["Ann Vale", "ann@vale.test", "my private phrase"] {
            assert_eq!(
                objects_holding(&env.db, org, needle).await,
                Vec::<String>::new(),
                "{needle}"
            );
        }
    })
    .await;
}

/// A meeting is shared: erasing a participant proposes it, and accepting
/// leaves its date but none of its text (crm.md D610).
#[tokio::test]
#[serial]
async fn erasing_a_participant_proposes_the_meeting() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, _) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/crm/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200);
        let zoe = object(&server, &env.alice, org, "person", "Zoe Hart", json!({})).await;
        let props = json!({"purpose": "Talk with Zoe Hart", "next_step": "Call Zoe Hart",
                           "place": "Zoe Hart's home", "date": "2026-11-20T10:00:00Z",
                           "invitation_uid": "uid-1"});
        let meeting = object(
            &server,
            &env.alice,
            org,
            "meeting",
            "meeting · Talk with Zoe Hart",
            props.clone(),
        )
        .await;
        let (st, l) = link(
            &server,
            &env.alice,
            org,
            "takes_part_in",
            &zoe,
            &meeting,
            json!({}),
        )
        .await;
        assert_eq!(st, 201, "{l}");
        let (st, o) = call(
            &server,
            "POST",
            &format!("/api/v1/ontology/objects/{zoe}/erase"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{o}");
        assert_eq!(
            objects_holding(&env.db, org, "Zoe Hart").await,
            vec![format!("meeting:{meeting}")],
            "the meeting waits for a person"
        );
        let (_, l) = call(&server, "GET", "/api/v1/proposals", &env.alice, org, None).await;
        let proposal = &l["results"][0];
        assert_eq!(proposal["kind"], "object_erase", "{l}");
        assert_eq!(proposal["refs"][0]["id"], meeting.as_str(), "{l}");
        // Its reference shows the meeting's live title; what it stores has no text.
        for stored in ["values", "evidence"] {
            assert!(
                !proposal[stored].to_string().contains("Zoe"),
                "no text in {stored}: {l}"
            );
        }
        let (st, p) = call(
            &server,
            "POST",
            &format!(
                "/api/v1/proposals/{}/decision",
                proposal["id"].as_str().unwrap()
            ),
            &env.alice,
            org,
            Some(json!({"decision": "accept"})),
        )
        .await;
        assert_eq!(st, 200, "{p}");
        assert!(!p.to_string().contains("Zoe"), "{p}");
        assert_eq!(
            objects_holding(&env.db, org, "Zoe Hart").await,
            Vec::<String>::new()
        );
        let (st, m) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{meeting}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{m}");
        assert_eq!(m["properties"]["invitation_uid"], "uid-1", "{m}");
        assert!(m["properties"]["date"].is_string(), "{m}");
        // The same invitation cannot bring it back.
        let (st, again) = call(
            &server,
            "POST",
            "/api/v1/ontology/objects",
            &env.alice,
            org,
            Some(json!({"type_key": "meeting", "title": "meeting · again", "properties": props})),
        )
        .await;
        assert_ne!(st, 201, "{again}");
    })
    .await;
}

/// A file brought back after an erasure does not recreate the contact, and
/// the other rows of the file still arrive (crm.md D610).
#[tokio::test]
#[serial]
async fn an_import_does_not_bring_back_an_erased_address() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, _) = call(
            &server,
            "POST",
            "/api/v1/ontology/packs/crm/install",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200);
        let (st, l) = call(
            &server,
            "GET",
            "/api/v1/ontology/imports",
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{l}");
        let imports = l["results"].as_array().unwrap();
        let import = imports
            .iter()
            .find(|i| i["name"] == "Trade show or association list (file)")
            .unwrap_or_else(|| panic!("{l}"));
        let id = import["id"].as_str().unwrap().to_string();

        let lee = object(
            &server,
            &env.alice,
            org,
            "lead",
            "Lee",
            json!({"status": "new", "email": "lee@gamma.test"}),
        )
        .await;
        let (st, o) = call(
            &server,
            "POST",
            &format!("/api/v1/ontology/objects/{lee}/erase"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{o}");

        let csv = "list,name,company,job_title,city,postal_code,email\n\
                   Fair,Lee Roe,Gamma,,,,lee@gamma.test\n\
                   Fair,Ann Vale,Vale,,,,ann@vale.test\n";
        let res = server
            .post(&format!(
                "/api/v1/ontology/imports/{id}/dry-run?filename=fair.csv"
            ))
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .add_header("X-Org-Id", org.to_string())
            .add_header("Content-Type", "application/octet-stream")
            .bytes(csv.as_bytes().to_vec().into())
            .await;
        assert_eq!(res.status_code().as_u16(), 202, "{}", res.text());
        let run = res.json::<Value>()["id"].as_str().unwrap().to_string();
        let settled = |run: String, id: String| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                for _ in 0..100 {
                    let (st, r) = call(
                        server,
                        "GET",
                        &format!("/api/v1/ontology/imports/{id}/runs/{run}"),
                        &token,
                        org,
                        None,
                    )
                    .await;
                    assert_eq!(st, 200, "{r}");
                    if r["run"]["state"] != "running" {
                        return r;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                panic!("run never settled");
            }
        };
        let p = settled(run.clone(), id.clone()).await;
        assert_eq!(p["run"]["state"], "preview", "{p}");
        // Shown left out from the preview on, so both count the same.
        let warned: Vec<_> = p["diff"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|w| w["token"] == "opposed_contact")
            .map(|w| w["row"].clone())
            .collect();
        assert!(!warned.is_empty(), "{p}");
        assert!(warned.iter().all(|r| r == 2), "only Lee's row: {p}");
        let before = objects_holding(&env.db, org, "").await.len();
        let (st, r) = call(
            &server,
            "POST",
            &format!("/api/v1/ontology/imports/{id}/apply"),
            &env.alice,
            org,
            Some(json!({"run_id": run, "expected_version": import["version"]})),
        )
        .await;
        assert_eq!(st, 202, "{r}");
        let done = settled(run.clone(), id.clone()).await;
        assert_eq!(done["run"]["state"], "applied", "{done}");
        assert_eq!(done["run"]["counts"], p["run"]["counts"], "{done}");
        let after = objects_holding(&env.db, org, "").await.len();
        assert_eq!(
            done["run"]["counts"]["created"].as_u64().unwrap() as usize,
            after - before,
            "the count is what was written: {done}"
        );
        assert_eq!(
            objects_holding(&env.db, org, "lee@gamma.test").await,
            Vec::<String>::new(),
            "the erased address stays out"
        );
        assert_eq!(
            objects_holding(&env.db, org, "ann@vale.test").await.len(),
            1
        );
    })
    .await;
}

/// A network profiles export makes one person per profile URL: run twice it
/// creates nothing more, and an erased profile does not come back (D610).
#[tokio::test]
#[serial]
async fn a_network_export_makes_one_person_per_profile() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, e) = call(&server, "POST", "/api/v1/ontology/packs/crm/install", &env.alice, org, None).await;
        assert_eq!(st, 200, "{e}");
        let (st, l) = call(&server, "GET", "/api/v1/ontology/imports", &env.alice, org, None).await;
        assert_eq!(st, 200, "{l}");
        let import = l["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "Network profiles export (file)")
            .unwrap_or_else(|| panic!("{l}"))
            .clone();
        let id = import["id"].as_str().unwrap().to_string();

        let gone = "https://www.linkedin.com/in/lee-roe";
        let lee = object(&server, &env.alice, org, "person", "Lee Roe", json!({"profile_url": gone})).await;
        let (st, o) = call(&server, "POST", &format!("/api/v1/ontology/objects/{lee}/erase"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{o}");

        let csv = "First Name,Last Name,Job Title,LinkedIn Profile URL,Company Name,Location\n\
                   Lee,Roe,CEO,https://www.linkedin.com/in/lee-roe,Gamma,Paris\n\
                   Ann,Vale,Plant manager, https://www.linkedin.com/in/ann-vale ,Mecaworks,\"Amboise, Centre-Val de Loire, France\"\n";
        let version = import["version"].clone();
        let mut counts = Vec::new();
        for _ in 0..2 {
            let res = server
                .post(&format!("/api/v1/ontology/imports/{id}/dry-run?filename=export.csv"))
                .add_header("Authorization", format!("Bearer {}", env.alice))
                .add_header("X-Org-Id", org.to_string())
                .add_header("Content-Type", "application/octet-stream")
                .bytes(csv.as_bytes().to_vec().into())
                .await;
            assert_eq!(res.status_code().as_u16(), 202, "{}", res.text());
            let run = res.json::<Value>()["id"].as_str().unwrap().to_string();
            let mut p = Value::Null;
            for _ in 0..100 {
                let (st, r) = call(&server, "GET", &format!("/api/v1/ontology/imports/{id}/runs/{run}"), &env.alice, org, None).await;
                assert_eq!(st, 200, "{r}");
                if r["run"]["state"] != "running" {
                    p = r;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            assert_eq!(p["run"]["state"], "preview", "{p}");
            let (st, r) = call(
                &server,
                "POST",
                &format!("/api/v1/ontology/imports/{id}/apply"),
                &env.alice,
                org,
                Some(json!({"run_id": run, "expected_version": version})),
            )
            .await;
            assert_eq!(st, 202, "{r}");
            for _ in 0..100 {
                let (_, r) = call(&server, "GET", &format!("/api/v1/ontology/imports/{id}/runs/{run}"), &env.alice, org, None).await;
                if r["run"]["state"] != "running" {
                    assert_eq!(r["run"]["state"], "applied", "{r}");
                    counts.push(r["run"]["counts"]["created"].as_u64().unwrap_or(0));
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
        assert_eq!(counts, vec![1, 0], "Ann once, Lee never");
        assert!(objects_holding(&env.db, org, "lee-roe").await.is_empty(), "the erased profile stays out");
        let ann = objects_holding(&env.db, org, "https://www.linkedin.com/in/ann-vale").await;
        assert_eq!(ann.len(), 1, "{ann:?}");
        let ann_id = ann[0].strip_prefix("person:").unwrap_or_else(|| panic!("{ann:?}"));
        let (st, a) = call(&server, "GET", &format!("/api/v1/ontology/objects/{ann_id}"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{a}");
        let props = &a["properties"];
        assert_eq!(
            (a["title"].clone(), props["profile_position"].clone(), props["profile_company"].clone(), props["profile_city"].clone()),
            (json!("Ann Vale"), json!("Plant manager"), json!("Mecaworks"), json!("Amboise")),
            "{a}"
        );
    })
    .await;
}

/// « elodie » finds « Élodie » (query and global search), and an objection
/// recorded on « Élodie » blocks « ELODIE » at the same account (crm.md D447).
#[tokio::test]
#[serial]
async fn names_match_without_accents_or_case() {
    with_app(|server, env| async move {
        let org = me(&server, &env.alice).await["orgs"][0]["id"].as_i64().unwrap();
        let (st, e) = call(&server, "POST", "/api/v1/ontology/packs/crm/install", &env.alice, org, None).await;
        assert_eq!(st, 200, "{e}");
        let acme = object(&server, &env.alice, org, "organization", "Acme", json!({})).await;
        let elodie = object(&server, &env.alice, org, "person", "Élodie Martin", json!({"notes": "Rencontrée à Lyon"})).await;
        object(&server, &env.alice, org, "person", "Bob Durand", json!({})).await;

        let (_, q) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org,
            Some(json!({"type_key": "person", "text": "ELODIE"}))).await;
        assert_eq!(q["count"], 1, "{q}");
        assert_eq!(q["rows"][0]["object"]["title"], "Élodie Martin");
        let (_, q) = call(&server, "POST", "/api/v1/ontology/query", &env.alice, org,
            Some(json!({"type_key": "person", "filters": [{"key": "notes", "op": "contains", "value": "rencontree a lyon"}]}))).await;
        assert_eq!(q["count"], 1, "{q}");

        let (st, s) = call(&server, "GET", "/api/v1/search?q=elodie", &env.alice, org, None).await;
        assert_eq!(st, 200, "{s}");
        assert!(s.to_string().contains("Élodie Martin"), "{s}");

        let (st, _) = link(&server, &env.alice, org, "employs", &acme, &elodie, json!({})).await;
        assert_eq!(st, 201);
        let (st, o) = call(&server, "POST", &format!("/api/v1/extension/people/{elodie}/opposition"), &env.alice, org, None).await;
        assert_eq!(st, 200, "{o}");
        let twin = object(&server, &env.alice, org, "person", "ELODIE  martin", json!({})).await;
        let (st, e) = link(&server, &env.alice, org, "employs", &acme, &twin, json!({})).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("prospecting-opposed")), "{e}");
    })
    .await;
}
