//! Parquet on the object store (data-pipelines.md D569): a sink that streams a
//! file through a multipart upload while hashing it, and a reader by byte range.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::{BooleanArray, RecordBatch};
use arrow_cast::cast::{can_cast_types, cast};
use arrow_cast::display::{ArrayFormatter, FormatOptions};
use arrow_schema::DataType;
use arrow_select::filter::filter_record_batch;
use bytes::Bytes;
use futures_util::future::BoxFuture;
use futures_util::{FutureExt, TryStreamExt};
use parquet::arrow::arrow_reader::ArrowReaderOptions;
use parquet::arrow::async_reader::{AsyncFileReader, ParquetRecordBatchStreamBuilder};
use parquet::arrow::async_writer::{AsyncArrowWriter, AsyncFileWriter};
use parquet::arrow::ProjectionMask;
use parquet::basic::{Compression, ZstdLevel};
use parquet::errors::{ParquetError, Result as ParquetResult};
use parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use parquet::file::properties::WriterProperties;
use pnex_core::datasets::SAMPLE_VALUE_MAX;
use sha2::{Digest, Sha256};

use crate::services::artifact_store::S3Store;

const SAMPLE_BATCH_ROWS: usize = 8;

/// Streams a Parquet file to one object.
pub struct S3Sink {
    writer: opendal::Writer,
    hash: Sha256,
    pub bytes: u64,
}

impl S3Sink {
    pub fn new(writer: opendal::Writer) -> Self {
        Self {
            writer,
            hash: Sha256::new(),
            bytes: 0,
        }
    }

    /// Hex sha256 of what went through.
    pub fn sha256(self) -> String {
        hex::encode(self.hash.finalize())
    }

    /// Drops the multipart upload of a failed write.
    pub async fn abort(mut self) {
        if let Err(e) = self.writer.abort().await {
            tracing::debug!("dataset upload not aborted: {e}");
        }
    }
}

impl AsyncFileWriter for S3Sink {
    fn write(&mut self, bs: Bytes) -> BoxFuture<'_, ParquetResult<()>> {
        self.hash.update(&bs);
        self.bytes += bs.len() as u64;
        async move {
            self.writer
                .write(bs)
                .await
                .map_err(|e| ParquetError::External(Box::new(e)))
        }
        .boxed()
    }

    fn complete(&mut self) -> BoxFuture<'_, ParquetResult<()>> {
        async move {
            self.writer
                .close()
                .await
                .map(|_| ())
                .map_err(|e| ParquetError::External(Box::new(e)))
        }
        .boxed()
    }
}

/// Reads a Parquet object by byte range.
struct S3Reader {
    store: S3Store,
    key: String,
    size: u64,
}

impl AsyncFileReader for S3Reader {
    fn get_bytes(&mut self, range: Range<u64>) -> BoxFuture<'_, ParquetResult<Bytes>> {
        async move {
            self.store
                .get_range(&self.key, range)
                .await
                .map_err(ParquetError::General)
        }
        .boxed()
    }

    fn get_metadata<'a>(
        &'a mut self,
        _options: Option<&'a ArrowReaderOptions>,
    ) -> BoxFuture<'a, ParquetResult<Arc<ParquetMetaData>>> {
        async move {
            let size = self.size;
            let metadata = ParquetMetaDataReader::new()
                .load_and_finish(self, size)
                .await?;
            Ok(Arc::new(metadata))
        }
        .boxed()
    }
}

