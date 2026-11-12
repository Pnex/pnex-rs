//! `read` (pipeline-engine.md D939): files into typed rows, by a format
//! reader that knows no source. Decimals stay exact; a time without an
//! offset is read in its IANA zone, daylight saving included.

use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, Date32Builder, Decimal128Builder, Float64Builder, Int64Builder, StringBuilder,
    TimestampMicrosecondBuilder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::SchemaRef;
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, NaiveDate, NaiveDateTime};
use pnex_core::pipeline::doc::{Column, ColumnType, Format, Header, Read};
use pnex_core::pipeline::glob_match;
use pnex_core::pipeline::{template, tz};
use serde_json::{json, Value};

use super::blob::{BatchWriter, Batches, Blob};
use super::{fail, Failure, Row};

/// Time zone of the timestamps the engine writes.
pub const UTC: &str = "+00:00";
/// Rows of one batch.
const BATCH_ROWS: usize = 200_000;
/// Largest JSON document one `read` parses whole; CSV is read as a stream.
pub const READ_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Where a value comes from in a record.
enum At {
    Index(Vec<usize>),
    Key(Vec<String>),
}

/// A time read in its zone: a fixed offset (`-05:00`) or an IANA name.
enum Zone {
    Utc,
    Fixed(i32),
    Named(tz::Tz),
}

impl Zone {
    fn of(name: Option<&str>) -> Result<Self, Failure> {
        let Some(name) = name.map(str::trim) else {
            return Ok(Zone::Utc);
        };
        let bad = || {
            fail(
                "invalid_value",
                json!({ "name": "timezone", "value": name }),
            )
        };
        if let Some(rest) = name.strip_prefix(['+', '-']) {
            let (h, m) = rest.split_once(':').ok_or_else(bad)?;
            let secs = h.parse::<i32>().map_err(|_| bad())? * 3600
                + m.parse::<i32>().map_err(|_| bad())? * 60;
            return Ok(Zone::Fixed(if name.starts_with('-') {
                -secs
            } else {
                secs
            }));
        }
        tz::zone(name).map(Zone::Named).map_err(|_| bad())
    }

    fn to_utc(&self, wall: NaiveDateTime) -> NaiveDateTime {
        match self {
            Zone::Utc => wall,
            Zone::Fixed(secs) => wall - chrono::Duration::seconds(i64::from(*secs)),
            Zone::Named(z) => tz::wall_to_utc(wall, z),
        }
    }
}

/// A cell read as its column's type.
enum Cell {
    Null,
    Text(String),
    Int(i64),
    Dec(i128),
    Float(f64),
    Bool(bool),
    Date(i32),
    Ts(i64),
}

impl Cell {
    /// `None` when the text does not read as the column's type.
    fn parse(c: &Column, format: Option<&str>, zone: &Zone, raw: &str) -> Option<Cell> {
        let v = raw.trim();
        Some(match c.kind {
            ColumnType::Text | ColumnType::Json => Cell::Text(raw.to_string()),
            ColumnType::Integer => Cell::Int(v.parse().ok()?),
            ColumnType::Decimal => Cell::Dec(decimal(v, c.scale.unwrap_or(0))?),
            ColumnType::Float => Cell::Float(v.parse::<f64>().ok().filter(|x| x.is_finite())?),
            ColumnType::Bool => Cell::Bool(match v.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => true,
                "false" | "0" | "no" => false,
                _ => return None,
            }),
            ColumnType::Date => {
                let d = NaiveDate::parse_from_str(v, format.unwrap_or("%Y-%m-%d")).ok()?;
                let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
                Cell::Date(i32::try_from((d - epoch).num_days()).ok()?)
            }
            ColumnType::Timestamp => Cell::Ts(timestamp(v, format, zone)?),
        })
    }

    /// Whether a number lies in `[lo, hi]`; anything else does.
    fn within(&self, c: &Column, [lo, hi]: [f64; 2]) -> bool {
        let x = match self {
            Cell::Int(v) => *v as f64,
            Cell::Float(v) => *v,
            Cell::Dec(v) => *v as f64 / 10f64.powi(c.scale.unwrap_or(0) as i32),
            _ => return true,
        };
        (lo..=hi).contains(&x)
    }
}

enum Builder {
    Text(StringBuilder),
    Int(Int64Builder),
    Dec(Decimal128Builder, u32),
    Float(Float64Builder),
    Bool(BooleanBuilder),
    Date(Date32Builder),
    Ts(TimestampMicrosecondBuilder),
}

