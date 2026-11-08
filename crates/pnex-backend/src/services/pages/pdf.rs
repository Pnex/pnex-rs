//! PDF of a named page version (media-vision-studio.md D209): the frozen
//! Yjs tree becomes `pnex_pdf` blocks, rendered off the async workers.
//!
//! The few words of the PDF itself (page numbers, AI notice) are chosen by
//! the requested language (`fr`, else English): a document leaves the UI,
//! so it cannot go through the front `t!` catalogue.

use pnex_core::analysis::AnalysisState;
use pnex_core::ontology::aggregate::{AggregateGroup, AggregateValue, MetricOp};
use pnex_core::ontology::{localized, PropertyDef};
use pnex_core::pages::{LiveBlock, FRAGMENT, LIVE_BLOCK};
use pnex_pdf::{Block, Meta, Span};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;
use yrs::types::text::YChange;
use yrs::types::xml::{XmlFragment, XmlOut};
use yrs::{Any, Doc, Out, ReadTxn, Text, Transact, Xml};

use super::{apply, PageError};
use crate::models::_entities::{doc_versions, docs, organizations, users};
use crate::services::analysis::{self, AnalysisError};
use crate::services::artifact_store::S3Store;
use crate::services::ontology::redact::Redaction;

/// Words of a rendered PDF.
struct Labels {
    lang: &'static str,
    ops: [&'static str; 5],
    page: &'static str,
    unreviewed: &'static str,
    approved: &'static str,
    as_of: &'static str,
    automatic: &'static str,
    denied: &'static str,
    failed: &'static str,
    untitled: &'static str,
    unset: &'static str,
    too_long: &'static str,
    no_rows: &'static str,
    more_rows: &'static str,
    group: &'static str,
    others: &'static str,
}

fn labels(lang: &str) -> Labels {
    if lang.starts_with("fr") {
        Labels {
            lang: "fr",
            ops: ["Nombre", "Somme", "Moyenne", "Minimum", "Maximum"],
            page: "Page {n} / {m}",
            unreviewed: "Contient du texte généré par IA, non relu.",
            approved: "Contient du texte généré par IA, relu et approuvé par {by} le {date}.",
            as_of: "Valeur au {date}",
            automatic: "Non calculé dans un export automatique.",
            denied: "Vous n'avez pas le droit de calculer ce bloc.",
            failed: "Le calcul a échoué : ouvrez la page pour en voir la raison.",
            untitled: "Bloc vivant",
            unset: "Bloc non configuré.",
            too_long: "Calcul trop long : relancez l'export.",
            no_rows: "Aucune ligne.",
            more_rows: "… et {n} lignes de plus.",
            group: "Groupe",
            others: "Autres",
        }
    } else {
        Labels {
            lang: "en",
            ops: ["Count", "Sum", "Average", "Minimum", "Maximum"],
            page: "Page {n} / {m}",
            unreviewed: "Holds AI-generated text, not reviewed.",
            approved: "Holds AI-generated text, reviewed and approved by {by} on {date}.",
            as_of: "Value as of {date}",
            automatic: "Not computed in an automatic export.",
            denied: "You may not compute this block.",
            failed: "The calculation failed: open the page to see why.",
            untitled: "Live block",
            unset: "Block not set up.",
            too_long: "The calculation took too long: export again.",
            no_rows: "No rows.",
            more_rows: "… and {n} more rows.",
            group: "Group",
            others: "Others",
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

/// A piece of a page: a block to draw, or a live block to compute first.
pub enum Part {
    Block(Block),
    Live(LiveBlock),
}

fn live_of<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef) -> LiveBlock {
    let get = |k: &str| match el.get_attribute(txn, k) {
        Some(Out::Any(Any::String(s))) => s.to_string(),
        _ => String::new(),
    };
    LiveBlock {
        bid: get("bid"),
        source: get("source"),
        sql: get("sql"),
        query: get("query"),
        title: get("title"),
    }
}

/// The blocks of a page tree, in order; live blocks left out.
pub fn blocks_of(doc: &Doc) -> Vec<Block> {
    parts_of(doc)
        .into_iter()
        .filter_map(|p| match p {
            Part::Block(b) => Some(b),
            Part::Live(_) => None,
        })
        .collect()
}

/// The parts of a page tree, in order.
pub fn parts_of(doc: &Doc) -> Vec<Part> {
    let txn = doc.transact();
    let Some(root) = txn.get_xml_fragment(FRAGMENT) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for child in root.children(&txn) {
        let XmlOut::Element(el) = child else {
            continue;
        };
        let tag = el.tag();
        if tag.as_ref() == LIVE_BLOCK {
            out.push(Part::Live(live_of(&txn, &el)));
            continue;
        }
        let mut blocks = Vec::new();
        match tag.as_ref() {
            "heading" => {
                let level = match el.get_attribute(&txn, "level") {
                    Some(Out::Any(Any::Number(v))) => v.as_i64().unwrap_or(2).clamp(1, 6) as u8,
                    _ => 2,
                };
                blocks.push(Block::Heading(level.clamp(1, 6), spans_of(&txn, &el)));
            }
            "blockquote" => {
                for p in el.children(&txn) {
                    if let XmlOut::Element(p) = p {
                        blocks.push(Block::Quote(spans_of(&txn, &p)));
                    }
                }
            }
            "horizontal_rule" => blocks.push(Block::Rule),
            "code_block" => {
                let text: String = spans_of(&txn, &el).into_iter().map(|s| s.text).collect();
                blocks.push(Block::Code(text));
            }
            "bullet_list" | "ordered_list" => list(&txn, &el, 0, &mut blocks),
            _ => blocks.push(Block::Paragraph(spans_of(&txn, &el))),
        }
        out.extend(blocks.into_iter().map(Part::Block));
    }
    out
}

/// Who the live blocks are computed for (D831–D832): the person exporting.
/// An automatic export (a flow) has none: its live blocks stay empty.
pub struct Reader {
    pub user: i64,
    pub hide: Redaction,
    /// `analysis:run`, for a SQL block.
    pub can_sql: bool,
    /// `ontology:read`, for a calculation on objects.
    pub can_objects: bool,
    pub store: Option<S3Store>,
}

/// Rows of a table drawn in a PDF; past it, a line says how many are left.
const PDF_ROWS_MAX: usize = 200;
/// Longest wait for a SQL block during an export (the worker's limit is 60 s).
const SQL_WAIT: std::time::Duration = std::time::Duration::from_secs(75);

fn number(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{x:.0}")
    } else {
        format!("{x:.2}")
    }
}

fn value_text(v: Option<&AggregateValue>) -> String {
    match v {
        Some(v) => match v.value {
            Some(x) => number(x),
            None => v
                .by_currency
                .iter()
                .map(|(c, x)| format!("{} {c}", number(*x)))
                .collect::<Vec<_>>()
                .join(" + "),
        },
        None => String::new(),
    }
}

fn table(columns: Vec<String>, mut rows: Vec<Vec<String>>, words: &Labels) -> Vec<Block> {
    if rows.is_empty() {
        return vec![Block::Paragraph(vec![Span::plain(words.no_rows)])];
    }
    let more = rows.len().saturating_sub(PDF_ROWS_MAX);
    rows.truncate(PDF_ROWS_MAX);
    let mut out = vec![Block::Table { columns, rows }];
    if more > 0 {
        out.push(Block::Paragraph(vec![Span::plain(
            words.more_rows.replace("{n}", &more.to_string()),
        )]));
    }
    out
}

/// The blocks a live block becomes, computed now for `reader`.
async fn live_blocks(
    db: &DatabaseConnection,
    org_id: i64,
    b: &LiveBlock,
    reader: Option<&Reader>,
    words: &Labels,
) -> Vec<Block> {
    let mut out = Vec::new();
    let title = match b.title.trim() {
        "" => words.untitled,
        t => t,
    };
    out.push(Block::Heading(4, vec![Span::plain(title)]));
    let note = |text: String| {
        Block::Paragraph(vec![Span {
            text,
            italic: true,
            ..Default::default()
        }])
    };
    let Some(r) = reader else {
        out.push(note(words.automatic.into()));
        return out;
    };
    let body = match b.source.as_str() {
        "objects" => match b.aggregate() {
            None => Err(words.unset.to_string()),
            Some(_) if !r.can_objects => Err(words.denied.to_string()),
            Some(q) => objects_table(db, org_id, &q, r, words).await,
        },
        "sql" if b.sql.trim().is_empty() => Err(words.unset.to_string()),
        "sql" if !r.can_sql || r.store.is_none() => Err(words.denied.to_string()),
        "sql" => sql_table(db, org_id, &b.sql, r, words).await,
        _ => Err(words.unset.to_string()),
    };
    let date = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();
    match body {
        Ok(blocks) => {
            out.push(note(words.as_of.replace("{date}", &date)));
            out.extend(blocks);
        }
        Err(reason) => out.push(note(reason)),
    }
    out
}

/// What a group is called: the linked object's title, the enum value's label
/// in the PDF's language, or the raw value.
fn group_label(g: &AggregateGroup, prop: Option<&PropertyDef>, lang: &str) -> String {
    if let Some(l) = g.labels.first().cloned().flatten() {
        return l;
    }
    let Some(key) = g.key.first().cloned().flatten() else {
        return "—".into();
    };
    match prop.and_then(|p| p.labels.get(&key)) {
        Some(names) => localized(&key, names, lang).to_string(),
        None => key,
    }
}

fn op_word(op: Option<MetricOp>, words: &Labels) -> &'static str {
    match op.unwrap_or(MetricOp::Count) {
        MetricOp::Count | MetricOp::CountDistinct => words.ops[0],
        MetricOp::Sum => words.ops[1],
        MetricOp::Avg => words.ops[2],
        MetricOp::Min => words.ops[3],
        MetricOp::Max => words.ops[4],
    }
}

async fn objects_table(
    db: &DatabaseConnection,
    org_id: i64,
    q: &pnex_core::ontology::aggregate::AggregateQuery,
    r: &Reader,
    words: &Labels,
) -> Result<Vec<Block>, String> {
    let res = crate::services::ontology::aggregate::run(db, org_id, q, r.hide.clone())
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "pages: live block not computed for the pdf");
            words.failed.to_string()
        })?;
    let keys: Vec<&str> = q.metrics.iter().map(|m| m.key.as_str()).collect();
    let grouped = !q.group_by.is_empty();
    let view = match q.query.type_key.as_deref() {
        Some(k) => crate::services::ontology::types::get(db, org_id, k)
            .await
            .ok(),
        None => None,
    };
    let prop = |key: &str| {
        view.as_ref()
            .and_then(|v| v.def.properties.iter().find(|p| p.key == key))
    };
    let name = |key: &str| match prop(key) {
        Some(p) if !p.name.trim().is_empty() => {
            localized(&p.name, &p.names, words.lang).to_string()
        }
        _ => key.to_string(),
    };
    let by = q.group_by.first().and_then(|g| g.property.as_deref());
    let mut columns = Vec::new();
    if grouped {
        columns.push(by.map(name).unwrap_or_else(|| words.group.to_string()));
    }
    columns.extend(q.metrics.iter().map(|m| match &m.property {
        Some(p) => format!("{} · {}", op_word(m.op, words), name(p)),
        None => op_word(m.op, words).to_string(),
    }));
    let row = |label: Option<String>, g: &AggregateGroup| {
        let mut cells: Vec<String> = label.into_iter().collect();
        cells.extend(keys.iter().map(|k| value_text(g.values.get(*k))));
        cells
    };
    let mut rows: Vec<Vec<String>> = res
        .groups
        .iter()
        .map(|g| {
            row(
                grouped.then(|| group_label(g, by.and_then(prop), words.lang)),
                g,
            )
        })
        .collect();
    if let Some(o) = &res.others {
        rows.push(row(grouped.then(|| words.others.to_string()), o));
    }
    Ok(table(columns, rows, words))
}

