//! `table_inspect` (D320): describes a staged table without changing it —
//! format, encoding, separator, exact row count, per-column fill rate,
//! distinct values and examples, a few sample rows — and stops the chain
//! on a missing expected column (`import-schema-changed`, D303). Also the
//! table reader of `rows_filter`.

use std::collections::HashSet;

use pnex_core::err_codes;
use pnex_core::import_read::{read_records, text_of, UNZIPPED_MAX};
use pnex_core::ingest::{IngestConfig, IngestMsg, StagedEntry};
use pnex_core::ontology::import::{parse_csv, Mapping, SourceFormat};
use serde_json::{json, Map, Value};

use super::{input_of, unavailable, Cx, Outcome};

const SAMPLE_ROWS: usize = 5;
const EXAMPLES: usize = 3;
/// Distinct values counted exactly up to this, then reported as a floor.
const DISTINCT_MAX: usize = 100_000;

/// A table read from the staging area.
pub(crate) struct Table {
    pub format: &'static str,
    pub encoding: &'static str,
    pub bom: bool,
    /// CSV only.
    pub delimiter: Option<char>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

fn delimiter_of(head: &str) -> char {
    [';', ',', '\t', '|']
        .into_iter()
        .max_by_key(|d| head.matches(*d).count())
        .unwrap_or(',')
}

fn cell_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Parses a staged file: CSV without the record cap (filtering comes
/// before it), the other formats through the D301 readers.
pub(crate) fn read_table(bytes: &[u8], name: &str) -> Result<Table, &'static str> {
    let unreadable = err_codes::IMPORT_SOURCE_UNREADABLE;
    let trimmed = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .map_or(&bytes[..0], |i| &bytes[i..]);
    let fmt = if bytes.starts_with(b"PK\x03\x04") {
        SourceFormat::Xlsx
    } else {
        match SourceFormat::of_name(name) {
            Some(f) => f,
            None if trimmed.starts_with(b"{") || trimmed.starts_with(b"[") => SourceFormat::Json,
            None if trimmed.starts_with(b"<") => SourceFormat::Xml,
            None => SourceFormat::Csv,
        }
    };
    let encoding = if std::str::from_utf8(bytes).is_ok() {
        "utf-8"
    } else {
        "latin-1"
    };
    let bom = bytes.starts_with(&[0xef, 0xbb, 0xbf]);
    if fmt == SourceFormat::Csv {
        let text = text_of(bytes);
        let head = text
            .trim_start_matches('\u{feff}')
            .lines()
            .next()
            .unwrap_or_default();
        let delim = delimiter_of(head);
        let mut table = parse_csv(&text, Some(delim)).into_iter();
        let columns: Vec<String> = table
            .next()
            .unwrap_or_default()
            .into_iter()
            .map(|h| h.trim().to_string())
            .collect();
        return Ok(Table {
            format: "csv",
            encoding,
            bom,
            delimiter: Some(delim),
            columns,
            rows: table.collect(),
        });
    }
    let m: Mapping =
        serde_json::from_value(json!({ "key": "inspect", "name": "inspect", "format": fmt }))
            .map_err(|_| unreadable)?;
    let records = read_records(bytes, name, &m).map_err(|e| match e {
        pnex_core::import_read::ReadError::TooLarge => err_codes::IMPORT_SOURCE_TOO_LARGE,
        _ => unreadable,
    })?;
    let mut columns: Vec<String> = Vec::new();
    for r in &records {
        for k in r.as_object().into_iter().flat_map(Map::keys) {
            if !columns.contains(k) {
                columns.push(k.clone());
            }
        }
    }
    let rows = records
        .iter()
        .map(|r| {
            columns
                .iter()
                .map(|c| r.get(c).map(cell_text).unwrap_or_default())
                .collect()
        })
        .collect();
    Ok(Table {
        format: match fmt {
            SourceFormat::Xlsx => "xlsx",
            SourceFormat::Json => "json",
            SourceFormat::Xml => "xml",
            SourceFormat::Csv => "csv",
        },
        encoding,
        bom,
        delimiter: None,
        columns,
        rows,
    })
}

/// The manifest of a table.
pub(crate) fn describe(t: &Table) -> Value {
    let n = t.rows.len();
    let columns: Vec<Value> = t
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let (mut filled, mut seen, mut examples) = (0usize, HashSet::new(), Vec::new());
            for r in &t.rows {
                let v = r.get(i).map(|s| s.trim()).unwrap_or_default();
                if v.is_empty() {
                    continue;
                }
                filled += 1;
                if seen.len() < DISTINCT_MAX && seen.insert(v) && examples.len() < EXAMPLES {
                    examples.push(v.to_string());
                }
            }
            json!({
                "name": name,
                "filled_pct": if n == 0 { 0.0 } else { (filled as f64 * 1000.0 / n as f64).round() / 10.0 },
                "distinct_estimate": seen.len(),
                "distinct_capped": seen.len() >= DISTINCT_MAX,
                "examples": examples,
            })
        })
        .collect();
    let sample: Vec<Value> = t
        .rows
        .iter()
        .take(SAMPLE_ROWS)
        .map(|r| {
            Value::Object(
                t.columns
                    .iter()
                    .zip(r.iter().chain(std::iter::repeat(&String::new())))
                    .map(|(c, v)| (c.clone(), Value::String(v.clone())))
                    .collect(),
            )
        })
        .collect();
    json!({
        "format": t.format,
        "encoding": t.encoding,
        "bom": t.bom,
        "delimiter": t.delimiter.map(String::from),
        "rows": n,
        "columns": columns,
        "sample": sample,
    })
}

