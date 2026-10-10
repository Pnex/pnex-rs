//! Time ranges (media-ingest.md D169): CRUD, idempotent upsert by
//! `external_id`, CSV / ICS import, org isolation (R1), viewer read-only
//! (R2), the flow runtime endpoint `/internal/flow/time-range` (service
//! token, foreign org refused) and the `range_upsert` deploy gate (engine
//! OFF: a deploy answers 503 only AFTER the gates).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use serde_json::{json, Value};
use serial_test::serial;

const TOKEN: &str = "tok-time-ranges";

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
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    unsafe { std::env::set_var("PNEX_FLOW_RUNTIME_TOKEN", TOKEN) };
    unsafe {
        std::env::set_var(
            "PNEX_FLOW_STATE_DIR",
            format!("/tmp/pnex-time-ranges-tests-{}", std::process::id()),
        )
    };
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
        "PATCH" => server.patch(path),
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
    let body = serde_json::from_str(&res.text()).unwrap_or(Value::Null);
    (status, body)
}

async fn upload(
    server: &axum_test::TestServer,
    path: &str,
    token: &str,
    org: i64,
    body: &str,
) -> (u16, Value) {
    let res = server
        .post(path)
        .add_header("Authorization", format!("Bearer {token}"))
        .add_header("X-Org-Id", org.to_string())
        .bytes(body.as_bytes().to_vec().into())
        .await;
    let status = res.status_code().as_u16();
    (
        status,
        serde_json::from_str(&res.text()).unwrap_or(Value::Null),
    )
}

/// A stream of the org; returns its id.
async fn stream(server: &axum_test::TestServer, token: &str, org: i64, name: &str) -> String {
    let (s, v) = call(
        server,
        "POST",
        "/api/v1/media/streams",
        token,
        org,
        Some(json!({ "name": name, "kind": "icecast",
            "url": "https://icecast.radio.example/inter.mp3" })),
    )
    .await;
    assert_eq!(s, 201, "{v}");
    v["id"].as_str().unwrap().to_string()
}

fn range(scope: &str, external: &str, label: &str) -> Value {
    json!({
        "scope_kind": "stream", "scope_id": scope,
        "external_id": external, "label": label, "category": "news",
        "planned_start": "2026-10-12T07:00:00+02:00",
        "planned_end": "2026-10-12T09:00:00+02:00",
        "source_url": "https://radio.example/grille",
        "attrs": { "host": "A" }
    })
}

fn day_query(scope: &str) -> String {
    format!(
        "/api/v1/time-ranges?scope_kind=stream&scope_id={scope}\
         &from=2026-10-12T00:00:00%2B02:00&to=2026-10-13T00:00:00%2B02:00"
    )
}

