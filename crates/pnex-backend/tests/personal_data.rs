//! Personal data (crm.md D442–D447, account-mapping.md D341): masking by
//! role, erasure as a shell, the objection enforced on new links, and the
//! retention queue of erasure proposals decided by a human.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL), database emptied between tests.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use pnex_backend::services::profiles::propose_erasures_in;
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

/// Alice's org with the prospecting pack, and Bob as a member.
async fn org(server: &axum_test::TestServer, env: &Env) -> i64 {
    let _ = me(server, &env.bob).await;
    let org = me(server, &env.alice).await["orgs"][0]["id"]
        .as_i64()
        .unwrap();
    let (st, e) = call(
        server,
        "POST",
        "/api/v1/ontology/packs/prospection_b2b/install",
        &env.alice,
        org,
        None,
    )
    .await;
    assert_eq!(st, 200, "{e}");
    let (st, e) = call(
        server,
        "POST",
        &format!("/api/v1/orgs/{org}/members"),
        &env.alice,
        org,
        Some(json!({"email": "bob@example.com", "role": "member"})),
    )
    .await;
    assert!(st == 200 || st == 201, "{e}");
    org
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

async fn employs(
    server: &axum_test::TestServer,
    token: &str,
    org: i64,
    account: &str,
    person: &str,
) -> (u16, Value) {
    call(
        server,
        "POST",
        "/api/v1/ontology/links",
        token,
        org,
        Some(json!({
            "link_type": "employs", "source_id": account, "target_id": person,
            "attributes": {"position": "Plant manager", "source": "registry"}
        })),
    )
    .await
}

#[tokio::test]
#[serial]
async fn a_role_without_personal_read_sees_masked_people() {
    with_app(|server, env| async move {
        let org = org(&server, &env).await;
        let acme = object(&server, &env.alice, org, "organization", "Acme", json!({})).await;
        let ann = object(
            &server,
            &env.alice,
            org,
            "person",
            "Ann Smith",
            json!({"notes": "met at the fair", "persona": "decideur", "wikidata": "Q1"}),
        )
        .await;
        let (st, e) = employs(&server, &env.alice, org, &acme, &ann).await;
        assert_eq!(st, 201, "{e}");

        // Full rights: everything.
        let (_, o) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{ann}"),
            &env.bob,
            org,
            None,
        )
        .await;
        assert_eq!(o["title"], "Ann Smith");
        assert!(o.get("redacted").is_none(), "{o}");

        // A deny on personal data, given to Bob.
        let (st, role) = call(
            &server,
            "POST",
            "/api/v1/iam/roles",
            &env.alice,
            org,
            Some(json!({
                "key": "no-personal", "name": "No personal data",
                "statements": [{"effect": "deny", "permissions": ["ontology.personal:read"]}]
            })),
        )
        .await;
        assert_eq!(st, 201, "{role}");
        let bob_id = me(&server, &env.bob).await["id"].as_i64().unwrap();
        let (st, a) = call(
            &server,
            "POST",
            "/api/v1/iam/assignments",
            &env.alice,
            org,
            Some(json!({"user_id": bob_id, "role_id": role["id"]})),
        )
        .await;
        assert_eq!(st, 201, "{a}");

        let (st, o) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{ann}"),
            &env.bob,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{o}");
        assert_eq!(o["title"], format!("•••• {}", &ann[..8]));
        assert_eq!(o["properties"], json!({"wikidata": "Q1"}), "{o}");
        let redacted: Vec<&str> = o["redacted"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            redacted.contains(&"notes")
                && redacted.contains(&"persona")
                && redacted.contains(&"title"),
            "{o}"
        );

        let (_, links) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{acme}/links"),
            &env.bob,
            org,
            None,
        )
        .await;
        let l = &links["results"][0];
        assert_eq!(l["attributes"], json!({"source": "registry"}), "{links}");
        assert_eq!(l["target"]["title"], format!("•••• {}", &ann[..8]));

        let (_, q) = call(
            &server,
            "POST",
            "/api/v1/ontology/query",
            &env.bob,
            org,
            Some(json!({"type_key": "person"})),
        )
        .await;
        assert_eq!(
            q["rows"][0]["object"]["title"],
            format!("•••• {}", &ann[..8]),
            "{q}"
        );

        // A search, a filter or a sort never reveals a hidden value.
        for (who, seen) in [(&env.bob, 0), (&env.alice, 1)] {
            for body in [
                json!({"type_key": "person", "text": "smith"}),
                json!({"type_key": "person", "filters": [
                    {"key": "notes", "op": "contains", "value": "fair"}]}),
                json!({"type_key": "person", "filters": [
                    {"key": "persona", "op": "eq", "value": "decideur"}]}),
            ] {
                let (_, q) = call(
                    &server,
                    "POST",
                    "/api/v1/ontology/query",
                    who,
                    org,
                    Some(body),
                )
                .await;
                assert_eq!(q["count"], seen, "{q}");
            }
        }

        // A case naming the person is masked the same way (iam.md I5).
        let (st, case) = call(
            &server,
            "POST",
            "/api/v1/cases",
            &env.alice,
            org,
            Some(json!({"title": "Inquiry"})),
        )
        .await;
        assert_eq!(st, 201, "{case}");
        let cid = case["id"].as_str().unwrap().to_string();
        let (_, c) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{cid}"),
            &env.alice,
            org,
            None,
        )
        .await;
        let mut props = c["properties"].clone();
        props["legal_basis"] = json!("legitimate interest");
        let basis =
            json!({"expected_version": c["version"], "title": "Inquiry", "properties": props});
        let (st, e) = call(
            &server,
            "PUT",
            &format!("/api/v1/ontology/objects/{cid}"),
            &env.alice,
            org,
            Some(basis),
        )
        .await;
        assert_eq!(st, 200, "{e}");
        let (st, e) = call(
            &server,
            "POST",
            &format!("/api/v1/cases/{cid}/attach"),
            &env.alice,
            org,
            Some(json!({"object_id": ann})),
        )
        .await;
        assert_eq!(st, 200, "{e}");
        let workspace = format!("/api/v1/cases/{cid}/workspace");
        let (_, w) = call(&server, "GET", &workspace, &env.bob, org, None).await;
        assert_eq!(
            w["undated"][0]["title"],
            format!("•••• {}", &ann[..8]),
            "{w}"
        );
        assert!(!w.to_string().contains("Ann Smith"), "{w}");
        let (_, w) = call(&server, "GET", &workspace, &env.alice, org, None).await;
        assert_eq!(w["undated"][0]["title"], "Ann Smith", "{w}");

        // Alice still reads everything.
        let (_, o) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{ann}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(o["properties"]["notes"], "met at the fair");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_erased_person_is_a_shell_never_recreated_at_its_account() {
    with_app(|server, env| async move {
        let org = org(&server, &env).await;
        let acme = object(&server, &env.alice, org, "organization", "Acme", json!({})).await;
        let other = object(
            &server,
            &env.alice,
            org,
            "organization",
            "Other Co",
            json!({}),
        )
        .await;
        let ann = object(
            &server,
            &env.alice,
            org,
            "person",
            "Ann Smith",
            json!({"notes": "met at the fair", "wikidata": "Q1"}),
        )
        .await;
        let (st, _) = employs(&server, &env.alice, org, &acme, &ann).await;
        assert_eq!(st, 201);

        // Another org never reaches it (R1); without the delete permission,
        // 403 (R2).
        let bob_org = me(&server, &env.bob).await["orgs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["id"].as_i64().unwrap())
            .find(|id| *id != org)
            .unwrap();
        let erase = format!("/api/v1/ontology/objects/{ann}/erase");
        let (st, _) = call(&server, "POST", &erase, &env.bob, bob_org, None).await;
        assert_eq!(st, 404);
        let (_, role) = call(
            &server,
            "POST",
            "/api/v1/iam/roles",
            &env.alice,
            org,
            Some(json!({
                "key": "no-delete", "name": "No delete",
                "statements": [{"effect": "deny", "permissions": ["ontology:delete"]}]
            })),
        )
        .await;
        let bob_id = me(&server, &env.bob).await["id"].as_i64().unwrap();
        let (st, a) = call(
            &server,
            "POST",
            "/api/v1/iam/assignments",
            &env.alice,
            org,
            Some(json!({"user_id": bob_id, "role_id": role["id"]})),
        )
        .await;
        assert_eq!(st, 201, "{a}");
        let (st, e) = call(&server, "POST", &erase, &env.bob, org, None).await;
        assert_eq!(st, 403, "{e}");

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
        assert_eq!(o["title"], format!("∅ {}", &ann[..8]));
        assert_eq!(o["properties"], json!({"wikidata": "Q1"}), "{o}");
        assert!(o["erased_at"].is_string(), "{o}");
        let version = o["version"].clone();

        // Idempotent.
        let (st, again) = call(
            &server,
            "POST",
            &format!("/api/v1/ontology/objects/{ann}/erase"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!((st, again["version"].clone()), (200, version.clone()));

        // The shell keeps its link, without the personal attribute.
        let (_, links) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{acme}/links"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(links["count"], 1, "{links}");
        assert_eq!(
            links["results"][0]["attributes"],
            json!({"source": "registry"})
        );

        // The shell is never filled again nor given a new link.
        let refill = json!({"expected_version": version, "title": "Ann Smith", "properties": {"notes": "back"}});
        let (st, e) = call(&server, "PUT", &format!("/api/v1/ontology/objects/{ann}"), &env.alice, org, Some(refill)).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-erased")), "{e}");
        let (st, e) = employs(&server, &env.alice, org, &other, &ann).await;
        assert_eq!((st, e["error"].as_str()), (409, Some("ontology-erased")), "{e}");

        // The same name at the same account is refused, elsewhere accepted.
        let again = object(&server, &env.alice, org, "person", "ann  SMITH", json!({})).await;
        let (st, e) = employs(&server, &env.alice, org, &acme, &again).await;
        assert_eq!(
            (st, e["error"].as_str()),
            (409, Some("prospection-opposed")),
            "{e}"
        );
        let (st, e) = employs(&server, &env.alice, org, &other, &again).await;
        assert_eq!(st, 201, "{e}");

        // The objection list holds the person and its name at its account.
        let n = env
            .db
            .query_one_raw(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Postgres,
                format!("SELECT count(*) AS n FROM prospection_oppositions WHERE org_id = {org}"),
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(n, 2, "person:<id> and title:ann smith@acme");
    })
    .await;
}

#[tokio::test]
#[serial]
async fn an_objection_dates_the_person_and_blocks_its_account() {
    with_app(|server, env| async move {
        let org = org(&server, &env).await;
        let acme = object(&server, &env.alice, org, "organization", "Acme", json!({})).await;
        let ann = object(&server, &env.alice, org, "person", "Ann Smith", json!({})).await;
        let (st, _) = employs(&server, &env.alice, org, &acme, &ann).await;
        assert_eq!(st, 201);
        let (st, o) = call(
            &server,
            "POST",
            &format!("/api/v1/extension/people/{ann}/opposition"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert_eq!(st, 200, "{o}");
        let (_, o) = call(
            &server,
            "GET",
            &format!("/api/v1/ontology/objects/{ann}"),
            &env.alice,
            org,
            None,
        )
        .await;
        assert!(o["properties"]["opposition_le"].is_string(), "{o}");
        let twin = object(&server, &env.alice, org, "person", "Ann Smith", json!({})).await;
        let (st, _) = employs(&server, &env.alice, org, &acme, &twin).await;
        assert_eq!(st, 409);
    })
    .await;
}

#[tokio::test]
#[serial]
async fn the_retention_queue_proposes_and_a_human_erases() {
    with_app(|server, env| async move {
        let org = org(&server, &env).await;
        let old = object(&server, &env.alice, org, "person", "Old Contact", json!({"notes": "x"})).await;
        let kept = object(&server, &env.alice, org, "person", "Kept Contact", json!({})).await;
        // A person changed lately is not proposed.
        object(&server, &env.alice, org, "person", "Fresh Contact", json!({})).await;
        env.db
            .execute_unprepared(&format!(
                "UPDATE objects SET updated_at = now() - interval '40 months' WHERE id IN ('{old}', '{kept}')"
            ))
            .await
            .unwrap();

        assert_eq!(propose_erasures_in(&env.db, org, 36).await.unwrap(), 2);
        // One open proposal per person.
        assert_eq!(propose_erasures_in(&env.db, org, 36).await.unwrap(), 0);
        let (_, l) = call(&server, "GET", "/api/v1/proposals", &env.alice, org, None).await;
        assert_eq!(l["count"], 2, "{l}");
        let id_of = |target: &str| {
            l["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["refs"][0]["id"] == target)
                .map(|p| p["id"].as_str().unwrap().to_string())
                .unwrap()
        };
        assert_eq!(l["results"][0]["kind"], "object_erase");

        // Accepted by a person: erased.
        let (st, p) = call(&server, "POST", &format!("/api/v1/proposals/{}/decision", id_of(&old)), &env.alice, org,
            Some(json!({"decision": "accept"}))).await;
        assert_eq!((st, p["status"].clone()), (200, json!("accepted")), "{p}");
        let (_, o) = call(&server, "GET", &format!("/api/v1/ontology/objects/{old}"), &env.alice, org, None).await;
        assert!(o["erased_at"].is_string(), "{o}");
        assert!(o["properties"].get("notes").is_none());

        // Rejected: kept, and the refusal restarts the period.
        let (st, p) = call(&server, "POST", &format!("/api/v1/proposals/{}/decision", id_of(&kept)), &env.alice, org,
            Some(json!({"decision": "reject", "reason": "active customer"}))).await;
        assert_eq!((st, p["status"].clone()), (200, json!("rejected")), "{p}");
        assert_eq!(propose_erasures_in(&env.db, org, 36).await.unwrap(), 0);
        let (_, o) = call(&server, "GET", &format!("/api/v1/ontology/objects/{kept}"), &env.alice, org, None).await;
        assert!(o.get("erased_at").is_none(), "{o}");
    })
    .await;
}
