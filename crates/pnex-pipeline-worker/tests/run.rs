//! Runs of the worker against PostgreSQL and RustFS, as its own database role:
//! a join between two datasets, lineage, skip, refusals, org isolation.
//!
//! Needs TEST_DATABASE_URL and the compose `rustfs` service.

use std::sync::Arc;
use std::time::Duration;

use datafusion::arrow::array::{ArrayRef, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::parquet::arrow::ArrowWriter;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::{path::Path, ObjectStoreExt};
use pnex_migration::{Migrator, MigratorTrait};
use pnex_pipeline_worker::engine::Limits;
use pnex_pipeline_worker::{Config, Worker};
use serial_test::serial;
use sqlx::{PgPool, Row};
use uuid::Uuid;

fn database_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://pnex:pnex@localhost:5432/pnex_test".to_string())
}

fn config() -> Config {
    Config {
        database_url: database_url(),
        database_role: Some("pnex_pipeline_worker".to_string()),
        s3_endpoint: "http://localhost:9000".to_string(),
        s3_bucket: "pnex-test".to_string(),
        s3_region: "us-east-1".to_string(),
        s3_access_key: std::env::var("PNEX_S3_ACCESS_KEY").unwrap_or("rustfsadmin".to_string()),
        s3_secret_key: std::env::var("PNEX_S3_SECRET_KEY").unwrap_or("rustfsadmin".to_string()),
        s3_path_style: true,
        limits: Limits {
            memory_bytes: 256 * 1024 * 1024,
            partitions: 2,
            spill_dir: std::env::temp_dir()
                .join(format!("pnex-worker-test-{}", std::process::id())),
            spill_max_bytes: 1024 * 1024 * 1024,
        },
        transform_timeout: Duration::from_secs(60),
        run_timeout: Duration::from_secs(300),
        pseudonym_key: Some(b"test-master-key-of-at-least-32-bytes!".to_vec()),
    }
}

struct Env {
    admin: PgPool,
    store: AmazonS3,
    worker: Worker,
}

async fn env() -> Env {
    let db = sea_orm_migration::sea_orm::Database::connect(database_url())
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    let admin = PgPool::connect(&database_url()).await.unwrap();
    // Runs other tests left behind would be taken first.
    sqlx::query("DELETE FROM pipeline_jobs")
        .execute(&admin)
        .await
        .unwrap();
    let c = config();
    let store = AmazonS3Builder::new()
        .with_bucket_name(&c.s3_bucket)
        .with_region(&c.s3_region)
        .with_endpoint(&c.s3_endpoint)
        .with_access_key_id(&c.s3_access_key)
        .with_secret_access_key(&c.s3_secret_key)
        .with_allow_http(true)
        .build()
        .unwrap();
    let worker = Worker::connect(c).await.unwrap();
    Env {
        admin,
        store,
        worker,
    }
}

async fn org(e: &Env) -> i64 {
    sqlx::query(
        "INSERT INTO organizations (name, created_at, updated_at) VALUES ($1, now(), now()) RETURNING id",
    )
    .bind(format!("worker-test-{}", Uuid::new_v4()))
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("id")
}

