//! Field notes, lot PD1d (parts-docs.md §3.7): a member proposes a note on an
//! equipment with a photo; the author never accepts it, a member denied
//! `maintenance:validate` neither; another member accepts it and the note
//! exists with its links. Filing twice answers the same proposal.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::ConnectionTrait;
use serde_json::{json, Value};
use serial_test::serial;

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

#[tokio::test]
#[serial]
async fn a_field_note_waits_for_another_validator() {
    let base = common::spawn_mock_rauthy().await;
    unsafe { std::env::set_var("RAUTHY_URL", &base) };
    unsafe { std::env::set_var("PNEX_FLOW_ENABLED", "false") };
    let token = |n: &str, c: char| {
        common::valid_token(
            &base,
            &format!("00000000-0000-0000-0000-00000000000{c}"),
            n,
            &format!("{n}@example.com"),
        )
    };
    let (alice, bob, carol) = (token("alice", 'a'), token("bob", 'b'), token("carol", 'c'));
    let config: RequestConfig = RequestConfigBuilder::new().build();
    loco_rs::testing::request::request_with_config::<App, _, _>(config, move |server, ctx| async move {
        common::seed_catalogue(&ctx.db).await;
        let org = me(&server, &alice).await["orgs"][0]["id"].as_i64().unwrap();
        let carol_id = me(&server, &carol).await["id"].as_i64().unwrap();
        me(&server, &bob).await;
        common::join_org(&server, &alice, &bob, org, "bob@example.com", "member").await;
        common::join_org(&server, &alice, &carol, org, "carol@example.com", "member").await;
        let (st, r) = call(&server, "/api/v1/ontology/packs/maintenance/install", &alice, org, json!(null)).await;
        assert!(st < 300, "{r}");
        let (st, press) = call(&server, "/api/v1/ontology/objects", &alice, org,
            json!({"type_key": "machine", "title": "Press 1", "properties": {"serial": "P-1"}})).await;
        assert_eq!(st, 201, "{press}");
        let press = press["id"].as_str().unwrap().to_string();
        let photo = ctx.db.query_one_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "INSERT INTO media_assets (id, org_id, kind, name) VALUES (gen_random_uuid(), $1, 'photo', 'plate.jpg') RETURNING object_id",
            [org.into()],
        )).await.unwrap().unwrap().try_get::<uuid::Uuid>("", "object_id").unwrap().to_string();

        let note = json!({
            "type_key": "field_note", "title": "Bearing noise",
            "values": {"text": "Left bearing rattles at full speed."},
            "link_type": "note_on", "link_target_id": press,
            "links": [{"link_type": "note_photo", "target_id": photo}]
        });
        // Invalid input: unknown type, unknown link type, object of no one.
        for (bad, field) in [
            (json!({"type_key": "nope", "title": "x"}), "type_key"),
            (json!({"type_key": "field_note", "title": "x", "links": [{"link_type": "nope", "target_id": photo}]}), "link_type"),
            (json!({"type_key": "field_note", "title": "x", "link_type": "note_on"}), "link_target_id"),
            (json!({"type_key": "field_note", "title": " "}), "title"),
        ] {
            let (st, e) = call(&server, "/api/v1/proposals", &bob, org, bad.clone()).await;
            assert_eq!(st, 400, "{bad}: {e}");
            assert!(e["errors"]["fields"].get(field).is_some(), "{bad}: {e}");
        }
        let (st, e) = call(&server, "/api/v1/proposals", &bob, org,
            json!({"type_key": "field_note", "title": "x", "link_type": "note_on", "link_target_id": uuid::Uuid::new_v4()})).await;
        assert_eq!(st, 404, "{e}");

        let (st, p) = call(&server, "/api/v1/proposals", &bob, org, note.clone()).await;
        assert_eq!(st, 201, "{p}");
        assert_eq!((p["status"].as_str(), p["origin"].as_str()), (Some("proposed"), Some("member")), "{p}");
        let id = p["id"].as_str().unwrap().to_string();
        // An offline queue sending twice gets the same proposal.
        let (st, again) = call(&server, "/api/v1/proposals", &bob, org, note.clone()).await;
        assert_eq!((st, &again["id"]), (201, &p["id"]), "{again}");

        let decision = format!("/api/v1/proposals/{id}/decision");
        let accept = json!({"decision": "accept"});
        let (st, e) = call(&server, &decision, &bob, org, accept.clone()).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("proposal-own")), "{e}");

        let (st, role) = call(&server, "/api/v1/iam/roles", &alice, org, json!({
            "key": "no-validate", "name": "No validation",
            "statements": [{"effect": "deny", "permissions": ["maintenance:validate"]}]
        })).await;
        assert_eq!(st, 201, "{role}");
        let (st, a) = call(&server, "/api/v1/iam/assignments", &alice, org,
            json!({"user_id": carol_id, "role_id": role["id"]})).await;
        assert_eq!(st, 201, "{a}");
        let (st, e) = call(&server, &decision, &carol, org, accept.clone()).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("access-denied")), "{e}");

        let (st, v) = call(&server, &decision, &alice, org, accept).await;
        assert_eq!((st, v["status"].as_str()), (200, Some("accepted")), "{v}");
        let note_id = v["applied"].as_str().unwrap();
        let links = ctx.db.query_all_raw(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "SELECT relation, target_object_id::text AS t FROM resource_edges \
             WHERE source_object_id = $1::uuid ORDER BY relation",
            [note_id.into()],
        )).await.unwrap();
        let got: Vec<(String, String)> = links.iter()
            .map(|r| (r.try_get("", "relation").unwrap(), r.try_get("", "t").unwrap()))
            .collect();
        assert_eq!(got, vec![("note_on".into(), press), ("note_photo".into(), photo)]);
    }).await;
}
