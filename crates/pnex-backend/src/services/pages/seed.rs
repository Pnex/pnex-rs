//! Page content built on the server from Markdown (pages.md P13 templates,
//! D209 flow writes): a Markdown subset turned into the `y-prosemirror`
//! tree the editor reads.
//!
//! Blocks: headings `#`…`######`, paragraphs (consecutive lines joined),
//! bullet and ordered lists, `>` quotes, `---` rules, fenced code. Inline:
//! `**strong**`, `*em*` / `_em_`, `` `code` ``, `[text](url)` links
//! (allowed schemes only, else plain text). Pages have no table node: a
//! table row becomes a paragraph of its cells joined by " · ".

use pnex_core::pages::{link_allowed, FRAGMENT};
use yrs::types::text::Text;
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::types::Attrs;
use yrs::{Any, Doc, ReadTxn, StateVector, Transact, TransactionMut, Xml, XmlElementRef};

enum Block {
    Heading(u8, String),
    Paragraph(String),
    List(bool, Vec<String>),
    Quote(String),
    Rule,
    Code(String),
}

/// A table row (`| a | b |`) as its cells; `Some(None)` for a separator.
fn table_row(line: &str) -> Option<Option<String>> {
    let inner = line.trim().strip_prefix('|')?;
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    let cells: Vec<&str> = inner.split('|').map(str::trim).collect();
    if cells
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':')))
    {
        return Some(None);
    }
    Some(Some(cells.join(" · ")))
}

fn parse(md: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut code: Option<Vec<&str>> = None;
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(para.join(" ")));
            para.clear();
        }
    };
    for raw in md.lines() {
        if let Some(lines) = code.as_mut() {
            if raw.trim_start().starts_with("```") {
                out.push(Block::Code(lines.join("\n")));
                code = None;
            } else {
                lines.push(raw);
            }
            continue;
        }
        // Trailing spaces kept: an empty item ("- ") is a slot to fill in.
        let line = raw.trim_start();
        if line.starts_with("```") {
            flush(&mut para, &mut out);
            code = Some(Vec::new());
            continue;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        let ordered = line
            .split_once(". ")
            .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .map(|(_, t)| t);
        let item = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .map(|t| (false, t))
            .or(ordered.map(|t| (true, t)));
        let trimmed = line.trim();
        if trimmed.is_empty() {
            flush(&mut para, &mut out);
        } else if matches!(trimmed, "---" | "***" | "___") {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
        } else if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            flush(&mut para, &mut out);
            out.push(Block::Heading(
                hashes as u8,
                line[hashes + 1..].trim().into(),
            ));
        } else if let Some(q) = line.strip_prefix('>') {
            flush(&mut para, &mut out);
            out.push(Block::Quote(q.trim().into()));
        } else if let Some(row) = table_row(line) {
            flush(&mut para, &mut out);
            if let Some(row) = row {
                out.push(Block::Paragraph(row));
            }
        } else if let Some((num, text)) = item {
            flush(&mut para, &mut out);
            match out.last_mut() {
                Some(Block::List(n, items)) if *n == num => items.push(text.trim().into()),
                _ => out.push(Block::List(num, vec![text.trim().into()])),
            }
        } else {
            para.push(line.trim_end());
        }
    }
    if let Some(lines) = code {
        out.push(Block::Code(lines.join("\n")));
    }
    flush(&mut para, &mut out);
    out
}