/// Commits `rows` of text columns as a snapshot of dataset `name`.
async fn seed(e: &Env, org: i64, name: &str, columns: &[&str], rows: &[&[&str]]) -> Uuid {
    let id: Uuid = sqlx::query(
        "INSERT INTO datasets (org_id, name) VALUES ($1, $2) \
         ON CONFLICT (org_id, name) DO UPDATE SET name = EXCLUDED.name RETURNING id",
    )
    .bind(org)
    .bind(name)
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("id");
    let schema = Arc::new(Schema::new(
        columns
            .iter()
            .map(|c| Field::new(*c, DataType::Utf8, true))
            .collect::<Vec<_>>(),
    ));
    let arrays: Vec<ArrayRef> = (0..columns.len())
        .map(|c| Arc::new(StringArray::from_iter_values(rows.iter().map(|r| r[c]))) as ArrayRef)
        .collect();
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    let mut bytes = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut bytes, schema, None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
    let tx: Uuid = sqlx::query(
        "INSERT INTO dataset_transactions (org_id, dataset_id, kind, state, seq, committed_at, rows, bytes) \
         VALUES ($1, $2, 'snapshot', 'committed', nextval('dataset_commit_seq'), now(), $3, $4) RETURNING id",
    )
    .bind(org)
    .bind(id)
    .bind(rows.len() as i64)
    .bind(bytes.len() as i64)
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("id");
    let key = format!("org_{org}/datasets/{id}/{tx}/part-0.parquet");
    sqlx::query(
        "INSERT INTO dataset_files (transaction_id, key, bytes, rows, sha256) VALUES ($1, $2, $3, $4, '')",
    )
    .bind(tx)
    .bind(&key)
    .bind(bytes.len() as i64)
    .bind(rows.len() as i64)
    .execute(&e.admin)
    .await
    .unwrap();
    e.store.put(&Path::from(key), bytes.into()).await.unwrap();
    id
}

async fn pipeline(e: &Env, org: i64, yaml: &str) -> Uuid {
    let id: Uuid = sqlx::query(
        "INSERT INTO pipelines (org_id, name, document) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(org)
    .bind(Uuid::new_v4().to_string())
    .bind(yaml)
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("id");
    sqlx::query(
        "INSERT INTO pipeline_versions (pipeline_id, version, document) VALUES ($1, 1, $2)",
    )
    .bind(id)
    .bind(yaml)
    .execute(&e.admin)
    .await
    .unwrap();
    id
}

/// Queues a run and works the queue. Returns state, code and args.
async fn run(
    e: &Env,
    org: i64,
    pipeline: Uuid,
) -> (String, Option<String>, Option<serde_json::Value>) {
    let job: Uuid = sqlx::query(
        "INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger) \
         VALUES ($1, $2, 1, 'manual') RETURNING id",
    )
    .bind(org)
    .bind(pipeline)
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("id");
    assert!(e.worker.run_next().await.unwrap(), "a run was queued");
    let row = sqlx::query("SELECT state, code, args FROM pipeline_jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&e.admin)
        .await
        .unwrap();
    (row.get("state"), row.get("code"), row.get("args"))
}

async fn transactions(e: &Env, org: i64, name: &str) -> Vec<(String, i64)> {
    sqlx::query(
        "SELECT t.state, t.rows FROM dataset_transactions t JOIN datasets d ON d.id = t.dataset_id \
         WHERE d.org_id = $1 AND d.name = $2 ORDER BY t.created_at",
    )
    .bind(org)
    .bind(name)
    .fetch_all(&e.admin)
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get("state"), r.get("rows")))
    .collect()
}

const JOIN: &str = r#"
transforms:
  - output: active_partners
    inputs: { p: partners, s: sheets }
    sql: |
      SELECT p.Numero_Fiche AS code, left(digits(p.Siret), 9) AS siren
      FROM p JOIN s ON p.Numero_Fiche = s.Numero_Fiche WHERE s.Actif = 'ACTIVE'
  - output: partner_count
    inputs: { a: active_partners }
    sql: SELECT a.code, count(*) AS n FROM a GROUP BY a.code
"#;

async fn seed_join(e: &Env, org: i64) {
    seed(
        e,
        org,
        "partners",
        &["Numero_Fiche", "Siret"],
        &[
            &["RNCP1", "\t73282932000074"],
            &["RNCP1", "\t55208131766522"],
            &["RNCP2", "\t44306184100047"],
        ],
    )
    .await;
    seed(
        e,
        org,
        "sheets",
        &["Numero_Fiche", "Actif"],
        &[&["RNCP1", "ACTIVE"], &["RNCP2", "INACTIVE"]],
    )
    .await;
}

