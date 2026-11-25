//! A Table written into the ontology by an org pipeline (pipelines.md D577,
//! D1050): the delta is by planned entity, so an edit survives until the source
//! changes that value, and a row gone does not close what its sibling rows still
//! hold.
//!
//! Needs PostgreSQL (TEST_DATABASE_URL) and the compose `rustfs` service.

mod common;

use loco_rs::testing::request::{RequestConfig, RequestConfigBuilder};
use pnex_backend::app::App;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, QueryResult, Statement};
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
            f(server, alice, ctx).await;
        },
    )
    .await;
}

struct Api<'a> {
    server: &'a axum_test::TestServer,
    token: &'a str,
    org: i64,
}

impl Api<'_> {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let req = match method {
            "GET" => self.server.get(path),
            "POST" => self.server.post(path),
            "PUT" => self.server.put(path),
            "DELETE" => self.server.delete(path),
            _ => unreachable!(),
        }
        .add_header("Authorization", format!("Bearer {}", self.token))
        .add_header("X-Org-Id", self.org.to_string());
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

    async fn load(&self, id: &str, csv: &[u8]) {
        let res = self
            .server
            .post(&format!("/api/v1/datasets/{id}/transactions/csv"))
            .add_header("Authorization", format!("Bearer {}", self.token))
            .add_header("X-Org-Id", self.org.to_string())
            .bytes(csv.to_vec().into())
            .await;
        assert_eq!(res.status_code().as_u16(), 201, "{}", res.text());
    }

    /// `csv` and entities that never change: a closure stays under the share a
    /// run may close.
    async fn load_partners(&self, id: &str, csv: &[u8]) {
        let mut all = String::from_utf8_lossy(csv).trim_end().to_string();
        for n in 0..30u32 {
            let head = format!("{:08}", 10_000_000 + n);
            let sum: u32 = head
                .chars()
                .rev()
                .enumerate()
                .map(|(i, c)| {
                    let d = c.to_digit(10).unwrap();
                    match i % 2 == 0 {
                        true if d * 2 > 9 => d * 2 - 9,
                        true => d * 2,
                        false => d,
                    }
                })
                .sum();
            let check = (10 - sum % 10) % 10;
            all.push_str(&format!("\n{head}{check}00017;Filler {n};;RS1;F"));
        }
        all.push('\n');
        self.load(id, all.as_bytes()).await;
    }

    async fn dataset(&self, name: &str) -> String {
        let (s, d) = self
            .call("POST", "/api/v1/datasets", Some(json!({ "name": name })))
            .await;
        assert_eq!(s, 201, "{d}");
        d["id"].as_str().unwrap().to_string()
    }

    /// The org pipeline `name` writing objects from the Table `table` with `mapping`.
    async fn pipeline(&self, name: &str, table: &str, mapping: &str, snapshot: bool) -> String {
        let document = common::objects_pipeline(table, mapping, snapshot);
        common::create_pipeline(self.server, self.token, self.org, name, &document).await
    }

    async fn run(&self, ctx: &loco_rs::app::AppContext, id: &str) -> Value {
        common::run_org_pipeline(self.server, ctx, self.token, self.org, id).await
    }
}

async fn sql(db: &DatabaseConnection, text: &str) -> Vec<QueryResult> {
    db.query_all_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        text.to_string(),
    ))
    .await
    .unwrap()
}

/// `nda` of the live organization with this SIREN, its id and version.
async fn org_of(
    db: &DatabaseConnection,
    siren: &str,
) -> Option<(String, i32, Option<String>, String)> {
    sql(db, &format!(
        "SELECT id::text AS id, version, title, properties ->> 'nda' AS nda FROM objects \
         WHERE type_key = 'organization' AND valid_to IS NULL AND properties ->> 'siren' = '{siren}'"
    ))
    .await
    .first()
    .map(|r| {
        (
            r.try_get("", "id").unwrap(),
            r.try_get("", "version").unwrap(),
            r.try_get("", "nda").unwrap(),
            r.try_get("", "title").unwrap(),
        )
    })
}

