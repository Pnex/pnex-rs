//! PDF of a named page version (media-vision-studio.md D209): the frozen
//! Yjs tree becomes `pnex_pdf` blocks, rendered off the async workers.
//!
//! The few words of the PDF itself (page numbers, AI notice) are chosen by
//! the requested language (`fr`, else English): a document leaves the UI,
//! so it cannot go through the front `t!` catalogue.

use pnex_core::pages::FRAGMENT;
use pnex_pdf::{Block, Meta, Span};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;
use yrs::types::text::YChange;
use yrs::types::xml::{XmlFragment, XmlOut};
use yrs::{Any, Doc, Out, ReadTxn, Text, Transact, Xml};

use super::{apply, PageError};
use crate::models::_entities::{doc_versions, docs, organizations, users};

/// Words of a rendered PDF.
struct Labels {
    page: &'static str,
    unreviewed: &'static str,
    approved: &'static str,
}

fn labels(lang: &str) -> Labels {
    if lang.starts_with("fr") {
        Labels {
            page: "Page {n} / {m}",
            unreviewed: "Contient du texte généré par IA, non relu.",
            approved: "Contient du texte généré par IA, relu et approuvé par {by} le {date}.",
        }
    } else {
        Labels {
            page: "Page {n} / {m}",
            unreviewed: "Holds AI-generated text, not reviewed.",
            approved: "Holds AI-generated text, reviewed and approved by {by} on {date}.",
        }
    }
}

fn spans_of<T: ReadTxn, E: XmlFragment>(txn: &T, el: &E) -> Vec<Span> {
    let mut out = Vec::new();
    for child in el.children(txn) {
        match child {
            XmlOut::Text(text) => {
                for chunk in text.diff(txn, YChange::identity) {
                    let Out::Any(Any::String(s)) = chunk.insert else {
                        continue;
                    };
                    let has =
                        |m: &str| chunk.attributes.as_ref().is_some_and(|a| a.contains_key(m));
                    out.push(Span {
                        text: s.to_string(),
                        bold: has("strong"),
                        italic: has("em"),
                        code: has("code"),
                        link: has("link"),
                    });
                }
            }
            // hard_break and any other inline element: a space.
            _ => out.push(Span::plain(" ")),
        }
    }
    out
}

fn list<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef, depth: u8, out: &mut Vec<Block>) {
    let ordered = el.tag().as_ref() == "ordered_list";
    let mut n: i64 = match el.get_attribute(txn, "order") {
        Some(Out::Any(Any::Number(v))) => v.as_i64().unwrap_or(1),
        _ => 1,
    };
    for item in el.children(txn) {
        let XmlOut::Element(item) = item else {
            continue;
        };
        let mut first = true;
        for part in item.children(txn) {
            let XmlOut::Element(part) = part else {
                continue;
            };
            match part.tag().as_ref() {
                "bullet_list" | "ordered_list" => list(txn, &part, depth + 1, out),
                _ => {
                    let marker = match (first, ordered) {
                        (true, true) => format!("{n}."),
                        (true, false) => "•".to_string(),
                        (false, _) => String::new(),
                    };
                    out.push(Block::Item {
                        marker,
                        depth,
                        spans: spans_of(txn, &part),
                    });
                    first = false;
                }
            }
        }
        n += 1;
    }
}

/// The blocks of a page tree, in order.
pub fn blocks_of(doc: &Doc) -> Vec<Block> {
    let txn = doc.transact();
    let Some(root) = txn.get_xml_fragment(FRAGMENT) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for child in root.children(&txn) {
        let XmlOut::Element(el) = child else {
            continue;
        };
        match el.tag().as_ref() {
            "heading" => {
                let level = match el.get_attribute(&txn, "level") {
                    Some(Out::Any(Any::Number(v))) => v.as_i64().unwrap_or(2).clamp(1, 6) as u8,
                    _ => 2,
                };
                out.push(Block::Heading(level.clamp(1, 6), spans_of(&txn, &el)));
            }
            "blockquote" => {
                for p in el.children(&txn) {
                    if let XmlOut::Element(p) = p {
                        out.push(Block::Quote(spans_of(&txn, &p)));
                    }
                }
            }
            "horizontal_rule" => out.push(Block::Rule),
            "code_block" => {
                let text: String = spans_of(&txn, &el).into_iter().map(|s| s.text).collect();
                out.push(Block::Code(text));
            }
            "bullet_list" | "ordered_list" => list(&txn, &el, 0, &mut out),
            _ => out.push(Block::Paragraph(spans_of(&txn, &el))),
        }
    }
    out
}