/// Rows of a file whose row group may hold `time_column` values in
/// [`from_us`, `to_us`), only `columns`: the footer statistics skip the other
/// groups, rows are not filtered here.
pub async fn read_time_range(
    store: &S3Store,
    key: &str,
    size: u64,
    columns: &[&str],
    time_column: &str,
    from_us: i64,
    to_us: i64,
) -> ParquetResult<Vec<RecordBatch>> {
    use parquet::file::statistics::Statistics;
    let reader = S3Reader {
        store: store.clone(),
        key: key.to_string(),
        size,
    };
    let builder = ParquetRecordBatchStreamBuilder::new(reader).await?;
    let schema = builder.parquet_schema();
    let index = |name: &str| (0..schema.num_columns()).find(|i| schema.column(*i).name() == name);
    let Some(time) = index(time_column) else {
        return Ok(Vec::new());
    };
    let wanted: Vec<usize> = columns.iter().filter_map(|c| index(c)).collect();
    let groups: Vec<usize> = builder
        .metadata()
        .row_groups()
        .iter()
        .enumerate()
        .filter(|(_, g)| match g.column(time).statistics() {
            Some(Statistics::Int64(st)) => match (st.min_opt(), st.max_opt()) {
                (Some(min), Some(max)) => *max >= from_us && *min < to_us,
                _ => true,
            },
            // No statistics: the group is read.
            _ => true,
        })
        .map(|(i, _)| i)
        .collect();
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let mask = ProjectionMask::leaves(schema, wanted);
    builder
        .with_row_groups(groups)
        .with_projection(mask)
        .build()?
        .try_collect()
        .await
}

/// Column names and the first `limit` rows of a file, values as text cut to
/// [`SAMPLE_VALUE_MAX`] characters.
/// Empty values per column of a file, from its footer: no row is read. `None` when a
/// row group does not record them.
pub async fn null_counts(
    store: &S3Store,
    key: &str,
    size: u64,
) -> ParquetResult<Vec<(String, Option<i64>)>> {
    let reader = S3Reader {
        store: store.clone(),
        key: key.to_string(),
        size,
    };
    let builder = ParquetRecordBatchStreamBuilder::new(reader).await?;
    let names: Vec<String> = builder
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let groups = builder.metadata().row_groups();
    Ok(names
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let nulls = groups
                .iter()
                .map(|g| {
                    g.columns()
                        .get(i)?
                        .statistics()?
                        .null_count_opt()
                        .map(|n| n as i64)
                })
                .sum::<Option<i64>>();
            (name, nulls)
        })
        .collect())
}

pub async fn sample(
    store: &S3Store,
    key: &str,
    size: u64,
    limit: usize,
) -> ParquetResult<(Vec<String>, Vec<Vec<Option<String>>>)> {
    let reader = S3Reader {
        store: store.clone(),
        key: key.to_string(),
        size,
    };
    let builder = ParquetRecordBatchStreamBuilder::new(reader).await?;
    let names = builder
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    // A cell may be 16 MB and compresses to nothing: few rows decoded at a time, and
    // each batch dropped once cut, or 200 such rows would hold 3 GB in the API.
    let mut stream = builder
        .with_limit(limit)
        .with_batch_size(SAMPLE_BATCH_ROWS)
        .build()?;
    let options = FormatOptions::default();
    let mut rows = Vec::new();
    while let Some(batch) = stream.try_next().await? {
        let formatters = batch
            .columns()
            .iter()
            .map(|c| ArrayFormatter::try_new(c.as_ref(), &options).map(|f| (c, f)))
            .collect::<Result<Vec<_>, _>>()?;
        for r in 0..batch.num_rows() {
            rows.push(
                formatters
                    .iter()
                    .map(|(column, f)| {
                        (!column.is_null(r)).then(|| {
                            f.value(r)
                                .to_string()
                                .chars()
                                .take(SAMPLE_VALUE_MAX)
                                .collect()
                        })
                    })
                    .collect(),
            );
        }
    }
    Ok((names, rows))
}

/// Column name → fingerprints of the values to remove (`erasure::fingerprint`).
pub type Targets = HashMap<String, HashSet<String>>;

/// What a rewrite left.
pub struct Rewritten {
    pub bytes: u64,
    pub rows: u64,
    pub removed: u64,
    pub sha256: String,
}

/// For each row, whether a target column holds a target value.
pub fn matches(batch: &RecordBatch, targets: &Targets) -> ParquetResult<Vec<bool>> {
    let mut hit = vec![false; batch.num_rows()];
    for (column, values) in targets {
        // Absent from an older version whose columns differed.
        let Some(array) = batch.column_by_name(column) else {
            continue;
        };
        if !can_cast_types(array.data_type(), &DataType::Utf8) {
            continue;
        }
        let text = cast(array, &DataType::Utf8)?;
        for (i, value) in text.as_string::<i32>().iter().enumerate() {
            let print = value.and_then(pnex_core::datasets::fingerprint);
            if print.is_some_and(|v| values.contains(&v)) {
                hit[i] = true;
            }
        }
    }
    Ok(hit)
}