#[tokio::test]
#[serial]
async fn crud_upsert_and_field_tokens() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let s = stream(&server, &env.alice, org, "France Inter").await;

        let (st, a) = call(&server, "POST", "/api/v1/time-ranges", &env.alice, org, Some(range(&s, "7-9", "Le 7/9"))).await;
        assert_eq!(st, 201, "{a}");
        assert_eq!(a["origin"], "manual");
        assert_eq!(a["scope_kind"], "stream");
        assert_eq!(a["scope_id"], s);
        assert_eq!(a["attrs"]["host"], "A");
        assert!(a["source_ref"].as_str().unwrap().starts_with("manual:"), "{a}");
        let id = a["id"].as_str().unwrap().to_string();

        // Same external id in the same scope: updated, not duplicated.
        let mut again = range(&s, "7-9", "Le 7/9 (spécial)");
        again["actual_start"] = json!("2026-10-12T07:01:00+02:00");
        again["actual_end"] = json!("2026-10-12T09:02:00+02:00");
        let (st, b) = call(&server, "POST", "/api/v1/time-ranges", &env.alice, org, Some(again)).await;
        assert_eq!(st, 200, "{b}");
        assert_eq!(b["id"], id);
        assert_eq!(b["label"], "Le 7/9 (spécial)");
        assert!(b["actual_start"].as_str().is_some(), "{b}");

        // Listed by scope and day; another day is empty.
        let (st, page) = call(&server, "GET", &day_query(&s), &env.alice, org, None).await;
        assert_eq!(st, 200, "{page}");
        assert_eq!(page["count"], 1, "{page}");
        let (_, other) = call(
            &server,
            "GET",
            &format!("/api/v1/time-ranges?scope_kind=stream&scope_id={s}&from=2026-10-13T00:00:00Z&to=2026-10-14T00:00:00Z"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(other["count"], 0, "{other}");

        // Patch: absent keeps, empty clears.
        let path = format!("/api/v1/time-ranges/{id}");
        let (st, p) = call(&server, "PATCH", &path, &env.alice, org, Some(json!({ "category": "", "actual_start": "", "actual_end": "" }))).await;
        assert_eq!(st, 200, "{p}");
        assert_eq!(p["category"], Value::Null);
        assert_eq!(p["actual_start"], Value::Null);
        assert_eq!(p["label"], "Le 7/9 (spécial)");

        // Field tokens.
        for (body, field, token) in [
            (json!({ "planned_end": "2026-10-12T06:00:00+02:00" }), "planned_end", "invalid"),
            (json!({ "source_url": "javascript:alert(1)" }), "source_url", "invalid"),
            (json!({ "planned_start": "", "planned_end": "" }), "planned_start", "required"),
            (json!({ "attrs": [1] }), "attrs", "invalid"),
            (json!({ "label": "" }), "label", "required"),
        ] {
            let (st, e) = call(&server, "PATCH", &path, &env.alice, org, Some(body)).await;
            assert_eq!(st, 400, "{e}");
            assert_eq!(e[field], token, "{e}");
        }
        let mut no_scope = range(&s, "x", "x");
        no_scope["scope_kind"] = json!("device");
        let (st, e) = call(&server, "POST", "/api/v1/time-ranges", &env.alice, org, Some(no_scope)).await;
        assert_eq!(st, 400);
        assert_eq!(e["scope_kind"], "invalid", "{e}");

        // Org scope: scope_id is the org, set by the server.
        let (st, o) = call(&server, "POST", "/api/v1/time-ranges", &env.alice, org, Some(json!({
            "scope_kind": "org", "label": "Shift", "external_id": "s1",
            "planned_start": "2026-10-12T06:00:00Z", "planned_end": "2026-10-12T14:00:00Z"
        }))).await;
        assert_eq!(st, 201, "{o}");
        assert_eq!(o["scope_id"], org.to_string());

        let (st, _) = call(&server, "DELETE", &path, &env.alice, org, None).await;
        assert_eq!(st, 204);
        let (st, e) = call(&server, "PATCH", &path, &env.alice, org, Some(json!({ "label": "x" }))).await;
        assert_eq!(st, 404);
        assert_eq!(e["error"], "time-range-not-found");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn csv_and_ics_import_count_and_stay_idempotent() {
    with_app(|server, env| async move {
        let org = personal_org(&server, &env.alice).await;
        let s = stream(&server, &env.alice, org, "Inter").await;
        let csv_path = format!("/api/v1/time-ranges/import?format=csv&scope_kind=stream&scope_id={s}");
        let csv = "external_id;label;planned_start;planned_end;category\n\
                   a1;Le journal;2026-10-12T07:00:00+02:00;2026-10-12T07:30:00+02:00;news\n\
                   a2;\"Débat; spécial\";2026-10-12T08:00:00+02:00;2026-10-12T09:00:00+02:00;\n\
                   a3;Cassé;2026-10-12T10:00:00+02:00;2026-10-12T09:00:00+02:00;\n\
                   ;Sans id;2026-10-12T11:00:00+02:00;2026-10-12T12:00:00+02:00;\n";
        let (st, r) = upload(&server, &csv_path, &env.alice, org, csv).await;
        assert_eq!(st, 200, "{r}");
        assert_eq!((r["created"].as_i64(), r["updated"].as_i64(), r["rejected"].as_i64()), (Some(2), Some(0), Some(2)), "{r}");
        assert_eq!(r["errors"][0], json!({ "line": 4, "field": "planned_end", "token": "invalid" }));
        assert_eq!(r["errors"][1], json!({ "line": 5, "field": "external_id", "token": "required" }));
        // Same file again: updates, never duplicates.
        let (_, r) = upload(&server, &csv_path, &env.alice, org, csv).await;
        assert_eq!((r["created"].as_i64(), r["updated"].as_i64()), (Some(0), Some(2)), "{r}");

        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\n\
BEGIN:VEVENT\r\nUID:ev-1\r\nSUMMARY:Soir\r\nDTSTART:20261012T200000Z\r\nDURATION:PT1H\r\nURL:https://radio.example/nuit\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:ev-2\r\nSUMMARY:Matin\r\nDTSTART;TZID=Europe/Paris:20261012T060000\r\nDTEND;TZID=Europe/Paris:20261012T070000\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";
        let ics_path = format!("/api/v1/time-ranges/import?format=ics&scope_kind=stream&scope_id={s}");
        let (st, r) = upload(&server, &ics_path, &env.alice, org, ics).await;
        assert_eq!(st, 200, "{r}");
        assert_eq!((r["created"].as_i64(), r["rejected"].as_i64()), (Some(2), Some(0)), "{r}");

        let (_, page) = call(&server, "GET", &day_query(&s), &env.alice, org, None).await;
        assert_eq!(page["count"], 4, "{page}");
        let rows = page["results"].as_array().unwrap();
        assert!(rows.iter().all(|r| r["origin"] == "import"), "{page}");
        assert!(rows.iter().all(|r| r["source_ref"].as_str().unwrap().starts_with("import:")));
        let morning = rows.iter().find(|r| r["external_id"] == "ev-2").unwrap();
        assert_eq!(
            chrono::DateTime::parse_from_rfc3339(morning["planned_start"].as_str().unwrap()).unwrap(),
            chrono::DateTime::parse_from_rfc3339("2026-10-12T04:00:00Z").unwrap()
        );

        // Bad format, unreadable body, too large.
        let (st, e) = upload(&server, "/api/v1/time-ranges/import?format=xml&scope_kind=org", &env.alice, org, "x").await;
        assert_eq!(st, 400);
        assert_eq!(e["format"], "invalid");
        let (st, e) = upload(&server, &ics_path, &env.alice, org, "not a calendar").await;
        assert_eq!(st, 400);
        assert_eq!(e["error"], "time-range-import-invalid");
        let big = format!("label\n{}", "x\n".repeat(5001));
        let (st, e) = upload(&server, &csv_path, &env.alice, org, &big).await;
        assert_eq!(st, 413, "{e}");
        assert_eq!(e["error"], "time-range-import-too-large");
        assert_eq!(e["errors"]["args"]["max_rows"], "5000");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn org_isolation_and_viewer_read_only() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let s = stream(&server, &env.alice, org_a, "Inter").await;
        let (st, a) = call(
            &server,
            "POST",
            "/api/v1/time-ranges",
            &env.alice,
            org_a,
            Some(range(&s, "7-9", "Le 7/9")),
        )
        .await;
        assert_eq!(st, 201, "{a}");
        let path = format!("/api/v1/time-ranges/{}", a["id"].as_str().unwrap());

        // Bob, in his org: alice's stream is not a scope, her range is unknown.
        let (_, all) = call(&server, "GET", "/api/v1/time-ranges", &env.bob, org_b, None).await;
        assert_eq!(all["count"], 0, "{all}");
        let (st, e) = call(&server, "GET", &day_query(&s), &env.bob, org_b, None).await;
        assert_eq!(st, 400);
        assert_eq!(e["scope_id"], "invalid");
        let (st, e) = call(
            &server,
            "POST",
            "/api/v1/time-ranges",
            &env.bob,
            org_b,
            Some(range(&s, "x", "x")),
        )
        .await;
        assert_eq!(st, 400);
        assert_eq!(e["scope_id"], "invalid");
        let (st, e) = upload(
            &server,
            &format!("/api/v1/time-ranges/import?format=csv&scope_kind=stream&scope_id={s}"),
            &env.bob,
            org_b,
            "label,external_id\nx,1\n",
        )
        .await;
        assert_eq!(st, 400);
        assert_eq!(e["scope_id"], "invalid");
        // Bob cannot reach alice's org scope either.
        let (st, _) = call(
            &server,
            "POST",
            "/api/v1/time-ranges",
            &env.bob,
            org_b,
            Some(json!({
                "scope_kind": "org", "scope_id": org_a.to_string(), "label": "x",
                "planned_start": "2026-10-12T06:00:00Z", "planned_end": "2026-10-12T07:00:00Z"
            })),
        )
        .await;
        assert_eq!(st, 400);
        for (m, body) in [("PATCH", Some(json!({ "label": "x" }))), ("DELETE", None)] {
            let (st, e) = call(&server, m, &path, &env.bob, org_b, body).await;
            assert_eq!(st, 404, "{m}");
            assert_eq!(e["error"], "time-range-not-found");
        }

        // Viewer of alice's org: reads, never writes.
        let (st, _) = call(
            &server,
            "POST",
            &format!("/api/v1/orgs/{org_a}/members"),
            &env.alice,
            org_a,
            Some(json!({ "email": "bob@example.com", "role": "viewer" })),
        )
        .await;
        assert!(st == 200 || st == 201, "{st}");
        let (st, page) = call(&server, "GET", &day_query(&s), &env.bob, org_a, None).await;
        assert_eq!(st, 200);
        assert_eq!(page["count"], 1);
        for (m, p, body) in [
            (
                "POST",
                "/api/v1/time-ranges".to_string(),
                Some(range(&s, "v", "v")),
            ),
            ("PATCH", path.clone(), Some(json!({ "label": "v" }))),
            ("DELETE", path.clone(), None),
        ] {
            let (st, e) = call(&server, m, &p, &env.bob, org_a, body).await;
            assert_eq!(st, 403, "{m} {p}");
            assert_eq!(e["error"], "time-range-write-forbidden");
        }
        let (st, e) = upload(
            &server,
            &format!("/api/v1/time-ranges/import?format=csv&scope_kind=stream&scope_id={s}"),
            &env.bob,
            org_a,
            "label\nx\n",
        )
        .await;
        assert_eq!(st, 403);
        assert_eq!(e["error"], "time-range-write-forbidden");
    })
    .await;
}

/// Creates a flow in the org; returns its id.
async fn flow(server: &axum_test::TestServer, token: &str, org: i64, graph: Value) -> i64 {
    let (s, created) = call(
        server,
        "POST",
        "/api/v1/flows",
        token,
        org,
        Some(json!({ "name": "ranges", "graph": graph })),
    )
    .await;
    assert_eq!(s, 201, "{created}");
    created["id"].as_i64().expect("flow id")
}

#[tokio::test]
#[serial]
async fn internal_endpoint_needs_the_token_and_the_org() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let s = stream(&server, &env.alice, org_a, "Inter").await;
        let s_b = stream(&server, &env.bob, org_b, "Bob").await;
        let dbg = json!({ "nodes": [{ "id": "d", "kind": "debug" }] });
        let flow_a = flow(&server, &env.alice, org_a, dbg.clone()).await;
        let flow_b = flow(&server, &env.bob, org_b, dbg).await;
        let body = |org: i64, flow: i64, scope: &str, origin: &str| {
            json!({
                "org_id": org, "flow_id": flow, "version": 3, "node_id": "r",
                "scope_kind": "stream", "scope_id": scope, "origin": origin,
                "range": {
                    "external_id": "ann-1", "label": "Le journal",
                    "actual_start": "2026-10-12T07:00:20+02:00",
                    "actual_end": "2026-10-12T07:29:50+02:00",
                    "scope_kind": "org"
                }
            })
        };
        let post = |token: Option<&'static str>, b: Value| {
            let mut req = server.post("/internal/flow/time-range");
            if let Some(t) = token {
                req = req.add_header("x-pnex-flow-token", t);
            }
            async move {
                let res = req.json(&b).await;
                (
                    res.status_code().as_u16(),
                    serde_json::from_str::<Value>(&res.text()).unwrap_or(Value::Null),
                )
            }
        };

        assert_eq!(post(None, body(org_a, flow_a, &s, "detected")).await.0, 401);
        assert_eq!(
            post(Some("wrong"), body(org_a, flow_a, &s, "detected"))
                .await
                .0,
            401
        );
        // Foreign scope: bob's stream with alice's org, alice's stream with bob's org.
        let (st, e) = post(Some(TOKEN), body(org_a, flow_a, &s_b, "detected")).await;
        assert_eq!(st, 404, "{e}");
        assert_eq!(e["code"], "time-range-scope-unknown");
        let (st, e) = post(Some(TOKEN), body(org_b, flow_b, &s, "detected")).await;
        assert_eq!(st, 404, "{e}");
        assert_eq!(e["code"], "time-range-scope-unknown");
        // A flow of another org cannot sign the provenance.
        let (st, _) = post(Some(TOKEN), body(org_a, flow_b, &s, "detected")).await;
        assert_eq!(st, 404);
        // Origins of the UI are refused.
        let (st, _) = post(Some(TOKEN), body(org_a, flow_a, &s, "manual")).await;
        assert_eq!(st, 400);

        let (st, ack) = post(Some(TOKEN), body(org_a, flow_a, &s, "detected")).await;
        assert_eq!(st, 200, "{ack}");
        assert_eq!(ack["created"], true);
        let (st, again) = post(Some(TOKEN), body(org_a, flow_a, &s, "detected")).await;
        assert_eq!(st, 200);
        assert_eq!(again["created"], false);
        assert_eq!(again["id"], ack["id"]);

        let (_, page) = call(&server, "GET", &day_query(&s), &env.alice, org_a, None).await;
        assert_eq!(page["count"], 1, "{page}");
        let r = &page["results"][0];
        assert_eq!(r["origin"], "detected");
        assert_eq!(r["source_ref"], format!("flow:{flow_a}@3"));
        // The payload's scope is ignored: the range is the node's stream's.
        assert_eq!(r["scope_kind"], "stream");
        assert_eq!(r["scope_id"], s);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn range_upsert_deploy_needs_a_stream_of_the_org() {
    with_app(|server, env| async move {
        let org_a = personal_org(&server, &env.alice).await;
        let org_b = personal_org(&server, &env.bob).await;
        let s = stream(&server, &env.alice, org_a, "Inter").await;
        let s_b = stream(&server, &env.bob, org_b, "Bob").await;
        let graph = |scope: &str| {
            json!({ "nodes": [
                { "id": "r", "kind": "range_upsert",
                  "config": { "scope_kind": "stream", "scope_id": scope },
                  "outputs": [{ "port": 0, "targets": ["dbg"] }] },
                { "id": "dbg", "kind": "debug" }
            ]})
        };
        let deploy = |g: Value| {
            let server = &server;
            let token = env.alice.clone();
            async move {
                let id = flow(server, &token, org_a, g).await;
                call(
                    server,
                    "POST",
                    &format!("/api/v1/flows/{id}/deploy"),
                    &token,
                    org_a,
                    Some(json!({})),
                )
                .await
            }
        };
        let (st, body) = deploy(graph(&s_b)).await;
        assert_eq!(st, 400, "{body}");
        let violations = body["violations"].as_array().expect("violations");
        assert_eq!(violations.len(), 1, "{body}");
        assert_eq!(violations[0]["code"], "time-range-scope-unknown");
        assert_eq!(violations[0]["node_id"], "r");

        // Own stream: the gate lets it through (503 = engine off).
        let (st, body) = deploy(graph(&s)).await;
        assert_eq!(st, 503, "{body}");
    })
    .await;
}