#[tokio::test]
#[serial]
async fn a_join_writes_its_outputs_lineage_and_skips_what_did_not_change() {
    let e = env().await;
    let org = org(&e).await;
    seed_join(&e, org).await;
    let p = pipeline(&e, org, JOIN).await;

    let (state, code, args) = run(&e, org, p).await;
    assert_eq!(state, "done", "{code:?} {args:?}");
    assert_eq!(
        transactions(&e, org, "active_partners").await,
        [("committed".to_string(), 2)]
    );
    assert_eq!(
        transactions(&e, org, "partner_count").await,
        [("committed".to_string(), 1)]
    );

    // Lineage is what the run read, and types come from the engine.
    let links: i64 = sqlx::query(
        "SELECT count(*) AS n FROM dataset_transaction_inputs i \
         JOIN dataset_transactions t ON t.id = i.transaction_id \
         JOIN datasets d ON d.id = t.dataset_id WHERE d.org_id = $1",
    )
    .bind(org)
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("n");
    assert_eq!(links, 3);
    let schema: serde_json::Value =
        sqlx::query("SELECT schema FROM datasets WHERE org_id = $1 AND name = 'partner_count'")
            .bind(org)
            .fetch_one(&e.admin)
            .await
            .unwrap()
            .get("schema");
    assert_eq!(schema[0]["data_type"], "text");
    assert_eq!(schema[1]["data_type"], "int64");

    // Nothing changed: no new transaction.
    assert_eq!(run(&e, org, p).await.0, "done");
    assert_eq!(transactions(&e, org, "active_partners").await.len(), 1);

    // An input changed: both outputs follow, in order.
    seed(
        &e,
        org,
        "sheets",
        &["Numero_Fiche", "Actif"],
        &[&["RNCP1", "ACTIVE"], &["RNCP2", "ACTIVE"]],
    )
    .await;
    assert_eq!(run(&e, org, p).await.0, "done");
    let rows: Vec<i64> = transactions(&e, org, "active_partners")
        .await
        .into_iter()
        .map(|t| t.1)
        .collect();
    assert_eq!(rows, [2, 3]);
    assert_eq!(transactions(&e, org, "partner_count").await[1].1, 2);
}

/// An output declared for erasure stays clean when an input no one declared brings
/// the person back; the run keeps no cell of a failure.
#[tokio::test]
#[serial]
async fn what_a_run_writes_leaves_out_the_erased() {
    let e = env().await;
    let org = org(&e).await;
    seed_join(&e, org).await;
    let p = pipeline(&e, org, JOIN).await;
    let (state, code, args) = run(&e, org, p).await;
    assert_eq!(state, "done", "{code:?} {args:?}");
    let dataset: Uuid =
        sqlx::query("SELECT id FROM datasets WHERE org_id = $1 AND name = 'active_partners'")
            .bind(org)
            .fetch_one(&e.admin)
            .await
            .unwrap()
            .get("id");
    sqlx::query(
        "INSERT INTO dataset_erasures (org_id, dataset_id, column_name, value, type_key, done_at) \
         VALUES ($1, $2, 'siren', $3, 't', now())",
    )
    .bind(org)
    .bind(dataset)
    .bind(pnex_core::datasets::fingerprint("732829320").unwrap())
    .execute(&e.admin)
    .await
    .unwrap();
    // The input changes and still holds the row.
    seed_join(&e, org).await;
    let (state, code, args) = run(&e, org, p).await;
    assert_eq!(state, "done", "{code:?} {args:?}");
    let versions = transactions(&e, org, "active_partners").await;
    assert_eq!(versions.last().unwrap().1, 1, "{versions:?}");

    // A cast of a cell fails while it runs: the message is not kept.
    let bad = r#"
transforms:
  - output: bad_cast
    inputs: { s: sheets }
    sql: SELECT CAST(s.Actif AS BIGINT) AS n FROM s
"#;
    let p = pipeline(&e, org, bad).await;
    let (state, code, args) = run(&e, org, p).await;
    assert_eq!(state, "failed");
    assert_eq!(code.as_deref(), Some("pipeline-sql-invalid"));
    assert!(!args.unwrap().to_string().contains("ACTIVE"));
}

