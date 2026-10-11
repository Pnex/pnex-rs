//! First content of a page built on the server from a pack template
//! (pages.md P13): a Markdown subset turned into the `y-prosemirror` tree
//! the editor reads (headings, paragraphs, bullet and ordered lists).

use pnex_core::pages::FRAGMENT;
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::{Any, Doc, Transact, Xml};

enum Block {
    Heading(u8, String),
    Paragraph(String),
    List(bool, Vec<String>),
}

fn parse(md: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Paragraph(para.join(" ")));
            para.clear();
        }
    };
    // Trailing spaces kept: an empty item ("- ") is a slot to fill in.
    for line in md.lines().map(str::trim_start) {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        let ordered = line
            .split_once(". ")
            .filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .map(|(_, t)| t);
        let item = line
            .strip_prefix("- ")
            .map(|t| (false, t))
            .or(ordered.map(|t| (true, t)));
        if line.trim().is_empty() {
            flush(&mut para, &mut out);
        } else if (1..=3).contains(&hashes) && line[hashes..].starts_with(' ') {
            flush(&mut para, &mut out);
            out.push(Block::Heading(
                hashes as u8,
                line[hashes + 1..].trim().into(),
            ));
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
    flush(&mut para, &mut out);
    out
}

/// A page state (v1 update) holding `md`.
pub fn from_markdown(md: &str) -> Vec<u8> {
    let doc = Doc::new();
    let root = doc.get_or_insert_xml_fragment(FRAGMENT);
    {
        let mut txn = doc.transact_mut();
        let text_block = |tag: &str, text: &str| {
            let children = if text.is_empty() {
                vec![]
            } else {
                vec![XmlTextPrelim::new(text).into()]
            };
            XmlElementPrelim::new(tag, children)
        };
        for b in parse(md) {
            match b {
                Block::Heading(level, text) => {
                    let el = root.push_back(&mut txn, text_block("heading", &text));
                    el.insert_attribute(&mut txn, "level", Any::Number(i64::from(level).into()));
                }
                Block::Paragraph(text) => {
                    root.push_back(&mut txn, text_block("paragraph", &text));
                }
                Block::List(ordered, items) => {
                    let tag = if ordered {
                        "ordered_list"
                    } else {
                        "bullet_list"
                    };
                    let items = items.iter().map(|t| {
                        XmlElementPrelim::new("list_item", [text_block("paragraph", t).into()])
                            .into()
                    });
                    root.push_back(&mut txn, XmlElementPrelim::new(tag, items));
                }
            }
        }
    }
    super::encode(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::types::xml::XmlOut;
    use yrs::{GetString, ReadTxn};

    #[test]
    fn template_becomes_a_valid_page() {
        let state = from_markdown(
            "## Problem\n\nWhat failed,\nand when.\n\n1. Why?\n2. \n\n- Method\n- \n",
        );
        let doc = Doc::new();
        super::super::apply(&doc, &state).unwrap();
        super::super::schema::validate(&doc).unwrap();
        let txn = doc.transact();
        let root = txn.get_xml_fragment(FRAGMENT).unwrap();
        let tags: Vec<String> = root
            .children(&txn)
            .map(|c| match c {
                XmlOut::Element(e) => e.tag().to_string(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(
            tags,
            ["heading", "paragraph", "ordered_list", "bullet_list"]
        );
        let XmlOut::Element(p) = root.get(&txn, 1).unwrap() else {
            panic!()
        };
        assert!(p.get_string(&txn).contains(">What failed, and when.<"));
    }
}
