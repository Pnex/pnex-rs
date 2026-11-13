//! `extract` and `unpack` (pipeline-engine.md D936): rows drawn from the
//! files of a working table. Fetched files are hostile input: archives are
//! capped by entries and by unpacked bytes, pages are only parsed.

use std::io::Read;
use std::sync::Arc;

use pnex_core::pipeline::doc::{Archive, Extract, Read as ReadStep, Unpack, FETCH_MAX_BYTES};
use pnex_core::pipeline::glob_match;
use serde_json::{json, Map, Value};

use super::blob::{merged, Batches, Blob};
use super::{fail, Failure, Row};

/// Entries one archive may hold.
const ENTRIES_MAX: usize = 10_000;
/// Entries of one streamed tar at most (a day of traces, one per aircraft).
const STREAM_ENTRIES_MAX: usize = 2_000_000;
/// Entry bytes read together into one batch file of a streamed tar.
const STREAM_GROUP_BYTES: u64 = 64 * 1024 * 1024;
/// Rows one extraction gives from one file.
const EXTRACT_MAX: usize = 100_000;
/// Largest page or document an extraction parses.
const PAGE_MAX: u64 = 64 * 1024 * 1024;

fn unreadable(what: &str) -> Failure {
    fail("unreadable", json!({ "what": what }))
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        v => Some(v.to_string()),
    }
}

/// Rows of a page or a document, then the `where` and `latest_by` of the step.
pub fn extract(e: &Extract, input: &[Row]) -> Result<Vec<Row>, Failure> {
    let mut out = Vec::new();
    for row in input {
        let Some(file) = &row.file else { continue };
        let base = row.text("url").and_then(|u| reqwest::Url::parse(&u).ok());
        let absolute = |href: &str| match &base {
            Some(b) => b.join(href).map(|u| u.to_string()).ok(),
            None => reqwest::Url::parse(href).map(|u| u.to_string()).ok(),
        };
        let child = |cols: Map<String, Value>| {
            let mut r = Row {
                cols: row.cols.clone(),
                file: None,
            };
            r.cols.extend(cols);
            r
        };
        let bytes = file.bytes(PAGE_MAX)?;
        let text = String::from_utf8_lossy(&bytes);
        if let Some(sel) = &e.form {
            let sel = scraper::Selector::parse(sel).map_err(|_| unreadable("selector"))?;
            let html = scraper::Html::parse_document(&text);
            let Some(form) = html.select(&sel).next() else {
                continue;
            };
            let inputs =
                scraper::Selector::parse("input[name]").map_err(|_| unreadable("selector"))?;
            let mut fields = Map::new();
            let mut cols = Map::new();
            for i in form.select(&inputs) {
                let name = i.value().attr("name").unwrap_or_default().to_string();
                let value = Value::String(i.value().attr("value").unwrap_or_default().to_string());
                // A field never hides a column of the row it came from.
                if pnex_core::pipeline::doc::valid_name(&name) && !row.cols.contains_key(&name) {
                    cols.insert(name.clone(), value.clone());
                }
                fields.insert(name, value);
            }
            let action = form.value().attr("action").unwrap_or_default();
            cols.insert("fields".into(), Value::Object(fields));
            if let Some(a) = absolute(action) {
                cols.insert("action".into(), Value::String(a));
            }
            out.push(child(cols));
        } else if let Some(sel) = &e.links {
            let sel = scraper::Selector::parse(sel).map_err(|_| unreadable("selector"))?;
            let html = scraper::Html::parse_document(&text);
            for a in html.select(&sel).take(EXTRACT_MAX) {
                let Some(href) = a.value().attr("href").and_then(absolute) else {
                    continue;
                };
                let mut cols = Map::new();
                cols.insert("url".into(), Value::String(href));
                cols.insert(
                    "text".into(),
                    Value::String(a.text().collect::<String>().trim().to_string()),
                );
                out.push(child(cols));
            }
        } else if let Some(pointer) = &e.json {
            let doc: Value = serde_json::from_slice(&bytes).map_err(|_| unreadable("json"))?;
            let items = doc
                .pointer(pointer)
                .and_then(Value::as_array)
                .ok_or_else(|| unreadable("json"))?;
            for item in items.iter().take(EXTRACT_MAX) {
                let cols = match item {
                    Value::Object(o) => o.clone(),
                    v => Map::from_iter([("value".to_string(), v.clone())]),
                };
                out.push(child(cols));
            }
        } else if let Some(pattern) = &e.regex {
            let re = regex::Regex::new(pattern).map_err(|_| unreadable("regex"))?;
            for caps in re.captures_iter(&text).take(EXTRACT_MAX) {
                let cols = re
                    .capture_names()
                    .flatten()
                    .filter_map(|n| {
                        Some((n.to_string(), Value::String(caps.name(n)?.as_str().into())))
                    })
                    .collect();
                out.push(child(cols));
            }
        }
    }
    out.retain(|r| {
        e.r#where.iter().all(|(c, glob)| {
            r.cols
                .get(c)
                .and_then(text_of)
                .is_some_and(|v| glob_match(glob, &v))
        })
    });
    if let Some(col) = &e.latest_by {
        let best = out
            .iter()
            .enumerate()
            .filter_map(|(i, r)| Some((r.cols.get(col).and_then(text_of)?, i)))
            .max()
            .map(|(_, i)| i);
        out = best.map(|i| vec![out.swap_remove(i)]).unwrap_or_default();
    }
    Ok(out)
}

