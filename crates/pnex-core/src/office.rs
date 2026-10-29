//! Office documents read and written by flows (network-check.md D416,
//! D420): `document_text` reads the text of a document of the media
//! library, `sheet_write` writes an xlsx workbook into it. Configs and the
//! bodies of the internal routes; the backend checks the node against the
//! deployed version. serde-only, wasm-safe, no `flatten`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ontology::flow::FlowNodeRef;

/// Characters of text a `document_text` answer carries at most.
pub const DOCUMENT_TEXT_MAX: usize = 200_000;
/// Bounds of a workbook written by `sheet_write`.
pub const SHEETS_MAX: usize = 16;
pub const CELLS_MAX: usize = 20_000;
pub const CELL_TEXT_MAX: usize = 32_000;
pub const NAME_MAX: usize = 120;

/// `document_text` has no setting: `msg.payload.media_id` names the media.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DocumentTextConfig {}

/// `POST /internal/flow/document-text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentTextRequest {
    pub node: FlowNodeRef,
    pub media_id: String,
}

/// The text of the current version of a document.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DocumentTextReply {
    pub media_id: String,
    pub version: i64,
    /// Sections joined by blank lines, cut at [`DOCUMENT_TEXT_MAX`].
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

/// `sheet_write` has no setting: `msg.payload` is the workbook.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SheetWriteConfig {}

/// One cell: a reference (`B8`) and its text; no formula.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Cell {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub value: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrap: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Sheet {
    pub name: String,
    #[serde(default)]
    pub cells: Vec<Cell>,
    /// Merged ranges (`B1:E1`).
    #[serde(default)]
    pub merges: Vec<String>,
    /// Column widths in characters, by column letter.
    #[serde(default)]
    pub widths: BTreeMap<String, f64>,
}

/// `msg.payload` of `sheet_write`: the file name and its sheets.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Workbook {
    pub name: String,
    pub sheets: Vec<Sheet>,
}

/// `POST /internal/flow/sheet-write`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SheetWriteRequest {
    pub node: FlowNodeRef,
    pub workbook: Workbook,
}

/// The media written: one asset per node and file name, a new version at
/// each write.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SheetWriteReply {
    pub media_id: String,
    pub version: i64,
    pub filename: String,
}

/// Column letters and 1-based row of a cell reference (`AB12`).
pub fn split_ref(r: &str) -> Option<(&str, u32)> {
    let i = r.find(|c: char| c.is_ascii_digit())?;
    let (col, row) = r.split_at(i);
    let ok = !col.is_empty() && col.len() <= 3 && col.chars().all(|c| c.is_ascii_uppercase());
    let row: u32 = row.parse().ok().filter(|n| (1..=1_048_576).contains(n))?;
    ok.then_some((col, row))
}

/// 1-based column number of letters (`A` = 1, `AA` = 27).
pub fn column_number(col: &str) -> u32 {
    col.bytes().fold(0, |n, b| n * 26 + u32::from(b - b'A' + 1))
}

impl Workbook {
    /// Shape check: the first wrong field.
    pub fn check(&self) -> Option<&'static str> {
        let name_ok = |n: &str| {
            !n.trim().is_empty()
                && n.chars().count() <= NAME_MAX
                && !n.contains(['/', '\\', ':', '*', '?', '[', ']'])
        };
        if !name_ok(&self.name) {
            return Some("name");
        }
        if self.sheets.is_empty() || self.sheets.len() > SHEETS_MAX {
            return Some("sheets");
        }
        let cells: usize = self.sheets.iter().map(|s| s.cells.len()).sum();
        if cells > CELLS_MAX {
            return Some("cells");
        }
        for s in &self.sheets {
            if !name_ok(&s.name) || s.name.chars().count() > 31 {
                return Some("sheets");
            }
            if s.cells.iter().any(|c| {
                split_ref(&c.reference).is_none() || c.value.chars().count() > CELL_TEXT_MAX
            }) {
                return Some("cells");
            }
            let range_ok = |m: &String| {
                m.split_once(':')
                    .is_some_and(|(a, b)| split_ref(a).is_some() && split_ref(b).is_some())
            };
            if !s.merges.iter().all(range_ok)
                || s.widths.iter().any(|(k, w)| {
                    split_ref(&format!("{k}1")).is_none() || !(1.0..=255.0).contains(w)
                })
            {
                return Some("sheets");
            }
        }
        None
    }

    /// File name with the `.xlsx` extension.
    pub fn filename(&self) -> String {
        let n = self.name.trim();
        if n.to_ascii_lowercase().ends_with(".xlsx") {
            n.to_string()
        } else {
            format!("{n}.xlsx")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_references_parse() {
        assert_eq!(split_ref("B8"), Some(("B", 8)));
        assert_eq!(split_ref("AB12"), Some(("AB", 12)));
        assert_eq!(split_ref("b8"), None);
        assert_eq!(split_ref("8"), None);
        assert_eq!(split_ref("B0"), None);
        assert_eq!(column_number("A"), 1);
        assert_eq!(column_number("AA"), 27);
    }

    #[test]
    fn workbook_shape_is_checked() {
        let mut w = Workbook {
            name: "Cross table".into(),
            sheets: vec![Sheet {
                name: "RS0101".into(),
                cells: vec![Cell {
                    reference: "B1".into(),
                    value: "x".into(),
                    ..Default::default()
                }],
                merges: vec!["B1:E1".into()],
                widths: [("B".to_string(), 60.0)].into(),
            }],
        };
        assert_eq!(w.check(), None);
        assert_eq!(w.filename(), "Cross table.xlsx");
        w.sheets[0].merges.push("B1".into());
        assert_eq!(w.check(), Some("sheets"));
        w.sheets[0].merges.pop();
        w.name = "a/b".into();
        assert_eq!(w.check(), Some("name"));
    }
}
