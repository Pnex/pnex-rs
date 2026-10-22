//! A pack page template filled from one object (account-mapping.md D340):
//! generic placeholders in the template body, read at instantiation, so
//! the business content lives in the pack (D190) and the core only knows
//! objects, links and hierarchies. The result is plain Markdown: a snapshot
//! of the day, edited freely afterwards.
//!
//! - `{{title}}`, `{{today}}`, `{{prop:<key>}}`
//! - `{{links:<link type>}}` (from the object) or `{{links:<link type>:in}}`
//!   (to it): one bullet per open link, with its attributes;
//! - `{{tree:<hierarchy>}}`, `{{coverage:<hierarchy>}}`,
//!   `{{white_space:<hierarchy>}}`: the tree from the object, its counts
//!   and sums, its uncovered nodes (D330).
//!
//! An unknown placeholder is left as written, for the author to see.

use std::collections::HashMap;

use serde_json::{Map, Value};

use super::api::{HierarchyView, LinkView};

/// Bullets listed at most per placeholder.
pub const LIST_MAX: usize = 100;

/// What a template is filled with.
pub struct RenderCtx<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub properties: &'a Map<String, Value>,
    pub today: &'a str,
    /// Open links touching the object.
    pub links: &'a [LinkView],
    /// Hierarchy key → tree from the object.
    pub trees: &'a HashMap<String, HierarchyView>,
}

/// The placeholders of a body, in order (`links:knows:in`…).
pub fn placeholders(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { break };
        out.push(after[..j].trim().to_string());
        rest = &after[j + 2..];
    }
    out
}