/// The org that waited most goes first, whatever one org has queued.
#[tokio::test]
#[serial]
async fn one_org_does_not_hold_the_queue() {
    let e = env().await;
    let busy = org(&e).await;
    seed_join(&e, busy).await;
    let first = pipeline(&e, busy, JOIN).await;
    let (state, ..) = run(&e, busy, first).await;
    assert_eq!(state, "done");
    let second = pipeline(
        &e,
        busy,
        &JOIN
            .replace("active_partners", "other_output")
            .replace("partner_count", "other_count"),
    )
    .await;
    let queued = |org: i64, p: Uuid| {
        let admin = e.admin.clone();
        async move {
            sqlx::query("INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger) VALUES ($1, $2, 1, 'manual') RETURNING id")
                .bind(org)
                .bind(p)
                .fetch_one(&admin)
                .await
                .unwrap()
                .get::<Uuid, _>("id")
        }
    };
    let busy_job = queued(busy, second).await;
    let quiet = crate::org(&e).await;
    seed_join(&e, quiet).await;
    let theirs = pipeline(&e, quiet, JOIN).await;
    let quiet_job = queued(quiet, theirs).await;
    assert!(e.worker.run_next().await.unwrap());
    let state = |job: Uuid| {
        let admin = e.admin.clone();
        async move {
            sqlx::query("SELECT state FROM pipeline_jobs WHERE id = $1")
                .bind(job)
                .fetch_one(&admin)
                .await
                .unwrap()
                .get::<String, _>("state")
        }
    };
    assert_eq!(state(quiet_job).await, "done");
    assert_eq!(state(busy_job).await, "queued");
    assert!(e.worker.run_next().await.unwrap());
}

/// A run whose worker died is tried once more; one that took two workers down
/// fails, instead of taking each new worker down with it.
#[tokio::test]
#[serial]
async fn a_run_that_killed_its_worker_twice_is_not_tried_again() {
    let e = env().await;
    let org = org(&e).await;
    seed_join(&e, org).await;
    let p = pipeline(&e, org, JOIN).await;
    let dead = |attempts: i32| {
        let admin = e.admin.clone();
        async move {
            sqlx::query(
                "INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger, state, \
                 attempts, heartbeat_at, started_at) \
                 VALUES ($1, $2, 1, 'manual', 'running', $3, now() - interval '12 minutes', now()) \
                 RETURNING id",
            )
            .bind(org)
            .bind(p)
            .bind(attempts)
            .fetch_one(&admin)
            .await
            .unwrap()
            .get::<Uuid, _>("id")
        }
    };
    let twice = dead(2).await;
    e.worker.run_next().await.unwrap();
    let row = sqlx::query("SELECT state, code FROM pipeline_jobs WHERE id = $1")
        .bind(twice)
        .fetch_one(&e.admin)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "failed");
    assert_eq!(
        row.get::<Option<String>, _>("code").as_deref(),
        Some("pipeline-worker-lost")
    );

    // The dead run left its output transaction open, as a killed worker does.
    sqlx::query(
        "WITH d AS (INSERT INTO datasets (org_id, name) VALUES ($1, 'active_partners') \
           ON CONFLICT (org_id, name) DO UPDATE SET name = EXCLUDED.name RETURNING id) \
         INSERT INTO dataset_transactions (org_id, dataset_id, kind, heartbeat_at) \
         SELECT $1, id, 'snapshot', now() - interval '11 minutes' FROM d",
    )
    .bind(org)
    .execute(&e.admin)
    .await
    .unwrap();
    let once = dead(1).await;
    assert!(e.worker.run_next().await.unwrap(), "the run is tried again");
    let state: String = sqlx::query("SELECT state FROM pipeline_jobs WHERE id = $1")
        .bind(once)
        .fetch_one(&e.admin)
        .await
        .unwrap()
        .get("state");
    assert_eq!(state, "done");
}

