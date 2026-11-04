//! DataFusion side of the OP1.0 bench.
//!   df convert <input.csv> <out.parquet>
//!   df sql "<query>" <out.parquet> name=path...   (csv or parquet, local or s3://)
//!   df put <local> s3://bucket/key
//! Env: OP1_MEM_MB (spill threshold), OP1_CONF, OP1_INFER, OP1_POOL, OP1_DELIM, S3_ENDPOINT, S3_KEY, S3_SECRET.
use datafusion::dataframe::DataFrameWriteOptions;
use datafusion::execution::SessionStateBuilder;
use datafusion::execution::memory_pool::{FairSpillPool, TrackConsumersPool};
use datafusion::execution::runtime_env::RuntimeEnvBuilder;
use datafusion::prelude::*;
use object_store::aws::AmazonS3Builder;
use std::sync::Arc;

type R<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn delim() -> u8 {
    std::env::var("OP1_DELIM").ok().and_then(|d| d.bytes().next()).unwrap_or(b';')
}

fn s3(bucket: &str) -> R<object_store::aws::AmazonS3> {
    Ok(AmazonS3Builder::new()
        .with_bucket_name(bucket)
        .with_region("us-east-1")
        .with_endpoint(std::env::var("S3_ENDPOINT")?)
        .with_access_key_id(std::env::var("S3_KEY")?)
        .with_secret_access_key(std::env::var("S3_SECRET")?)
        .with_allow_http(true)
        .build()?)
}

fn context() -> R<SessionContext> {
    let mb: usize = std::env::var("OP1_MEM_MB").map_or(Ok(1024), |v| v.parse())?;
    let bytes = mb * 1024 * 1024;
    let builder = RuntimeEnvBuilder::new();
    // OP1_POOL=fair: spillable consumers share the pool evenly instead of first come first served.
    let runtime = if std::env::var("OP1_POOL").as_deref() == Ok("fair") {
        let pool = TrackConsumersPool::new(FairSpillPool::new(bytes), std::num::NonZeroUsize::new(5).unwrap());
        builder.with_memory_pool(Arc::new(pool)).build_arc()?
    } else {
        builder.with_memory_limit(bytes, 1.0).build_arc()?
    };
    let mut config = SessionConfig::new().with_target_partitions(4);
    // OP1_CONF="key=value,key=value": engine settings under test (join strategy, spill).
    for pair in std::env::var("OP1_CONF").unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').ok_or("key=value expected")?;
        config = config.set_str(k, v);
    }
    let state = SessionStateBuilder::new()
        .with_config(config)
        .with_runtime_env(runtime)
        .with_default_features()
        .build();
    Ok(SessionContext::new_with_state(state))
}

async fn register(ctx: &SessionContext, name: &str, path: &str) -> R<()> {
    if let Some(rest) = path.strip_prefix("s3://") {
        let bucket = rest.split('/').next().unwrap_or_default();
        let url = url::Url::parse(&format!("s3://{bucket}"))?;
        ctx.register_object_store(&url, Arc::new(s3(bucket)?));
    }
    if path.ends_with(".parquet") {
        ctx.register_parquet(name, path, ParquetReadOptions::default()).await?;
    } else {
        let d = delim();
        let mut opts = CsvReadOptions::new().delimiter(d).has_header(true).newlines_in_values(true);
        // OP1_INFER=0: no type inference, every column is read as text.
        if let Some(n) = std::env::var("OP1_INFER").ok().and_then(|v| v.parse().ok()) {
            opts = opts.schema_infer_max_records(n);
        }
        ctx.register_csv(name, path, opts).await?;
    }
    Ok(())
}

async fn write(df: DataFrame, out: &str) -> R<u64> {
    let batches = df
        .write_parquet(out, DataFrameWriteOptions::new().with_single_file_output(true), None)
        .await?;
    // The sink returns one row holding the number of rows written.
    let col = batches[0].column(0);
    let n = col.as_any().downcast_ref::<datafusion::arrow::array::UInt64Array>();
    Ok(n.map_or(0, |a| a.value(0)))
}

#[tokio::main]
async fn main() -> R<()> {
    let args: Vec<String> = std::env::args().collect();
    match args[1].as_str() {
        "convert" => {
            let ctx = context()?;
            register(&ctx, "t", &args[2]).await?;
            let n = write(ctx.table("t").await?, &args[3]).await?;
            println!("rows={n}");
        }
        "sql" => {
            let ctx = context()?;
            for pair in &args[4..] {
                let (name, path) = pair.split_once('=').ok_or("name=path expected")?;
                register(&ctx, name, path).await?;
            }
            // The D572 barrier: refusal is decided on the logical plan, not the text.
            let guard = SQLOptions::new()
                .with_allow_ddl(false)
                .with_allow_dml(false)
                .with_allow_statements(false);
            let df = ctx.sql_with_options(&args[2], guard).await?;
            println!("rows={}", write(df, &args[3]).await?);
        }
        "put" => {
            use object_store::{ObjectStoreExt, WriteMultipart, path::Path};
            let rest = args[3].strip_prefix("s3://").ok_or("s3:// expected")?;
            let (bucket, key) = rest.split_once('/').ok_or("bucket/key expected")?;
            let store = s3(bucket)?;
            let mut w = WriteMultipart::new(store.put_multipart(&Path::from(key)).await?);
            w.write(&std::fs::read(&args[2])?);
            w.finish().await?;
        }
        other => return Err(format!("unknown command {other}").into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use object_store::delimited::newline_delimited_stream;

    async fn split(chunks: &[&str]) -> Vec<String> {
        let input: Vec<object_store::Result<bytes::Bytes>> =
            chunks.iter().map(|c| Ok(bytes::Bytes::from(c.to_string()))).collect();
        let s = newline_delimited_stream(futures::stream::iter(input));
        s.map(|c| String::from_utf8(c.unwrap().to_vec()).unwrap()).collect().await
    }

    /// One chunk ends inside a quoted value, an earlier record holds a quoted newline.
    /// object_store 0.13.2 (scans back with forward quote state) cuts inside the value,
    /// which breaks CSV schema inference; 0.14 fixes it (issue #650). Expected to fail
    /// until DataFusion moves to 0.14: then drop `should_panic` and OP1_INFER=0.
    #[tokio::test]
    #[should_panic]
    async fn splitter_keeps_quoted_newline_when_chunk_ends_inside_quotes() {
        let out = split(&["\"1\";\"a\nb\";\"z\"\n\"2\";\"c", "d\";\"z\"\n"]).await;
        assert_eq!(out[0], "\"1\";\"a\nb\";\"z\"\n", "chunks: {out:?}");
    }

    /// Control: same bytes, chunk boundary outside quotes.
    #[tokio::test]
    async fn splitter_is_right_when_chunk_ends_outside_quotes() {
        let out = split(&["\"1\";\"a\nb\";\"z\"\n", "\"2\";\"cd\";\"z\"\n"]).await;
        assert_eq!(out[0], "\"1\";\"a\nb\";\"z\"\n", "chunks: {out:?}");
    }
}
