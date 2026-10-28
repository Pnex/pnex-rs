//! Proposals (proposals.md D371–D374): one inbox, a decision written
//! through the shared services, a rejected fingerprint never proposed
//! again, a newer proposal of the same target superseding the older, a
//! stale update refused, viewers reading only (R2).
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::ontology::proposals::propose;
use pnex_core::ontology::proposal::{ProposalInput, ProposalKind};
use sea_orm::DatabaseConnection;
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

fn group_link(from: &str, to: &str, nature: &str) -> ProposalInput {
    ProposalInput {
        kind: Some(ProposalKind::LinkCreate),
        type_key: "subsidiary_of".into(),
        source_id: Some(from.into()),
        link_target_id: Some(to.into()),
        values: json!({"nature": nature, "source": "wikidata.org"})
            .as_object()
            .cloned()
            .unwrap(),
        evidence: json!({"url": "https://www.wikidata.org/wiki/Q1"}),
        origin: "import:test@1".into(),
        confidence: Some(60.0),
        ..Default::default()
    }
}

#[tokio::test]
#[serial]
async fn proposals_wait_for_a_human_decision() {
    with_app(|server, env| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {}", env.alice))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let (st, _) = call(&server, "POST", "/api/v1/ontology/packs/prospecting_b2b/install", &env.alice, org, None).await;
        assert_eq!(st, 200);
        let mk = |title: &'static str| {
            let (server, token) = (&server, env.alice.clone());
            async move {
                let (st, o) = call(server, "POST", "/api/v1/ontology/objects", &token, org,
                    Some(json!({"type_key": "organization", "title": title, "properties": {}}))).await;
                assert_eq!(st, 201, "{o}");
                o["id"].as_str().unwrap().to_string()
            }
        };
        let (sub, mother, other) = (mk("Filiale").await, mk("Mère").await, mk("Autre").await);

        // Proposed once: the same fingerprint is not proposed twice.
        let id = propose(&env.db, org, &group_link(&sub, &mother, "wikidata")).await.unwrap().expect("new");
        assert!(propose(&env.db, org, &group_link(&sub, &mother, "wikidata")).await.unwrap().is_none());
        let (st, l) = call(&server, "GET", "/api/v1/proposals", &env.alice, org, None).await;
        assert_eq!((st, l["count"].clone()), (200, json!(1)), "{l}");
        let titles: Vec<&str> = l["results"][0]["refs"].as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap()).collect();
        assert_eq!(titles.len(), 2);
        assert!(titles.contains(&"Mère"), "{l}");
        let (_, by_object) = call(&server, "GET", &format!("/api/v1/proposals?object_id={mother}"), &env.alice, org, None).await;
        assert_eq!(by_object["count"], 1);
        // Nothing written before the decision.
        let (_, links) = call(&server, "GET", &format!("/api/v1/ontology/objects/{sub}/links"), &env.alice, org, None).await;
        assert!(!links.to_string().contains("subsidiary_of"), "{links}");

        // A viewer reads, never decides (R2).
        let (st, _) = call(&server, "GET", "/api/v1/user-info", &env.bob, org, None).await;
        assert_eq!(st, 200);
        let (st, _) = call(&server, "POST", &format!("/api/v1/orgs/{org}/members"), &env.alice, org,
            Some(json!({"email": "bob@example.com", "role": "viewer"}))).await;
        assert!(st == 200 || st == 201);
        let (st, _) = call(&server, "GET", "/api/v1/proposals", &env.bob, org, None).await;
        assert_eq!(st, 200);
        let decision = |d: &str| json!({"decision": d});
        let (st, _) = call(&server, "POST", &format!("/api/v1/proposals/{id}/decision"), &env.bob, org, Some(decision("accept"))).await;
        assert_eq!(st, 403);

        // Accepted: written through the link service, provenance proposal:<id>.
        let (st, p) = call(&server, "POST", &format!("/api/v1/proposals/{id}/decision"), &env.alice, org, Some(decision("accept"))).await;
        assert_eq!((st, p["status"].clone()), (200, json!("accepted")), "{p}");
        assert!(p["decided_by"].as_str().is_some_and(|n| !n.contains('@')), "{p}");
        let (_, links) = call(&server, "GET", &format!("/api/v1/ontology/objects/{sub}/links"), &env.alice, org, None).await;
        let rows = links.as_array().or(links["results"].as_array()).or(links["links"].as_array()).cloned().unwrap_or_default();
        let link = rows.iter().find(|l| l["link_type"] == "subsidiary_of").cloned().expect("written");
        assert_eq!(link["source_ref"], format!("proposal:{id}"));
        assert_eq!(link["attributes"]["nature"], "wikidata");
        // The same decision again is harmless; the other one is refused.
        let (st, _) = call(&server, "POST", &format!("/api/v1/proposals/{id}/decision"), &env.alice, org, Some(decision("accept"))).await;
        assert_eq!(st, 200);
        let (st, e) = call(&server, "POST", &format!("/api/v1/proposals/{id}/decision"), &env.alice, org, Some(decision("reject"))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("proposal-decided")), "{e}");

        // Rejected: never proposed again; another value is a new proposal.
        let r = propose(&env.db, org, &group_link(&sub, &other, "wikidata")).await.unwrap().unwrap();
        let (st, p) = call(&server, "POST", &format!("/api/v1/proposals/{r}/decision"), &env.alice, org,
            Some(json!({"decision": "reject", "reason": "pas la bonne mère"}))).await;
        assert_eq!((st, p["reason"].clone()), (200, json!("pas la bonne mère")), "{p}");
        assert!(propose(&env.db, org, &group_link(&sub, &other, "wikidata")).await.unwrap().is_none());

        // A newer proposal of the same target supersedes the older one.
        let first = propose(&env.db, org, &group_link(&other, &mother, "wikidata")).await.unwrap().unwrap();
        let second = propose(&env.db, org, &group_link(&other, &mother, "manual_entry")).await.unwrap().unwrap();
        let (_, old) = call(&server, "GET", &format!("/api/v1/proposals/{first}"), &env.alice, org, None).await;
        assert_eq!(old["status"], "superseded");
        let (st, _) = call(&server, "POST", &format!("/api/v1/proposals/{first}/decision"), &env.alice, org, Some(decision("accept"))).await;
        assert_eq!(st, 409);
        // Accepted with a correction.
        let (st, p) = call(&server, "POST", &format!("/api/v1/proposals/{second}/decision"), &env.alice, org,
            Some(json!({"decision": "accept", "values": {"nature": "declared", "source": "manual entry"}}))).await;
        assert_eq!(st, 200, "{p}");
        let (_, links) = call(&server, "GET", &format!("/api/v1/ontology/objects/{other}/links"), &env.alice, org, None).await;
        assert!(links.to_string().contains("declared"), "{links}");

        // An update on an object that moved since it was read is refused.
        let (_, o) = call(&server, "GET", &format!("/api/v1/ontology/objects/{mother}"), &env.alice, org, None).await;
        let version = o["version"].as_i64().unwrap() as i32;
        let stale = ProposalInput {
            kind: Some(ProposalKind::ObjectUpdate),
            type_key: "organization".into(),
            target_id: Some(mother.clone()),
            values: json!({"country": "FR"}).as_object().cloned().unwrap(),
            base_version: Some(version),
            origin: "llm:test".into(),
            ..Default::default()
        };
        let u = propose(&env.db, org, &stale).await.unwrap().unwrap();
        let (st, _) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{mother}"), &env.alice, org,
            Some(json!({"type_key": "organization", "title": "Mère SA", "properties": {}, "expected_version": version}))).await;
        assert_eq!(st, 200);
        let (st, e) = call(&server, "POST", &format!("/api/v1/proposals/{u}/decision"), &env.alice, org, Some(decision("accept"))).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-version-conflict")), "{e}");
        let (_, still) = call(&server, "GET", &format!("/api/v1/proposals/{u}"), &env.alice, org, None).await;
        assert_eq!(still["status"], "proposed", "a refused acceptance changes nothing");
    })
    .await;
}