fn kind_of(head: &[u8], declared: Archive) -> Archive {
    match declared {
        Archive::Auto if head.starts_with(b"PK\x03\x04") => Archive::Zip,
        Archive::Auto if head.starts_with(&[0x1f, 0x8b]) => Archive::Gzip,
        Archive::Auto if head.len() > 262 && &head[257..262] == b"ustar" => Archive::Tar,
        other => other,
    }
}

/// One row per entry kept, its file the unpacked bytes, spilled to disk
/// past a few MB.
pub fn unpack(u: &Unpack, input: &[Row]) -> Result<Vec<Row>, Failure> {
    let mut out = Vec::new();
    for row in input {
        let Some(file) = &row.file else { continue };
        let name_of_file = row
            .text("url")
            .and_then(|s| s.rsplit('/').next().map(str::to_string))
            .unwrap_or_default();
        let mut head = [0u8; 263];
        let n = file
            .open()?
            .read(&mut head)
            .map_err(|_| unreadable("archive"))?;
        let mut budget = FETCH_MAX_BYTES;
        let mut entries: Vec<(String, Blob)> = Vec::new();
        let keep =
            |name: &str| u.entries.is_empty() || u.entries.iter().any(|g| glob_match(g, name));
        let kind = kind_of(&head[..n], u.format);
        match kind {
            Archive::Zip => {
                let mut z = zip::ZipArchive::new(file.open()?).map_err(|_| unreadable("zip"))?;
                if z.len() > ENTRIES_MAX {
                    return Err(fail("too_large", json!({ "entries": ENTRIES_MAX })));
                }
                for i in 0..z.len() {
                    let f = z.by_index(i).map_err(|_| unreadable("zip"))?;
                    let name = f.name().map_err(|_| unreadable("zip"))?.to_string();
                    if f.is_dir() || !keep(&name) {
                        continue;
                    }
                    let b = Blob::from_reader(f, budget)?;
                    budget -= b.len();
                    entries.push((name, b));
                }
            }
            Archive::Tar => {
                let mut t = tar::Archive::new(file.open()?);
                for (n, entry) in t.entries().map_err(|_| unreadable("tar"))?.enumerate() {
                    if n >= ENTRIES_MAX {
                        return Err(fail("too_large", json!({ "entries": ENTRIES_MAX })));
                    }
                    let entry = entry.map_err(|_| unreadable("tar"))?;
                    let name = entry
                        .path()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if !entry.header().entry_type().is_file() || !keep(&name) {
                        continue;
                    }
                    let b = Blob::from_reader(entry, budget)?;
                    budget -= b.len();
                    entries.push((name, b));
                }
            }
            Archive::Gzip => {
                let name = name_of_file.trim_end_matches(".gz").to_string();
                let b = Blob::from_reader(flate2::read::MultiGzDecoder::new(file.open()?), budget)?;
                entries.push((name, b));
            }
            Archive::Lzma => {
                // Small files (an hour of ticks): read whole, capped by the reader.
                let raw = file.bytes(PAGE_MAX)?;
                let bytes = pnex_core::import_read::unlzma(&raw).map_err(|_| unreadable("lzma"))?;
                entries.push((
                    name_of_file.trim_end_matches(".bi5").to_string(),
                    Blob::Mem(bytes),
                ));
            }
            Archive::Auto => return Err(unreadable("archive")),
        }
        // Entries asked by name that the archive lacks: a changed export,
        // not an empty one.
        if entries.is_empty()
            && !u.entries.is_empty()
            && !matches!(kind, Archive::Gzip | Archive::Lzma)
        {
            return Err(fail("entry_missing", json!({ "entries": u.entries })));
        }
        for (name, b) in entries {
            let mut r = row.clone();
            r.cols.insert("entry".into(), Value::String(name));
            r.file = Some(Arc::new(b));
            out.push(r);
        }
    }
    Ok(out)
}

