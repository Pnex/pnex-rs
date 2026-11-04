//! Incremental CSV reader (data-pipelines.md D569): bytes in, Arrow batches of
//! text columns out, in memory bounded by [`RECORD_MAX`] and the batch size,
//! whatever the size of the file. The input is hostile (D319, D320): a record
//! or a header past the bounds is refused, never buffered.

use std::sync::Arc;

use arrow_array::builder::StringBuilder;
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use csv_core::{ReadRecordResult, Reader, ReaderBuilder};
use pnex_core::import_read::text_of;

/// Bytes of one record once unquoted.
pub const RECORD_MAX: usize = 16 * 1024 * 1024;
pub const COLUMNS_MAX: usize = 2000;
/// Bytes read to find the end of the first line, which picks the delimiter.
const HEAD_MAX: usize = 1024 * 1024;
const BATCH_ROWS: usize = 8192;
const BATCH_BYTES: usize = 8 * 1024 * 1024;

/// Characters of a column name: names live in the catalogue, read at each listing.
const COLUMN_NAME_MAX: usize = 255;

#[derive(Debug, PartialEq, Eq)]
pub enum CsvError {
    /// No header line.
    Empty,
    RecordTooLarge {
        line: u64,
    },
    TooManyColumns {
        line: u64,
    },
    DuplicateColumn {
        name: String,
    },
    ColumnNameTooLong,
    FieldCount {
        line: u64,
        expected: usize,
        got: usize,
    },
}

pub struct CsvBatches {
    /// Bytes held until the delimiter is known, then empty.
    head: Vec<u8>,
    delimiter: Option<u8>,
    reader: Option<Reader>,
    out: Vec<u8>,
    out_len: usize,
    ends: Vec<usize>,
    ends_len: usize,
    schema: Option<SchemaRef>,
    builders: Vec<StringBuilder>,
    batch_rows: usize,
    batch_bytes: usize,
}

impl CsvBatches {
    /// `delimiter`: `None` picks the most frequent of `;`, `,` and tab on the
    /// first line.
    pub fn new(delimiter: Option<u8>) -> Self {
        Self {
            head: Vec::new(),
            delimiter,
            reader: None,
            out: vec![0; 64 * 1024],
            out_len: 0,
            ends: vec![0; COLUMNS_MAX + 1],
            ends_len: 0,
            schema: None,
            builders: Vec::new(),
            batch_rows: 0,
            batch_bytes: 0,
        }
    }

    /// Known once the header line has been read.
    pub fn schema(&self) -> Option<SchemaRef> {
        self.schema.clone()
    }