/// The table of an import is not an output: it feeds a sync run with its author's role.
#[tokio::test]
#[serial]
async fn a_run_cannot_write_the_table_of_an_import() {
    let e = env().await;
    let org = org(&e).await;
    seed_join(&e, org).await;
    let import: Uuid = sqlx::query(
        "INSERT INTO object_imports (id, org_id, name, mapping_yaml) \
         VALUES (gen_random_uuid(), $1, 'x', 'x') RETURNING id",
    )
    .bind(org)
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("id");
    sqlx::query(
        "INSERT INTO datasets (org_id, name, import_id, retain_days) \
         VALUES ($1, 'active_partners', $2, 30)",
    )
    .bind(org)
    .bind(import)
    .execute(&e.admin)
    .await
    .unwrap();
    let p = pipeline(&e, org, JOIN).await;
    let (state, code, _) = run(&e, org, p).await;
    assert_eq!(state, "failed");
    assert_eq!(code.as_deref(), Some("dataset-reserved"));
    assert!(transactions(&e, org, "active_partners").await.is_empty());
    let kept: Option<i32> = sqlx::query(
        "SELECT retain_days FROM datasets WHERE org_id = $1 AND name = 'active_partners'",
    )
    .bind(org)
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("retain_days");
    assert_eq!(
        kept,
        Some(30),
        "a refused run leaves the import's table as it is"
    );
}

/// A pending erasure holds the org's runs back, and a purged output is computed
/// again though its inputs did not change.
#[tokio::test]
#[serial]
async fn an_erasure_holds_runs_and_a_purged_output_is_computed_again() {
    let e = env().await;
    let org = org(&e).await;
    seed_join(&e, org).await;
    let p = pipeline(&e, org, JOIN).await;
    let (state, code, args) = run(&e, org, p).await;
    assert_eq!(state, "done", "{code:?} {args:?}");

    let dataset: Uuid =
        sqlx::query("SELECT id FROM datasets WHERE org_id = $1 AND name = 'active_partners'")
            .bind(org)
            .fetch_one(&e.admin)
            .await
            .unwrap()
            .get("id");
    sqlx::query(
        "INSERT INTO dataset_erasures (org_id, dataset_id, column_name, value, type_key) \
         VALUES ($1, $2, 'c', 'v', 't')",
    )
    .bind(org)
    .bind(dataset)
    .execute(&e.admin)
    .await
    .unwrap();
    sqlx::query("INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger) VALUES ($1, $2, 1, 'erasure')")
        .bind(org)
        .bind(p)
        .execute(&e.admin)
        .await
        .unwrap();
    assert!(
        !e.worker.run_next().await.unwrap(),
        "a run started over a pending erasure"
    );

    sqlx::query("UPDATE dataset_erasures SET done_at = now() WHERE org_id = $1")
        .bind(org)
        .execute(&e.admin)
        .await
        .unwrap();
    sqlx::query("UPDATE dataset_transactions SET purged_at = now() WHERE dataset_id = $1")
        .bind(dataset)
        .execute(&e.admin)
        .await
        .unwrap();
    assert!(e.worker.run_next().await.unwrap());
    assert_eq!(transactions(&e, org, "active_partners").await.len(), 2);
    // What reads it follows, since its input is a new transaction.
    assert_eq!(transactions(&e, org, "partner_count").await.len(), 2);
}

#[tokio::test]
#[serial]
async fn refusals_carry_a_code_and_commit_nothing() {
    let e = env().await;
    let org = org(&e).await;
    seed_join(&e, org).await;
    let job = |output: &str, sql: &str| {
        format!("transforms:\n  - output: {output}\n    inputs: {{ p: partners }}\n    sql: \"{sql}\"\n")
    };
    for (output, sql, want) in [
        ("o1", "SELECT nope FROM p", "pipeline-sql-invalid"),
        (
            "o2",
            "SELECT * FROM read_csv('/etc/passwd')",
            "pipeline-sql-invalid",
        ),
        (
            "o3",
            "COPY p TO '/tmp/pnex-escaped.csv'",
            "pipeline-sql-invalid",
        ),
        (
            "o4",
            "SELECT p.Siret, q.Siret FROM p JOIN p AS q ON true",
            "pipeline-sql-invalid",
        ),
    ] {
        let id = pipeline(&e, org, &job(output, sql)).await;
        let (state, code, _) = run(&e, org, id).await;
        assert_eq!(state, "failed", "{sql}");
        assert_eq!(code.as_deref(), Some(want), "{sql}");
        let visible: Vec<_> = transactions(&e, org, output)
            .await
            .into_iter()
            .filter(|t| t.0 == "committed")
            .collect();
        assert!(visible.is_empty(), "{sql}");
    }
    let missing = "transforms:\n  - output: o5\n    inputs: { x: nothing }\n    sql: SELECT 1\n";
    let id = pipeline(&e, org, missing).await;
    let (_, code, args) = run(&e, org, id).await;
    assert_eq!(code.as_deref(), Some("pipeline-input-missing"));
    assert_eq!(args.unwrap()["dataset"], "nothing");
}