async fn scopes(db: &DatabaseConnection) -> Vec<String> {
    let mut out: Vec<String> = sql(db,
        "SELECT o.properties ->> 'siren' AS s, e.attributes ->> 'scope' AS scope FROM resource_edges e \
         JOIN objects o ON o.id = e.source_object_id \
         WHERE e.relation = 'accredited_for' AND e.valid_to IS NULL \
         AND o.properties ->> 'siren' NOT LIKE '1000%'")
        .await
        .iter()
        .map(|r| {
            format!(
                "{}={}",
                r.try_get::<String>("", "s").unwrap(),
                r.try_get::<String>("", "scope").unwrap()
            )
        })
        .collect();
    out.sort();
    out
}

const MAPPING: &str = r#"
key: partners
name: Partners
objects:
  - as: org
    type: organization
    key: siren
    title: { from: name, convert: [trim] }
    close_missing: true
    properties:
      siren: { from: siret, convert: [digits, "left:9", luhn] }
      nda: { from: nda, convert: [digits] }
  - as: cert
    type: certification
    key: code
    create: false
    properties:
      code: { from: code, convert: [trim, upper] }
links:
  - type: accredited_for
    from: org
    to: cert
    attributes:
      scope: { from: scope, convert: [{ enum: { O: both, F: preparation, A: assessment } }] }
    on_conflict: { scope: both }
"#;

// Two SIRET of one SIREN (73282932000074, 73282932000082) with different scopes.
const V1: &[u8] = b"siret;name;nda;code;scope\n\
73282932000074;Acme;11111111111;RS1;F\n\
73282932000082;Acme;11111111111;RS1;A\n\
55210055400013;Beta;22222222222;RS1;F\n";

#[tokio::test]
#[serial]
async fn an_edit_survives_until_the_source_changes_that_value() {
    with_app(|server, alice, ctx| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: &alice, org };
        let db = &ctx.db;
        let (s, e) = api
            .call("POST", "/api/v1/ontology/packs/quali_watch/install", None)
            .await;
        assert_eq!(s, 200, "{e}");
        let (s, e) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({ "type_key": "certification", "title": "RS1", "properties": { "code": "RS1" } })),
            )
            .await;
        assert_eq!(s, 201, "{e}");

        let worker = common::spawn_pipeline_worker();
        let dataset = api.dataset("partners").await;
        api.load_partners(&dataset, V1).await;
        let id = api.pipeline("partners", "partners", MAPPING, true).await;
        let acme = "732829320";
        let beta = "552100554";

        // First run: no base, it applies. Sibling rows merge into one link.
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert_eq!(scopes(db).await, [format!("{beta}=preparation"), format!("{acme}=both")]);
        let (org_id, version, nda, title) = org_of(db, acme).await.unwrap();
        assert_eq!((nda.as_deref(), title.as_str()), (Some("11111111111"), "Acme"));

        // Same transaction: nothing to do.
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert_eq!(org_of(db, acme).await.unwrap().1, version);

        // A person edits the number and the title in the ontology.
        let (s, e) = api
            .call(
                "PUT",
                &format!("/api/v1/ontology/objects/{org_id}"),
                Some(json!({ "title": "Acme edited", "expected_version": version,
                             "properties": { "siren": acme, "nda": "99999999999" } })),
            )
            .await;
        assert_eq!(s, 200, "{e}");

        // Reloaded as is, a new transaction: the edit stays.
        api.load_partners(&dataset, V1).await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        let (_, _, nda, title) = org_of(db, acme).await.unwrap();
        assert_eq!((nda.as_deref(), title.as_str()), (Some("99999999999"), "Acme edited"));

        // Another entity changes: only it moves, the edit stays.
        let v2 = String::from_utf8_lossy(V1).replace("22222222222", "33333333333");
        api.load_partners(&dataset, v2.as_bytes()).await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert_eq!(org_of(db, beta).await.unwrap().2.as_deref(), Some("33333333333"));
        assert_eq!(org_of(db, acme).await.unwrap().2.as_deref(), Some("99999999999"));

        // The source changes the edited value: the last change wins, the title stays.
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n\
73282932000074;Acme;44444444444;RS1;F\n\
73282932000082;Acme;44444444444;RS1;A\n\
55210055400013;Beta;33333333333;RS1;F\n").await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        let (_, _, nda, title) = org_of(db, acme).await.unwrap();
        assert_eq!((nda.as_deref(), title.as_str()), (Some("44444444444"), "Acme edited"));

        // A sibling row goes: the object and the link stay, the scope follows.
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n\
73282932000074;Acme;44444444444;RS1;F\n\
55210055400013;Beta;33333333333;RS1;F\n\
44306184100047;Gamma;55555555555;RS1;F\n").await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert_eq!(
            scopes(db).await,
            ["443061841=preparation".to_string(), format!("{beta}=preparation"), format!("{acme}=preparation")]
        );

        // Archived by a person and unchanged in the source: it stays archived.
        let (gamma_id, ..) = org_of(db, "443061841").await.unwrap();
        let (s, _) = api.call("DELETE", &format!("/api/v1/ontology/objects/{gamma_id}"), None).await;
        assert_eq!(s, 204);
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n\
73282932000074;Acme;44444444444;RS1;F\n\
55210055400013;Beta;33333333333;RS1;F\n\
44306184100047;Gamma;55555555555;RS1;F\n").await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert!(org_of(db, "443061841").await.is_none());

        // All the rows of an entity go: it closes. The mapping stays valid.
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n\
73282932000074;Acme;44444444444;RS1;F\n\
44306184100047;Gamma;55555555555;RS1;F\n").await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert!(org_of(db, beta).await.is_none());
        // Compared for real this time, and the archived one is still not back.
        assert!(org_of(db, "443061841").await.is_none());

        // Most of what it holds gone at once: refused, nothing written.
        api.load(&dataset, b"siret;name;nda;code;scope\n73282932000074;Acme;44444444444;RS1;F\n").await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "failed", "{r}");
        assert_eq!(r["code"], "ingest-suspicious-closures", "{r}");
        let fillers = sql(db, "SELECT 1 AS x FROM objects WHERE type_key = 'organization' \
            AND valid_to IS NULL AND properties ->> 'siren' LIKE '1000%'").await;
        assert_eq!(fillers.len(), 30);

        // The Table is gone: the run fails.
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        pnex_backend::services::datasets::delete(db, &store, org, dataset.parse().unwrap())
            .await
            .unwrap();
        let r = api.run(&ctx, &id).await;
        worker.abort();
        assert_eq!(r["state"], "failed", "{r}");
    })
    .await;
}

