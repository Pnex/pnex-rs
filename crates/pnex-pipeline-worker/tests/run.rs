//! Runs of the worker against PostgreSQL and RustFS, as its own database role:
//! the `sql` steps a server run queues (refusals, org isolation, time limit)
//! and the queries of a person. Whole runs are the server's (D934).
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

/// A query of `sql` queued as the server would, run by the worker.
async fn query(
    e: &Env,
    org: i64,
    user: i64,
    sql: &str,
    tables: serde_json::Value,
    refused: serde_json::Value,
    objects: serde_json::Value,
) -> (String, serde_json::Value, Option<String>) {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO analysis_queries (id, org_id, user_id, sql, tables, refused, objects) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(org)
    .bind(user)
    .bind(sql)
    .bind(tables)
    .bind(refused)
    .bind(objects)
    .execute(&e.admin)
    .await
    .unwrap();
    assert!(
        e.worker.run_next().await.unwrap(),
        "the query is taken first"
    );
    let r = sqlx::query("SELECT state, result, code FROM analysis_queries WHERE id = $1")
        .bind(id)
        .fetch_one(&e.admin)
        .await
        .unwrap();
    (
        r.get("state"),
        r.get::<Option<serde_json::Value>, _>("result")
            .unwrap_or_default(),
        r.get("code"),
    )
}

#[tokio::test]
#[serial]
async fn a_query_reads_only_what_the_server_fixed() {
    let e = env().await;
    sqlx::query("DELETE FROM analysis_queries")
        .execute(&e.admin)
        .await
        .unwrap();
    let org = org(&e).await;
    let user: i64 = sqlx::query("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(format!("{}@example.org", Uuid::new_v4()))
        .fetch_one(&e.admin)
        .await
        .unwrap()
        .get("id");
    let staff = seed(
        &e,
        org,
        "staff",
        &["name", "salary"],
        &[&["Ada", "5000"], &["Bob", "4000"]],
    )
    .await;
    seed(&e, org, "secret", &["x"], &[&["1"]]).await;
    let tables = serde_json::json!({ "staff": staff.to_string() });

    let (state, result, _) = query(
        &e,
        org,
        user,
        "SELECT name FROM staff ORDER BY salary DESC",
        tables.clone(),
        serde_json::json!([]),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(state, "done");
    assert_eq!(result["columns"], serde_json::json!(["name"]));
    assert_eq!(result["rows"], serde_json::json!([["Ada"], ["Bob"]]));
    assert_eq!(result["truncated"], false);

    // Frozen objects read as `objects.<type>`.
    let key = format!("org_{org}/analysis/{}/employee.parquet", Uuid::new_v4());
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, true),
        Field::new("title", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec!["o1"])) as ArrayRef,
            Arc::new(StringArray::from(vec!["Ada"])) as ArrayRef,
        ],
    )
    .unwrap();
    let mut bytes = Vec::new();
    let mut w = ArrowWriter::try_new(&mut bytes, schema, None).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    e.store
        .put(&Path::from(key.as_str()), bytes.into())
        .await
        .unwrap();
    let (state, result, _) = query(
        &e,
        org,
        user,
        "SELECT o.title, s.salary FROM objects.employee o JOIN staff s ON s.name = o.title",
        tables.clone(),
        serde_json::json!([]),
        serde_json::json!({ "employee": key }),
    )
    .await;
    assert_eq!(state, "done", "{result}");
    assert_eq!(result["rows"], serde_json::json!([["Ada", "5000"]]));

    // Refused, unknown, written: a code each, nothing read.
    for (sql, code) in [
        ("SELECT * FROM secret", "marking-required"),
        ("SELECT * FROM missing_table", "analysis-table-unknown"),
        ("SELECT * FROM objects.nope", "analysis-table-unknown"),
        ("DELETE FROM staff", "analysis-sql-invalid"),
        ("SELECT 1; SELECT 2", "analysis-sql-invalid"),
    ] {
        let (state, _, got) = query(
            &e,
            org,
            user,
            sql,
            tables.clone(),
            serde_json::json!(["secret"]),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(
            (state.as_str(), got.as_deref()),
            ("failed", Some(code)),
            "{sql}"
        );
    }
}

