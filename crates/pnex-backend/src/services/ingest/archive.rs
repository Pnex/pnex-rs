//! `archive_extract` (D319): zip, tar, tar.gz and a lone gz, told by their
//! magic bytes (no rar: its only library is not free software). Hostile
//! input: entry count and real inflated size capped (zip bombs), paths
//! leaving their folder refuse the whole archive. The manifest lists every
//! entry, selected or not; the selected ones are written to the staging
//! area.

use std::io::{Cursor, Read};

use pnex_core::err_codes;
use pnex_core::import_read::UNZIPPED_MAX;
use pnex_core::ingest::{glob_match, IngestConfig, IngestMsg, StagedEntry};
use serde::Serialize;
use serde_json::{json, Value};

use super::{input_of, unavailable, Cx, Outcome};

const ENTRIES_MAX: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct TocEntry {
    pub name: String,
    pub bytes: u64,
    pub compressed: u64,
    pub format_guess: &'static str,
    pub selected: bool,
}

/// Selected entries: name and bytes.
type Files = Vec<(String, Vec<u8>)>;
type Extracted = (&'static str, Vec<TocEntry>, Files);

fn guess(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or_default();
    match ext {
        "csv" | "tsv" | "txt" => "csv",
        "xlsx" | "xls" | "ods" => "xlsx",
        "json" | "geojson" => "json",
        "xml" => "xml",
        "zip" | "gz" | "tgz" | "tar" => "archive",
        _ => "other",
    }
}

/// A path that stays inside its folder.
fn safe_path(name: &str) -> bool {
    let n = name.replace('\\', "/");
    !n.starts_with('/') && !n.contains(':') && !n.split('/').any(|c| c == "..")
}

fn selected(globs: &[String], name: &str) -> bool {
    globs.is_empty() || globs.iter().any(|g| glob_match(g.trim(), name))
}

/// Reads `r` to its end, at most `left` bytes (`TooLarge` beyond).
fn read_capped(r: impl Read, left: &mut u64) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    r.take(*left + 1)
        .read_to_end(&mut out)
        .map_err(|_| err_codes::IMPORT_SOURCE_UNREADABLE)?;
    if out.len() as u64 > *left {
        return Err(err_codes::IMPORT_SOURCE_TOO_LARGE);
    }
    *left -= out.len() as u64;
    Ok(out)
}

fn zip_entries(bytes: &[u8], globs: &[String]) -> Result<Extracted, &'static str> {
    let unreadable = err_codes::IMPORT_SOURCE_UNREADABLE;
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| unreadable)?;
    if z.len() > ENTRIES_MAX {
        return Err(err_codes::IMPORT_SOURCE_TOO_LARGE);
    }
    let (mut toc, mut out, mut left) = (Vec::new(), Vec::new(), UNZIPPED_MAX);
    for i in 0..z.len() {
        let f = z.by_index(i).map_err(|_| unreadable)?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().map_err(|_| unreadable)?.to_string();
        if !safe_path(&name) {
            return Err(err_codes::INGEST_UNSAFE_PATH);
        }
        let pick = selected(globs, &name);
        toc.push(TocEntry {
            format_guess: guess(&name),
            bytes: f.size(),
            compressed: f.compressed_size(),
            selected: pick,
            name: name.clone(),
        });
        if pick {
            // The declared size can lie: the inflate itself is capped.
            out.push((name, read_capped(f, &mut left)?));
        }
    }
    Ok(("zip", toc, out))
}

fn tar_entries(
    r: impl Read,
    globs: &[String],
    left: &mut u64,
) -> Result<(Vec<TocEntry>, Files), &'static str> {
    let unreadable = err_codes::IMPORT_SOURCE_UNREADABLE;
    let mut a = tar::Archive::new(r);
    let (mut toc, mut out) = (Vec::new(), Vec::new());
    for e in a.entries().map_err(|_| unreadable)? {
        let e = e.map_err(|_| unreadable)?;
        if !e.header().entry_type().is_file() {
            continue;
        }
        if toc.len() >= ENTRIES_MAX {
            return Err(err_codes::IMPORT_SOURCE_TOO_LARGE);
        }
        let name = String::from_utf8_lossy(&e.path_bytes()).to_string();
        if !safe_path(&name) {
            return Err(err_codes::INGEST_UNSAFE_PATH);
        }
        let size = e.header().size().unwrap_or(0);
        let pick = selected(globs, &name);
        toc.push(TocEntry {
            format_guess: guess(&name),
            bytes: size,
            compressed: size,
            selected: pick,
            name: name.clone(),
        });
        if pick {
            out.push((name, read_capped(e, left)?));
        }
    }
    Ok((toc, out))
}

fn is_tar(head: &[u8]) -> bool {
    head.len() > 262 && &head[257..262] == b"ustar"
}

/// Format, table of contents and selected entries of an archive.
pub(crate) fn extract(
    bytes: &[u8],
    name: &str,
    globs: &[String],
) -> Result<Extracted, &'static str> {
    let mut left = UNZIPPED_MAX;
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return zip_entries(bytes, globs);
    }
    if is_tar(bytes) {
        let (toc, out) = tar_entries(Cursor::new(bytes), globs, &mut left)?;
        return Ok(("tar", toc, out));
    }
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let inner = read_capped(flate2::read::GzDecoder::new(bytes), &mut left)?;
        if is_tar(&inner) {
            let mut left = UNZIPPED_MAX;
            let (toc, out) = tar_entries(Cursor::new(&inner), globs, &mut left)?;
            return Ok(("tar.gz", toc, out));
        }
        let base = name.rsplit('/').next().unwrap_or(name);
        let inner_name = base
            .strip_suffix(".gz")
            .or_else(|| base.strip_suffix(".GZ"))
            .unwrap_or(base)
            .to_string();
        let pick = selected(globs, &inner_name);
        let toc = vec![TocEntry {
            format_guess: guess(&inner_name),
            bytes: inner.len() as u64,
            compressed: bytes.len() as u64,
            selected: pick,
            name: inner_name.clone(),
        }];
        let out = if pick {
            vec![(inner_name, inner)]
        } else {
            vec![]
        };
        return Ok(("gz", toc, out));
    }
    Err(err_codes::INGEST_ARCHIVE_UNSUPPORTED)
}