impl Builder {
    fn new(c: &Column) -> Self {
        match c.kind {
            ColumnType::Text | ColumnType::Json => Builder::Text(StringBuilder::new()),
            ColumnType::Integer => Builder::Int(Int64Builder::new()),
            ColumnType::Decimal => Builder::Dec(Decimal128Builder::new(), c.scale.unwrap_or(0)),
            ColumnType::Float => Builder::Float(Float64Builder::new()),
            ColumnType::Bool => Builder::Bool(BooleanBuilder::new()),
            ColumnType::Date => Builder::Date(Date32Builder::new()),
            ColumnType::Timestamp => Builder::Ts(TimestampMicrosecondBuilder::new()),
        }
    }

    fn data_type(c: &Column) -> DataType {
        match c.kind {
            ColumnType::Text | ColumnType::Json => DataType::Utf8,
            ColumnType::Integer => DataType::Int64,
            ColumnType::Decimal => DataType::Decimal128(38, c.scale.unwrap_or(0) as i8),
            ColumnType::Float => DataType::Float64,
            ColumnType::Bool => DataType::Boolean,
            ColumnType::Date => DataType::Date32,
            ColumnType::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, Some(UTC.into())),
        }
    }

    fn null(&mut self) {
        match self {
            Builder::Text(b) => b.append_null(),
            Builder::Int(b) => b.append_null(),
            Builder::Dec(b, _) => b.append_null(),
            Builder::Float(b) => b.append_null(),
            Builder::Bool(b) => b.append_null(),
            Builder::Date(b) => b.append_null(),
            Builder::Ts(b) => b.append_null(),
        }
    }

    fn append(&mut self, cell: Cell) {
        match (self, cell) {
            (b, Cell::Null) => b.null(),
            (Builder::Text(b), Cell::Text(v)) => b.append_value(v),
            (Builder::Int(b), Cell::Int(v)) => b.append_value(v),
            (Builder::Dec(b, _), Cell::Dec(v)) => b.append_value(v),
            (Builder::Float(b), Cell::Float(v)) => b.append_value(v),
            (Builder::Bool(b), Cell::Bool(v)) => b.append_value(v),
            (Builder::Date(b), Cell::Date(v)) => b.append_value(v),
            (Builder::Ts(b), Cell::Ts(v)) => b.append_value(v),
            (b, _) => b.null(),
        }
    }

    fn finish(self, c: &Column) -> Result<ArrayRef, Failure> {
        let bad = |_| fail("invalid_value", json!({ "name": c.name }));
        Ok(match self {
            Builder::Text(mut b) => Arc::new(b.finish()),
            Builder::Int(mut b) => Arc::new(b.finish()),
            Builder::Dec(mut b, scale) => Arc::new(
                b.finish()
                    .with_precision_and_scale(38, scale as i8)
                    .map_err(bad)?,
            ),
            Builder::Float(mut b) => Arc::new(b.finish()),
            Builder::Bool(mut b) => Arc::new(b.finish()),
            Builder::Date(mut b) => Arc::new(b.finish()),
            Builder::Ts(mut b) => Arc::new(b.finish().with_timezone(UTC)),
        })
    }
}

/// An exact decimal with `scale` places: more places is a refusal, never a
/// rounding.
pub fn decimal(s: &str, scale: u32) -> Option<i128> {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    if (int.is_empty() && frac.is_empty())
        || frac.len() > scale as usize
        || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let mut v: i128 = if int.is_empty() { 0 } else { int.parse().ok()? };
    for _ in 0..scale {
        v = v.checked_mul(10)?;
    }
    if !frac.is_empty() {
        let pad = 10i128.checked_pow(scale - frac.len() as u32)?;
        v = v.checked_add(frac.parse::<i128>().ok()?.checked_mul(pad)?)?;
    }
    Some(if neg { -v } else { v })
}

