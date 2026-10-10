//! Document text extraction and chunking for the media library search
//! (doc-search.md P1, decision #22).
//!
//! Format detection and chunking are pure and wasm-safe; the extractors
//! themselves (zip, PDF, spreadsheets) live behind the `doc-extract`
//! feature, enabled by the backend worker only. Shared with the import of
//! `pages.md` (P11).

use serde::{Deserialize, Serialize};

/// Media kind of text documents (txt, md, docx, pdf).
pub const KIND_DOCUMENT: &str = "document";
/// Media kind of tabular files (csv, xlsx, ods).
pub const KIND_TABLE: &str = "table";

/// Indexing states of `media_text_index.status`.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_EXTRACTING: &str = "extracting";
pub const STATUS_INDEXED: &str = "indexed";
pub const STATUS_ERROR: &str = "error";
pub const STATUS_NEEDS_OCR: &str = "needs_ocr";

/// Target chunk size in characters (~500 tokens).
pub const CHUNK_CHARS: usize = 2000;
/// Overlap between consecutive chunks of one section (~15 %).
pub const CHUNK_OVERLAP: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocFormat {
    Text,
    Markdown,
    Docx,
    Pdf,
    Csv,
    Xlsx,
    Ods,
}

impl DocFormat {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Csv | Self::Xlsx | Self::Ods => KIND_TABLE,
            _ => KIND_DOCUMENT,
        }
    }
}

/// Detects an indexable format from magic bytes, the extension only
/// disambiguating within a container (zip) or among plain text files.
pub fn format_of(filename: &str, bytes: &[u8]) -> Option<DocFormat> {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if bytes.starts_with(b"%PDF-") {
        return Some(DocFormat::Pdf);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        // A bare zip renamed .docx is refused: the container must hold the
        // part that makes it an office document (entry names are stored in
        // clear in the local and central headers).
        return match ext.as_str() {
            "docx" if contains(bytes, b"word/document.xml") => Some(DocFormat::Docx),
            "xlsx" if contains(bytes, b"xl/workbook.xml") => Some(DocFormat::Xlsx),
            // ODF mandates an uncompressed `mimetype` first entry (offset 30).
            "ods"
                if bytes.get(30..).is_some_and(|b| {
                    b.starts_with(b"mimetypeapplication/vnd.oasis.opendocument.spreadsheet")
                }) =>
            {
                Some(DocFormat::Ods)
            }
            _ => None,
        };
    }
    let format = match ext.as_str() {
        "txt" | "text" | "log" => DocFormat::Text,
        "md" | "markdown" => DocFormat::Markdown,
        "csv" | "tsv" => DocFormat::Csv,
        _ => return None,
    };
    // Plain text only: a NUL byte in the head means binary under a text name.
    let head = &bytes[..bytes.len().min(8192)];
    (!head.contains(&0)).then_some(format)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// One structural unit of a document: the citation granularity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub heading: Option<String>,
    /// 1-based page (PDF) or sheet number (spreadsheets).
    pub page: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedDoc {
    pub sections: Vec<Section>,
    pub page_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub ord: u32,
    pub page: Option<u32>,
    pub heading: Option<String>,
    pub content: String,
}

/// Splits sections into chunks of ~[`CHUNK_CHARS`] with overlap, cutting on
/// whitespace; a chunk never spans two sections (hence two pages).
pub fn chunk(doc: &ExtractedDoc) -> Vec<Chunk> {
    let mut out = Vec::new();
    for s in &doc.sections {
        let text = s.text.trim();
        if text.is_empty() {
            continue;
        }
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut start = 0;
        while start < chars.len() {
            let mut end = (start + CHUNK_CHARS).min(chars.len());
            if end < chars.len() {
                // Back off to the last whitespace in the second half.
                if let Some(ws) = (start + CHUNK_CHARS / 2..end)
                    .rev()
                    .find(|&i| chars[i].1.is_whitespace())
                {
                    end = ws;
                }
            }
            let from = chars[start].0;
            let to = chars.get(end).map_or(text.len(), |c| c.0);
            out.push(Chunk {
                ord: out.len() as u32,
                page: s.page,
                heading: s.heading.clone(),
                content: text[from..to].trim().to_string(),
            });
            if end >= chars.len() {
                break;
            }
            start = end.saturating_sub(CHUNK_OVERLAP).max(start + 1);
        }
    }
    out
}

/// Markdown and plain text: `#` headings start sections (outside fences).
pub fn extract_text(text: &str, markdown: bool) -> ExtractedDoc {
    let mut sections = vec![Section::default()];
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if markdown && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = !in_fence;
        }
        if markdown && !in_fence && trimmed.starts_with('#') {
            let heading = trimmed.trim_start_matches('#').trim();
            if !heading.is_empty() {
                sections.push(Section {
                    heading: Some(heading.to_string()),
                    ..Section::default()
                });
                continue;
            }
        }
        let cur = sections.last_mut().expect("never empty");
        cur.text.push_str(line);
        cur.text.push('\n');
    }
    sections.retain(|s| !s.text.trim().is_empty() || s.heading.is_some());
    ExtractedDoc {
        sections,
        page_count: None,
    }
}

