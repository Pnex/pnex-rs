//! Server-side check of a page tree against the editor schema allowlist
//! (`pnex_core::pages`, P15): every update is applied to a scratch copy and
//! refused if the result holds an unknown root, block, attribute or mark,
//! a non-text inline, or a link outside the allowed schemes.

use pnex_core::pages::{link_allowed, node_attrs, FRAGMENT, LINK_ATTRS, MARKS};
use yrs::types::text::YChange;
use yrs::types::xml::{XmlFragment, XmlOut};
use yrs::{Any, Doc, Out, ReadTxn, Text, Transact, Xml};

/// Why a tree was refused (logged, never shown to the user verbatim).
#[derive(Debug, PartialEq, Eq)]
pub struct Invalid(pub String);

/// Checks the whole page held by `doc`.
pub fn validate(doc: &Doc) -> Result<(), Invalid> {
    let txn = doc.transact();
    for (name, _) in txn.root_refs() {
        if name != FRAGMENT {
            return Err(Invalid(format!("root type `{name}`")));
        }
    }
    let Some(root) = txn.get_xml_fragment(FRAGMENT) else {
        return Ok(());
    };
    for child in root.children(&txn) {
        check_node(&txn, child, 0)?;
    }
    Ok(())
}

/// Nesting cap: ProseMirror lists nest, but not without bound.
const MAX_DEPTH: usize = 32;

fn check_node<T: ReadTxn>(txn: &T, node: XmlOut, depth: usize) -> Result<(), Invalid> {
    if depth > MAX_DEPTH {
        return Err(Invalid("nesting too deep".into()));
    }
    match node {
        XmlOut::Element(el) => {
            let tag = el.tag();
            let allowed = node_attrs(tag).ok_or_else(|| Invalid(format!("node `{tag}`")))?;
            for (key, value) in el.attributes(txn) {
                if !allowed.contains(&key) {
                    return Err(Invalid(format!("attribute `{key}` on `{tag}`")));
                }
                check_attr(key, &value)?;
            }
            for child in el.children(txn) {
                check_node(txn, child, depth + 1)?;
            }
            Ok(())
        }
        XmlOut::Text(text) => {
            if text.attributes(txn).next().is_some() {
                return Err(Invalid("attribute on text".into()));
            }
            for chunk in text.diff(txn, YChange::identity) {
                if !matches!(chunk.insert, Out::Any(Any::String(_))) {
                    return Err(Invalid("embedded value in text".into()));
                }
                for (mark, value) in chunk.attributes.iter().flat_map(|a| a.iter()) {
                    check_mark(mark, value)?;
                }
            }
            Ok(())
        }
        XmlOut::Fragment(_) => Err(Invalid("nested fragment".into())),
    }
}

/// Typed node attributes: ProseMirror drops a node whose number attribute
/// is not a number, so a bad value would make every editor delete it.
fn check_attr(key: &str, value: &Out) -> Result<(), Invalid> {
    let n = match value {
        Out::Any(Any::Number(n)) => match (*n).as_i64() {
            Some(n) => n,
            None => return Err(Invalid(format!("attribute `{key}` is not an integer"))),
        },
        _ => return Err(Invalid(format!("attribute `{key}` is not an integer"))),
    };
    let ok = match key {
        "level" => (1..=6).contains(&n),
        "order" => (1..=1_000_000).contains(&n),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(Invalid(format!("attribute `{key}` out of range")))
    }
}

fn check_mark(mark: &str, value: &Any) -> Result<(), Invalid> {
    if !MARKS.contains(&mark) {
        return Err(Invalid(format!("mark `{mark}`")));
    }
    if mark != "link" {
        return Ok(());
    }
    let Any::Map(attrs) = value else {
        return Err(Invalid("link without attributes".into()));
    };
    for (key, v) in attrs.iter() {
        if !LINK_ATTRS.contains(&key.as_str()) {
            return Err(Invalid(format!("link attribute `{key}`")));
        }
        if key == "href" {
            match v {
                Any::String(href) if link_allowed(href) => {}
                _ => return Err(Invalid("link href".into())),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::types::xml::{XmlElementPrelim, XmlTextPrelim};
    use yrs::types::Attrs;

    fn page(build: impl FnOnce(&mut yrs::TransactionMut, yrs::XmlFragmentRef)) -> Doc {
        let doc = Doc::new();
        let root = doc.get_or_insert_xml_fragment(FRAGMENT);
        let mut txn = doc.transact_mut();
        build(&mut txn, root);
        drop(txn);
        doc
    }

    fn link(href: &str) -> Attrs {
        let mut a = Attrs::new();
        a.insert(
            "link".into(),
            Any::from(std::collections::HashMap::from([(
                "href".to_string(),
                Any::from(href),
            )])),
        );
        a
    }

    #[test]
    fn accepts_schema_tree() {
        let doc = page(|txn, root| {
            let h = root.push_back(txn, XmlElementPrelim::empty("heading"));
            h.insert_attribute(txn, "level", 2);
            let p = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
            let t = p.push_back(txn, XmlTextPrelim::new("see "));
            t.insert_with_attributes(txn, 4, "docs", link("https://pnex.io"));
        });
        assert_eq!(validate(&doc), Ok(()));
    }

    #[test]
    fn refuses_unknown_node_attribute_and_link() {
        let script = page(|txn, root| {
            root.push_back(txn, XmlElementPrelim::empty("script"));
        });
        assert!(validate(&script).is_err());
        let level = page(|txn, root| {
            let h = root.push_back(txn, XmlElementPrelim::empty("heading"));
            h.insert_attribute(txn, "level", "1");
        });
        assert!(validate(&level).is_err(), "string level");
        let deep = page(|txn, root| {
            let h = root.push_back(txn, XmlElementPrelim::empty("heading"));
            h.insert_attribute(txn, "level", 9);
        });
        assert!(validate(&deep).is_err(), "level 9");
        let onclick = page(|txn, root| {
            let p = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
            p.insert_attribute(txn, "onclick", "x");
        });
        assert!(validate(&onclick).is_err());
        let js = page(|txn, root| {
            let p = root.push_back(txn, XmlElementPrelim::empty("paragraph"));
            let t = p.push_back(txn, XmlTextPrelim::new(""));
            t.insert_with_attributes(txn, 0, "x", link("javascript:alert(1)"));
        });
        assert!(validate(&js).is_err());
    }

    /// State written by the real editor stack (Yjs 13.6 + y-prosemirror 1.3,
    /// js/doc.js schema) from Markdown covering every node and mark: the
    /// server must accept what the client produces.
    #[test]
    fn accepts_state_from_the_js_editor() {
        use base64::Engine as _;
        let b64 = include_str!("yjs_client_state.b64").trim();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        let doc = Doc::new();
        crate::services::pages::apply(&doc, &bytes).unwrap();
        assert_eq!(validate(&doc), Ok(()));
    }

    #[test]
    fn refuses_other_roots() {
        let doc = Doc::new();
        let m = doc.get_or_insert_map("evil");
        yrs::Map::insert(&m, &mut doc.transact_mut(), "k", "v");
        assert!(validate(&doc).is_err());
    }
}
