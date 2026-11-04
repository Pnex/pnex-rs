//! Polars side of the OP1.0 bench, same interface as `df`:
//!   pl convert <input.csv> <out.parquet>
//!   pl sql "<query>" <out.parquet> name=path...
//! Polars has no memory limit of its own: the cgroup is the only bound.
use polars::prelude::*;
use polars::sql::SQLContext;

type R<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn scan(path: &str) -> PolarsResult<LazyFrame> {
    if path.ends_with(".parquet") {
        return LazyFrame::scan_parquet(PlRefPath::new(path), ScanArgsParquet::default());
    }
    let sep = std::env::var("OP1_DELIM").ok().and_then(|d| d.bytes().next()).unwrap_or(b';');
    LazyCsvReader::new(PlRefPath::new(path)).with_separator(sep).with_has_header(true).finish()
}

fn write(lf: LazyFrame, out: &str) -> PolarsResult<()> {
    lf.sink(
        SinkDestination::File {
            target: SinkTarget::Path(PlRefPath::new(out)),
        },
        FileWriteFormat::Parquet(Arc::new(ParquetWriteOptions::default())),
        UnifiedSinkArgs::default(),
    )?
    .collect_with_engine(Engine::Streaming)?;
    Ok(())
}

fn main() -> R<()> {
    let args: Vec<String> = std::env::args().collect();
    match args[1].as_str() {
        "convert" => write(scan(&args[2])?, &args[3])?,
        "sql" => {
            let mut ctx = SQLContext::new();
            for pair in &args[4..] {
                let (name, path) = pair.split_once('=').ok_or("name=path expected")?;
                ctx.register(name, scan(path)?);
            }
            write(ctx.execute(&args[2])?, &args[3])?;
        }
        other => return Err(format!("unknown command {other}").into()),
    }
    Ok(())
}
