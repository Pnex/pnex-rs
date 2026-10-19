//! Readers of import sources (partner-watch.md D301): hostile input, read
//! with caps (§5): zip bombs refused from the central directory and the
//! inflate itself capped, XML without DTD or entities and with a bounded
//! depth, sheets cut at the row cap. Each reader yields the records the
//! mapping plans (`ontology::import`).

use std::io::{Cursor, Read};

use serde_json::{Map, Value};

use crate::ontology::import::{json_rows, parse_csv, table_rows, Mapping, SourceFormat, ROWS_MAX};

/// Decompressed size of a source (zip content, xlsx parts).
pub const UNZIPPED_MAX: u64 = 200 * 1024 * 1024;
const ZIP_ENTRIES_MAX: usize = 10_000;
const XML_DEPTH_MAX: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadError {
    /// Not the expected format, or broken.
    Malformed,
    TooLarge,
    /// The format cannot be told (no mapping format, no known extension).
    UnknownFormat,
}

fn zip_guard(bytes: &[u8]) -> Result<zip::ZipArchive<Cursor<&[u8]>>, ReadError> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| ReadError::Malformed)?;
    if z.len() > ZIP_ENTRIES_MAX {
        return Err(ReadError::TooLarge);
    }
    let mut total = 0u64;
    for i in 0..z.len() {
        let f = z.by_index_raw(i).map_err(|_| ReadError::Malformed)?;
        total = total.saturating_add(f.size());
    }
    if total > UNZIPPED_MAX {
        return Err(ReadError::TooLarge);
    }
    Ok(z)
}

/// The first file of a zip whose extension gives a format (or `want`).
fn unzip(bytes: &[u8], want: Option<SourceFormat>) -> Result<(Vec<u8>, SourceFormat), ReadError> {
    let mut z = zip_guard(bytes)?;
    for i in 0..z.len() {
        let f = z.by_index(i).map_err(|_| ReadError::Malformed)?;
        let Some(fmt) = f.name().ok().and_then(|n| SourceFormat::of_name(&n)) else {
            continue;
        };
        if f.is_dir() || want.is_some_and(|w| w != fmt) {
            continue;
        }
        let mut out = Vec::new();
        // The declared size can lie: cap the actual inflate too.
        f.take(UNZIPPED_MAX + 1)
            .read_to_end(&mut out)
            .map_err(|_| ReadError::Malformed)?;
        if out.len() as u64 > UNZIPPED_MAX {
            return Err(ReadError::TooLarge);
        }
        return Ok((out, fmt));
    }
    Err(ReadError::UnknownFormat)
}

/// UTF-8, else Latin-1 (a CSV saved by a spreadsheet on Windows).
fn text_of(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        // shortcut: Latin-1, not Windows-1252 (0x80–0x9F differ: €, curly
        // quotes); use encoding_rs when such files show up.
        Err(_) => bytes.iter().map(|b| *b as char).collect(),
    }
}

/// Records of a source. `name` (file name or URL path) gives the format
/// when the mapping does not.
pub fn read_records(bytes: &[u8], name: &str, m: &Mapping) -> Result<Vec<Value>, ReadError> {
    let mut fmt = m.format.or_else(|| SourceFormat::of_name(name));
    let owned;
    let mut bytes = bytes;
    let is_zip = bytes.starts_with(b"PK\x03\x04");
    if is_zip && fmt != Some(SourceFormat::Xlsx) {
        let (inner, f) = unzip(bytes, fmt)?;
        owned = inner;
        bytes = &owned;
        fmt = Some(f);
    }
    let rows = match fmt.ok_or(ReadError::UnknownFormat)? {
        SourceFormat::Csv => table_rows(parse_csv(&text_of(bytes), m.delimiter)),
        SourceFormat::Xlsx => sheet_rows(bytes)?,
        SourceFormat::Json => {
            let doc: Value = serde_json::from_slice(bytes).map_err(|_| ReadError::Malformed)?;
            json_rows(&doc, &m.rows).ok_or(ReadError::Malformed)?
        }
        SourceFormat::Xml => xml_rows(&text_of(bytes), &m.rows)?,
    };
    if rows.len() > ROWS_MAX {
        return Err(ReadError::TooLarge);
    }
    Ok(rows)
}

