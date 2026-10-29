//! Office documents of the flows (network-check.md D416, D420):
//! `document_text` reads a document of the media library, `sheet_write`
//! writes an xlsx workbook into it. Both check the node against the
//! deployed version and stay in its org (R1).

use std::io::Write;

use loco_rs::prelude::*;
use pnex_core::err_codes::{DOCUMENT_TEXT_NO_TEXT, DOCUMENT_TEXT_UNREADABLE};
use pnex_core::flow::FlowNodeKind;
use pnex_core::office::{
    column_number, split_ref, DocumentTextReply, DocumentTextRequest, SheetWriteReply,
    SheetWriteRequest, Workbook, DOCUMENT_TEXT_MAX,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use crate::models::_entities::{media_assets, media_versions};
use crate::services::media::MediaSettings;
use crate::services::ontology::flow_nodes::{deployed, NodeError};

/// Why a call is refused (runtime diagnostics, codes kept).
#[derive(Debug)]
pub enum OfficeError {
    Node(NodeError),
    /// Media absent, of another org, or without a version.
    NotFound,
    /// A field of the message (`args.field`).
    Invalid(&'static str),
    /// Machine code of a document that cannot be read.
    Coded(&'static str),
    Internal(String),
}

impl From<NodeError> for OfficeError {
    fn from(e: NodeError) -> Self {
        Self::Node(e)
    }
}

impl From<sea_orm::DbErr> for OfficeError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Internal(e.to_string())
    }
}

async fn current_version(
    db: &DatabaseConnection,
    org_id: i64,
    asset_id: Uuid,
) -> Result<media_versions::Model, OfficeError> {
    media_assets::Entity::find_by_id(asset_id)
        .filter(media_assets::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(OfficeError::NotFound)?;
    media_versions::Entity::find()
        .filter(media_versions::Column::AssetId.eq(asset_id))
        .filter(media_versions::Column::OrgId.eq(org_id))
        .order_by_desc(media_versions::Column::VersionNumber)
        .one(db)
        .await?
        .ok_or(OfficeError::NotFound)
}

/// `document_text`: the text of the current version.
pub async fn document_text(
    ctx: &AppContext,
    req: &DocumentTextRequest,
) -> Result<DocumentTextReply, OfficeError> {
    use pnex_core::doc_extract::{self, ExtractError, Limits};
    let FlowNodeKind::DocumentText { .. } = deployed(&ctx.db, &req.node).await? else {
        return Err(NodeError::NotDeployed.into());
    };
    let raw = Uuid::parse_str(req.media_id.trim()).map_err(|_| OfficeError::Invalid("media_id"))?;
    // The media id, or the id of its ontology object (a `ref` property).
    let id = match crate::models::_entities::objects::Entity::find_by_id(raw)
        .filter(crate::models::_entities::objects::Column::OrgId.eq(req.node.org_id))
        .filter(
            crate::models::_entities::objects::Column::TypeKey
                .eq(pnex_core::resources::KIND_MEDIA_ASSET),
        )
        .one(&ctx.db)
        .await?
    {
        Some(o) => Uuid::parse_str(&o.native_id).map_err(|_| OfficeError::NotFound)?,
        None => raw,
    };
    let v = current_version(&ctx.db, req.node.org_id, id).await?;
    let store = MediaSettings::from_config(&ctx.config)
        .store()
        .map_err(|e| OfficeError::Internal(e.to_string()))?;
    let bytes = store
        .get(&v.storage_key)
        .await
        .map_err(|e| OfficeError::Internal(e.to_string()))?;
    let filename = v.filename.clone();
    // Third-party parsers may panic on hostile input: a JoinError is
    // an unreadable document.
    let job = tokio::task::spawn_blocking(move || {
        let format = doc_extract::format_of(&filename, &bytes).ok_or(ExtractError::Unsupported)?;
        doc_extract::extract(format, &bytes, &Limits::default())
    });
    let doc = match tokio::time::timeout(std::time::Duration::from_secs(30), job).await {
        Ok(Ok(Ok(doc))) => doc,
        Ok(Ok(Err(ExtractError::NeedsOcr))) => {
            return Err(OfficeError::Coded(DOCUMENT_TEXT_NO_TEXT))
        }
        _ => return Err(OfficeError::Coded(DOCUMENT_TEXT_UNREADABLE)),
    };
    let full = doc
        .sections
        .into_iter()
        .map(|s| s.text)
        .collect::<Vec<_>>()
        .join("\n\n");
    if full.trim().is_empty() {
        return Err(OfficeError::Coded(DOCUMENT_TEXT_NO_TEXT));
    }
    let truncated = full.chars().count() > DOCUMENT_TEXT_MAX;
    Ok(DocumentTextReply {
        media_id: id.to_string(),
        version: v.version_number,
        text: full.chars().take(DOCUMENT_TEXT_MAX).collect(),
        pages: doc.page_count,
        truncated,
    })
}

/// `sheet_write`: a new version of the node's workbook of that name.
pub async fn sheet_write(
    ctx: &AppContext,
    req: &SheetWriteRequest,
) -> Result<SheetWriteReply, OfficeError> {
    let FlowNodeKind::SheetWrite { .. } = deployed(&ctx.db, &req.node).await? else {
        return Err(NodeError::NotDeployed.into());
    };
    if let Some(field) = req.workbook.check() {
        return Err(OfficeError::Invalid(field));
    }
    crate::services::ontology::flow_nodes::spend(req.node.org_id)?;
    let filename = req.workbook.filename();
    let bytes = xlsx(&req.workbook).map_err(|e| OfficeError::Internal(e.to_string()))?;
    let org_id = req.node.org_id;
    // One asset per flow, node and file name, found again by its metadata.
    let owner = format!("{}:{}:{}", req.node.flow_id, req.node.node_id, filename);
    let found = media_assets::Entity::find()
        .filter(media_assets::Column::OrgId.eq(org_id))
        .filter(sea_orm::sea_query::Expr::cust_with_values(
            "metadata->>'sheet_write' = $1",
            [owner.clone()],
        ))
        .one(&ctx.db)
        .await?;
    let asset = match found {
        Some(a) => a,
        None => {
            use sea_orm::{ActiveModelTrait, Set};
            media_assets::ActiveModel {
                id: Set(Uuid::new_v4()),
                org_id: Set(org_id),
                kind: Set("table".into()),
                name: Set(filename.clone()),
                description: Set(Some(format!(
                    "Workbook written by flow #{}.",
                    req.node.flow_id
                ))),
                metadata: Set(Some(serde_json::json!({ "sheet_write": owner }))),
                ..Default::default()
            }
            .insert(&ctx.db)
            .await?
        }
    };
    let settings = MediaSettings::from_config(&ctx.config);
    let v = crate::controllers::media::write_version(
        ctx,
        org_id,
        &asset,
        crate::controllers::media::IncomingVersion {
            filename: filename.clone(),
            content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                .into(),
            note: None,
            bytes: axum::body::Bytes::from(bytes),
        },
        &settings,
    )
    .await
    .map_err(|e| OfficeError::Internal(format!("workbook not stored: {e}")))?;
    Ok(SheetWriteReply {
        media_id: asset.id.to_string(),
        version: v.version_number,
        filename,
    })
}

/// XML text: escaped, characters XML 1.0 forbids dropped.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// Style index of a cell: 0 plain, 1 bold, 2 wrapped, 3 both.
fn style(bold: bool, wrap: bool) -> u8 {
    u8::from(bold) + 2 * u8::from(wrap)
}

fn sheet_xml(s: &pnex_core::office::Sheet) -> String {
    let mut rows: std::collections::BTreeMap<u32, Vec<(u32, &pnex_core::office::Cell)>> =
        Default::default();
    for c in &s.cells {
        if let Some((col, row)) = split_ref(&c.reference) {
            rows.entry(row).or_default().push((column_number(col), c));
        }
    }
    let mut x = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">"#,
    );
    if !s.widths.is_empty() {
        x.push_str("<cols>");
        let mut widths: Vec<(u32, f64)> = s
            .widths
            .iter()
            .map(|(k, w)| (column_number(k), *w))
            .collect();
        widths.sort_by_key(|(n, _)| *n);
        for (n, w) in widths {
            x.push_str(&format!(
                r#"<col min="{n}" max="{n}" width="{w}" customWidth="1"/>"#
            ));
        }
        x.push_str("</cols>");
    }
    x.push_str("<sheetData>");
    for (row, mut cells) in rows {
        cells.sort_by_key(|(n, _)| *n);
        // A reference written twice keeps its last value.
        cells.dedup_by_key(|(n, _)| *n);
        x.push_str(&format!(r#"<row r="{row}">"#));
        for (_, c) in cells {
            x.push_str(&format!(
                r#"<c r="{}" t="inlineStr" s="{}"><is><t xml:space="preserve">{}</t></is></c>"#,
                c.reference,
                style(c.bold, c.wrap),
                esc(&c.value)
            ));
        }
        x.push_str("</row>");
    }
    x.push_str("</sheetData>");
    if !s.merges.is_empty() {
        x.push_str(&format!(r#"<mergeCells count="{}">"#, s.merges.len()));
        for m in &s.merges {
            x.push_str(&format!(r#"<mergeCell ref="{}"/>"#, esc(m)));
        }
        x.push_str("</mergeCells>");
    }
    x.push_str("</worksheet>");
    x
}

/// The workbook as an xlsx file: inline strings, four styles, no formula.
pub fn xlsx(w: &Workbook) -> zip::result::ZipResult<Vec<u8>> {
    const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border/></borders><cellStyleXfs count="1"><xf/></cellStyleXfs><cellXfs count="4"><xf fontId="0"/><xf fontId="1" applyFont="1"/><xf fontId="0" applyAlignment="1"><alignment wrapText="1" vertical="top"/></xf><xf fontId="1" applyFont="1" applyAlignment="1"><alignment wrapText="1" vertical="top"/></xf></cellXfs></styleSheet>"#;
    let n = w.sheets.len();
    let mut types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#,
    );
    let mut book = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#,
    );
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (i, s) in w.sheets.iter().enumerate() {
        let k = i + 1;
        types.push_str(&format!(r#"<Override PartName="/xl/worksheets/sheet{k}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#));
        book.push_str(&format!(
            r#"<sheet name="{}" sheetId="{k}" r:id="rId{k}"/>"#,
            esc(&s.name)
        ));
        rels.push_str(&format!(r#"<Relationship Id="rId{k}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{k}.xml"/>"#));
    }
    types.push_str("</Types>");
    book.push_str("</sheets></workbook>");
    rels.push_str(&format!(r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#, n + 1));
    let root = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut put = |name: &str, body: &str| -> zip::result::ZipResult<()> {
        zip.start_file(name, opts)?;
        zip.write_all(body.as_bytes())?;
        Ok(())
    };
    put("[Content_Types].xml", &types)?;
    put("_rels/.rels", root)?;
    put("xl/workbook.xml", &book)?;
    put("xl/_rels/workbook.xml.rels", &rels)?;
    put("xl/styles.xml", STYLES)?;
    for (i, s) in w.sheets.iter().enumerate() {
        put(&format!("xl/worksheets/sheet{}.xml", i + 1), &sheet_xml(s))?;
    }
    Ok(zip.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::office::{Cell, Sheet};

    #[test]
    fn workbook_reads_back_as_xlsx() {
        let w = Workbook {
            name: "Cross table".into(),
            sheets: vec![Sheet {
                name: "RS0101".into(),
                cells: vec![
                    Cell {
                        reference: "B1".into(),
                        value: "Référencement <naturel> & SEO".into(),
                        bold: true,
                        ..Default::default()
                    },
                    Cell {
                        reference: "C8".into(),
                        value: "Chapitre 01\nCadrer".into(),
                        wrap: true,
                        ..Default::default()
                    },
                    Cell {
                        reference: "B8".into(),
                        value: "C1. Cadrer le projet".into(),
                        ..Default::default()
                    },
                ],
                merges: vec!["B1:E1".into()],
                widths: [("B".to_string(), 60.0)].into(),
            }],
        };
        let bytes = xlsx(&w).unwrap();
        let format = pnex_core::doc_extract::format_of("x.xlsx", &bytes).expect("an xlsx");
        let doc = pnex_core::doc_extract::extract(format, &bytes, &Default::default()).unwrap();
        let text: String = doc.sections.iter().map(|s| s.text.as_str()).collect();
        assert!(text.contains("Référencement <naturel> & SEO"), "{text}");
        assert!(text.contains("C1. Cadrer le projet"), "{text}");
        assert!(text.contains("Chapitre 01"), "{text}");
    }
}