/// A rendered version: file name and PDF bytes.
pub struct PagePdf {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub title: String,
}

/// PDF of version `n` of page `doc_id` of the org (`None` = not found).
pub async fn render_version(
    db: &DatabaseConnection,
    org_id: i64,
    doc_id: Uuid,
    n: i64,
    lang: &str,
) -> Result<Option<PagePdf>, PageError> {
    let Some(page) = docs::Entity::find_by_id(doc_id)
        .filter(docs::Column::OrgId.eq(org_id))
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let Some(v) = doc_versions::Entity::find()
        .filter(doc_versions::Column::DocId.eq(doc_id))
        .filter(doc_versions::Column::VersionNumber.eq(n))
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let org_name = organizations::Entity::find_by_id(org_id)
        .one(db)
        .await?
        .map(|o| o.name)
        .unwrap_or_default();
    let words = labels(lang);
    let date = v.created_at.format("%Y-%m-%d").to_string();
    let notice = if v.ai_generated {
        Some(match (v.approved_by, v.approved_at) {
            (Some(uid), Some(at)) => {
                let by = users::Entity::find_by_id(uid)
                    .one(db)
                    .await?
                    .map(|u| u.full_name.unwrap_or(u.email))
                    .unwrap_or_default();
                words
                    .approved
                    .replace("{by}", &by)
                    .replace("{date}", &at.format("%Y-%m-%d").to_string())
            }
            _ => words.unreviewed.to_string(),
        })
    } else {
        None
    };
    let doc = Doc::new();
    apply(&doc, &v.snapshot)?;
    let blocks = blocks_of(&doc);
    let meta = Meta {
        title: page.title.clone(),
        header: format!("{org_name} · v{} {}", v.version_number, v.label),
        footer: date,
        page_label: words.page.to_string(),
        notice,
    };
    let bytes = tokio::task::spawn_blocking(move || pnex_pdf::render(&meta, &blocks))
        .await
        .map_err(|e| PageError::Corrupt(e.to_string()))?
        .map_err(PageError::Corrupt)?;
    let safe: String = page
        .title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(Some(PagePdf {
        filename: format!("{} v{}.pdf", safe.trim(), v.version_number),
        bytes,
        title: page.title,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_tree_becomes_pdf_blocks() {
        let doc = Doc::new();
        super::super::seed::append_markdown(
            &doc,
            "# Report\n\nA **calm** week.\n\n1. first\n2. second\n\n- a\n\n> note\n\n---\n\n```\ncode\n```",
        );
        let b = blocks_of(&doc);
        assert!(matches!(&b[0], Block::Heading(1, s) if s[0].text == "Report"));
        let Block::Paragraph(p) = &b[1] else {
            panic!("{b:?}")
        };
        assert!(p.iter().any(|s| s.text == "calm" && s.bold));
        assert!(matches!(&b[2], Block::Item { marker, .. } if marker == "1."));
        assert!(matches!(&b[3], Block::Item { marker, .. } if marker == "2."));
        assert!(matches!(&b[4], Block::Item { marker, .. } if marker == "•"));
        assert!(matches!(&b[5], Block::Quote(_)));
        assert!(matches!(&b[6], Block::Rule));
        assert!(matches!(&b[7], Block::Code(c) if c == "code"));
        let pdf = pnex_pdf::render(&Meta::default(), &b).unwrap();
        assert!(pdf.starts_with(b"%PDF-"));
    }
}