#[cfg(feature = "doc-extract")]
pub use extract::{extract, ExtractError, Limits};

#[cfg(feature = "doc-extract")]
mod extract {
    use super::*;
    use std::io::{Cursor, Read};

    /// Resource ceilings of one extraction (zip bombs, huge sheets).
    #[derive(Debug, Clone, Copy)]
    pub struct Limits {
        /// Sum of the uncompressed sizes of a zip container.
        pub max_unzipped: u64,
        pub max_zip_entries: usize,
        /// Extracted text kept, in bytes; the rest is dropped.
        pub max_text: usize,
        pub max_pages: u32,
    }

    impl Default for Limits {
        fn default() -> Self {
            Self {
                max_unzipped: 256 * 1024 * 1024,
                max_zip_entries: 10_000,
                max_text: 32 * 1024 * 1024,
                max_pages: 2000,
            }
        }
    }

    /// Machine codes, mapped to `err_codes` by the backend.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ExtractError {
        Unsupported,
        TooLarge,
        Malformed,
        /// A PDF with no text layer (scanned): OCR is phase 5.
        NeedsOcr,
    }

    pub fn extract(
        format: DocFormat,
        bytes: &[u8],
        limits: &Limits,
    ) -> Result<ExtractedDoc, ExtractError> {
        let mut doc = match format {
            DocFormat::Text | DocFormat::Markdown | DocFormat::Csv => {
                let text = String::from_utf8_lossy(bytes);
                // CSV is searched as text (lines keep their cells together);
                // SQL over tables is P3.
                extract_text(&text, format == DocFormat::Markdown)
            }
            DocFormat::Docx => docx(bytes, limits)?,
            DocFormat::Pdf => pdf(bytes, limits)?,
            DocFormat::Xlsx | DocFormat::Ods => sheets(bytes, limits)?,
        };
        truncate(&mut doc, limits.max_text);
        Ok(doc)
    }

    fn truncate(doc: &mut ExtractedDoc, max: usize) {
        let mut left = max;
        doc.sections.retain_mut(|s| {
            if left == 0 {
                return false;
            }
            if s.text.len() > left {
                let mut cut = left;
                while !s.text.is_char_boundary(cut) {
                    cut -= 1;
                }
                s.text.truncate(cut);
            }
            left -= s.text.len();
            true
        });
    }

    /// Rejects zip bombs from the central directory before any inflate.
    fn zip_guard(bytes: &[u8], limits: &Limits) -> Result<(), ExtractError> {
        let mut zip =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| ExtractError::Malformed)?;
        if zip.len() > limits.max_zip_entries {
            return Err(ExtractError::TooLarge);
        }
        let mut total = 0u64;
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i).map_err(|_| ExtractError::Malformed)?;
            total = total.saturating_add(f.size());
        }
        if total > limits.max_unzipped {
            return Err(ExtractError::TooLarge);
        }
        Ok(())
    }

    fn docx(bytes: &[u8], limits: &Limits) -> Result<ExtractedDoc, ExtractError> {
        use quick_xml::events::Event;
        zip_guard(bytes, limits)?;
        let mut zip =
            zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| ExtractError::Malformed)?;
        let mut xml = String::new();
        zip.by_name("word/document.xml")
            .map_err(|_| ExtractError::Malformed)?
            // The declared size can lie: cap the actual inflate too.
            .take(limits.max_unzipped)
            .read_to_string(&mut xml)
            .map_err(|_| ExtractError::Malformed)?;

        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut sections = vec![Section::default()];
        let (mut para, mut heading_para, mut in_text) = (String::new(), false, false);
        loop {
            match reader.read_event().map_err(|_| ExtractError::Malformed)? {
                Event::Start(e) | Event::Empty(e) => match e.name().as_ref() {
                    b"w:pStyle" => {
                        heading_para = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.as_ref() == b"w:val")
                            .is_some_and(|a| is_heading_style(&a.value));
                    }
                    b"w:t" => in_text = true,
                    b"w:tab" => para.push('\t'),
                    b"w:br" | b"w:cr" => para.push('\n'),
                    _ => {}
                },
                Event::Text(t) if in_text => {
                    para.push_str(&t.decode().map_err(|_| ExtractError::Malformed)?);
                }
                Event::GeneralRef(r) if in_text => {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        para.push(c);
                    } else {
                        para.push_str(match &*r {
                            b"amp" => "&",
                            b"lt" => "<",
                            b"gt" => ">",
                            b"quot" => "\"",
                            b"apos" => "'",
                            _ => "",
                        });
                    }
                }
                Event::End(e) => match e.name().as_ref() {
                    b"w:t" => in_text = false,
                    b"w:p" => {
                        let text = std::mem::take(&mut para);
                        if heading_para && !text.trim().is_empty() {
                            sections.push(Section {
                                heading: Some(text.trim().to_string()),
                                ..Section::default()
                            });
                        } else {
                            let cur = sections.last_mut().expect("never empty");
                            cur.text.push_str(&text);
                            cur.text.push('\n');
                        }
                        heading_para = false;
                    }
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
        }
        sections.retain(|s| !s.text.trim().is_empty() || s.heading.is_some());
        Ok(ExtractedDoc {
            sections,
            page_count: None,
        })
    }

    /// Built-in heading style ids: `Heading1`/`Title` (English Word),
    /// `Titre1`/`Titre` (French Word).
    fn is_heading_style(v: &[u8]) -> bool {
        let v = String::from_utf8_lossy(v).to_ascii_lowercase();
        v.starts_with("heading") || v.starts_with("titre") || v == "title"
    }

    fn pdf(bytes: &[u8], limits: &Limits) -> Result<ExtractedDoc, ExtractError> {
        let pages = pdf_extract::extract_text_from_mem_by_pages(bytes)
            .map_err(|_| ExtractError::Malformed)?;
        if pages.len() > limits.max_pages as usize {
            return Err(ExtractError::TooLarge);
        }
        if pages.iter().all(|p| p.trim().is_empty()) {
            return Err(ExtractError::NeedsOcr);
        }
        Ok(ExtractedDoc {
            page_count: Some(pages.len() as u32),
            sections: pages
                .into_iter()
                .enumerate()
                .map(|(i, text)| Section {
                    heading: None,
                    page: Some(i as u32 + 1),
                    text,
                })
                .collect(),
        })
    }

    /// One section per sheet, one line per row, cells tab-separated.
    fn sheets(bytes: &[u8], limits: &Limits) -> Result<ExtractedDoc, ExtractError> {
        use calamine::Reader;
        zip_guard(bytes, limits)?;
        let mut book = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
            .map_err(|_| ExtractError::Malformed)?;
        let mut sections = Vec::new();
        for (i, name) in book.sheet_names().into_iter().enumerate() {
            let Ok(range) = book.worksheet_range(&name) else {
                continue;
            };
            let mut text = String::new();
            for row in range.rows() {
                let cells: Vec<String> = row.iter().map(ToString::to_string).collect();
                text.push_str(cells.join("\t").trim_end());
                text.push('\n');
                if text.len() > limits.max_text {
                    break;
                }
            }
            sections.push(Section {
                heading: Some(name),
                page: Some(i as u32 + 1),
                text,
            });
        }
        let count = sections.len() as u32;
        Ok(ExtractedDoc {
            sections,
            page_count: Some(count),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Write;

        fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
            let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (name, body) in entries {
                w.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                w.write_all(body.as_bytes()).unwrap();
            }
            w.finish().unwrap().into_inner()
        }

        #[test]
        fn docx_headings_split_sections() {
            let xml = r#"<w:document xmlns:w="x"><w:body>
              <w:p><w:pPr><w:pStyle w:val="Titre1"/></w:pPr><w:r><w:t>Pump P2</w:t></w:r></w:p>
              <w:p><w:r><w:t>Fault E-0457 &amp; vibration</w:t></w:r></w:p>
              <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Wiring</w:t></w:r></w:p>
              <w:p><w:r><w:t>Red</w:t><w:tab/><w:t>24V</w:t></w:r></w:p>
            </w:body></w:document>"#;
            let bytes = zip_of(&[("word/document.xml", xml)]);
            assert_eq!(format_of("a.docx", &bytes), Some(DocFormat::Docx));
            let doc = extract(DocFormat::Docx, &bytes, &Limits::default()).unwrap();
            assert_eq!(doc.sections.len(), 2);
            assert_eq!(doc.sections[0].heading.as_deref(), Some("Pump P2"));
            assert_eq!(doc.sections[0].text.trim(), "Fault E-0457 & vibration");
            assert_eq!(doc.sections[1].text.trim(), "Red\t24V");
        }

        #[test]
        fn zip_bomb_is_refused_before_inflate() {
            let big = "a".repeat(4096);
            let bytes = zip_of(&[("word/document.xml", &big)]);
            let limits = Limits {
                max_unzipped: 1024,
                ..Limits::default()
            };
            assert_eq!(
                extract(DocFormat::Docx, &bytes, &limits),
                Err(ExtractError::TooLarge)
            );
        }

        #[test]
        fn garbage_is_malformed_not_a_panic() {
            let limits = Limits::default();
            assert_eq!(
                extract(DocFormat::Docx, b"PK\x03\x04junk", &limits),
                Err(ExtractError::Malformed)
            );
            assert!(extract(DocFormat::Pdf, b"%PDF-1.4 junk", &limits).is_err());
        }

        /// Real files (Word, LibreOffice, PDF exports): every file of
        /// `PNEX_DOC_SAMPLES` must be detected, extracted and chunked.
        #[test]
        #[ignore = "needs PNEX_DOC_SAMPLES, a folder of real documents"]
        fn real_documents_extract() {
            let dir = std::env::var("PNEX_DOC_SAMPLES").expect("PNEX_DOC_SAMPLES");
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let bytes = std::fs::read(entry.path()).unwrap();
                let format = format_of(&name, &bytes).unwrap_or_else(|| panic!("{name}: format"));
                let doc = extract(format, &bytes, &Limits::default())
                    .unwrap_or_else(|e| panic!("{name}: {e:?}"));
                let chunks = chunk(&doc);
                assert!(!chunks.is_empty(), "{name}: no chunk");
                for c in &chunks {
                    let text = c.content.replace('\n', " | ");
                    println!("{name} p{:?} [{:?}] {text}", c.page, c.heading);
                }
            }
        }

        #[test]
        fn text_is_truncated_on_a_char_boundary() {
            let mut doc = extract_text("éééé\n", false);
            truncate(&mut doc, 3);
            assert_eq!(doc.sections[0].text, "é");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_magic_then_extension() {
        assert_eq!(format_of("x.bin", b"%PDF-1.7"), Some(DocFormat::Pdf));
        assert_eq!(format_of("notes.md", b"# Hi"), Some(DocFormat::Markdown));
        assert_eq!(format_of("m.csv", b"a,b\n1,2"), Some(DocFormat::Csv));
        assert_eq!(format_of("a.txt", b"\x00\x01"), None);
        assert_eq!(format_of("a.zip", b"PK\x03\x04"), None);
        // A plain zip renamed to an office extension is not a document.
        assert_eq!(format_of("a.docx", b"PK\x03\x04photos/a.jpg"), None);
        assert_eq!(format_of("a.xlsx", b"PK\x03\x04word/document.xml"), None);
        let mut ods = b"PK\x03\x04".to_vec();
        ods.resize(30, 0);
        ods.extend_from_slice(b"mimetypeapplication/vnd.oasis.opendocument.spreadsheet");
        assert_eq!(format_of("a.ods", &ods), Some(DocFormat::Ods));
        assert_eq!(format_of("a.jpg", b"\xFF\xD8"), None);
        assert_eq!(DocFormat::Xlsx.kind(), KIND_TABLE);
    }

    #[test]
    fn markdown_headings_ignore_fences() {
        let doc = extract_text("intro\n# A\nbody\n```\n# not a heading\n```\n## B\nx", true);
        let headings: Vec<_> = doc.sections.iter().map(|s| s.heading.clone()).collect();
        assert_eq!(headings, [None, Some("A".into()), Some("B".into())]);
        assert!(doc.sections[1].text.contains("# not a heading"));
    }

    #[test]
    fn chunks_overlap_and_stay_in_their_section() {
        let words = "word ".repeat(1000); // 5000 chars
        let doc = ExtractedDoc {
            sections: vec![
                Section {
                    page: Some(1),
                    text: words.clone(),
                    ..Section::default()
                },
                Section {
                    page: Some(2),
                    text: "short".into(),
                    ..Section::default()
                },
            ],
            page_count: Some(2),
        };
        let chunks = chunk(&doc);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.content.len() <= CHUNK_CHARS));
        assert_eq!(chunks.last().unwrap().page, Some(2));
        assert_eq!(chunks.last().unwrap().content, "short");
        let ords: Vec<u32> = chunks.iter().map(|c| c.ord).collect();
        assert_eq!(ords, (0..chunks.len() as u32).collect::<Vec<_>>());
        // Overlap: the first section's chunks hold more than its text.
        let first: usize = chunks[..chunks.len() - 1]
            .iter()
            .map(|c| c.content.len())
            .sum();
        assert!(first > words.trim().len());
    }
}
