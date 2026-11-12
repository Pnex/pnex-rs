//! Files of a run (pipeline-engine.md D935): a fetched body, an archive
//! entry, a table read. Small ones stay in memory; past a few MB they spill
//! to a temporary file, deleted with the last row that holds it.

use std::io::{Read, Seek, Write};
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::SchemaRef;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{fail, Failure};

/// Bytes kept in memory before a file spills to disk.
const IN_MEMORY: usize = 4 * 1024 * 1024;

fn io_failure(_: std::io::Error) -> Failure {
    fail("storage", json!({ "what": "temporary file" }))
}

fn batches_failure(e: arrow_schema::ArrowError) -> Failure {
    tracing::warn!(error = %e, "pipeline batches not written or read");
    fail("storage", json!({ "what": "batches" }))
}

fn temp() -> std::io::Result<tempfile::NamedTempFile> {
    match std::env::var_os("PNEX_PIPELINE_TMP") {
        Some(dir) => tempfile::NamedTempFile::new_in(dir),
        None => tempfile::NamedTempFile::new(),
    }
}

/// A file of a row.
#[derive(Debug)]
pub enum Blob {
    Mem(Vec<u8>),
    Disk(tempfile::TempPath, u64),
}

/// Something both readable and seekable (a zip index needs both).
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

impl Blob {
    pub fn len(&self) -> u64 {
        match self {
            Blob::Mem(b) => b.len() as u64,
            Blob::Disk(_, n) => *n,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn open(&self) -> Result<Box<dyn ReadSeek + '_>, Failure> {
        Ok(match self {
            Blob::Mem(b) => Box::new(std::io::Cursor::new(b.as_slice())),
            Blob::Disk(path, _) => Box::new(std::fs::File::open(path).map_err(io_failure)?),
        })
    }

    /// The whole file, refused past `cap` bytes (a page, a JSON document).
    pub fn bytes(&self, cap: u64) -> Result<std::borrow::Cow<'_, [u8]>, Failure> {
        if self.len() > cap {
            return Err(fail("too_large", json!({ "max": cap })));
        }
        Ok(match self {
            Blob::Mem(b) => std::borrow::Cow::Borrowed(b.as_slice()),
            Blob::Disk(path, _) => {
                std::borrow::Cow::Owned(std::fs::read(path).map_err(io_failure)?)
            }
        })
    }

    pub fn sha256(&self) -> Result<String, Failure> {
        let (mut h, mut r) = (Sha256::new(), self.open()?);
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = r.read(&mut buf).map_err(io_failure)?;
            if n == 0 {
                return Ok(hex::encode(h.finalize()));
            }
            h.update(&buf[..n]);
        }
    }

    /// Reads `r` whole, refused past `cap` bytes: in memory while small,
    /// then on disk.
    pub fn from_reader(mut r: impl Read, cap: u64) -> Result<Blob, Failure> {
        let mut mem = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut disk: Option<tempfile::NamedTempFile> = None;
        let mut total: u64 = 0;
        loop {
            let n = r
                .read(&mut buf)
                .map_err(|_| fail("unreadable", json!({ "what": "stream" })))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > cap {
                return Err(fail("too_large", json!({ "max": cap })));
            }
            match &mut disk {
                Some(f) => f.write_all(&buf[..n]).map_err(io_failure)?,
                None if mem.len() + n > IN_MEMORY => {
                    let mut f = temp().map_err(io_failure)?;
                    f.write_all(&mem).map_err(io_failure)?;
                    f.write_all(&buf[..n]).map_err(io_failure)?;
                    mem = Vec::new();
                    disk = Some(f);
                }
                None => mem.extend_from_slice(&buf[..n]),
            }
        }
        Ok(match disk {
            Some(f) => Blob::Disk(f.into_temp_path(), total),
            None => Blob::Mem(mem),
        })
    }
}

/// Spools chunks as they arrive: in memory while small, then on disk.
pub struct Spool {
    mem: Vec<u8>,
    disk: Option<tempfile::NamedTempFile>,
    total: u64,
    cap: u64,
}