/// Queues a `sql` step as a server run does, over tables of the org, and
/// works it. Returns state, code, args and the output key.
async fn step(
    e: &Env,
    org: i64,
    sql: &str,
    tables: serde_json::Value,
) -> (String, Option<String>, Option<serde_json::Value>, String) {
    let id = Uuid::new_v4();
    let key = format!("org_{org}/steps/{id}.parquet");
    sqlx::query(
        "INSERT INTO pipeline_step_jobs (id, org_id, sql, tables, output_key) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(org)
    .bind(sql)
    .bind(tables)
    .bind(&key)
    .execute(&e.admin)
    .await
    .unwrap();
    assert!(e.worker.run_next().await.unwrap(), "a step was queued");
    let row = sqlx::query("SELECT state, code, args FROM pipeline_step_jobs WHERE id = $1")
        .bind(id)
        .fetch_one(&e.admin)
        .await
        .unwrap();
    (row.get("state"), row.get("code"), row.get("args"), key)
}

/// A join over two tables of the org writes its rows; each refusal carries a
/// code and writes nothing.
#[tokio::test]
#[serial]
async fn a_step_joins_tables_and_refusals_write_nothing() {
    let e = env().await;
    let org = org(&e).await;
    seed_join(&e, org).await;
    let tables = serde_json::json!({ "p": "partners", "s": "sheets" });
    let join = "SELECT p.Numero_Fiche AS code FROM p JOIN s ON p.Numero_Fiche = s.Numero_Fiche \
                WHERE s.Actif = 'ACTIVE'";
    let (state, code, args, key) = step(&e, org, join, tables.clone()).await;
    assert_eq!(state, "done", "{code:?} {args:?}");
    assert!(e.store.head(&Path::from(key)).await.is_ok());
    for sql in [
        "SELECT nope FROM p",
        "SELECT * FROM read_csv('/etc/passwd')",
        "COPY p TO '/tmp/pnex-escaped.csv'",
    ] {
        let (state, code, _, key) = step(&e, org, sql, tables.clone()).await;
        assert_eq!(state, "failed", "{sql}");
        assert_eq!(code.as_deref(), Some("pipeline-sql-invalid"), "{sql}");
        assert!(e.store.head(&Path::from(key)).await.is_err(), "{sql}");
    }
    let (_, code, args, _) = step(&e, org, "SELECT 1", serde_json::json!({ "x": "nothing" })).await;
    assert_eq!(code.as_deref(), Some("pipeline-input-missing"));
    assert_eq!(args.unwrap()["dataset"], "nothing");
}

/// A cell that fails a cast is not repeated in the failure (R4).
#[tokio::test]
#[serial]
async fn a_failed_step_keeps_no_cell() {
    let e = env().await;
    let org = org(&e).await;
    seed_join(&e, org).await;
    let sql = "SELECT CAST(s.Actif AS BIGINT) AS n FROM s";
    let (state, code, args, _) = step(&e, org, sql, serde_json::json!({ "s": "sheets" })).await;
    assert_eq!(state, "failed");
    assert_eq!(code.as_deref(), Some("pipeline-sql-invalid"));
    assert!(!args.unwrap().to_string().contains("ACTIVE"));
}

/// R1: a table name resolves inside the org of the step, never in another one.
#[tokio::test]
#[serial]
async fn a_step_cannot_read_another_org() {
    let e = env().await;
    let (a, b) = (org(&e).await, org(&e).await);
    seed(&e, a, "secret", &["x"], &[&["1"]]).await;
    let (_, code, _, _) = step(
        &e,
        b,
        "SELECT * FROM s",
        serde_json::json!({ "s": "secret" }),
    )
    .await;
    assert_eq!(code.as_deref(), Some("pipeline-input-missing"));
}

/// A step past the worker's time limit stops with a code.
#[tokio::test]
#[serial]
async fn a_step_over_its_time_limit_stops() {
    let mut c = config();
    c.transform_timeout = Duration::from_nanos(1);
    let e = env().await;
    let worker = Worker::connect(c).await.unwrap();
    let e = Env { worker, ..e };
    let org = org(&e).await;
    seed_join(&e, org).await;
    let (state, code, _, _) = step(
        &e,
        org,
        "SELECT * FROM p",
        serde_json::json!({ "p": "partners" }),
    )
    .await;
    assert_eq!(
        (state.as_str(), code.as_deref()),
        ("failed", Some("pipeline-timeout"))
    );
}

/// A step whose worker died fails after its deadline instead of staying
/// running, so the server run waiting on it ends.
#[tokio::test]
#[serial]
async fn a_step_left_running_fails_after_its_deadline() {
    let e = env().await;
    let org = org(&e).await;
    let id: Uuid = sqlx::query(
        "INSERT INTO pipeline_step_jobs (id, org_id, sql, output_key, state, attempts, started_at) \
         VALUES (gen_random_uuid(), $1, 'SELECT 1', 'k', 'running', 1, now() - interval '16 minutes') \
         RETURNING id",
    )
    .bind(org)
    .fetch_one(&e.admin)
    .await
    .unwrap()
    .get("id");
    e.worker.run_next().await.unwrap();
    let row = sqlx::query("SELECT state, code FROM pipeline_step_jobs WHERE id = $1")
        .bind(id)
        .fetch_one(&e.admin)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("state"), "failed");
    assert_eq!(
        row.get::<Option<String>, _>("code").as_deref(),
        Some("pipeline-timeout")
    );
}