    /// Feeds a chunk, cut anywhere. Returns the batches it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<RecordBatch>, CsvError> {
        let mut done = Vec::new();
        if self.reader.is_some() {
            self.feed(chunk, false, &mut done)?;
        } else {
            self.head.extend_from_slice(chunk);
            if self.head.contains(&b'\n') || self.head.len() >= HEAD_MAX {
                self.start()?;
                let head = std::mem::take(&mut self.head);
                self.feed(strip_bom(&head), false, &mut done)?;
            }
        }
        Ok(done)
    }

    /// End of the input: the batches left. The input may have had no `\n` at
    /// all (old Mac line ends), so there can be several.
    pub fn finish(&mut self) -> Result<Vec<RecordBatch>, CsvError> {
        let mut done = Vec::new();
        if self.reader.is_none() {
            self.start()?;
            let head = std::mem::take(&mut self.head);
            self.feed(strip_bom(&head), false, &mut done)?;
        }
        self.feed(&[], true, &mut done)?;
        if self.schema.is_none() {
            return Err(CsvError::Empty);
        }
        if self.batch_rows > 0 {
            done.push(self.take_batch());
        }
        Ok(done)
    }

    fn start(&mut self) -> Result<(), CsvError> {
        let delimiter = self
            .delimiter
            .unwrap_or_else(|| sniff(strip_bom(&self.head)));
        self.reader = Some(ReaderBuilder::new().delimiter(delimiter).build());
        Ok(())
    }

    fn feed(
        &mut self,
        input: &[u8],
        eof: bool,
        done: &mut Vec<RecordBatch>,
    ) -> Result<(), CsvError> {
        let mut pos = 0;
        loop {
            // An empty slice means end of file to csv-core.
            if pos == input.len() && !eof {
                return Ok(());
            }
            let reader = self.reader.as_mut().expect("started before feeding");
            let (result, read, written, ends) = reader.read_record(
                &input[pos..],
                &mut self.out[self.out_len..],
                &mut self.ends[self.ends_len..],
            );
            pos += read;
            self.out_len += written;
            self.ends_len += ends;
            match result {
                ReadRecordResult::InputEmpty => {
                    if !eof {
                        return Ok(());
                    }
                }
                ReadRecordResult::End => return Ok(()),
                ReadRecordResult::OutputFull => {
                    if self.out.len() >= RECORD_MAX {
                        let line = reader.line();
                        return Err(CsvError::RecordTooLarge { line });
                    }
                    let size = (self.out.len() * 2).min(RECORD_MAX);
                    self.out.resize(size, 0);
                }
                ReadRecordResult::OutputEndsFull => {
                    let line = reader.line();
                    return Err(CsvError::TooManyColumns { line });
                }
                ReadRecordResult::Record => {
                    let line = reader.line();
                    self.record(line)?;
                    self.out_len = 0;
                    self.ends_len = 0;
                    if self.batch_rows >= BATCH_ROWS || self.batch_bytes >= BATCH_BYTES {
                        done.push(self.take_batch());
                    }
                }
            }
        }
    }

    fn record(&mut self, line: u64) -> Result<(), CsvError> {
        let Some(schema) = &self.schema else {
            return self.header();
        };
        let expected = schema.fields().len();
        if self.ends_len != expected {
            let got = self.ends_len;
            return Err(CsvError::FieldCount {
                line,
                expected,
                got,
            });
        }
        let mut start = 0;
        for (builder, end) in self.builders.iter_mut().zip(&self.ends) {
            match &self.out[start..*end] {
                [] => builder.append_null(),
                value => match std::str::from_utf8(value) {
                    Ok(text) => builder.append_value(text),
                    Err(_) => builder.append_value(text_of(value)),
                },
            }
            start = *end;
        }
        self.batch_rows += 1;
        self.batch_bytes += self.out_len;
        Ok(())
    }

    fn header(&mut self) -> Result<(), CsvError> {
        let (mut names, mut start) = (Vec::with_capacity(self.ends_len), 0);
        for (i, end) in self.ends[..self.ends_len].iter().enumerate() {
            let name = text_of(&self.out[start..*end]).trim().to_string();
            if name.chars().count() > COLUMN_NAME_MAX {
                return Err(CsvError::ColumnNameTooLong);
            }
            // A trailing delimiter gives a column with no name.
            names.push(if name.is_empty() {
                format!("column_{}", i + 1)
            } else {
                name
            });
            start = *end;
        }
        for (i, name) in names.iter().enumerate() {
            if names[..i].contains(name) {
                let name = name.clone();
                return Err(CsvError::DuplicateColumn { name });
            }
        }
        let fields: Vec<Field> = names
            .into_iter()
            .map(|n| Field::new(n, DataType::Utf8, true))
            .collect();
        self.builders = fields.iter().map(|_| StringBuilder::new()).collect();
        self.schema = Some(Arc::new(Schema::new(fields)));
        Ok(())
    }

    fn take_batch(&mut self) -> RecordBatch {
        let schema = self.schema.clone().expect("rows come after the header");
        let columns: Vec<ArrayRef> = self
            .builders
            .iter_mut()
            .map(|b| Arc::new(b.finish()) as ArrayRef)
            .collect();
        self.batch_rows = 0;
        self.batch_bytes = 0;
        RecordBatch::try_new(schema, columns).expect("one text column per field")
    }
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
}