/// Microseconds since the epoch, UTC. Formats: a chrono pattern (the wall
/// clock of `zone`), `epoch_s`, `epoch_ms`, `epoch_us`, or RFC 3339 when
/// absent.
fn timestamp(v: &str, format: Option<&str>, zone: &Zone) -> Option<i64> {
    let epoch = |mult: i64| v.parse::<i64>().ok()?.checked_mul(mult);
    match format {
        Some("epoch_s") => epoch(1_000_000),
        Some("epoch_ms") => epoch(1_000),
        Some("epoch_us") => epoch(1),
        // The unit from the number of digits: Binance moved from ms to µs in 2025.
        Some("epoch") => match v.trim_start_matches('-').len() {
            0..=10 => epoch(1_000_000),
            11..=13 => epoch(1_000),
            14..=16 => epoch(1),
            _ => v.parse::<i64>().ok().map(|ns| ns / 1_000),
        },
        Some(f) => {
            let wall = NaiveDateTime::parse_from_str(v, f).ok()?;
            Some(zone.to_utc(wall).and_utc().timestamp_micros())
        }
        None => match DateTime::parse_from_rfc3339(v) {
            Ok(t) => Some(t.timestamp_micros()),
            Err(_) => {
                let wall = NaiveDateTime::parse_from_str(v, "%Y-%m-%dT%H:%M:%S").ok()?;
                Some(zone.to_utc(wall).and_utc().timestamp_micros())
            }
        },
    }
}

fn decode(bytes: &[u8], encoding: Option<&str>) -> Result<String, Failure> {
    let latin1 = |b: &[u8]| b.iter().map(|&c| c as char).collect::<String>();
    match encoding {
        Some("latin1") => Ok(latin1(bytes)),
        Some(_) => String::from_utf8(bytes.to_vec())
            .map_err(|_| fail("unreadable", json!({ "what": "utf-8" }))),
        None => Ok(String::from_utf8(bytes.to_vec()).unwrap_or_else(|e| latin1(e.as_bytes()))),
    }
}

fn at_of(c: &Column, header: &[String]) -> Result<At, Failure> {
    // Named as the file writes it: the column a user looks for in the source.
    let source_name = match &c.at {
        Some(Value::String(s)) => s.clone(),
        _ => c.name.clone(),
    };
    let missing = || {
        fail(
            "schema_changed",
            json!({ "column": c.name, "columns": source_name }),
        )
    };
    let one = |v: &Value| -> Result<At, Failure> {
        match v {
            Value::Number(n) => Ok(At::Index(vec![n.as_u64().ok_or_else(missing)? as usize])),
            Value::String(s) if s.starts_with('/') || s.starts_with("^/") => {
                Ok(At::Key(vec![s.clone()]))
            }
            Value::String(s) if header.is_empty() => Ok(At::Key(vec![s.clone()])),
            Value::String(s) => header
                .iter()
                .position(|h| h.trim() == s)
                .map(|i| At::Index(vec![i]))
                .ok_or_else(missing),
            _ => Err(missing()),
        }
    };
    match &c.at {
        None => one(&Value::String(c.name.clone())),
        Some(Value::Array(parts)) => {
            let (mut idx, mut keys) = (Vec::new(), Vec::new());
            for p in parts {
                match one(p)? {
                    At::Index(i) => idx.extend(i),
                    At::Key(k) => keys.extend(k),
                }
            }
            Ok(if keys.is_empty() {
                At::Index(idx)
            } else {
                At::Key(keys)
            })
        }
        Some(v) => one(v),
    }
}

fn cell_of_json(record: &Value, key: &str, as_json: bool) -> Option<String> {
    let v = match key {
        // The whole record, for a `json` column the mapping reads.
        "/" => Some(record),
        k if k.starts_with('/') => record.pointer(k),
        k => record.get(k),
    }?;
    match v {
        Value::Null => None,
        v if as_json => Some(v.to_string()),
        Value::String(s) => Some(s.clone()),
        v => Some(v.to_string()),
    }
}

/// Typed rows of one file, built batch by batch.
struct Typed {
    columns: Vec<Column>,
    /// Column name → globs one of which its cell must match (`where`).
    r_where: std::collections::BTreeMap<String, Vec<String>>,
    skip_invalid: bool,
    /// Per column: not a constant (`None`), a null, or a text of the file's row.
    fixed: Vec<Option<Option<String>>>,
    /// Per column: the divisor's power of ten, resolved for this file.
    divide: Vec<Option<u32>>,
    /// Per column: the instant a binary count of time is added to, µs.
    base_us: Vec<Option<i64>>,
    ats: Vec<Option<At>>,
    zones: Vec<Zone>,
    schema: SchemaRef,
    builders: Vec<Builder>,
    pending: usize,
    out: Option<BatchWriter>,
}

