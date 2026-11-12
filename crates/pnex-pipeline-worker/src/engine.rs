//! The DataFusion session of a transform (data-pipelines.md D572). User SQL is a
//! hostile input: a session holds only the declared inputs, refuses anything but
//! a query on the logical plan, and offers functions from an allow list.
//!
//! Settings come from the OP1.0 measure (§9.7 bis): the default greedy pool fails
//! as soon as two operators spill, and a hash join never spills.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, BooleanArray, StringArray};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::DataType;
use datafusion::error::{DataFusionError, Result as DfResult};
use datafusion::execution::disk_manager::{DiskManagerBuilder, DiskManagerMode};
use datafusion::execution::memory_pool::{FairSpillPool, TrackConsumersPool};
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::execution::SessionStateBuilder;
use datafusion::logical_expr::{create_udf, ColumnarValue, Volatility};
use datafusion::prelude::{SQLOptions, SessionConfig, SessionContext};

/// Canonical names; their aliases come along (`length`, `substring`, `trim`).
/// Nothing here can make an unbounded allocation from an argument: `repeat`,
/// `lpad` and `rpad` stay out on purpose, as does anything about the host.
const SCALAR_FUNCTIONS: &[&str] = &[
    "abs",
    "ascii",
    "btrim",
    "ceil",
    "character_length",
    "coalesce",
    "concat",
    "concat_ws",
    // Bars of N minutes from a finer history (markets-research.md D823).
    "date_bin",
    "date_part",
    "date_trunc",
    "ends_with",
    "floor",
    "greatest",
    "initcap",
    "least",
    "left",
    "lower",
    "ltrim",
    "md5",
    "nullif",
    "nvl",
    "octet_length",
    "power",
    "regexp_like",
    "regexp_match",
    "regexp_replace",
    "replace",
    "reverse",
    "right",
    "round",
    "rtrim",
    "sha256",
    "split_part",
    "starts_with",
    "strpos",
    "substr",
    "to_char",
    "to_date",
    "to_timestamp",
    "translate",
    "trunc",
    "upper",
];
const AGGREGATE_FUNCTIONS: &[&str] = &[
    "approx_distinct",
    "approx_percentile_cont",
    "avg",
    "count",
    "first_value",
    "last_value",
    "max",
    "median",
    "min",
    "stddev",
    "sum",
    "var",
];
const WINDOW_FUNCTIONS: &[&str] = &[
    "dense_rank",
    "first_value",
    "lag",
    "last_value",
    "lead",
    "ntile",
    "rank",
    "row_number",
];

/// Bounds of a session.
#[derive(Clone, Debug)]
pub struct Limits {
    pub memory_bytes: usize,
    pub partitions: usize,
    pub spill_dir: PathBuf,
    pub spill_max_bytes: u64,
}

/// The plan may only read: no DDL, no DML, no session statement.
pub fn read_only() -> SQLOptions {
    SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(false)
        .with_allow_statements(false)
}

fn text_arg(args: &[ColumnarValue]) -> DfResult<ArrayRef> {
    let arrays = ColumnarValue::values_to_arrays(args)?;
    let first = arrays
        .first()
        .ok_or_else(|| DataFusionError::Plan("one argument expected".into()))?;
    Ok(cast(first, &DataType::Utf8)?)
}

fn as_text(a: &ArrayRef) -> DfResult<&StringArray> {
    a.as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| DataFusionError::Plan("text expected".into()))
}

/// `digits(text)`: the D301 conversion, keeps the ASCII digits.
fn digits(args: &[ColumnarValue]) -> DfResult<ColumnarValue> {
    let input = text_arg(args)?;
    let out: StringArray = as_text(&input)?
        .iter()
        .map(|v| v.map(|s| s.chars().filter(char::is_ascii_digit).collect::<String>()))
        .collect();
    Ok(ColumnarValue::Array(Arc::new(out)))
}