fn cell(c: &calamine::Data) -> String {
    use calamine::Data;
    match c {
        Data::Empty | Data::Error(_) => String::new(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        Data::DateTime(d) => {
            let serial = d.as_f64();
            let day = chrono::NaiveDate::from_ymd_opt(1899, 12, 30).and_then(|o| {
                o.checked_add_days(chrono::Days::new(serial.floor().max(0.0) as u64))
            });
            match day {
                Some(day) => day.format("%Y-%m-%d").to_string(),
                None => String::new(),
            }
        }
        other => other.to_string(),
    }
}

/// The first sheet, its first row as headers.
fn sheet_rows(bytes: &[u8]) -> Result<Vec<Value>, ReadError> {
    use calamine::Reader;
    zip_guard(bytes)?;
    let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|_| ReadError::Malformed)?;
    let Some(first) = book.sheet_names().into_iter().next() else {
        return Ok(Vec::new());
    };
    let range = book
        .worksheet_range(&first)
        .map_err(|_| ReadError::Malformed)?;
    let table: Vec<Vec<String>> = range
        .rows()
        .take(ROWS_MAX + 2)
        .map(|r| r.iter().map(cell).collect())
        .filter(|r: &Vec<String>| r.iter().any(|c| !c.is_empty()))
        .collect();
    Ok(table_rows(table))
}