impl Typed {
    fn new(r: &Read, row: &Row, header: &[String]) -> Result<Self, Failure> {
        let columns: Vec<Column> = if r.columns.is_empty() {
            header
                .iter()
                .map(|h| Column {
                    name: h.trim().to_string(),
                    optional: true,
                    ..Default::default()
                })
                .collect()
        } else {
            r.columns.clone()
        };
        let ats = columns
            .iter()
            .map(|c| {
                if c.value.is_some() {
                    Ok(None)
                } else {
                    at_of(c, header).map(Some)
                }
            })
            .collect::<Result<_, _>>()?;
        let zones = columns
            .iter()
            .map(|c| Zone::of(c.timezone.as_deref()))
            .collect::<Result<_, _>>()?;
        let fields: Vec<Field> = columns
            .iter()
            .map(|c| Field::new(&c.name, Builder::data_type(c), true))
            .collect();
        let of_row = |t: &str| {
            template::fill(t, false, |n| row.text(n))
                .map_err(|n| fail("missing_value", json!({ "name": n })))
        };
        let fixed = columns
            .iter()
            .map(|c| match &c.value {
                None => Ok(None),
                Some(Value::Null) => Ok(Some(None)),
                Some(Value::String(t)) => of_row(t).map(|v| Some(Some(v))),
                Some(v) => Ok(Some(Some(v.to_string()))),
            })
            .collect::<Result<_, Failure>>()?;
        let divide = columns
            .iter()
            .map(|c| {
                c.divide
                    .as_deref()
                    .map(|t| {
                        of_row(t)?
                            .trim()
                            .parse::<u32>()
                            .ok()
                            .filter(|d| *d <= 18)
                            .ok_or_else(|| {
                                fail("invalid_value", json!({ "name": c.name, "value": t }))
                            })
                    })
                    .transpose()
            })
            .collect::<Result<_, Failure>>()?;
        let base_us = columns
            .iter()
            .map(|c| {
                c.base
                    .as_deref()
                    .map(|t| {
                        let v = of_row(t)?;
                        chrono::DateTime::parse_from_rfc3339(v.trim())
                            .map(|d| d.timestamp_micros())
                            .map_err(|_| {
                                fail("invalid_value", json!({ "name": c.name, "value": v }))
                            })
                    })
                    .transpose()
            })
            .collect::<Result<_, Failure>>()?;
        let builders = columns.iter().map(Builder::new).collect();
        Ok(Typed {
            r_where: r.r#where.clone(),
            skip_invalid: r.skip_invalid,
            columns,
            fixed,
            divide,
            base_us,
            ats,
            zones,
            schema: Arc::new(Schema::new(fields)),
            builders,
            pending: 0,
            out: None,
        })
    }

    /// Whether every typed cell of a CSV record reads as its type (an `auto`
    /// header is the first line that does not).
    fn reads(&self, cells: &[String]) -> bool {
        self.columns.iter().zip(&self.ats).all(|(c, at)| match at {
            Some(At::Index(idx)) if c.kind != ColumnType::Text => {
                let raw: Vec<&str> = idx
                    .iter()
                    .filter_map(|k| cells.get(*k).map(String::as_str))
                    .collect();
                Cell::parse(c, c.format.as_deref(), &Zone::Utc, &raw.join(" ")).is_some()
            }
            _ => true,
        })
    }