/// R1: a name resolves inside the org of the job, never in another one.
#[tokio::test]
#[serial]
async fn a_run_cannot_read_another_org() {
    let e = env().await;
    let (a, b) = (org(&e).await, org(&e).await);
    seed(&e, a, "secret", &["x"], &[&["1"]]).await;
    let yaml =
        "transforms:\n  - output: stolen\n    inputs: { s: secret }\n    sql: SELECT * FROM s\n";
    let id = pipeline(&e, b, yaml).await;
    let (_, code, _) = run(&e, b, id).await;
    assert_eq!(code.as_deref(), Some("pipeline-input-missing"));
}

/// D573: the role reaches the catalogue and the queue, and nothing else.
#[tokio::test]
#[serial]
async fn the_worker_role_sees_no_business_table() {
    let e = env().await;
    let options: sqlx::postgres::PgConnectOptions = database_url().parse().unwrap();
    let pool = PgPool::connect_with(options.options([("role", "pnex_pipeline_worker")]))
        .await
        .unwrap();
    for query in [
        "SELECT 1 FROM users LIMIT 1",
        "SELECT 1 FROM organizations LIMIT 1",
        "SELECT 1 FROM objects LIMIT 1",
        "SELECT 1 FROM org_secrets LIMIT 1",
    ] {
        let denied = sqlx::query(query).fetch_optional(&pool).await;
        let e = denied.expect_err(query).to_string();
        assert!(e.contains("permission denied"), "{query}: {e}");
    }
    sqlx::query("SELECT 1 FROM datasets LIMIT 1")
        .fetch_optional(&pool)
        .await
        .unwrap();
    let _ = e;
}

#[tokio::test]
#[serial]
async fn a_query_over_its_time_limit_stops() {
    let mut c = config();
    c.transform_timeout = Duration::from_nanos(1);
    let e = env().await;
    let worker = Worker::connect(c).await.unwrap();
    let e = Env { worker, ..e };
    let org = org(&e).await;
    seed_join(&e, org).await;
    let id = pipeline(
        &e,
        org,
        "transforms:\n  - output: slow\n    inputs: { p: partners }\n    sql: SELECT * FROM p\n",
    )
    .await;
    let (state, code, _) = run(&e, org, id).await;
    assert_eq!(state, "failed");
    assert_eq!(code.as_deref(), Some("pipeline-timeout"));
}