/// A batch without its target rows.
pub fn without(batch: &RecordBatch, targets: &Targets) -> ParquetResult<RecordBatch> {
    let keep: BooleanArray = matches(batch, targets)?.iter().map(|h| Some(!h)).collect();
    Ok(filter_record_batch(batch, &keep)?)
}

/// Whether a file holds a target row. Reads the target columns only.
pub async fn has_match(
    store: &S3Store,
    key: &str,
    size: u64,
    targets: &Targets,
) -> ParquetResult<bool> {
    let reader = S3Reader {
        store: store.clone(),
        key: key.to_string(),
        size,
    };
    let builder = ParquetRecordBatchStreamBuilder::new(reader).await?;
    let roots: Vec<usize> = builder
        .schema()
        .fields()
        .iter()
        .enumerate()
        .filter(|(_, f)| targets.contains_key(f.name()))
        .map(|(i, _)| i)
        .collect();
    if roots.is_empty() {
        return Ok(false);
    }
    let mask = ProjectionMask::roots(builder.parquet_schema(), roots);
    let mut stream = builder.with_projection(mask).build()?;
    while let Some(batch) = stream.try_next().await? {
        if matches(&batch, targets)?.contains(&true) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Copies a file to `new_key` without its target rows, in a stream.
pub async fn rewrite_without(
    store: &S3Store,
    key: &str,
    size: u64,
    new_key: &str,
    targets: &Targets,
) -> ParquetResult<Rewritten> {
    let reader = S3Reader {
        store: store.clone(),
        key: key.to_string(),
        size,
    };
    let builder = ParquetRecordBatchStreamBuilder::new(reader).await?;
    let schema = builder.schema().clone();
    let mut stream = builder.build()?;
    let properties = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::default()))
        .build();
    let upload = store
        .writer(new_key)
        .await
        .map_err(|e| ParquetError::General(e.to_string()))?;
    let mut writer = AsyncArrowWriter::try_new(S3Sink::new(upload), schema, Some(properties))?;
    let (mut rows, mut removed) = (0u64, 0u64);
    let copied: ParquetResult<()> = async {
        while let Some(batch) = stream.try_next().await? {
            let kept = without(&batch, targets)?;
            rows += kept.num_rows() as u64;
            removed += (batch.num_rows() - kept.num_rows()) as u64;
            writer.write(&kept).await?;
            if writer.in_progress_size() > super::ROW_GROUP_BYTES {
                writer.flush().await?;
            }
        }
        writer.finish().await?;
        Ok(())
    }
    .await;
    let sink = writer.into_inner();
    if let Err(e) = copied {
        sink.abort().await;
        return Err(e);
    }
    let bytes = sink.bytes;
    Ok(Rewritten {
        bytes,
        rows,
        removed,
        sha256: sink.sha256(),
    })
}

/// The batches of a file, in a stream.
pub async fn batches(
    store: &S3Store,
    key: &str,
    size: u64,
) -> ParquetResult<impl futures_util::Stream<Item = ParquetResult<RecordBatch>> + Unpin> {
    let reader = S3Reader {
        store: store.clone(),
        key: key.to_string(),
        size,
    };
    ParquetRecordBatchStreamBuilder::new(reader).await?.build()
}

/// Records of a batch as an import reads them: values as text, a null absent.
pub fn records(batch: &RecordBatch) -> ParquetResult<Vec<serde_json::Value>> {
    let options = FormatOptions::default();
    let schema = batch.schema();
    let formatters = batch
        .columns()
        .iter()
        .map(|c| ArrayFormatter::try_new(c.as_ref(), &options).map(|f| (c, f)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((0..batch.num_rows())
        .map(|r| {
            let mut row = serde_json::Map::with_capacity(formatters.len());
            for (field, (column, f)) in schema.fields().iter().zip(&formatters) {
                if !column.is_null(r) {
                    row.insert(field.name().clone(), f.value(r).to_string().into());
                }
            }
            serde_json::Value::Object(row)
        })
        .collect())
}