/// One run of inline text and its marks (`(name, attributes)`).
type Run = (String, Vec<(&'static str, Any)>);

fn empty_map() -> Any {
    Any::Map(Default::default())
}

/// `[label](url)` at the start of `rest`: label, url, bytes consumed.
fn link_at(rest: &str) -> Option<(String, String, usize)> {
    let body = rest.strip_prefix('[')?;
    let close = body.find("](")?;
    let label = &body[..close];
    if label.is_empty() || label.contains(']') {
        return None;
    }
    let tail = &body[close + 2..];
    let end = tail.find(')')?;
    Some((
        label.to_string(),
        tail[..end].trim().to_string(),
        1 + close + 2 + end + 1,
    ))
}

/// `open…close` span at the start of `rest`: body, bytes consumed.
fn span(rest: &str, open: &str, close: &str) -> Option<(String, usize)> {
    let body = rest.strip_prefix(open)?;
    let end = body.find(close)?;
    (end > 0).then(|| (body[..end].to_string(), open.len() + end + close.len()))
}

/// Inline Markdown → runs. Unclosed markers stay as text.
fn inline(text: &str) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    let flush = |plain: &mut String, runs: &mut Vec<Run>| {
        if !plain.is_empty() {
            runs.push((std::mem::take(plain), Vec::new()));
        }
    };
    while let Some(c) = rest.chars().next() {
        let marked = span(rest, "**", "**")
            .map(|(t, n)| (t, n, "strong"))
            .or_else(|| span(rest, "`", "`").map(|(t, n)| (t, n, "code")))
            .or_else(|| {
                span(rest, "*", "*")
                    .or_else(|| span(rest, "_", "_"))
                    .map(|(t, n)| (t, n, "em"))
            });
        if let Some((t, n, mark)) = marked {
            flush(&mut plain, &mut runs);
            runs.push((t, vec![(mark, empty_map())]));
            rest = &rest[n..];
        } else if let Some((label, url, n)) = link_at(rest) {
            flush(&mut plain, &mut runs);
            if link_allowed(&url) {
                let mut attrs = std::collections::HashMap::new();
                attrs.insert("href".to_string(), Any::from(url));
                runs.push((label, vec![("link", Any::from(attrs))]));
            } else {
                runs.push((label, Vec::new()));
            }
            rest = &rest[n..];
        } else {
            plain.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    flush(&mut plain, &mut runs);
    runs
}

/// Appends a block holding `text` (with its inline marks) under `parent`.
fn push_text_block<P: XmlFragment>(
    txn: &mut TransactionMut,
    parent: &P,
    tag: &str,
    text: &str,
    marks: bool,
) -> XmlElementRef {
    let el = parent.push_back(txn, XmlElementPrelim::empty(tag));
    if !text.is_empty() {
        let t = el.push_back(txn, XmlTextPrelim::new(""));
        let runs = if marks {
            inline(text)
        } else {
            vec![(text.to_string(), Vec::new())]
        };
        for (s, m) in runs {
            let at = t.len(txn);
            // Always explicit: a plain insert would inherit the marks of the
            // run before it.
            let attrs: Attrs = m.into_iter().map(|(k, v)| (k.into(), v)).collect();
            t.insert_with_attributes(txn, at, &s, attrs);
        }
    }
    el
}

/// Appends the blocks of `md` at the end of the page held by `doc`.
pub fn append_markdown(doc: &Doc, md: &str) {
    let root = doc.get_or_insert_xml_fragment(FRAGMENT);
    let mut txn = doc.transact_mut();
    for b in parse(md) {
        match b {
            Block::Heading(level, text) => {
                let el = push_text_block(&mut txn, &root, "heading", &text, true);
                el.insert_attribute(&mut txn, "level", Any::Number(i64::from(level).into()));
            }
            Block::Paragraph(text) => {
                push_text_block(&mut txn, &root, "paragraph", &text, true);
            }
            Block::Quote(text) => {
                let q = root.push_back(&mut txn, XmlElementPrelim::empty("blockquote"));
                push_text_block(&mut txn, &q, "paragraph", &text, true);
            }
            Block::Rule => {
                root.push_back(&mut txn, XmlElementPrelim::empty("horizontal_rule"));
            }
            Block::Code(text) => {
                push_text_block(&mut txn, &root, "code_block", &text, false);
            }
            Block::List(ordered, items) => {
                let tag = if ordered {
                    "ordered_list"
                } else {
                    "bullet_list"
                };
                let list = root.push_back(&mut txn, XmlElementPrelim::empty(tag));
                for item in items {
                    let li = list.push_back(&mut txn, XmlElementPrelim::empty("list_item"));
                    push_text_block(&mut txn, &li, "paragraph", &item, true);
                }
            }
        }
    }
}

/// A page state (v1 update) holding `md`.
pub fn from_markdown(md: &str) -> Vec<u8> {
    let doc = Doc::new();
    append_markdown(&doc, md);
    super::encode(&doc)
}

/// The update that appends `md` to the page `doc` (v1 encoding); `doc`
/// holds the result afterwards.
pub fn append_update(doc: &Doc, md: &str) -> Vec<u8> {
    let before: StateVector = doc.transact().state_vector();
    append_markdown(doc, md);
    doc.transact().encode_state_as_update_v1(&before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::types::xml::XmlOut;
    use yrs::GetString;

    fn tags(doc: &Doc) -> Vec<String> {
        let txn = doc.transact();
        let root = txn.get_xml_fragment(FRAGMENT).unwrap();
        root.children(&txn)
            .map(|c| match c {
                XmlOut::Element(e) => e.tag().to_string(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn template_becomes_a_valid_page() {
        let state = from_markdown(
            "## Problem\n\nWhat failed,\nand when.\n\n1. Why?\n2. \n\n- Method\n- \n",
        );
        let doc = Doc::new();
        super::super::apply(&doc, &state).unwrap();
        super::super::schema::validate(&doc).unwrap();
        assert_eq!(
            tags(&doc),
            ["heading", "paragraph", "ordered_list", "bullet_list"]
        );
        let txn = doc.transact();
        let root = txn.get_xml_fragment(FRAGMENT).unwrap();
        let XmlOut::Element(p) = root.get(&txn, 1).unwrap() else {
            panic!()
        };
        assert!(p.get_string(&txn).contains(">What failed, and when.<"));
    }

    #[test]
    fn report_markdown_with_marks_is_a_valid_page() {
        let md = "# Weekly report\n\nA **calm** week, *one* alert at `07:12` \
                  ([details](https://pnex.io/x), [bad](javascript:alert(1))).\n\n\
                  > Generated by AI\n\n---\n\n| Day | Events |\n|---|---|\n| Mon | 3 |\n\n\
                  ```\nraw = 1\n```\n";
        let state = from_markdown(md);
        let doc = Doc::new();
        super::super::apply(&doc, &state).unwrap();
        super::super::schema::validate(&doc).unwrap();
        assert_eq!(
            tags(&doc),
            [
                "heading",
                "paragraph",
                "blockquote",
                "horizontal_rule",
                "paragraph",
                "paragraph",
                "code_block"
            ]
        );
        let txn = doc.transact();
        let root = txn.get_xml_fragment(FRAGMENT).unwrap();
        let XmlOut::Element(p) = root.get(&txn, 1).unwrap() else {
            panic!()
        };
        let html = p.get_string(&txn);
        assert!(html.contains("<strong>calm</strong>"), "{html}");
        assert!(html.contains("<em>one</em>"), "{html}");
        assert!(html.contains("<code>07:12</code>"), "{html}");
        assert!(html.contains("details"), "{html}");
        assert!(!html.contains("javascript"), "{html}");
        let XmlOut::Element(row) = root.get(&txn, 4).unwrap() else {
            panic!()
        };
        assert!(row.get_string(&txn).contains("Day · Events"));
    }

    #[test]
    fn append_update_adds_to_an_existing_page() {
        let doc = Doc::new();
        super::super::apply(&doc, &from_markdown("# Log\n\nfirst")).unwrap();
        let base = super::super::encode(&doc);
        let update = append_update(&doc, "## Monday\n\nsecond");
        // A peer holding the old page receives only the update.
        let peer = Doc::new();
        super::super::apply(&peer, &base).unwrap();
        super::super::apply(&peer, &update).unwrap();
        assert_eq!(
            tags(&peer),
            ["heading", "paragraph", "heading", "paragraph"]
        );
        super::super::schema::validate(&peer).unwrap();
    }
}