/// D576: a check that fails leaves the previous version current and says why; one
/// that passes is kept on the version.
#[tokio::test]
#[serial]
async fn a_failed_check_keeps_the_current_version() {
    let e = env().await;
    let org = org(&e).await;
    seed(
        &e,
        org,
        "raw",
        &["k", "v"],
        &[&["a", "1"], &["b", "2"], &["c", "3"], &["d", "4"]],
    )
    .await;
    let yaml = |checks: &str, filter: &str| {
        format!(
            "transforms:\n  - output: clean\n    inputs: {{ r: raw }}\n    sql: {filter}\n    checks: {checks}\n"
        )
    };
    let all = "SELECT k, v FROM r";
    let id = pipeline(
        &e,
        org,
        &yaml("{ min_rows: 1, unique: [k], max_empty_pct: { v: 0 } }", all),
    )
    .await;
    let (state, code, _) = run(&e, org, id).await;
    assert_eq!((state.as_str(), code), ("done", None));
    let clean: Uuid = sqlx::query("SELECT id FROM datasets WHERE org_id = $1 AND name = 'clean'")
        .bind(org)
        .fetch_one(&e.admin)
        .await
        .unwrap()
        .get("id");
    let last = || async {
        let r = sqlx::query(
            "SELECT state, code, checks FROM dataset_transactions WHERE dataset_id = $1 \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(clean)
        .fetch_one(&e.admin)
        .await
        .unwrap();
        (
            r.get::<String, _>("state"),
            r.get::<Option<String>, _>("code"),
            r.get::<serde_json::Value, _>("checks"),
        )
    };
    let (state, _, checks) = last().await;
    assert_eq!(state, "committed");
    assert_eq!(checks.as_array().unwrap().len(), 3, "{checks}");
    assert!(
        checks
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["passed"] == true),
        "{checks}"
    );

    // Each check alone fails with what it measured; the committed version stays.
    let doubled = "SELECT k, v FROM r UNION ALL SELECT k, v FROM r";
    let blanks = "SELECT k, CASE WHEN k = ''a'' THEN v ELSE '' '' END AS v FROM r";
    let renamed = "SELECT k, v AS other FROM r";
    let few = "SELECT k, v FROM r WHERE k = ''a''";
    for (checks, sql, check, value) in [
        ("{ min_rows: 5 }", all, "min_rows", "4"),
        ("{ max_rows: 3 }", all, "max_rows", "4"),
        ("{ unique: [k] }", doubled, "unique", "4"),
        ("{ unique: [gone] }", all, "unique", "-1"),
        (
            "{ max_empty_pct: { v: 50 } }",
            blanks,
            "max_empty_pct",
            "75",
        ),
        (
            "{ max_empty_pct: { gone: 100 } }",
            all,
            "max_empty_pct",
            "100",
        ),
        ("{ max_drop_pct: 50 }", few, "max_drop_pct", "75"),
        (
            "{ same_columns: true }",
            renamed,
            "same_columns",
            "k, other",
        ),
    ] {
        let quoted = format!("'{sql}'");
        sqlx::query("UPDATE pipelines SET version = version + 1, document = $2 WHERE id = $1")
            .bind(id)
            .bind(yaml(checks, &quoted))
            .execute(&e.admin)
            .await
            .unwrap();
        let version: i32 = sqlx::query(
            "INSERT INTO pipeline_versions (pipeline_id, version, document) \
             SELECT id, version, document FROM pipelines WHERE id = $1 RETURNING version",
        )
        .bind(id)
        .fetch_one(&e.admin)
        .await
        .unwrap()
        .get("version");
        let job: Uuid = sqlx::query(
            "INSERT INTO pipeline_jobs (org_id, pipeline_id, version, trigger) \
             VALUES ($1, $2, $3, 'manual') RETURNING id",
        )
        .bind(org)
        .bind(id)
        .bind(version)
        .fetch_one(&e.admin)
        .await
        .unwrap()
        .get("id");
        assert!(e.worker.run_next().await.unwrap());
        let r = sqlx::query("SELECT state, code, args FROM pipeline_jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&e.admin)
            .await
            .unwrap();
        assert_eq!(r.get::<String, _>("state"), "failed", "{checks}");
        assert_eq!(
            r.get::<Option<String>, _>("code").as_deref(),
            Some("pipeline-check-failed"),
            "{checks}"
        );
        let args: serde_json::Value = r.get("args");
        assert_eq!(args["check"], check, "{args}");
        assert_eq!(args["value"], value, "{args}");
        let (state, code, kept) = last().await;
        assert_eq!(state, "aborted", "{checks}");
        assert_eq!(code.as_deref(), Some("pipeline-check-failed"));
        assert_eq!(kept[0]["passed"], false, "{kept}");
        let visible: Vec<_> = transactions(&e, org, "clean")
            .await
            .into_iter()
            .filter(|t| t.0 == "committed")
            .collect();
        assert_eq!(visible, [("committed".to_string(), 4)], "{checks}");
    }
}