    fn push(
        &mut self,
        line: usize,
        cell: impl Fn(&Column, &At) -> Option<String>,
    ) -> Result<(), Failure> {
        let mut cells = Vec::with_capacity(self.columns.len());
        for (j, c) in self.columns.iter().enumerate() {
            let raw = match (&self.fixed[j], &self.ats[j]) {
                (Some(v), _) => v.clone(),
                (None, Some(at)) => cell(c, at),
                (None, None) => None,
            };
            let raw = raw.map(|s| {
                c.map
                    .get(s.trim())
                    .or_else(|| c.map.get("*"))
                    .cloned()
                    .unwrap_or(s)
            });
            let raw = match self.divide[j] {
                Some(d) => raw.map(|s| shifted(&s, d)),
                None => raw,
            };
            let raw = match self.base_us[j] {
                Some(base) => raw.and_then(|s| {
                    let unit = match c.format.as_deref() {
                        Some("epoch_s") => 1_000_000,
                        Some("epoch_us") => 1,
                        _ => 1_000,
                    };
                    let n: i64 = s.trim().parse().ok()?;
                    Some(base.checked_add(n.checked_mul(unit)?)?.to_string())
                }),
                None => raw,
            };
            let raw = raw.filter(|s| {
                let t = s.trim();
                let empty = if c.kind == ColumnType::Text {
                    s.is_empty()
                } else {
                    t.is_empty()
                };
                !empty && !c.null_if.iter().any(|n| n.eq_ignore_ascii_case(t))
            });
            if let Some(globs) = self.r_where.get(&c.name) {
                let kept = raw
                    .as_deref()
                    .is_some_and(|v| globs.iter().any(|g| pnex_core::pipeline::glob_match(g, v)));
                if !kept {
                    return Ok(());
                }
            }
            // A count added to a base is already in µs.
            let format = match self.base_us[j] {
                Some(_) => Some("epoch_us"),
                None => c.format.as_deref(),
            };
            let cell = match raw {
                None if c.optional || c.value.is_some() => Some(Cell::Null),
                None => None,
                Some(s) => match Cell::parse(c, format, &self.zones[j], &s) {
                    Some(v) if c.range.is_none_or(|r| v.within(c, r)) => Some(v),
                    Some(_) if c.optional => Some(Cell::Null),
                    _ => None,
                },
            };
            match cell {
                Some(v) => cells.push(v),
                None if self.skip_invalid => return Ok(()),
                None => {
                    return Err(fail(
                        "unreadable",
                        json!({ "line": line, "column": c.name }),
                    ));
                }
            }
        }
        for (b, v) in self.builders.iter_mut().zip(cells) {
            b.append(v);
        }
        self.pending += 1;
        if self.pending == BATCH_ROWS {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Failure> {
        if self.pending == 0 {
            return Ok(());
        }
        let builders = std::mem::replace(
            &mut self.builders,
            self.columns.iter().map(Builder::new).collect(),
        );
        let arrays: Vec<ArrayRef> = builders
            .into_iter()
            .zip(&self.columns)
            .map(|(b, c)| b.finish(c))
            .collect::<Result<_, _>>()?;
        let batch = RecordBatch::try_new(self.schema.clone(), arrays)
            .map_err(|_| fail("unreadable", json!({ "what": "batch" })))?;
        if self.out.is_none() {
            self.out = Some(BatchWriter::new(self.schema.clone())?);
        }
        if let Some(w) = &mut self.out {
            w.write(&batch)?;
        }
        self.pending = 0;
        Ok(())
    }

    fn finish(mut self) -> Result<Option<Arc<Batches>>, Failure> {
        self.flush()?;
        self.out.map(BatchWriter::finish).transpose()
    }
}

fn read_csv(r: &Read, row: &Row, file: &Blob) -> Result<Option<Arc<Batches>>, Failure> {
    let mut rd = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(r.delimiter.map_or(b',', |d| d as u8))
        .from_reader(std::io::BufReader::new(file.open()?));
    let mut records = rd.byte_records();
    let cells = |rec: csv::ByteRecord| -> Result<Vec<String>, Failure> {
        rec.iter()
            .map(|f| decode(f, r.encoding.as_deref()))
            .collect()
    };
    let unreadable = |line: usize| fail("unreadable", json!({ "line": line }));
    let mut first = match records.next() {
        None => return Ok(None),
        Some(rec) => Some(cells(rec.map_err(|_| unreadable(1))?)?),
    };
    if let Some(f) = first.as_mut().and_then(|f| f.first_mut()) {
        *f = f.trim_start_matches('\u{feff}').to_string();
    }
    let header = match r.header {
        Header::Yes => first.take().unwrap_or_default(),
        Header::No | Header::Auto => Vec::new(),
    };
    let mut typed = Typed::new(r, row, &header)?;
    let mut line = 1;
    if let Some(f) = first {
        // An `auto` first line that does not read as the columns is a header.
        if r.header == Header::No || typed.reads(&f) {
            typed.push(line, |_, at| cell_of(at, &f))?;
        }
    }
    for rec in records {
        line += 1;
        let rec = cells(rec.map_err(|_| unreadable(line))?)?;
        if rec.len() == 1 && rec[0].trim().is_empty() {
            continue;
        }
        typed.push(line, |_, at| cell_of(at, &rec))?;
    }
    typed.finish()
}

fn cell_of(at: &At, cells: &[String]) -> Option<String> {
    match at {
        At::Index(idx) => {
            let parts: Option<Vec<&str>> = idx
                .iter()
                .map(|k| cells.get(*k).map(String::as_str))
                .collect();
            parts.map(|p| p.join(" "))
        }
        At::Key(_) => None,
    }
}

/// Records a JSON document gives, at most this many.
const JSON_RECORDS_MAX: usize = 5_000_000;

/// The records under a pointer whose `*` segments walk every value: each
/// with the keys it was reached through.
fn walk<'a>(
    v: &'a Value,
    segments: &[&str],
    keys: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, &'a Value)>,
) {
    if out.len() >= JSON_RECORDS_MAX {
        return;
    }
    let Some((first, rest)) = segments.split_first() else {
        out.push((keys.clone(), v));
        return;
    };
    match (*first, v) {
        ("*", Value::Object(o)) => {
            for (k, child) in o {
                keys.push(k.clone());
                walk(child, rest, keys, out);
                keys.pop();
            }
        }
        ("*", Value::Array(a)) => {
            for (i, child) in a.iter().enumerate() {
                keys.push(i.to_string());
                walk(child, rest, keys, out);
                keys.pop();
            }
        }
        (seg, _) => {
            let seg = seg.replace("~1", "/").replace("~0", "~");
            let child = match v {
                Value::Object(o) => o.get(&seg),
                Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i)),
                _ => None,
            };
            if let Some(c) = child {
                walk(c, rest, keys, out);
            }
        }
    }
}

