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

/// A session with no table, ready for the inputs of one transform.
pub fn session(limits: &Limits) -> DfResult<SessionContext> {
    let pool = TrackConsumersPool::new(
        FairSpillPool::new(limits.memory_bytes),
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
        .with_target_partitions(limits.partitions)
        .with_information_schema(false);
    let options = config.options_mut();
    // A sort-merge join sorts both sides: joining 105 rows to 44 M sorted the 44 M
    // and the pod died outside the pool (dev, 2026-11-06). A hash join builds the
    // small side; one too big for the pool ends in a clean memory error.
    options.optimizer.prefer_hash_join = true;
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
        let ctx = session(&limits).unwrap();
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
}