/// A lookup whose object does not exist yet is left out. The source does not change
/// when the object appears, and what was left out is still written then.
#[tokio::test]
#[serial]
async fn what_waited_for_an_object_is_written_when_it_appears() {
    with_app(|server, alice, ctx| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: &alice, org };
        let db = &ctx.db;
        api.call("POST", "/api/v1/ontology/packs/quali_watch/install", None).await;
        let dataset = api.dataset("partners").await;
        let csv = "siret;name;nda;code;scope\n73282932000074;Acme;11111111111;RS9;F\n";
        api.load(&dataset, csv.as_bytes()).await;
        let mapping = MAPPING.replace("create: false", "create: false\n    missing: skip");
        let worker = common::spawn_pipeline_worker();
        let id = api.pipeline("partners", "partners", &mapping, false).await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert!(scopes(db).await.is_empty());

        // Same table, the object now exists: the link opens at once.
        let (s, e) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({ "type_key": "certification", "title": "RS9", "properties": { "code": "RS9" } })),
            )
            .await;
        assert_eq!(s, 201, "{e}");
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert_eq!(scopes(db).await, ["732829320=preparation"]);

        // Written once: the edit of a person is not undone by the next run.
        let (org_id, version, ..) = org_of(db, "732829320").await.unwrap();
        let (s, e) = api
            .call(
                "PUT",
                &format!("/api/v1/ontology/objects/{org_id}"),
                Some(json!({ "title": "Edited", "expected_version": version,
                             "properties": { "siren": "732829320", "nda": "99999999999" } })),
            )
            .await;
        assert_eq!(s, 200, "{e}");
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");
        assert_eq!(org_of(db, "732829320").await.unwrap().2.as_deref(), Some("99999999999"));
        worker.abort();

        // The same file again is no new version; another one is.
        let committed = format!(
            "SELECT 1 AS x FROM dataset_transactions WHERE dataset_id = '{dataset}' AND state = 'committed'"
        );
        api.load(&dataset, csv.as_bytes()).await;
        assert_eq!(sql(db, &committed).await.len(), 1);
        api.load(&dataset, b"siret;name;nda;code;scope\n73282932000074;Acme;11111111111;RS9;A\n").await;
        assert_eq!(sql(db, &committed).await.len(), 2);
    })
    .await;
}

