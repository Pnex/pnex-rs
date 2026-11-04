//! Sync of a dataset into the ontology (data-pipelines.md D577, §9.7 ter): the
//! delta is by planned entity, so an edit survives until the source changes that
//! value, and a row gone does not close what its sibling rows still hold.
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

    /// Polls until the run leaves `running`, within `secs`.
    async fn settle(&self, import: &str, run: &str, secs: u32) -> Value {
        let path = format!("/api/v1/ontology/imports/{import}/runs/{run}");
        for _ in 0..secs * 10 {
            let (s, r) = self.call("GET", &path, None).await;
            // The answer of a busy database, during the bulk write of a whole file.
            if s == 503 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            assert_eq!(s, 200, "{r}");
            if r["run"]["state"] != "running" {
                return r;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("run {run} never settled");
    }

    async fn run(&self, import: &str, secs: u32) -> Value {
        let (s, r) = self
            .call(
                "POST",
                &format!("/api/v1/ontology/imports/{import}/runs"),
                None,
            )
            .await;
        assert_eq!(s, 202, "{r}");
        self.settle(import, r["id"].as_str().unwrap(), secs).await
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

        let dataset = api.dataset("partners").await;
        api.load_partners(&dataset, V1).await;
        // A mapping that belongs to another source, a dataset of nobody: refused.
        let input = |source_url: &str, mapping: &str| {
            json!({ "name": "partners", "mapping": mapping, "source": "dataset",
                    "source_url": source_url, "mode": "snapshot" })
        };
        let imports = "/api/v1/ontology/imports";
        let other = uuid::Uuid::new_v4().to_string();
        let (s, e) = api.call("POST", imports, Some(input(&other, MAPPING))).await;
        assert_eq!(s, 400, "{e}");
        assert_eq!(e["errors"]["fields"]["source_url"], "invalid", "{e}");
        let stepped = format!("{MAPPING}steps:\n  - {{ step: file_download }}\n");
        let (s, e) = api.call("POST", imports, Some(input(&dataset, &stepped))).await;
        assert_eq!(s, 400, "{e}");

        let (s, imp) = api.call("POST", imports, Some(input(&dataset, MAPPING))).await;
        assert_eq!(s, 201, "{imp}");
        let id = imp["id"].as_str().unwrap().to_string();
        let acme = "732829320";
        let beta = "552100554";

        // First run: no reference, it applies. Sibling rows merge into one link.
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(scopes(db).await, [format!("{beta}=preparation"), format!("{acme}=both")]);
        let (org_id, version, nda, title) = org_of(db, acme).await.unwrap();
        assert_eq!((nda.as_deref(), title.as_str()), (Some("11111111111"), "Acme"));

        // Same transaction: nothing to do.
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
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
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let (_, _, nda, title) = org_of(db, acme).await.unwrap();
        assert_eq!((nda.as_deref(), title.as_str()), (Some("99999999999"), "Acme edited"));

        // Another entity changes: only it moves, the edit stays.
        let v2 = String::from_utf8_lossy(V1).replace("22222222222", "33333333333");
        api.load_partners(&dataset, v2.as_bytes()).await;
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(org_of(db, beta).await.unwrap().2.as_deref(), Some("33333333333"));
        assert_eq!(org_of(db, acme).await.unwrap().2.as_deref(), Some("99999999999"));

        // The source changes the edited value: the last change wins, the title stays.
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n\
73282932000074;Acme;44444444444;RS1;F\n\
73282932000082;Acme;44444444444;RS1;A\n\
55210055400013;Beta;33333333333;RS1;F\n").await;
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        let (_, _, nda, title) = org_of(db, acme).await.unwrap();
        assert_eq!((nda.as_deref(), title.as_str()), (Some("44444444444"), "Acme edited"));

        // A sibling row goes: the object and the link stay, the scope follows.
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n\
73282932000074;Acme;44444444444;RS1;F\n\
55210055400013;Beta;33333333333;RS1;F\n\
44306184100047;Gamma;55555555555;RS1;F\n").await;
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
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
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert!(org_of(db, "443061841").await.is_none());

        // All the rows of an entity go: it closes. The mapping stays valid.
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n\
73282932000074;Acme;44444444444;RS1;F\n\
44306184100047;Gamma;55555555555;RS1;F\n").await;
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert!(org_of(db, beta).await.is_none());
        // Compared for real this time, and the archived one is still not back.
        assert!(org_of(db, "443061841").await.is_none());

        // Most of what it holds gone at once: refused, nothing written.
        api.load(&dataset, b"siret;name;nda;code;scope\n73282932000074;Acme;44444444444;RS1;F\n").await;
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "refused", "{r}");
        assert_eq!(r["run"]["state_code"], "ingest-suspicious-closures", "{r}");
        let fillers = sql(db, "SELECT 1 AS x FROM objects WHERE type_key = 'organization' \
            AND valid_to IS NULL AND properties ->> 'siren' LIKE '1000%'").await;
        assert_eq!(fillers.len(), 30);
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n73282932000074;Acme;44444444444;RS1;F\n44306184100047;Gamma;55555555555;RS1;F\n").await;

        // Mapping changed: no reference, a run stops at a preview showing what it
        // overwrites, and a preview whose source moved is stale.
        let (_, current) = api.call("GET", &format!("{imports}/{id}"), None).await;
        let (s, updated) = api
            .call(
                "PUT",
                &format!("{imports}/{id}"),
                Some(json!({ "name": "partners", "source": "dataset", "mode": "snapshot",
                    "mapping": format!("{MAPPING}description: changed\n"),
                    "expected_version": current["version"] })),
            )
            .await;
        assert_eq!(s, 200, "{updated}");
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "preview", "{r}");
        api.load_partners(&dataset, b"siret;name;nda;code;scope\n73282932000074;Acme;44444444444;RS1;F\n44306184100047;Gamma;55555555555;RS1;A\n").await;
        let apply = format!("{imports}/{id}/apply");
        let body = json!({ "run_id": r["run"]["id"], "expected_version": updated["version"] });
        let (s, e) = api.call("POST", &apply, Some(body)).await;
        assert_eq!(s, 409, "{e}");
        assert_eq!(e["error"], "import-run-stale", "{e}");
        let (s, p) = api.call("POST", &format!("{imports}/{id}/dry-run"), None).await;
        assert_eq!(s, 202, "{p}");
        let r = api.settle(&id, p["id"].as_str().unwrap(), 30).await;
        assert_eq!(r["run"]["state"], "preview", "{r}");
        let body = json!({ "run_id": r["run"]["id"], "expected_version": updated["version"] });
        let (s, p) = api.call("POST", &apply, Some(body)).await;
        assert_eq!(s, 202, "{p}");
        let r = api.settle(&id, r["run"]["id"].as_str().unwrap(), 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");

        // The dataset is gone: the run says so.
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        pnex_backend::services::datasets::delete(db, &store, org, dataset.parse().unwrap())
            .await
            .unwrap();
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "failed", "{r}");
        assert_eq!(r["run"]["state_code"], "dataset-not-found", "{r}");
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
        let (s, imp) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(json!({ "name": "partners", "mapping": mapping, "source": "dataset",
                             "source_url": dataset, "mode": "incremental" })),
            )
            .await;
        assert_eq!(s, 201, "{imp}");
        let id = imp["id"].as_str().unwrap().to_string();
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
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
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
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
        let r = api.run(&id, 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");
        assert_eq!(org_of(db, "732829320").await.unwrap().2.as_deref(), Some("99999999999"));

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

/// A table anyone with `datasets:write` can load does not write with the rights of
/// the import's author; the table of an import's steps cannot be loaded at all.
#[tokio::test]
#[serial]
async fn a_table_does_not_lend_the_rights_of_the_author() {
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
                Some(json!({ "def": {
                    "key": "vault", "name": "Vault", "write_role": "admin",
                    "properties": [{ "key": "code", "name": "Code", "kind": "text", "unique": true }]
                } })),
            )
            .await;
        assert_eq!(s, 201, "{e}");
        let dataset = api.dataset("vaults").await;
        api.load(&dataset, b"code\nA1\n").await;
        let mapping = "key: vaults\nname: Vaults\nobjects:\n  - as: v\n    type: vault\n    key: code\n    properties:\n      code: { from: code }\n";
        let (s, imp) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(json!({ "name": "vaults", "mapping": mapping, "source": "dataset",
                             "source_url": dataset, "mode": "incremental" })),
            )
            .await;
        assert_eq!(s, 201, "{imp}");
        // A sensitive value does not enter a table any viewer can read.
        let (s, e) = api
            .call(
                "POST",
                "/api/v1/ontology/types",
                Some(json!({ "def": {
                    "key": "patient", "name": "Patient",
                    "properties": [{ "key": "code", "name": "Code", "kind": "text", "unique": true,
                                     "classification": "sensitive" }]
                } })),
            )
            .await;
        assert_eq!(s, 201, "{e}");
        let (s, e) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(json!({ "name": "patients", "source": "dataset", "source_url": dataset,
                    "mode": "incremental",
                    "mapping": mapping.replace("type: vault", "type: patient") })),
            )
            .await;
        assert_eq!(s, 400, "{e}");
        assert_eq!(e["errors"]["fields"]["mapping"], "invalid", "{e}");

        // Alice owns the org: as its author, her role would be enough.
        let r = api.run(imp["id"].as_str().unwrap(), 30).await;
        assert_eq!(r["run"]["state"], "failed", "{r}");
        assert_eq!(r["run"]["state_code"], "ontology-write-forbidden", "{r}");

        // Owned by an import: no load, whoever asks.
        let id: uuid::Uuid = imp["id"].as_str().unwrap().parse().unwrap();
        ctx.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE datasets SET import_id = $1 WHERE id = $2::uuid",
                [id.into(), dataset.clone().into()],
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
            .call("PUT", &format!("/api/v1/datasets/{dataset}/identity"), Some(json!({ "columns": [] })))
            .await;
        assert_eq!(s, 409, "{e}");
        let (s, e) = api.call("DELETE", &format!("/api/v1/datasets/{dataset}"), None).await;
        assert_eq!(s, 409, "{e}");
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
        let document = "transforms:\n  - output: shouted\n    inputs: { p: people }\n    sql: SELECT upper(p.name) AS who FROM p\n";
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
        let finished = |run: String| {
            let (api, runs) = (&api, &runs);
            async move {
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

/// Erasing a person reaches the rows of a dataset through the identity columns its
/// mapping declares by itself.
#[tokio::test]
#[serial]
async fn the_mapping_declares_the_columns_erasure_reaches() {
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
        let (s, e) = api
            .call(
                "POST",
                "/api/v1/ontology/types",
                Some(json!({ "def": {
                    "key": "member",
                    "name": "Member",
                    "title_classification": "personal",
                    "properties": [{ "key": "email", "name": "Email", "kind": "text",
                                     "unique": true, "classification": "personal" }]
                } })),
            )
            .await;
        assert_eq!(s, 201, "{e}");
        let dataset = api.dataset("people").await;
        api.load(
            &dataset,
            b"name;mail\nAnn Smith;ann@example.com\nBob Jones;bob@example.com\n",
        )
        .await;
        let mapping = r#"
key: people
name: People
objects:
  - as: p
    type: member
    key: email
    title: { from: name, convert: [trim] }
    properties:
      email: { from: mail, convert: [trim] }
"#;
        let (s, imp) = api
            .call(
                "POST",
                "/api/v1/ontology/imports",
                Some(
                    json!({ "name": "people", "mapping": mapping, "source": "dataset",
                             "source_url": dataset, "mode": "incremental" }),
                ),
            )
            .await;
        assert_eq!(s, 201, "{imp}");
        let (_, d) = api
            .call("GET", &format!("/api/v1/datasets/{dataset}"), None)
            .await;
        assert_eq!(d["identity"].as_array().unwrap().len(), 2, "{d}");
        assert_eq!(d["identity"][0]["column"], "mail", "{d}");
        assert_eq!(d["identity"][1]["property"], "$title", "{d}");
        let r = api.run(imp["id"].as_str().unwrap(), 30).await;
        assert_eq!(r["run"]["state"], "applied", "{r}");

        api.load(&dataset, b"name;mail\nAnn Smith;ann@example.com\nBob Jones;bob@example.com\nCarl Fox;carl@example.com\n").await;
        let (s, p) = api
            .call("POST", &format!("/api/v1/ontology/imports/{}/dry-run", imp["id"].as_str().unwrap()), None)
            .await;
        assert_eq!(s, 202, "{p}");
        let held = api.settle(imp["id"].as_str().unwrap(), p["id"].as_str().unwrap(), 30).await;
        assert_eq!(held["run"]["state"], "preview", "{held}");
        assert!(held.to_string().contains("carl@example.com"), "{held}");

        let ann = sql(
            &ctx.db,
            "SELECT id::text AS id FROM objects WHERE title = 'Ann Smith' AND type_key = 'member'",
        )
        .await;
        let ann: String = ann[0].try_get("", "id").unwrap();
        let (s, e) = api
            .call(
                "POST",
                &format!("/api/v1/ontology/objects/{ann}/erase"),
                None,
            )
            .await;
        assert_eq!(s, 200, "{e}");
        // A preview kept for review held her too: it is gone with the erasure.
        let r2 = api.call("GET", &format!("/api/v1/ontology/imports/{}/runs/{}", imp["id"].as_str().unwrap(), held["run"]["id"].as_str().unwrap()), None).await.1;
        assert_eq!(r2["run"]["state"], "stale", "{r2}");
        assert!(!r2.to_string().contains("carl@example.com"), "{r2}");
        let store = pnex_backend::services::datasets::store_of(&ctx.config).unwrap();
        pnex_backend::services::datasets::erasure::erase_pass(&ctx.db, &store)
            .await
            .unwrap();
        let (_, v) = api
            .call("GET", &format!("/api/v1/datasets/{dataset}/sample"), None)
            .await;
        assert_eq!(
            v["rows"],
            json!([["Bob Jones", "bob@example.com"], ["Carl Fox", "carl@example.com"]])
        );
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
        let import = |name: &str, dataset: &str, mapping: &str| {
            json!({ "name": name, "mapping": mapping, "source": "dataset",
                    "source_url": dataset, "mode": "snapshot" })
        };
        let imports = "/api/v1/ontology/imports";

        let sheets = api.dataset("sheets").await;
        api.load(&sheets, &file(&dir, "_Standard_")).await;
        let (s, imp) = api
            .call(
                "POST",
                imports,
                Some(import(
                    "sheets",
                    &sheets,
                    r#"
key: sheets
name: Sheets
objects:
  - as: cert
    type: certification
    key: code
    properties:
      code: { from: Numero_Fiche, convert: [trim, upper] }
"#,
                )),
            )
            .await;
        assert_eq!(s, 201, "{imp}");
        let r = api.run(imp["id"].as_str().unwrap(), 600).await;
        assert_eq!(r["run"]["state"], "applied", "{}", r["run"]);

        let partners = api.dataset("partners").await;
        let csv = file(&dir, "_Partenaires_");
        api.load(&partners, &csv).await;
        let (s, imp) = api
            .call(
                "POST",
                imports,
                Some(import(
                    "partners",
                    &partners,
                    r#"
key: partners
name: Partners
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
"#,
                )),
            )
            .await;
        assert_eq!(s, 201, "{imp}");
        let id = imp["id"].as_str().unwrap();

        let start_rss = peak_rss_mb();
        let begin = std::time::Instant::now();
        let r = api.run(id, 3600).await;
        println!(
            "first sync: {:?}, {:?}, peak memory {} → {} MB",
            begin.elapsed(), r["run"]["counts"], start_rss, peak_rss_mb()
        );
        assert_eq!(r["run"]["state"], "applied", "{}", r["run"]);
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
        let r = api.run(id, 3600).await;
        println!(
            "one row less: {:?}, {:?}, peak memory {} MB",
            begin.elapsed(), r["run"]["counts"], peak_rss_mb()
        );
        assert_eq!(r["run"]["state"], "applied", "{}", r["run"]);
        let c = &r["run"]["counts"];
        assert_eq!((&c["created"], &c["updated"]), (&json!(0), &json!(0)), "{c}");
        assert!(c["closed"].as_u64().unwrap() <= 2, "{c}");
        let (_, o) = api.call("GET", &path, None).await;
        assert_eq!(o["title"], "Edited by hand", "{o}");
    })
    .await;
}