/// `luhn(text)`: the D301 check of a SIREN or a SIRET, from the same code.
fn luhn(args: &[ColumnarValue]) -> DfResult<ColumnarValue> {
    let input = text_arg(args)?;
    let out: BooleanArray = as_text(&input)?
        .iter()
        .map(|v| v.map(pnex_core::ontology::import::luhn))
        .collect();
    Ok(ColumnarValue::Array(Arc::new(out)))
}

/// Key of `pseudonym()` for one org (governance.md D805): derived from the worker's
/// master key and the org, never stored, the same for every run of that org.
pub fn org_key(master: &[u8], org_id: i64) -> [u8; 32] {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac = <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(master)
        .unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"));
    mac.update(format!("pseudonym:{org_id}").as_bytes());
    mac.finalize().into_bytes().into()
}

/// `pseudonym(text)`: a stable stand-in for a value, the same within an org so that
/// joins still match, telling nothing of the value without the key (D805).
pub fn register_pseudonym(ctx: &SessionContext, key: [u8; 32]) {
    let f = move |args: &[ColumnarValue]| -> DfResult<ColumnarValue> {
        use hmac::{Hmac, KeyInit, Mac};
        let input = text_arg(args)?;
        let out: StringArray = as_text(&input)?
            .iter()
            .map(|v| {
                v.map(|v| {
                    let mut mac = <Hmac<sha2::Sha256> as KeyInit>::new_from_slice(&key)
                        .unwrap_or_else(|_| unreachable!("HMAC takes a key of any length"));
                    mac.update(v.as_bytes());
                    mac.finalize().into_bytes()[..16]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                })
            })
            .collect();
        Ok(ColumnarValue::Array(Arc::new(out)))
    };
    ctx.register_udf(create_udf(
        "pseudonym",
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(f),
    ));
}

/// How a session joins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joins {
    /// Builds the small side in memory: fast, never spills.
    Hash,
    /// Sorts both sides, spilling to disk: for two sides too big for the pool
    /// (C6). Half the pool: the engine also allocates outside its pool while
    /// sorting (§9.7 bis, cause 4), and a pod killed there loses the whole
    /// run. The partitions stay: with one, the planner joins by hash anyway.
    SortMerge,
}

/// True when a memory error comes from the hash table of a join, the one
/// case a sort-merge join can get through.
pub fn hash_join_too_big(e: &DataFusionError) -> bool {
    matches!(e.find_root(), DataFusionError::ResourcesExhausted(m) if m.contains("HashJoinInput"))
}