/// A table of dated values becomes the series of the objects it names:
/// the `@queried` column says whose value it is.
#[tokio::test]
#[serial]
async fn a_table_of_dated_values_becomes_series() {
    with_app(|server, alice, ctx| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api {
            server: &server,
            token: &alice,
            org,
        };
        let db = &ctx.db;
        api.call("POST", "/api/v1/ontology/packs/quali_watch/install", None)
            .await;
        let partners = api.dataset("partners").await;
        api.load(&partners, V1).await;
        let worker = common::spawn_pipeline_worker();
        let mapping = MAPPING.replace("create: false", "create: false\n    missing: skip");
        let id = api.pipeline("partners", "partners", &mapping, false).await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");

        let accounts = api.dataset("accounts").await;
        api.load(
            &accounts,
            b"@queried;metric;unit;start;end;value\n\
              732829320;revenue;EUR;2024-01-01;2024-12-31;1500000\n\
              732829320;revenue;EUR;2025-01-01;2025-12-31;1750000.5\n\
              552100554;revenue;EUR;2025-01-01;2025-12-31;not a number\n\
              999999999;revenue;EUR;2025-01-01;2025-12-31;42\n",
        )
        .await;
        let mapping = "observations: { type: organization, property: siren }\n";
        let id = api.pipeline("accounts", "accounts", mapping, false).await;
        let r = api.run(&ctx, &id).await;
        worker.abort();
        assert_eq!(r["state"], "done", "{r}");
        let values: Vec<f64> = sql(
            db,
            "SELECT m.value FROM market_observations m JOIN objects o ON o.id = m.object_id \
             WHERE m.metric = 'revenue' ORDER BY m.period_end",
        )
        .await
        .iter()
        .map(|r| r.try_get("", "value").unwrap())
        .collect();
        assert_eq!(values, [1_500_000.0, 1_750_000.5]);
        // The run counts what it stored, not the row naming no object.
        assert_eq!(common::written(&r)["observations"], 2, "{r}");
    })
    .await;
}

/// The Table an import still fills, until the import becomes a pipeline at boot,
/// cannot be loaded, declared or deleted (D1050).
#[tokio::test]
#[serial]
async fn the_table_of_an_import_left_is_reserved() {
    with_app(|server, alice, ctx| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api {
            server: &server,
            token: &alice,
            org,
        };
        let dataset = api.dataset("vaults").await;
        api.load(&dataset, b"code\nA1\n").await;
        ctx.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE datasets SET import_id = gen_random_uuid() WHERE id = $1::uuid",
                [dataset.clone().into()],
            ))
            .await
            .unwrap();
        let res = server
            .post(&format!("/api/v1/datasets/{dataset}/transactions/csv"))
            .add_header("Authorization", format!("Bearer {alice}"))
            .add_header("X-Org-Id", org.to_string())
            .bytes(b"code\nB2\n".to_vec().into())
            .await;
        assert_eq!(res.status_code().as_u16(), 409, "{}", res.text());
        assert_eq!(res.json::<Value>()["error"], "dataset-reserved");
        let (s, e) = api
            .call(
                "PUT",
                &format!("/api/v1/datasets/{dataset}/identity"),
                Some(json!({ "columns": [] })),
            )
            .await;
        assert_eq!(s, 409, "{e}");
        let (s, e) = api
            .call("DELETE", &format!("/api/v1/datasets/{dataset}"), None)
            .await;
        assert_eq!(s, 409, "{e}");
    })
    .await;
}