fn read_json(r: &Read, row: &Row, file: &Blob) -> Result<Option<Arc<Batches>>, Failure> {
    let unreadable = || fail("unreadable", json!({ "what": "json" }));
    let bytes = file.bytes(READ_MAX_BYTES as u64)?;
    // XML: `records` is the element path (`FICHES/FICHE`), each element a
    // JSON-like record (attributes `@name`, repeated children as arrays).
    let (doc, pointer) = if r.format == Format::Xml {
        let text = pnex_core::import_read::text_of(&bytes);
        let path = r.records.as_deref().unwrap_or("");
        let records = pnex_core::import_read::xml_rows(&text, path)
            .map_err(|_| fail("unreadable", json!({ "what": "xml" })))?;
        (Value::Array(records), "")
    } else {
        let doc = serde_json::from_slice(&bytes).map_err(|_| unreadable())?;
        (doc, r.records.as_deref().unwrap_or(""))
    };
    let segments: Vec<&str> = pointer.split('/').skip(1).collect();
    let mut found = Vec::new();
    walk(&doc, &segments, &mut Vec::new(), &mut found);
    // The records: each array reached, or an object of arrays read by position.
    let mut records: Vec<(Vec<String>, Value)> = Vec::new();
    for (keys, v) in found {
        match (r.columnar, v) {
            (true, Value::Object(o)) => {
                let n = o
                    .values()
                    .filter_map(Value::as_array)
                    .map(Vec::len)
                    .max()
                    .unwrap_or(0);
                for i in 0..n.min(JSON_RECORDS_MAX) {
                    let rec: serde_json::Map<String, Value> = o
                        .iter()
                        .map(|(k, a)| (k.clone(), a.get(i).cloned().unwrap_or(Value::Null)))
                        .collect();
                    records.push((keys.clone(), Value::Object(rec)));
                }
            }
            (false, Value::Array(a)) if !pointer.ends_with('*') => {
                records.extend(a.iter().map(|e| (keys.clone(), e.clone())));
            }
            (false, v) if pointer.contains('*') => records.push((keys, v.clone())),
            _ => return Err(unreadable()),
        }
    }
    if records.is_empty() && !pointer.contains('*') && doc.pointer(pointer).is_none() {
        return Err(unreadable());
    }
    let mut typed = Typed::new(r, row, &[])?;
    for (i, (keys, item)) in records.iter().enumerate() {
        typed.push(i + 1, |c, at| match at {
            At::Key(names) => {
                let as_json = c.kind == ColumnType::Json;
                let parts: Option<Vec<String>> = names
                    .iter()
                    .map(|k| match k.strip_prefix('^') {
                        // `^/cik`: a field of the document, beside the records.
                        Some(root) => cell_of_json(&doc, root, as_json),
                        None => match r.keys.iter().position(|n| n == k) {
                            Some(p) => keys.get(p).cloned(),
                            None => cell_of_json(item, k, as_json),
                        },
                    })
                    .collect();
                parts.map(|p| p.join(" "))
            }
            At::Index(_) => None,
        })?;
    }
    typed.finish()
}