fn sniff(head: &[u8]) -> u8 {
    let line = head.split(|b| *b == b'\n').next().unwrap_or_default();
    let count = |d: u8| line.iter().filter(|b| **b == d).count();
    // `max_by_key` keeps the last of a tie: a comma wins.
    (*b"\t;,")
        .into_iter()
        .max_by_key(|d| count(*d))
        .unwrap_or(b',')
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Array, StringArray};

    /// All rows of `input`, fed in chunks of `size` bytes.
    fn read(
        input: &[u8],
        size: usize,
    ) -> Result<(Vec<String>, Vec<Vec<Option<String>>>), CsvError> {
        let mut csv = CsvBatches::new(None);
        let mut batches = Vec::new();
        for chunk in input.chunks(size) {
            batches.extend(csv.push(chunk)?);
        }
        batches.extend(csv.finish()?);
        let schema = csv.schema().expect("header read");
        let names = schema.fields().iter().map(|f| f.name().clone()).collect();
        let mut rows = Vec::new();
        for b in &batches {
            for r in 0..b.num_rows() {
                rows.push(
                    (0..b.num_columns())
                        .map(|c| {
                            let a = b.column(c).as_any().downcast_ref::<StringArray>().unwrap();
                            (!a.is_null(r)).then(|| a.value(r).to_string())
                        })
                        .collect(),
                );
            }
        }
        Ok((names, rows))
    }

    /// The shape that broke the engine's chunked inference (§9.7 bis): a quoted
    /// newline in a middle column, CRLF lines, doubled quotes. Every chunk
    /// boundary must give the same rows.
    #[test]
    fn a_chunk_can_end_anywhere_even_inside_quotes() {
        let input =
            b"\xef\xbb\xbf\"a\";\"b\";\"c\"\r\n\"1\";\"x\ny \"\"q\"\"\";\"z\"\r\n\"2\";;\"w\"\r\n";
        let want = vec![
            vec![
                Some("1".to_string()),
                Some("x\ny \"q\"".into()),
                Some("z".into()),
            ],
            vec![Some("2".to_string()), None, Some("w".into())],
        ];
        for size in 1..input.len() {
            let (names, rows) = read(input, size).unwrap();
            assert_eq!(names, ["a", "b", "c"], "chunk size {size}");
            assert_eq!(rows, want, "chunk size {size}");
        }
    }

    #[test]
    fn delimiter_comes_from_the_first_line() {
        assert_eq!(
            read(b"a,b\n1,2\n", 3).unwrap().1[0][1].as_deref(),
            Some("2")
        );
        assert_eq!(
            read(b"a\tb\n1\t2", 3).unwrap().1[0][1].as_deref(),
            Some("2")
        );
    }

    #[test]
    fn file_without_final_newline_keeps_its_last_row() {
        assert_eq!(read(b"a;b\n1;2\n3;4", 4).unwrap().1.len(), 2);
    }

    #[test]
    fn latin1_values_are_kept_as_text() {
        let rows = read(b"a;b\nh\xe9;2\n", 64).unwrap().1;
        assert_eq!(rows[0][0].as_deref(), Some("hé"));
    }

    #[test]
    fn a_header_alone_is_an_empty_table() {
        let (names, rows) = read(b"a;b\n", 64).unwrap();
        assert_eq!(names, ["a", "b"]);
        assert!(rows.is_empty());
    }

    #[test]
    fn nothing_at_all_is_refused() {
        assert_eq!(read(b"\n", 64).unwrap_err(), CsvError::Empty);
        assert_eq!(CsvBatches::new(None).finish().unwrap_err(), CsvError::Empty);
    }

    #[test]
    fn a_wrong_field_count_names_its_line() {
        let e = read(b"a;b\n1;2\n3\n", 64).unwrap_err();
        assert_eq!(
            e,
            CsvError::FieldCount {
                line: 4,
                expected: 2,
                got: 1
            }
        );
    }

    #[test]
    fn duplicate_columns_are_refused() {
        let e = read(b"a;a\n1;2\n", 64).unwrap_err();
        assert_eq!(e, CsvError::DuplicateColumn { name: "a".into() });
    }

    #[test]
    fn too_many_columns_are_refused() {
        let header = vec![b';'; COLUMNS_MAX + 5];
        assert!(matches!(
            read(&header, 4096).unwrap_err(),
            CsvError::TooManyColumns { .. }
        ));
    }

    /// An unterminated quote must not grow past the bound.
    #[test]
    fn an_endless_record_is_refused() {
        let mut csv = CsvBatches::new(Some(b';'));
        csv.push(b"a\n\"").unwrap();
        let chunk = vec![b'x'; 1024 * 1024];
        let mut result = Ok(Vec::new());
        for _ in 0..(RECORD_MAX / chunk.len() + 2) {
            result = csv.push(&chunk);
            if result.is_err() {
                break;
            }
        }
        assert!(matches!(result, Err(CsvError::RecordTooLarge { .. })));
        assert!(csv.out.len() <= RECORD_MAX);
    }

    #[test]
    fn long_files_are_cut_in_batches() {
        let mut input = b"a\n".to_vec();
        for _ in 0..(BATCH_ROWS * 2 + 1) {
            input.extend_from_slice(b"v\n");
        }
        let mut csv = CsvBatches::new(None);
        let batches = csv.push(&input).unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(csv.finish().unwrap()[0].num_rows(), 1);
    }
}