impl Spool {
    pub fn new(cap: u64) -> Self {
        Spool {
            mem: Vec::new(),
            disk: None,
            total: 0,
            cap,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), Failure> {
        self.total += chunk.len() as u64;
        if self.total > self.cap {
            return Err(fail("too_large", json!({ "max": self.cap })));
        }
        if self.disk.is_none() && self.mem.len() + chunk.len() > IN_MEMORY {
            let mut f = temp().map_err(io_failure)?;
            f.write_all(&self.mem).map_err(io_failure)?;
            self.mem = Vec::new();
            self.disk = Some(f);
        }
        match &mut self.disk {
            Some(f) => f.write_all(chunk).map_err(io_failure),
            None => {
                self.mem.extend_from_slice(chunk);
                Ok(())
            }
        }
    }

    pub fn finish(self) -> Blob {
        match self.disk {
            Some(f) => Blob::Disk(f.into_temp_path(), self.total),
            None => Blob::Mem(self.mem),
        }
    }
}

/// Typed rows of a `read`, spilled to an Arrow IPC file batch by batch.
#[derive(Debug)]
pub struct Batches {
    path: tempfile::TempPath,
    pub schema: SchemaRef,
    pub rows: u64,
}

/// Writes batches of one schema to a [`Batches`].
pub struct BatchWriter {
    file: tempfile::NamedTempFile,
    writer: Option<arrow_ipc::writer::FileWriter<std::fs::File>>,
    schema: SchemaRef,
    rows: u64,
}

impl BatchWriter {
    pub fn new(schema: SchemaRef) -> Result<Self, Failure> {
        let file = temp().map_err(io_failure)?;
        let handle = file.reopen().map_err(io_failure)?;
        let writer =
            arrow_ipc::writer::FileWriter::try_new(handle, &schema).map_err(batches_failure)?;
        Ok(BatchWriter {
            file,
            writer: Some(writer),
            schema,
            rows: 0,
        })
    }

    pub fn write(&mut self, b: &RecordBatch) -> Result<(), Failure> {
        self.rows += b.num_rows() as u64;
        self.writer
            .as_mut()
            .map(|w| w.write(b))
            .transpose()
            .map_err(batches_failure)?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<Arc<Batches>, Failure> {
        if let Some(mut w) = self.writer.take() {
            w.finish().map_err(batches_failure)?;
        }
        Ok(Arc::new(Batches {
            path: self.file.into_temp_path(),
            schema: self.schema,
            rows: self.rows,
        }))
    }
}

/// Several batch files of one schema as one (a table read from several files).
pub fn merged(files: &[Arc<Batches>]) -> Result<Arc<Batches>, Failure> {
    let first = files
        .first()
        .ok_or_else(|| fail("storage", json!({ "what": "batches" })))?;
    let mut w = BatchWriter::new(first.schema.clone())?;
    for f in files {
        if f.schema != first.schema {
            return Err(fail("schema", json!({ "what": "batches" })));
        }
        for b in f.read()? {
            w.write(&b?)?;
        }
    }
    w.finish()
}

impl Batches {
    /// The batches, read back one at a time.
    pub fn read(&self) -> Result<impl Iterator<Item = Result<RecordBatch, Failure>>, Failure> {
        let f = std::fs::File::open(&self.path).map_err(io_failure)?;
        let r = arrow_ipc::reader::FileReader::try_new(std::io::BufReader::new(f), None)
            .map_err(batches_failure)?;
        Ok(r.map(|b| b.map_err(batches_failure)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_stream_spills_to_disk_and_is_capped() {
        let big = vec![7u8; IN_MEMORY + 10];
        let b = Blob::from_reader(big.as_slice(), u64::MAX).unwrap();
        assert!(matches!(b, Blob::Disk(_, n) if n == big.len() as u64));
        assert_eq!(b.bytes(u64::MAX).unwrap().len(), big.len());
        assert!(matches!(
            Blob::from_reader(&b"abc"[..], 10).unwrap(),
            Blob::Mem(_)
        ));
        assert_eq!(
            Blob::from_reader(&b"abcdef"[..], 3).unwrap_err().code,
            "too_large"
        );
    }
}