/// The fields a read writes, its columns typed.
pub fn fields(r: &Read) -> Vec<Field> {
    r.columns
        .iter()
        .map(|c| Field::new(&c.name, Builder::data_type(c), true))
        .collect()
}

/// An integer as a decimal of `places` places (`108012`, 5 → `1.08012`);
/// anything else is left for the column to refuse.
fn shifted(s: &str, places: u32) -> String {
    let t = s.trim();
    let (neg, digits) = match t.strip_prefix('-') {
        Some(d) => ("-", d),
        None => ("", t),
    };
    if places == 0 || digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return s.to_string();
    }
    let p = places as usize;
    let padded = format!("{digits:0>width$}", width = p + 1);
    let (int, frac) = padded.split_at(padded.len() - p);
    format!("{neg}{int}.{frac}")
}

/// Fixed-size records (D939): each column reads its type at its byte offset.
fn read_binary(r: &Read, row: &Row, file: &Blob) -> Result<Option<Arc<Batches>>, Failure> {
    let bytes = file.bytes(READ_MAX_BYTES as u64)?;
    let size = r.record_size.unwrap_or(0) as usize;
    if size == 0 || bytes.len() % size != 0 {
        return Err(fail("unreadable", json!({ "what": "record size" })));
    }
    let big = r.endian.as_deref() != Some("little");
    let mut typed = Typed::new(r, row, &[])?;
    for (i, rec) in bytes.chunks_exact(size).enumerate() {
        typed.push(i + 1, |c, at| {
            let At::Index(idx) = at else { return None };
            let off = *idx.first()?;
            let take = |n: usize| rec.get(off..off + n);
            macro_rules! num {
                ($t:ty, $n:expr) => {{
                    let b: [u8; $n] = take($n)?.try_into().ok()?;
                    if big {
                        <$t>::from_be_bytes(b)
                    } else {
                        <$t>::from_le_bytes(b)
                    }
                }};
            }
            Some(match c.binary.as_deref()? {
                "u8" => take(1)?[0].to_string(),
                "u16" => num!(u16, 2).to_string(),
                "u32" => num!(u32, 4).to_string(),
                "u64" => num!(u64, 8).to_string(),
                "i32" => num!(i32, 4).to_string(),
                "i64" => num!(i64, 8).to_string(),
                "f32" => num!(f32, 4).to_string(),
                "f64" => num!(f64, 8).to_string(),
                _ => return None,
            })
        })?;
    }
    typed.finish()
}

/// The typed rows of every file of the input that matches `entry`, one
/// batch file per input file.
pub fn read(r: &Read, input: &[Row]) -> Result<Vec<Arc<Batches>>, Failure> {
    let mut out = Vec::new();
    for row in input {
        let Some(file) = &row.file else { continue };
        if let Some(glob) = &r.entry {
            if !row.text("entry").is_some_and(|e| glob_match(glob, &e)) {
                continue;
            }
        }
        let batches = match r.format {
            Format::Csv => read_csv(r, row, file)?,
            Format::Json | Format::Xml => read_json(r, row, file)?,
            Format::Binary => read_binary(r, row, file)?,
            _ => {
                let format = format!("{:?}", r.format).to_lowercase();
                return Err(fail("format_unsupported", json!({ "format": format })));
            }
        };
        out.extend(batches);
    }
    Ok(out)
}