/// Expected columns absent from the table (trimmed, exact case).
pub(crate) fn missing(expected: &[String], columns: &[String]) -> Vec<String> {
    expected
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !columns.iter().any(|x| x == c))
        .collect()
}

/// Loads and parses the input file of a reading step.
pub(crate) async fn load(
    cx: &Cx<'_>,
    cfg: &IngestConfig,
    msg: &IngestMsg,
) -> Result<(StagedEntry, Table), Box<Outcome>> {
    let input = input_of(cfg, msg)?;
    // shortcut: the table is read whole (≤ 200 MB); stream it from S3 if
    // larger sources show up.
    match cx.store.size(&input.staging_key).await {
        Ok(Some(n)) if n > UNZIPPED_MAX => {
            return Err(Box::new(Outcome::fail(
                err_codes::IMPORT_SOURCE_TOO_LARGE,
                Value::Null,
            )))
        }
        Ok(Some(_)) => {}
        Ok(None) => {
            return Err(Box::new(Outcome::fail(
                err_codes::INGEST_ENTRY_MISSING,
                json!({ "entry": input.name }),
            )))
        }
        Err(_) => return Err(Box::new(unavailable())),
    }
    let Ok(Some(bytes)) = cx.store.get_bytes(&input.staging_key).await else {
        return Err(Box::new(unavailable()));
    };
    let name = input.name.clone();
    let table = tokio::task::spawn_blocking(move || read_table(&bytes, &name))
        .await
        .unwrap_or(Err(err_codes::IMPORT_SOURCE_UNREADABLE))
        .map_err(|code| Box::new(Outcome::fail(code, Value::Null)))?;
    Ok((input, table))
}

pub(super) async fn run(cx: &Cx<'_>, cfg: &IngestConfig, msg: IngestMsg) -> Outcome {
    let (input, table) = match load(cx, cfg, &msg).await {
        Ok(x) => x,
        Err(o) => return *o,
    };
    let mut manifest = describe(&table);
    manifest["entry"] = Value::String(input.name.clone());
    manifest["staging_key"] = Value::String(input.staging_key.clone());
    let gone = missing(&cfg.expected_columns, &table.columns);
    if !gone.is_empty() {
        manifest["missing_columns"] = json!(gone);
        let mut out = Outcome::fail(
            err_codes::IMPORT_SCHEMA_CHANGED,
            json!({ "columns": gone.join(", ") }),
        );
        out.manifest = manifest;
        return out;
    }
    Outcome::ok(
        IngestMsg {
            staging_key: input.staging_key,
            ..msg
        },
        manifest,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FC: &str = "\"Numero_Fiche\";\"Siret_Certificateur\";\"Nom_Certificateur\"\n\
        \"RS6765\";\"\t80259488700019\";\"TELEPILOTE\"\n\
        \"RS6766\";\"\t80259488700019\";\"TELEPILOTE\"\n\
        \"RNCP1\";\"\t13002526500013\";\"\"\n";

    #[test]
    fn describes_a_france_competences_csv() {
        let t = read_table(FC.as_bytes(), "Certificateurs.csv").unwrap();
        assert_eq!((t.format, t.delimiter, t.rows.len()), ("csv", Some(';'), 3));
        let m = describe(&t);
        assert_eq!(m["rows"], 3);
        assert_eq!(m["columns"][1]["name"], "Siret_Certificateur");
        assert_eq!(m["columns"][1]["distinct_estimate"], 2);
        assert_eq!(m["columns"][2]["filled_pct"], 66.7);
        assert_eq!(m["sample"][0]["Numero_Fiche"], "RS6765");
        assert_eq!(
            missing(&["Numero_Fiche".into(), "Siren".into()], &t.columns),
            ["Siren"]
        );
    }

    #[test]
    fn latin1_and_json() {
        let t = read_table(b"nom,ville\nCaf\xe9,Paris\n", "x.csv").unwrap();
        assert_eq!((t.encoding, t.rows[0][0].as_str()), ("latin-1", "Café"));
        let t = read_table(br#"[{"a":1,"b":"x"},{"a":2}]"#, "x").unwrap();
        assert_eq!(
            (t.format, t.columns.len(), t.rows[1][1].as_str()),
            ("json", 2, "")
        );
    }
}