async fn sql_table(
    db: &DatabaseConnection,
    org_id: i64,
    sql: &str,
    r: &Reader,
    words: &Labels,
) -> Result<Vec<Block>, String> {
    let Some(store) = &r.store else {
        return Err(words.denied.to_string());
    };
    // The page shows the reason in the reader's language; the PDF says where.
    let failed = |reason: &str| {
        tracing::info!(reason, "pages: live block failed in the pdf");
        words.failed.to_string()
    };
    let deadline = tokio::time::Instant::now() + SQL_WAIT;
    let pause = std::time::Duration::from_millis(500);
    let mut q = loop {
        match analysis::submit(
            db,
            store,
            org_id,
            r.user,
            r.hide.clone(),
            sql,
            Some(pnex_core::analysis::REUSE_MAX_SECS),
        )
        .await
        {
            Ok(q) => break q,
            // Another query of the same person is running: wait for it.
            Err(AnalysisError::Busy) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(pause).await
            }
            Err(AnalysisError::Busy) => return Err(words.too_long.to_string()),
            Err(e) => return Err(failed(&format!("{e:?}"))),
        }
    };
    loop {
        match q.state {
            AnalysisState::Done => break,
            AnalysisState::Failed => {
                return Err(failed(q.code.as_deref().unwrap_or("failed")));
            }
            _ if tokio::time::Instant::now() >= deadline => return Err(words.too_long.to_string()),
            _ => {}
        }
        tokio::time::sleep(pause).await;
        let id = q.id.parse().map_err(|_| failed("id"))?;
        q = analysis::get(db, org_id, r.user, id)
            .await
            .map_err(|e| failed(&format!("{e:?}")))?;
    }
    let rows = q
        .rows
        .into_iter()
        .map(|row| row.into_iter().map(Option::unwrap_or_default).collect())
        .collect();
    Ok(table(q.columns, rows, words))
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
    reader: Option<&Reader>,
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
    let parts = {
        let doc = Doc::new();
        apply(&doc, &v.snapshot)?;
        parts_of(&doc)
    };
    // Computed one after the other: a person runs one SQL query at a time.
    let mut blocks = Vec::new();
    for part in parts {
        match part {
            Part::Block(b) => blocks.push(b),
            Part::Live(l) => blocks.extend(live_blocks(db, org_id, &l, reader, &words).await),
        }
    }
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

    #[tokio::test]
    async fn an_automatic_export_leaves_live_blocks_uncomputed() {
        use yrs::types::xml::XmlElementPrelim;
        let doc = Doc::new();
        {
            let root = doc.get_or_insert_xml_fragment(FRAGMENT);
            let mut txn = doc.transact_mut();
            let b = root.push_back(&mut txn, XmlElementPrelim::empty(LIVE_BLOCK));
            b.insert_attribute(&mut txn, "source", "sql");
            b.insert_attribute(&mut txn, "sql", "SELECT 1");
            b.insert_attribute(&mut txn, "title", "Open deals");
        }
        let parts = parts_of(&doc);
        let [Part::Live(live)] = parts.as_slice() else {
            panic!("one live block")
        };
        assert_eq!(
            (live.source.as_str(), live.title.as_str()),
            ("sql", "Open deals")
        );
        let words = labels("en");
        let db = DatabaseConnection::default();
        let out = live_blocks(&db, 1, live, None, &words).await;
        assert!(matches!(&out[0], Block::Heading(4, s) if s[0].text == "Open deals"));
        assert!(matches!(&out[1], Block::Paragraph(s) if s[0].text == words.automatic));
    }
}