/// SEC-93: a pipeline writes objects no higher than the member who last loaded
/// each Table it reads. An owner's pipeline over a Table a member loaded cannot
/// write a type locked to admins; once the owner loads it again, it can.
#[tokio::test]
#[serial]
async fn a_member_loaded_table_writes_at_the_members_role() {
    with_app(|server, alice, ctx| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let bob = common::valid_token(
            &std::env::var("RAUTHY_URL").unwrap(),
            "00000000-0000-0000-0000-00000000000b",
            "bob",
            "bob@example.com",
        );
        server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {bob}"))
            .await;
        common::join_org(&server, &alice, &bob, org, "bob@example.com", "member").await;
        let owner = Api {
            server: &server,
            token: &alice,
            org,
        };
        let member = Api {
            server: &server,
            token: &bob,
            org,
        };
        let (s, e) = owner
            .call(
                "POST",
                "/api/v1/ontology/types",
                Some(json!({ "def": {
                    "key": "vault", "name": "Vault", "write_role": "admin",
                    "properties": [{ "key": "code", "name": "Code", "kind": "text", "unique": true }]
                } })),
            )
            .await;
        assert_eq!(s, 201, "{e}");
        let worker = common::spawn_pipeline_worker();
        let dataset = owner.dataset("vaults").await;
        member.load(&dataset, b"code\nA1\n").await;
        let mapping = "objects:\n  - {as: v, type: vault, key: code, properties: {code: {from: code}}}\n";
        let id = owner.pipeline("vaults", "vaults", mapping, false).await;
        let vaults = || async {
            sql(&ctx.db, &format!(
                "SELECT 1 AS x FROM objects WHERE org_id = {org} AND type_key = 'vault' AND valid_to IS NULL"
            ))
            .await
            .len()
        };

        let refused = owner.run(&ctx, &id).await;
        assert_eq!(refused["state"], "failed", "{refused}");
        assert_eq!(refused["code"], "objects_refused", "{refused}");
        assert_eq!(vaults().await, 0);

        // Another file: the same one again keeps the member's version.
        owner.load(&dataset, b"code\nA2\n").await;
        let done = owner.run(&ctx, &id).await;
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(vaults().await, 1);
        worker.abort();
    })
    .await;
}