/// The chunks of a streamed body as one reader, refused past `max` bytes.
struct ChunkReader {
    rx: tokio::sync::mpsc::Receiver<Result<Vec<u8>, ()>>,
    buf: Vec<u8>,
    pos: usize,
    total: u64,
    max: u64,
    over: bool,
}

impl Read for ChunkReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos == self.buf.len() {
            match self.rx.blocking_recv() {
                Some(Ok(b)) => {
                    self.total += b.len() as u64;
                    if self.total > self.max {
                        self.over = true;
                        return Err(std::io::Error::other("too large"));
                    }
                    (self.buf, self.pos) = (b, 0);
                }
                Some(Err(())) => return Err(std::io::Error::other("stream cut")),
                None => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// A streamed tar's entries kept by `u`, read by `r` in groups as they
/// pass: nothing of the archive stays but the typed rows (D943).
pub fn unpack_stream(
    u: &Unpack,
    r: &ReadStep,
    row: &Row,
    rx: tokio::sync::mpsc::Receiver<Result<Vec<u8>, ()>>,
    max: u64,
) -> Result<Vec<Arc<Batches>>, Failure> {
    let mut reader = ChunkReader {
        rx,
        buf: Vec::new(),
        pos: 0,
        total: 0,
        max,
        over: false,
    };
    let walked = walk_tar(u, r, row, &mut reader);
    if reader.over {
        return Err(fail("too_large", json!({ "max": max })));
    }
    let out = walked?;
    // The tar's padding: read to the end so the sender finishes cleanly.
    std::io::copy(&mut reader, &mut std::io::sink()).map_err(|_| unreadable("tar"))?;
    Ok(out)
}

fn walk_tar(
    u: &Unpack,
    r: &ReadStep,
    row: &Row,
    reader: &mut ChunkReader,
) -> Result<Vec<Arc<Batches>>, Failure> {
    let keep = |name: &str| u.entries.is_empty() || u.entries.iter().any(|g| glob_match(g, name));
    let (mut out, mut group, mut bytes, mut kept) = (Vec::new(), Vec::new(), 0u64, 0usize);
    let flush = |group: &mut Vec<Row>, out: &mut Vec<Arc<Batches>>| -> Result<(), Failure> {
        let files = super::read::read(r, group)?;
        group.clear();
        if !files.is_empty() {
            out.push(merged(&files)?);
        }
        Ok(())
    };
    let mut t = tar::Archive::new(reader);
    for (n, entry) in t.entries().map_err(|_| unreadable("tar"))?.enumerate() {
        if n >= STREAM_ENTRIES_MAX {
            return Err(fail("too_large", json!({ "entries": STREAM_ENTRIES_MAX })));
        }
        let entry = entry.map_err(|_| unreadable("tar"))?;
        let name = entry
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !entry.header().entry_type().is_file() || !keep(&name) {
            continue;
        }
        let b = Blob::from_reader(entry, super::read::READ_MAX_BYTES as u64)?;
        bytes += b.len();
        kept += 1;
        let mut e = row.clone();
        e.cols.insert("entry".into(), Value::String(name));
        e.file = Some(Arc::new(b));
        group.push(e);
        if bytes >= STREAM_GROUP_BYTES {
            flush(&mut group, &mut out)?;
            bytes = 0;
        }
    }
    flush(&mut group, &mut out)?;
    // Entries asked by name that a non-empty archive lacks: a changed export.
    if kept == 0 && !u.entries.is_empty() && t.into_inner().total > 0 {
        return Err(fail("entry_missing", json!({ "entries": u.entries })));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(body: &str) -> Row {
        let mut r = Row::default();
        r.cols.insert("pair".into(), "EURUSD".into());
        r.cols
            .insert("url".into(), "https://data.example.io/p/2024".into());
        r.file = Some(Arc::new(Blob::Mem(body.as_bytes().to_vec())));
        r
    }

    #[test]
    fn a_form_gives_its_fields_and_action() {
        let e: Extract = serde_json::from_value(json!({
            "as": "form", "from": "page", "form": "#file_down", "where": {"tk": "?*"}
        }))
        .unwrap();
        let html = r#"<form id="file_down" action="/get.php"><input name="tk" value="abc"/><input name="pair" value="x"/></form>"#;
        let rows = extract(&e, &[page(html)]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].text("action").as_deref(),
            Some("https://data.example.io/get.php")
        );
        assert_eq!(
            rows[0].text("pair").as_deref(),
            Some("EURUSD"),
            "a field never hides a column"
        );
        assert_eq!(rows[0].cols["fields"]["pair"], "x");
        let empty = r#"<form id="file_down"><input name="tk" value=""/></form>"#;
        assert!(extract(&e, &[page(empty)]).unwrap().is_empty());
    }

    #[test]
    fn the_latest_matching_resource_is_kept() {
        let e: Extract = serde_json::from_value(json!({
            "as": "link", "from": "c", "json": "/resources",
            "where": {"title": "export-*.zip"}, "latest_by": "last_modified"
        }))
        .unwrap();
        let body = r#"{"resources": [
            {"title": "export-1.zip", "url": "https://d.example.io/1", "last_modified": "2026-01-01"},
            {"title": "export-2.zip", "url": "https://d.example.io/2", "last_modified": "2026-03-01"},
            {"title": "notes.txt", "url": "https://d.example.io/3", "last_modified": "2026-05-01"}]}"#;
        let rows = extract(&e, &[page(body)]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].text("url").as_deref(),
            Some("https://d.example.io/2")
        );
    }

    #[test]
    fn a_zip_keeps_the_entries_asked_and_refuses_junk() {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in [("a.csv", "x"), ("a.txt", "y")] {
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        let mut row = Row::default();
        row.file = Some(Arc::new(Blob::Mem(w.finish().unwrap().into_inner())));
        let u: Unpack =
            serde_json::from_value(json!({"as": "f", "from": "z", "entries": ["*.csv"]})).unwrap();
        let rows = unpack(&u, &[row]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].text("entry").as_deref(), Some("a.csv"));
        let mut junk = Row::default();
        junk.file = Some(Arc::new(Blob::Mem(b"not an archive".to_vec())));
        assert_eq!(unpack(&u, &[junk]).unwrap_err().code, "unreadable");
    }
}