/// XML → JSON-like records: an element is an object of its attributes
/// (`@name`) and children (repeated ones as arrays), or its text when it
/// has neither. `path` names the record elements from the root
/// (`root/item`; the root alone when empty).
fn xml_rows(text: &str, path: &str) -> Result<Vec<Value>, ReadError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(text);
    // (name, attributes+children, text)
    let mut stack: Vec<(String, Map<String, Value>, String)> = Vec::new();
    let mut root: Option<(String, Value)> = None;
    let attrs = |e: &quick_xml::events::BytesStart| -> Result<Map<String, Value>, ReadError> {
        let mut m = Map::new();
        for a in e.attributes() {
            let a = a.map_err(|_| ReadError::Malformed)?;
            let v = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map_err(|_| ReadError::Malformed)?;
            m.insert(
                format!("@{}", String::from_utf8_lossy(a.key.local_name().as_ref())),
                Value::String(v.into_owned()),
            );
        }
        Ok(m)
    };
    let close = |name: String, map: Map<String, Value>, txt: String| -> (String, Value) {
        // Text is trimmed once whole (references split it in fragments).
        let txt = txt.trim().to_string();
        let v = if map.is_empty() {
            Value::String(txt)
        } else {
            let mut map = map;
            if !txt.is_empty() {
                map.insert("#text".into(), Value::String(txt));
            }
            Value::Object(map)
        };
        (name, v)
    };
    let push_child =
        |stack: &mut Vec<(String, Map<String, Value>, String)>, name: String, v: Value| {
            if let Some((_, parent, _)) = stack.last_mut() {
                match parent.get_mut(&name) {
                    Some(Value::Array(a)) => a.push(v),
                    Some(prev) => {
                        let first = prev.take();
                        *prev = Value::Array(vec![first, v]);
                    }
                    None => {
                        parent.insert(name, v);
                    }
                }
            }
        };
    loop {
        match reader.read_event().map_err(|_| ReadError::Malformed)? {
            // No DTD: no entity can be declared, expanded nor fetched.
            Event::DocType(_) => return Err(ReadError::Malformed),
            Event::Start(e) => {
                if stack.len() >= XML_DEPTH_MAX {
                    return Err(ReadError::TooLarge);
                }
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                stack.push((name, attrs(&e)?, String::new()));
            }
            Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let (n, v) = close(name, attrs(&e)?, String::new());
                if stack.is_empty() {
                    root = Some((n, v));
                } else {
                    push_child(&mut stack, n, v);
                }
            }
            Event::Text(t) => {
                let t = t.decode().map_err(|_| ReadError::Malformed)?;
                if let Some((_, _, txt)) = stack.last_mut() {
                    txt.push_str(&t);
                }
            }
            // `&amp;`, `&#233;`…: predefined and character references only.
            Event::GeneralRef(r) => {
                let c = match r.resolve_char_ref().map_err(|_| ReadError::Malformed)? {
                    Some(c) => c.to_string(),
                    None => {
                        let name = r.decode().map_err(|_| ReadError::Malformed)?;
                        quick_xml::escape::resolve_predefined_entity(&name)
                            .ok_or(ReadError::Malformed)?
                            .to_string()
                    }
                };
                if let Some((_, _, txt)) = stack.last_mut() {
                    txt.push_str(&c);
                }
            }
            Event::CData(t) => {
                if let Some((_, _, txt)) = stack.last_mut() {
                    txt.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::End(_) => {
                let Some((name, map, txt)) = stack.pop() else {
                    return Err(ReadError::Malformed);
                };
                let (n, v) = close(name, map, txt);
                if stack.is_empty() {
                    root = Some((n, v));
                } else {
                    push_child(&mut stack, n, v);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let (root_name, root) = root.ok_or(ReadError::Malformed)?;
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    match segs.next() {
        None => return Ok(vec![root]),
        Some(first) if first != root_name => return Ok(Vec::new()),
        Some(_) => {}
    }
    let mut level = vec![root];
    for seg in segs {
        level = level
            .iter()
            .filter_map(|v| v.get(seg))
            .flat_map(|v| match v {
                Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            })
            .collect();
        if level.len() > ROWS_MAX {
            return Err(ReadError::TooLarge);
        }
    }
    Ok(level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn mapping(format: Option<SourceFormat>, rows: &str) -> Mapping {
        crate::ontology::import::from_value(json!({
            "key": "m", "name": "m", "rows": rows,
            "format": format, "objects": []
        }))
        .unwrap()
    }

    #[test]
    fn xml_records_by_path() {
        let xml = r#"<?xml version="1.0"?><fiches><fiche id="1"><code>RNCP1</code><t>A &amp; B &#233;</t><certificateurs><c>A</c><c>B</c></certificateurs></fiche><fiche id="2"><code>RS2</code></fiche></fiches>"#;
        let rows = read_records(xml.as_bytes(), "x.xml", &mapping(None, "fiches/fiche")).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["@id"], json!("1"));
        assert_eq!(rows[0]["code"], json!("RNCP1"));
        assert_eq!(rows[0]["t"], json!("A & B é"));
        assert_eq!(rows[0]["certificateurs"]["c"], json!(["A", "B"]));
    }

    #[test]
    fn xml_with_a_dtd_is_refused() {
        let xml = r#"<?xml version="1.0"?><!DOCTYPE a [<!ENTITY x SYSTEM "file:///etc/passwd">]><a>&x;</a>"#;
        assert_eq!(
            read_records(xml.as_bytes(), "x.xml", &mapping(None, "")),
            Err(ReadError::Malformed)
        );
    }

    #[test]
    fn zipped_csv_is_read_and_bombs_refused() {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("readme.txt.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(b"ignored").unwrap();
        w.start_file("data.csv", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all("siret;nom\n1;Été\n".as_bytes()).unwrap();
        let bytes = w.finish().unwrap().into_inner();
        let rows = read_records(&bytes, "export.zip", &mapping(None, "")).unwrap();
        assert_eq!(rows, vec![json!({"siret": "1", "nom": "Été"})]);
        // Declared sizes over the cap: refused before any inflate.
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().large_file(true);
        w.start_file("big.csv", opts).unwrap();
        let chunk = vec![b'a'; 1 << 20];
        for _ in 0..201 {
            w.write_all(&chunk).unwrap();
        }
        let bomb = w.finish().unwrap().into_inner();
        assert_eq!(
            read_records(&bomb, "b.zip", &mapping(None, "")),
            Err(ReadError::TooLarge)
        );
    }

    #[test]
    fn latin1_csv_and_json_path() {
        let rows = read_records(b"nom\nB\xe9ziers\n", "a.csv", &mapping(None, "")).unwrap();
        assert_eq!(rows[0]["nom"], json!("Béziers"));
        let doc = br#"{"results": [{"siren": "1"}, {"siren": "2"}], "page": 1}"#;
        let rows = read_records(doc, "api", &mapping(Some(SourceFormat::Json), "results")).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            read_records(b"x", "noext", &mapping(None, "")),
            Err(ReadError::UnknownFormat)
        );
    }
}