/// Hierarchies a body needs.
pub fn hierarchies(body: &str) -> Vec<String> {
    let mut keys: Vec<String> = placeholders(body)
        .iter()
        .filter_map(|p| {
            let (kind, key) = p.split_once(':')?;
            matches!(kind, "tree" | "coverage" | "white_space").then(|| key.to_string())
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// Text of a property value: money as `amount currency`, booleans as
/// ✓ / ✗, lists joined.
pub fn text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "✓" } else { "✗" }.into(),
        Value::Number(n) => n.to_string(),
        Value::Object(o) if o.contains_key("amount") => {
            let amount = o.get("amount").map(text).unwrap_or_default();
            let currency = o.get("currency").and_then(Value::as_str).unwrap_or("");
            format!("{amount} {currency}").trim().to_string()
        }
        Value::Array(a) => a.iter().map(text).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

fn attrs(a: &Map<String, Value>) -> String {
    a.iter()
        .filter(|(_, v)| !text(v).is_empty())
        .map(|(k, v)| format!("{}: {}", words(k), text(v)))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn links(ctx: &RenderCtx, link_type: &str, incoming: bool) -> String {
    let lines: Vec<String> = ctx
        .links
        .iter()
        .filter(|l| l.link_type == link_type && l.valid_to.is_none())
        .filter_map(|l| {
            let (me, other) = if incoming {
                (&l.target, &l.source)
            } else {
                (&l.source, &l.target)
            };
            (me.id == ctx.id).then(|| {
                let a = attrs(&l.attributes);
                if a.is_empty() {
                    format!("- {}", other.title)
                } else {
                    format!("- {} — {a}", other.title)
                }
            })
        })
        .take(LIST_MAX)
        .collect();
    if lines.is_empty() {
        "—".into()
    } else {
        lines.join("\n")
    }
}

fn tree(v: &HierarchyView) -> String {
    let lines: Vec<String> = v
        .nodes
        .iter()
        .take(LIST_MAX)
        .map(|n| {
            let marks: Vec<String> = n
                .marks
                .iter()
                .map(|m| format!("[{}]", words(&m.value)))
                .collect();
            let mut line = format!("{}- {}", "  ".repeat(n.depth as usize), n.title);
            if !marks.is_empty() {
                line.push(' ');
                line.push_str(&marks.join(" "));
            }
            line
        })
        .collect();
    lines.join("\n")
}

/// A key or an enum value as words: `_` would open emphasis in Markdown.
fn words(k: &str) -> String {
    k.replace('_', " ")
}

fn coverage(v: &HierarchyView) -> String {
    let mut parts: Vec<String> = v
        .counts
        .iter()
        .map(|c| format!("{} {}", words(&c.value), c.count))
        .collect();
    parts.extend(
        v.totals
            .iter()
            .map(|t| format!("{} {}", words(&t.property), t.total)),
    );
    if v.has_coverage {
        parts.push(format!("white space {}", v.white_space));
    }
    if parts.is_empty() {
        "—".into()
    } else {
        parts.join(" · ")
    }
}

fn white_space(v: &HierarchyView) -> String {
    let mut seen = std::collections::HashSet::new();
    let lines: Vec<String> = v
        .nodes
        .iter()
        .filter(|n| n.white_space && seen.insert(n.id.clone()))
        .take(LIST_MAX)
        .map(|n| format!("- {}", n.title))
        .collect();
    if lines.is_empty() {
        "—".into()
    } else {
        lines.join("\n")
    }
}

fn fill(ctx: &RenderCtx, p: &str) -> Option<String> {
    let parts: Vec<&str> = p.split(':').collect();
    let tree_of = |k: &str| ctx.trees.get(k);
    Some(match parts.as_slice() {
        ["title"] => ctx.title.to_string(),
        ["today"] => ctx.today.to_string(),
        ["prop", key] => ctx.properties.get(*key).map(text).unwrap_or_default(),
        ["links", t] => links(ctx, t, false),
        ["links", t, "in"] => links(ctx, t, true),
        ["tree", k] => tree(tree_of(k)?),
        ["coverage", k] => coverage(tree_of(k)?),
        ["white_space", k] => white_space(tree_of(k)?),
        _ => return None,
    })
}

/// The body with its placeholders filled.
pub fn render(body: &str, ctx: &RenderCtx) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        let p = after[..j].trim();
        match fill(ctx, p) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[i..i + 2 + j + 2]),
        }
        rest = &after[j + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ontology::api::{HierarchyNode, MarkCount, NodeMark, ObjectRef, PropertyTotal};
    use serde_json::json;

    fn r(id: &str, title: &str) -> ObjectRef {
        ObjectRef {
            id: id.into(),
            type_key: "organization".into(),
            title: title.into(),
            native_id: String::new(),
        }
    }

    fn link(t: &str, from: ObjectRef, to: ObjectRef, attrs: Value) -> LinkView {
        LinkView {
            id: 1,
            link_type: t.into(),
            source: from,
            target: to,
            attributes: attrs.as_object().cloned().unwrap_or_default(),
            valid_from: "2026-01-01T00:00:00Z".into(),
            valid_to: None,
            source_ref: None,
        }
    }

    #[test]
    fn placeholders_fill_from_the_object_its_links_and_its_tree() {
        let me = r("m", "Mecachrome France");
        let links = vec![
            link(
                "utilise_solution",
                me.clone(),
                r("c", "Concurrent SA"),
                json!({"depuis": "2024-01-01"}),
            ),
            link(
                "signal_sur",
                r("s", "Ouverture usine"),
                me.clone(),
                json!({}),
            ),
        ];
        let node = |id: &str, title: &str, depth, white| HierarchyNode {
            id: id.into(),
            type_key: "organization".into(),
            title: title.into(),
            depth,
            link: String::new(),
            also_under: vec![],
            marks: vec![NodeMark {
                property: "statut_compte".into(),
                value: "client".into(),
                color: "green".into(),
            }],
            white_space: white,
        };
        let view = HierarchyView {
            nodes: vec![
                node("m", "Mecachrome France", 0, false),
                node("t", "Toulouse", 1, true),
            ],
            counts: vec![MarkCount {
                property: "statut_compte".into(),
                value: "client".into(),
                color: "green".into(),
                count: 2,
            }],
            totals: vec![PropertyTotal {
                property: "utilisateurs_actuels".into(),
                total: 50.0,
            }],
            has_coverage: true,
            white_space: 1,
            ..Default::default()
        };
        let trees = HashMap::from([("groupe_commercial".to_string(), view)]);
        let props =
            json!({"siren": "429853351", "ca_dernier": {"amount": 343838577.0, "currency": "EUR"}});
        let ctx = RenderCtx {
            id: "m",
            title: "Mecachrome France",
            properties: props.as_object().unwrap(),
            today: "2026-10-22",
            links: &links,
            trees: &trees,
        };
        let body = "# Plan — {{title}} ({{today}})\nSIREN {{prop:siren}}, CA {{prop:ca_dernier}}\n{{links:utilise_solution}}\n{{links:signal_sur:in}}\n{{tree:groupe_commercial}}\n{{coverage:groupe_commercial}}\n{{white_space:groupe_commercial}}\n{{nope}} {{links:knows}}";
        assert_eq!(hierarchies(body), vec!["groupe_commercial"]);
        let out = render(body, &ctx);
        assert_eq!(
            out,
            "# Plan — Mecachrome France (2026-10-22)\nSIREN 429853351, CA 343838577.0 EUR\n- Concurrent SA — depuis: 2024-01-01\n- Ouverture usine\n- Mecachrome France [client]\n  - Toulouse [client]\nclient 2 · utilisateurs actuels 50 · white space 1\n- Toulouse\n{{nope}} —"
        );
    }
}