pub(super) async fn run(cx: &Cx<'_>, cfg: &IngestConfig, msg: IngestMsg) -> Outcome {
    let input = match input_of(&IngestConfig::default(), &msg) {
        Ok(i) => i,
        Err(o) => return *o,
    };
    // shortcut: the archive is read whole (≤ 200 MB, zip needs random
    // access); stream tar/gz from S3 if bigger sources show up.
    match cx.store.size(&input.staging_key).await {
        Ok(Some(n)) if n > UNZIPPED_MAX => {
            return Outcome::fail(err_codes::IMPORT_SOURCE_TOO_LARGE, Value::Null)
        }
        Ok(Some(_)) => {}
        Ok(None) => {
            return Outcome::fail(
                err_codes::INGEST_ENTRY_MISSING,
                json!({ "entry": input.name }),
            )
        }
        Err(_) => return unavailable(),
    }
    let Ok(Some(bytes)) = cx.store.get_bytes(&input.staging_key).await else {
        return unavailable();
    };
    let (globs, name) = (cfg.entries.clone(), input.name.clone());
    let extracted = tokio::task::spawn_blocking(move || extract(&bytes, &name, &globs))
        .await
        .unwrap_or(Err(err_codes::IMPORT_SOURCE_UNREADABLE));
    let (format, toc, files) = match extracted {
        Ok(x) => x,
        Err(code) => return Outcome::fail(code, Value::Null),
    };
    let room = match cx.room().await {
        Ok(n) => n,
        Err(o) => return *o,
    };
    if files.iter().map(|(_, b)| b.len() as u64).sum::<u64>() > room {
        return Outcome::fail(err_codes::INGEST_STAGING_QUOTA, Value::Null);
    }
    let mut entries = Vec::with_capacity(files.len());
    for (name, body) in files {
        let key = cx.key("x", &name);
        let bytes = body.len() as u64;
        if cx.store.put_bytes(&key, body.into()).await.is_err() {
            let mut out = unavailable();
            out.written = entries;
            return out;
        }
        entries.push(StagedEntry {
            name,
            staging_key: key,
            bytes,
        });
    }
    let manifest = json!({
        "format": format,
        "entries": toc,
        "selected": entries.len(),
        "total": toc.len(),
    });
    let mut out = Outcome::ok(
        IngestMsg {
            url: msg.url,
            raw_key: msg.raw_key,
            staging_key: entries
                .first()
                .map(|e| e.staging_key.clone())
                .unwrap_or_default(),
            entries: entries.clone(),
            ..Default::default()
        },
        manifest,
    );
    out.written = entries;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (n, body) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn tar_of(files: &[(&str, &str)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (n, body) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, n, body.as_bytes()).unwrap();
        }
        b.into_inner().unwrap()
    }

    fn gz(bytes: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn zip_lists_everything_and_extracts_the_selection() {
        let z = zip_of(&[
            ("export/Ancienne_Nouvelle_Certification.csv", "a"),
            ("export/Standard.csv", "Numero_Fiche\nRNCP1"),
            ("export/Partenaires.csv", "x"),
        ]);
        let globs = vec!["*Standard*.csv".into(), "*Partenaires*.csv".into()];
        let (fmt, toc, out) = extract(&z, "f.zip", &globs).unwrap();
        assert_eq!(fmt, "zip");
        assert_eq!(toc.len(), 3);
        assert!(!toc[0].selected && toc[1].selected && toc[2].selected);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].1, b"Numero_Fiche\nRNCP1");
    }

    #[test]
    fn tar_gz_and_gz() {
        let t = gz(&tar_of(&[("a.csv", "1"), ("b.json", "{}")]));
        let (fmt, toc, out) = extract(&t, "x.tar.gz", &[]).unwrap();
        assert_eq!((fmt, toc.len(), out.len()), ("tar.gz", 2, 2));
        let (fmt, toc, out) = extract(&gz(b"a;b\n1;2"), "data.csv.gz", &[]).unwrap();
        assert_eq!((fmt, toc[0].name.as_str()), ("gz", "data.csv"));
        assert_eq!(out[0].1, b"a;b\n1;2");
        let (fmt, ..) = extract(&tar_of(&[("a.csv", "1")]), "x.tar", &[]).unwrap();
        assert_eq!(fmt, "tar");
    }

    #[test]
    fn hostile_archives_are_refused() {
        assert_eq!(
            extract(b"Rar!\x1a\x07\x00", "x.rar", &[]).unwrap_err(),
            err_codes::INGEST_ARCHIVE_UNSUPPORTED
        );
        let z = zip_of(&[("../../etc/passwd", "x")]);
        assert_eq!(
            extract(&z, "z.zip", &[]).unwrap_err(),
            err_codes::INGEST_UNSAFE_PATH
        );
        assert!(!safe_path("/abs.csv") && !safe_path("a\\..\\b") && safe_path("a/b..c.csv"));
    }
}