/// Worker in between: a column derived under another form loses the person too,
/// its history included. Needs `PNEX_WORKER_BIN`.
#[tokio::test]
#[serial]
#[ignore = "needs the worker binary in PNEX_WORKER_BIN"]
async fn an_erasure_reaches_what_the_worker_computed() {
    let bin = std::env::var("PNEX_WORKER_BIN").expect("PNEX_WORKER_BIN");
    with_app(|server, alice, ctx| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: &alice, org };
        let (s, e) = api
            .call(
                "POST",
                "/api/v1/ontology/types",
                Some(json!({ "def": { "key": "member", "name": "Member",
                    "title_classification": "personal", "properties": [] } })),
            )
            .await;
        assert_eq!(s, 201, "{e}");
        let (s, o) = api
            .call(
                "POST",
                "/api/v1/ontology/objects",
                Some(json!({ "type_key": "member", "title": "Ann Smith", "properties": {} })),
            )
            .await;
        assert_eq!(s, 201, "{o}");
        let people = api.dataset("people").await;
        api.load(&people, b"name;city\nAnn Smith;Lyon\nBob Jones;Nice\n").await;
        let (s, d) = api
            .call(
                "PUT",
                &format!("/api/v1/datasets/{people}/identity"),
                Some(json!({ "columns": [{ "column": "name", "type_key": "member", "property": "$title" }] })),
            )
            .await;
        assert_eq!(s, 200, "{d}");
        let document = common::sql_pipeline("shouted", &[("p", "people")], "SELECT upper(p.name) AS who FROM p");
        let (s, p) = api
            .call("POST", "/api/v1/pipelines", Some(json!({ "name": "shout", "document": document })))
            .await;
        assert_eq!(s, 201, "{p}");
        let runs = format!("/api/v1/pipelines/{}/runs", p["id"].as_str().unwrap());
        let (s, r) = api.call("POST", &runs, None).await;
        assert_eq!(s, 202, "{r}");

        let mut worker = std::process::Command::new(bin)
            .env("PNEX_PIPELINE_DATABASE_URL", std::env::var("TEST_DATABASE_URL").unwrap())
            .env("PNEX_PIPELINE_DATABASE_ROLE", "pnex_pipeline_worker")
            .env("PNEX_S3_ENDPOINT", "http://localhost:9000")
            .env("PNEX_S3_BUCKET", "pnex-test")
            .spawn()
            .unwrap();
        // The server takes each run; the worker runs its query.
        let finished = |run: String| {
            let (api, runs, ctx) = (&api, &runs, &ctx);
            async move {
                pnex_backend::services::pipelines::run_next(ctx).await.unwrap();
                for _ in 0..120 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    let (_, state) = api.call("GET", &format!("{runs}/{run}"), None).await;
                    if state["state"] == "done" || state["state"] == "failed" {
                        return state;
                    }
                }
                panic!("run {run} never finished");
            }
        };
        let state = finished(r["id"].as_str().unwrap().to_string()).await;
        assert_eq!(state["state"], "done", "{state}");
        let (_, all) = api.call("GET", "/api/v1/datasets", None).await;
        let shouted = all["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "shouted")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let sample = format!("/api/v1/datasets/{shouted}/sample");
        let before = chrono::Utc::now().to_rfc3339().replace('+', "%2B");
        let (_, v) = api.call("GET", &sample, None).await;
        assert_eq!(v["rows"].as_array().unwrap().len(), 2, "{v}");

        let (s, e) = api
            .call("POST", &format!("/api/v1/ontology/objects/{}/erase", o["id"].as_str().unwrap()), None)
            .await;
        assert_eq!(s, 200, "{e}");
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        pnex_backend::services::datasets::erasure::erase_pass(&ctx.db, &store)
            .await
            .unwrap();
        let queued = sql(&ctx.db, "SELECT id::text AS id FROM pipeline_jobs WHERE trigger = 'erasure'").await;
        assert_eq!(queued.len(), 1);
        let state = finished(queued[0].try_get("", "id").unwrap()).await;
        worker.kill().unwrap();
        worker.wait().unwrap();
        assert_eq!(state["state"], "done", "{state}");
        let (_, v) = api.call("GET", &sample, None).await;
        assert_eq!(v["rows"], json!([["BOB JONES"]]), "{v}");
        // The version that held her is gone, not only replaced.
        let (s, v) = api.call("GET", &format!("{sample}?as_of={before}"), None).await;
        assert_eq!(s, 410, "{v}");
    })
    .await;
}

fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|l| l.split_whitespace().next()?.parse::<u64>().ok())
        .unwrap()
        / 1024
}