/// A table of rows written as it is: every column as text.
pub fn rows_batch(rows: &[Row]) -> Result<Option<Arc<Batches>>, Failure> {
    let mut names: Vec<String> = Vec::new();
    for r in rows {
        for k in r.cols.keys() {
            if !names.contains(k) {
                names.push(k.clone());
            }
        }
    }
    if rows.is_empty() || names.is_empty() {
        return Ok(None);
    }
    let schema = Arc::new(Schema::new(
        names
            .iter()
            .map(|n| Field::new(n, DataType::Utf8, true))
            .collect::<Vec<_>>(),
    ));
    let arrays: Vec<ArrayRef> = names
        .iter()
        .map(|n| {
            Arc::new(arrow_array::StringArray::from(
                rows.iter().map(|r| r.text(n)).collect::<Vec<_>>(),
            )) as ArrayRef
        })
        .collect();
    let batch = RecordBatch::try_new(schema.clone(), arrays)
        .map_err(|_| fail("unreadable", json!({ "what": "batch" })))?;
    let mut w = BatchWriter::new(schema)?;
    w.write(&batch)?;
    w.finish().map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_are_exact_or_refused() {
        assert_eq!(decimal("1.10427", 8), Some(110_427_000));
        assert_eq!(decimal("-0.5", 2), Some(-50));
        assert_eq!(decimal("12", 0), Some(12));
        assert_eq!(decimal("1.123", 2), None, "never rounded");
        assert_eq!(decimal("1e5", 2), None);
        assert_eq!(decimal("", 2), None);
    }

    #[test]
    fn a_csv_without_header_reads_date_and_time_in_new_york() {
        let r: Read = serde_json::from_value(json!({
            "as": "bars", "from": "f", "format": "csv", "header": false,
            "columns": [
                {"name": "ts", "at": [0, 1], "type": "timestamp", "format": "%Y.%m.%d %H:%M", "timezone": "America/New_York"},
                {"name": "open", "at": 2, "type": "decimal", "scale": 8},
                {"name": "interval", "type": "text", "value": "1m"},
                {"name": "pair", "type": "text", "value": "{{pair}}"},
                {"name": "trades", "type": "integer", "value": null}
            ]
        }))
        .unwrap();
        let mut row = Row::default();
        row.cols.insert("pair".into(), "EURUSD".into());
        row.file = Some(Arc::new(Blob::Mem(
            b"2024.07.05,16:59,1.08,1,1,1,0\n2024.01.02,08:30,1.1,1,1,1,0\n".to_vec(),
        )));
        let rec = rows_of(&read(&r, &[row]).unwrap());
        assert!(
            rec[0]["ts"]
                .as_str()
                .unwrap()
                .starts_with("2024-07-05T20:59:00"),
            "{rec:?}"
        );
        assert!(rec[1]["ts"]
            .as_str()
            .unwrap()
            .starts_with("2024-01-02T13:30:00"));
        assert_eq!(rec[0]["open"], "1.08000000");
        assert_eq!(rec[0]["interval"], "1m");
        assert_eq!(rec[0]["pair"], "EURUSD");
        assert!(rec[0].get("trades").is_none());
    }

    #[test]
    fn a_bad_cell_names_its_line() {
        let r: Read = serde_json::from_value(json!({
            "as": "t", "from": "f", "format": "csv",
            "columns": [{"name": "v", "type": "decimal", "scale": 2}]
        }))
        .unwrap();
        let mut row = Row::default();
        row.file = Some(Arc::new(Blob::Mem(b"v\n1.5\nabc\n".to_vec())));
        let e = read(&r, &[row]).unwrap_err();
        assert_eq!((e.code, e.args["line"].as_u64()), ("unreadable", Some(3)));
    }

    fn rows_of(files: &[Arc<Batches>]) -> Vec<Value> {
        files
            .iter()
            .flat_map(|f| f.read().unwrap().map(Result::unwrap).collect::<Vec<_>>())
            .flat_map(|b| crate::services::datasets::parquet::records(&b).unwrap())
            .collect()
    }

    /// Binance's files: a header line in some, none in others; the time
    /// unit moved from ms to µs; `buyer is maker` read as a side.
    #[test]
    fn an_auto_header_epoch_times_and_mapped_values() {
        let r: Read = serde_json::from_value(json!({
            "as": "t", "from": "f", "format": "csv", "header": "auto",
            "columns": [
                {"name": "id", "at": 0, "type": "integer"},
                {"name": "ts", "at": 1, "type": "timestamp", "format": "epoch"},
                {"name": "side", "at": 2, "type": "text", "map": {"True": "sell", "False": "buy"}}
            ]
        }))
        .unwrap();
        let file = |text: &str| {
            let mut row = Row::default();
            row.file = Some(Arc::new(Blob::Mem(text.as_bytes().to_vec())));
            row
        };
        let rec = rows_of(
            &read(
                &r,
                &[file("id,time,is_buyer_maker\n1,1704067200000,True\n")],
            )
            .unwrap(),
        );
        assert_eq!(rec.len(), 1, "{rec:?}");
        assert!(rec[0]["ts"]
            .as_str()
            .unwrap()
            .starts_with("2024-01-01T00:00:00"));
        assert_eq!(rec[0]["side"], "sell");
        let rec = rows_of(&read(&r, &[file("2,1735689600000000,False\n")]).unwrap());
        assert!(
            rec[0]["ts"]
                .as_str()
                .unwrap()
                .starts_with("2025-01-01T00:00:00"),
            "{rec:?}"
        );
        assert_eq!(rec[0]["side"], "buy");
    }
}