/// A session with no table, ready for the inputs of one transform.
pub fn session(limits: &Limits, joins: Joins) -> DfResult<SessionContext> {
    let (memory, partitions) = match joins {
        Joins::Hash => (limits.memory_bytes, limits.partitions),
        Joins::SortMerge => (limits.memory_bytes / 2, limits.partitions.max(2)),
    };
    let pool = TrackConsumersPool::new(
        FairSpillPool::new(memory),
        std::num::NonZeroUsize::new(5).expect("not zero"),
    );
    let disk = DiskManagerBuilder::default()
        .with_mode(DiskManagerMode::Directories(vec![limits.spill_dir.clone()]))
        .with_max_temp_directory_size(limits.spill_max_bytes);
    let runtime = RuntimeEnvBuilder::new()
        .with_memory_pool(Arc::new(pool))
        .with_disk_manager_builder(disk)
        .build_arc()?;
    let mut config = SessionConfig::new()
        .with_target_partitions(partitions)
        // Merging spilled runs holds one batch per run: smaller batches, more
        // runs merged within the pool.
        .with_batch_size(match joins {
            Joins::Hash => 8192,
            Joins::SortMerge => 1024,
        })
        .with_information_schema(false);
    let options = config.options_mut();
    // A sort-merge join sorts both sides: joining 105 rows to 44 M sorted the 44 M
    // and the pod died outside the pool (dev, 2026-11-06). A hash join builds the
    // small side; one too big for the pool ends in a clean memory error.
    options.optimizer.prefer_hash_join = joins == Joins::Hash;
    if joins == Joins::SortMerge {
        // Each sort holds this much back for its merge (10 MB by default): with
        // two sides per partition it would take the pool before any row.
        options.execution.sort_spill_reservation_bytes =
            (memory / (partitions * 8)).clamp(256 * 1024, 10 * 1024 * 1024);
    }
    // A column keeps the case of its file: `p.Numero_Fiche` means what it says.
    options.sql_parser.enable_ident_normalization = false;
    // An endless recursive query would hold the job until its timeout.
    options.execution.enable_recursive_ctes = false;

    let allowed = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<HashSet<_>>();
    let scalars = allowed(SCALAR_FUNCTIONS);
    let aggregates = allowed(AGGREGATE_FUNCTIONS);
    let windows = allowed(WINDOW_FUNCTIONS);
    let mut builder = SessionStateBuilder::new()
        .with_config(config)
        .with_runtime_env(runtime)
        .with_default_features()
        // No way to reach a file or to make rows from nothing.
        .with_table_factories(Default::default())
        .with_table_function_list(Vec::new());
    if let Some(all) = builder.scalar_functions() {
        all.retain(|f| scalars.contains(f.name()));
    }
    if let Some(all) = builder.aggregate_functions() {
        all.retain(|f| aggregates.contains(f.name()));
    }
    if let Some(all) = builder.window_functions() {
        all.retain(|f| windows.contains(f.name()));
    }
    if let Some(all) = builder.higher_order_functions() {
        all.clear();
    }
    let state = builder.build();
    let ctx = SessionContext::new_with_state(state);
    ctx.register_udf(create_udf(
        "digits",
        vec![DataType::Utf8],
        DataType::Utf8,
        Volatility::Immutable,
        Arc::new(digits),
    ));
    ctx.register_udf(create_udf(
        "luhn",
        vec![DataType::Utf8],
        DataType::Boolean,
        Volatility::Immutable,
        Arc::new(luhn),
    ));
    for f in crate::tz::udfs() {
        ctx.register_udf(f);
    }
    Ok(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::RecordBatch;
    use datafusion::arrow::datatypes::{Field, Schema};
    use datafusion::datasource::MemTable;

    async fn with_table() -> SessionContext {
        let limits = Limits {
            memory_bytes: 64 * 1024 * 1024,
            partitions: 2,
            spill_dir: std::env::temp_dir(),
            spill_max_bytes: 1024 * 1024 * 1024,
        };
        let ctx = session(&limits, Joins::Hash).unwrap();
        let schema = Arc::new(Schema::new(vec![Field::new("Code", DataType::Utf8, true)]));
        let values: ArrayRef = Arc::new(StringArray::from(vec![" 732 829 320 ", "x"]));
        let batch = RecordBatch::try_new(schema.clone(), vec![values]).unwrap();
        let table = MemTable::try_new(schema, vec![vec![batch]]).unwrap();
        ctx.register_table("t", Arc::new(table)).unwrap();
        ctx
    }

    async fn run(ctx: &SessionContext, sql: &str) -> DfResult<Vec<RecordBatch>> {
        ctx.sql_with_options(sql, read_only())
            .await?
            .collect()
            .await
    }

    /// C6: two sides too big for the pool. The hash join stops on its table;
    /// sorting both sides on disk gets through.
    #[tokio::test]
    async fn a_join_too_big_for_the_pool_ends_when_sorted() {
        let rows = 600_000usize;
        let spill = std::env::temp_dir().join(format!("pnex-c6-{}", std::process::id()));
        std::fs::create_dir_all(&spill).unwrap();
        let limits = Limits {
            memory_bytes: 48 * 1024 * 1024,
            partitions: 2,
            spill_dir: spill.clone(),
            spill_max_bytes: 2 * 1024 * 1024 * 1024,
        };
        let side = |name: &str| {
            let schema = Arc::new(Schema::new(vec![
                Field::new("k", DataType::Utf8, false),
                Field::new(name, DataType::Utf8, false),
            ]));
            let keys: ArrayRef = Arc::new(StringArray::from_iter_values(
                (0..rows).map(|i| format!("{:014}", (i * 7919) % rows)),
            ));
            let values: ArrayRef = Arc::new(StringArray::from_iter_values(
                (0..rows).map(|i| format!("{name} value number {i:>12}")),
            ));
            let batch = RecordBatch::try_new(schema.clone(), vec![keys, values]).unwrap();
            let parts = (0..8)
                .map(|p| vec![batch.slice(p * rows / 8, rows / 8)])
                .collect();
            Arc::new(MemTable::try_new(schema, parts).unwrap())
        };
        let sql = "SELECT a.k, a.a, b.b FROM a JOIN b ON a.k = b.k";
        let mut outcome = Vec::new();
        for joins in [Joins::Hash, Joins::SortMerge] {
            let ctx = session(&limits, joins).unwrap();
            ctx.register_table("a", side("a")).unwrap();
            ctx.register_table("b", side("b")).unwrap();
            outcome.push(run(&ctx, sql).await);
        }
        let _ = std::fs::remove_dir_all(&spill);
        let e = outcome[0]
            .as_ref()
            .expect_err("the hash table fits: grow the sides");
        assert!(hash_join_too_big(e), "{e}");
        let batches = outcome[1].as_ref().expect("sorted join");
        assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), rows);
    }

    /// A time column fixed in a pipeline (D939): minutes stored 5 h ahead of
    /// New York all year moved to their real UTC instant.
    #[tokio::test]
    async fn time_zones_convert_in_sql() {
        let ctx = with_table().await;
        let sql = "SELECT CAST(local_to_utc(CAST('2015-07-10T21:59:00' AS TIMESTAMP) - INTERVAL '5 hours', 'America/New_York') AS VARCHAR) AS a, \
                   CAST(utc_to_local(CAST('2015-01-09T13:30:00' AS TIMESTAMP), 'America/New_York') AS VARCHAR) AS b";
        let batches = run(&ctx, sql).await.unwrap();
        let col = |i: usize| {
            let c = cast(batches[0].column(i), &DataType::Utf8).unwrap();
            c.as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0)
                .to_string()
        };
        assert!(col(0).starts_with("2015-07-10T20:59:00"), "{}", col(0));
        assert_eq!(col(1), "2015-01-09T08:30:00");
        let e = run(
            &ctx,
            "SELECT local_to_utc(CAST('2015-07-10T21:59:00' AS TIMESTAMP), 'EST+5')",
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("unknown time zone"), "{e}");
    }

    /// Every way out of the declared inputs must be refused (D572).
    #[tokio::test]
    async fn hostile_statements_are_refused() {
        let ctx = with_table().await;
        for sql in [
            "CREATE EXTERNAL TABLE x STORED AS CSV LOCATION '/etc/passwd'",
            "CREATE TABLE x AS SELECT 1",
            "CREATE VIEW v AS SELECT 1",
            "DROP TABLE t",
            "INSERT INTO t VALUES ('x')",
            "COPY t TO '/tmp/pnex-escaped.csv'",
            "COPY (SELECT * FROM t) TO '/tmp/pnex-escaped.csv' STORED AS CSV",
            "SET datafusion.execution.target_partitions = 1",
            "SELECT * FROM '/etc/passwd'",
            "SELECT * FROM read_csv('/etc/passwd')",
            "SELECT * FROM read_parquet('/etc/passwd')",
            "SELECT * FROM generate_series(1, 10000000000000)",
            "SELECT * FROM information_schema.tables",
            "SHOW TABLES",
            "SELECT repeat('x', 1000000000000)",
            "SELECT lpad('x', 1000000000000)",
            "SELECT version()",
            "WITH RECURSIVE r AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM r) SELECT * FROM r",
            "SELECT 1; SELECT 2",
        ] {
            assert!(run(&ctx, sql).await.is_err(), "not refused: {sql}");
        }
        assert!(!std::path::Path::new("/tmp/pnex-escaped.csv").exists());
    }

    #[tokio::test]
    async fn the_allowed_sql_works_and_keeps_the_case_of_columns() {
        let ctx = with_table().await;
        let out = run(
            &ctx,
            "SELECT left(digits(t.Code), 9) AS siren, luhn(digits(t.Code)) AS ok, count(*) AS n, \
             row_number() OVER (ORDER BY t.Code) AS r, length(t.Code) AS l, \
             substring(t.Code, 1, 2) AS s, trim(t.Code) AS tr, char_length(t.Code) AS c \
             FROM t WHERE upper(t.Code) IS NOT NULL GROUP BY t.Code",
        )
        .await
        .unwrap();
        assert_eq!(out.iter().map(|b| b.num_rows()).sum::<usize>(), 2);
    }

    /// D823: 1-minute bars become 1-hour and 5-minute bars in SQL alone.
    #[tokio::test]
    async fn minute_bars_are_resampled() {
        use datafusion::arrow::array::{Decimal128Array, TimestampMicrosecondArray};
        use datafusion::arrow::datatypes::TimeUnit;
        let limits = Limits {
            memory_bytes: 64 * 1024 * 1024,
            partitions: 2,
            spill_dir: std::env::temp_dir(),
            spill_max_bytes: 1024 * 1024 * 1024,
        };
        let ctx = session(&limits, Joins::Hash).unwrap();
        let dec = DataType::Decimal128(38, 8);
        let schema = Arc::new(Schema::new(vec![
            Field::new("instrument", DataType::Utf8, false),
            Field::new(
                "ts",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                false,
            ),
            Field::new("open", dec.clone(), false),
            Field::new("high", dec.clone(), false),
            Field::new("low", dec.clone(), false),
            Field::new("close", dec.clone(), false),
            Field::new("volume", dec, false),
        ]));
        // 10:58, 10:59, 11:00, 11:07 of 2025-01-01, minutes in µs.
        let at = |h: i64, m: i64| (1_735_725_600 + (h - 10) * 3600 + m * 60) * 1_000_000;
        let ts = vec![at(10, 58), at(10, 59), at(11, 0), at(11, 7)];
        let d = |v: Vec<i128>| -> ArrayRef {
            Arc::new(
                Decimal128Array::from(v.into_iter().map(|x| x * 100_000_000).collect::<Vec<_>>())
                    .with_precision_and_scale(38, 8)
                    .unwrap(),
            )
        };
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["BTCUSDT"; 4])),
                Arc::new(TimestampMicrosecondArray::from(ts).with_timezone("UTC")),
                d(vec![10, 11, 20, 21]),
                d(vec![12, 13, 25, 22]),
                d(vec![9, 10, 19, 18]),
                d(vec![11, 12, 21, 20]),
                d(vec![1, 2, 3, 4]),
            ],
        )
        .unwrap();
        ctx.register_table(
            "m1",
            Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap()),
        )
        .unwrap();
        let bars = |bucket: &str| {
            format!(
                "SELECT instrument, bucket AS ts, first_value(open ORDER BY ts) AS open, \
                 max(high) AS high, min(low) AS low, last_value(close ORDER BY ts) AS close, \
                 sum(volume) AS volume \
                 FROM (SELECT *, {bucket} AS bucket FROM m1) b \
                 GROUP BY instrument, bucket ORDER BY ts"
            )
        };
        let text = |out: &[RecordBatch]| {
            datafusion::arrow::util::pretty::pretty_format_batches(out)
                .unwrap()
                .to_string()
        };
        let h1 = run(&ctx, &bars("date_trunc('hour', ts)")).await.unwrap();
        let h1 = text(&h1);
        assert!(
            h1.contains("| 10.00000000 | 13.00000000 | 9.00000000  | 12.00000000 | 3.00000000"),
            "{h1}"
        );
        assert!(
            h1.contains("| 20.00000000 | 25.00000000 | 18.00000000 | 20.00000000 | 7.00000000"),
            "{h1}"
        );
        let m5 = run(
            &ctx,
            &bars("date_bin(INTERVAL '5 minutes', ts, TIMESTAMP '1970-01-01T00:00:00Z')"),
        )
        .await
        .unwrap();
        assert_eq!(
            m5.iter().map(|b| b.num_rows()).sum::<usize>(),
            3,
            "{}",
            text(&m5)
        );
    }

    /// A few rows joined to a big table build a hash table of the few rows:
    /// never a sort of the big table.
    #[tokio::test]
    async fn a_small_table_joined_to_a_big_one_sorts_nothing() {
        let ctx = with_table().await;
        let schema = Arc::new(Schema::new(vec![Field::new("Code", DataType::Utf8, true)]));
        let many: Vec<String> = (0..200_000).map(|i| i.to_string()).collect();
        let values: ArrayRef = Arc::new(StringArray::from(many));
        let batch = RecordBatch::try_new(schema.clone(), vec![values]).unwrap();
        let big = MemTable::try_new(schema, vec![vec![batch]]).unwrap();
        ctx.register_table("big", Arc::new(big)).unwrap();
        let plan = run(
            &ctx,
            "EXPLAIN SELECT big.Code FROM t JOIN big ON big.Code = t.Code",
        )
        .await
        .unwrap();
        let text = datafusion::arrow::util::pretty::pretty_format_batches(&plan)
            .unwrap()
            .to_string();
        assert!(text.contains("HashJoinExec: mode=CollectLeft"), "{text}");
        assert!(!text.contains("SortExec"), "{text}");
    }

    /// The filter compares canonical names: an alias here, or a typo, would
    /// silently narrow the SQL.
    #[test]
    fn the_allow_lists_name_real_functions() {
        let missing = |list: &[&'static str], names: HashSet<String>| -> Vec<&'static str> {
            list.iter()
                .copied()
                .filter(|n| !names.contains(*n))
                .collect()
        };
        let scalars = datafusion::functions::all_default_functions();
        let scalars = scalars.iter().map(|f| f.name().to_string()).collect();
        assert_eq!(missing(SCALAR_FUNCTIONS, scalars), Vec::<&str>::new());
        let aggregates = datafusion::functions_aggregate::all_default_aggregate_functions();
        let aggregates = aggregates.iter().map(|f| f.name().to_string()).collect();
        assert_eq!(missing(AGGREGATE_FUNCTIONS, aggregates), Vec::<&str>::new());
        let windows = datafusion::functions_window::all_default_window_functions();
        let windows = windows.iter().map(|f| f.name().to_string()).collect();
        assert_eq!(missing(WINDOW_FUNCTIONS, windows), Vec::<&str>::new());
    }

    #[tokio::test]
    async fn a_pseudonym_is_stable_in_an_org_and_differs_across_orgs() {
        let master = b"test-master-key-of-at-least-32-bytes!";
        let one =
            |org: i64| async move {
                let ctx = with_table().await;
                register_pseudonym(&ctx, org_key(master, org));
                let batches = ctx
                .sql("SELECT pseudonym('ada@example.org') AS a, pseudonym('ada@example.org') AS b, \
                      pseudonym('bob@example.org') AS c, pseudonym(NULL) AS n")
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
                let col = |i: usize| {
                    let a = batches[0]
                        .column(i)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .unwrap();
                    (!a.is_null(0)).then(|| a.value(0).to_string())
                };
                (col(0), col(1), col(2), col(3))
            };
        let (a, b, c, n) = one(1).await;
        assert_eq!(a, b, "stable: joins still match");
        assert_ne!(a, c);
        assert_eq!(a.as_ref().map(String::len), Some(32));
        assert!(!a.as_ref().unwrap().contains("ada"));
        assert_eq!(n, None, "NULL stays NULL");
        let (other, ..) = one(2).await;
        assert_ne!(a, other, "another org, another stand-in");
        // Without the key the function does not exist: the transform fails.
        let ctx = with_table().await;
        assert!(ctx.sql("SELECT pseudonym('x')").await.is_err());
    }
}