/// The OP1.3 acceptance: all of `Partenaires`, every sheet active or not, accepted;
/// then the same file again as a new transaction writes nothing and an edit stays.
/// Expected counts come from `OP1_FC_ORGS` and `OP1_FC_LINKS`, computed apart.
#[tokio::test]
#[serial]
#[ignore = "needs the France Compétences files in OP1_FC_DIR"]
async fn all_of_the_france_competences_partners() {
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
    let want = |name: &str| std::env::var(name).expect(name).parse::<usize>().unwrap();
    let (orgs, links) = (want("OP1_FC_ORGS"), want("OP1_FC_LINKS"));
    with_app(|server, alice, ctx| async move {
        let org = server
            .get("/api/v1/user-info")
            .add_header("Authorization", format!("Bearer {alice}"))
            .await
            .json::<Value>()["orgs"][0]["id"]
            .as_i64()
            .unwrap();
        let api = Api { server: &server, token: &alice, org };
        let db = &ctx.db;
        api.call("POST", "/api/v1/ontology/packs/quali_watch/install", None).await;
        let worker = common::spawn_pipeline_worker();

        let sheets = api.dataset("sheets").await;
        api.load(&sheets, &file(&dir, "_Standard_")).await;
        let sheets_mapping = r#"
objects:
  - as: cert
    type: certification
    key: code
    properties:
      code: { from: Numero_Fiche, convert: [trim, upper] }
"#;
        let id = api.pipeline("sheets", "sheets", sheets_mapping, true).await;
        let r = api.run(&ctx, &id).await;
        assert_eq!(r["state"], "done", "{r}");

        let partners = api.dataset("partners").await;
        let csv = file(&dir, "_Partenaires_");
        api.load(&partners, &csv).await;
        let partners_mapping = r#"
objects:
  - as: org
    type: organization
    key: siren
    title: { from: Nom_Partenaire, convert: [trim] }
    properties:
      siren: { from: Siret_Partenaire, convert: [digits, "left:9", luhn], invalid: skip }
  - as: cert
    type: certification
    key: code
    create: false
    properties:
      code: { from: Numero_Fiche, convert: [trim, upper] }
links:
  - type: accredited_for
    from: org
    to: cert
    attributes:
      scope: { from: Habilitation_Partenaire, convert: [{ enum: { HABILITATION_ORGA_FORM: both, HABILITATION_FORMER: preparation, HABILITATION_ORGANISER: assessment } }] }
      declared_fc: { value: true }
      source: { value: france_competences }
    on_conflict: { scope: both }
"#;
        let id = api.pipeline("partners", "partners", partners_mapping, true).await;

        let start_rss = peak_rss_mb();
        let begin = std::time::Instant::now();
        let r = api.run(&ctx, &id).await;
        println!(
            "first run: {:?}, {:?}, peak memory {} → {} MB",
            begin.elapsed(), common::written(&r), start_rss, peak_rss_mb()
        );
        assert_eq!(r["state"], "done", "{r}");
        let count = |q: &'static str| async move { sql(db, q).await.len() };
        assert_eq!(
            count("SELECT 1 AS x FROM objects WHERE type_key = 'organization' AND valid_to IS NULL").await,
            orgs
        );
        assert_eq!(
            count("SELECT 1 AS x FROM resource_edges WHERE relation = 'accredited_for' AND valid_to IS NULL").await,
            links
        );

        // An edit, then a new version of the file with one row less.
        let row = &sql(db, "SELECT id::text AS id, version, properties FROM objects \
            WHERE type_key = 'organization' AND valid_to IS NULL LIMIT 1").await[0];
        let edited: String = row.try_get("", "id").unwrap();
        let version: i32 = row.try_get("", "version").unwrap();
        let properties: Value = row.try_get("", "properties").unwrap();
        let path = format!("/api/v1/ontology/objects/{edited}");
        let (s, e) = api
            .call("PUT", &path, Some(json!({ "title": "Edited by hand",
                "expected_version": version, "properties": properties })))
            .await;
        assert_eq!(s, 200, "{e}");
        let text = String::from_utf8_lossy(&csv).into_owned();
        let last = text.trim_end().rfind('\n').unwrap();
        let (fewer, _) = text.split_at(last + 1);
        api.load(&partners, fewer.as_bytes()).await;
        let begin = std::time::Instant::now();
        let r = api.run(&ctx, &id).await;
        worker.abort();
        println!(
            "one row less: {:?}, {:?}, peak memory {} MB",
            begin.elapsed(), common::written(&r), peak_rss_mb()
        );
        assert_eq!(r["state"], "done", "{r}");
        let c = &common::written(&r);
        assert_eq!((&c["created"], &c["updated"]), (&json!(0), &json!(0)), "{c}");
        assert!(c["closed"].as_u64().unwrap() <= 2, "{c}");
        let (_, o) = api.call("GET", &path, None).await;
        assert_eq!(o["title"], "Edited by hand", "{o}");
    })
    .await;
}

/// pipelines.md D1050, step 5: once the boot conversion emptied the table of
/// imports, migration 000173 drops it, and the conversion finds nothing to do.
#[tokio::test]
#[serial]
async fn imports_once_dropped_leave_nothing_to_convert() {
    with_app(|_server, _alice, ctx| async move {
        let db = &ctx.db;
        let left = sql(db, "SELECT to_regclass('object_imports') IS NULL AS gone").await;
        assert!(left[0].try_get::<bool>("", "gone").unwrap());
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        let n = pnex_backend::services::ontology::imports::migrate_to_pipelines(db, Some(&store))
            .await
            .unwrap();
        assert_eq!(n, 0);
    })
    .await;
}
