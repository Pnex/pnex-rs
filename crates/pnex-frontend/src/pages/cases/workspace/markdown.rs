//! Markdown export of a case (cases-workspace.md K33): the frame, the
//! timeline with what each event involves, the undated pieces, the
//! hypotheses board, the pieces manifest and, from a frozen state, its
//! number, fingerprint and reviewed detections. Rendered here so the
//! labels follow the reader's language.

use pnex_core::ontology::api::{CaseExport, CaseProposal, CaseSnapshot};
use pnex_core::ontology::{CASE_LEGAL_BASIS, CASE_QUESTION, EVENT_NOTE};
use serde_json::Value;

use super::model;

/// The words of the document: fixed labels by key, roles, detections.
pub(super) struct Words<'a> {
    pub label: &'a dyn Fn(&str) -> String,
    pub role: &'a dyn Fn(&str) -> String,
    pub proposal: &'a dyn Fn(&CaseProposal) -> String,
}

/// One line of user text (titles may hold line breaks).
fn line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn render(
    doc: &CaseExport,
    frozen: Option<(&CaseSnapshot, &[CaseProposal])>,
    local: bool,
    w: &Words,
) -> String {
    let l = w.label;
    let ws = &doc.workspace;
    let text = |k: &str| {
        ws.case
            .properties
            .get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let mut out = format!("# {}\n\n", line(&ws.case.title));
    if let Some(q) = text(CASE_QUESTION) {
        out += &format!("- **{}**: {}\n", l("case-question"), line(&q));
    }
    if let Some(b) = text(CASE_LEGAL_BASIS) {
        out += &format!("- **{}**: {}\n", l("cw-legal-basis"), line(&b));
    }
    out += &format!("- **{}**: {}\n", l("cw-md-exported"), doc.exported_at);
    if let Some((s, _)) = frozen {
        let label = s.label.as_deref().map(line).unwrap_or_default();
        out += &format!(
            "- **{} {}** {} — {}{}\n- **SHA-256**: `{}`\n",
            l("cw-md-snapshot"),
            s.seq,
            label,
            s.taken_at,
            s.taken_by
                .as_deref()
                .map(|b| format!(", {b}"))
                .unwrap_or_default(),
            s.sha256
        );
    }
    if doc.persons > 0 {
        out += &format!("\n> {}\n", l("cw-md-persons"));
    }

    out += &format!("\n## {}\n", l("cw-md-timeline"));
    if ws.events.is_empty() {
        out += &format!("\n{}\n", l("cw-md-none"));
    }
    for e in &ws.events {
        let when = match model::span(&e.event) {
            Some((a, b)) if a == b => model::stamp(a, local),
            Some((a, b)) => format!("{} → {}", model::stamp(a, local), model::stamp(b, local)),
            None => String::new(),
        };
        out += &format!("\n### {when} — {}\n", line(&e.event.title));
        if let Some(n) = e.event.properties.get(EVENT_NOTE).and_then(Value::as_str) {
            out += &format!("\n{}\n", n.trim());
        }
        if !e.attachments.is_empty() {
            out.push('\n');
        }
        for a in &e.attachments {
            let detail = a
                .detail
                .as_deref()
                .map(|d| format!(" — {}", line(d)))
                .unwrap_or_default();
            out += &format!(
                "- {}: {} ({}){detail}\n",
                (w.role)(&a.role),
                line(&a.object.title),
                a.object.type_key
            );
        }
    }

    if !ws.undated.is_empty() {
        out += &format!("\n## {}\n\n", l("cw-undated"));
        for u in &ws.undated {
            out += &format!("- {} ({})\n", line(&u.title), u.type_key);
        }
    }

    let b = &doc.board;
    if !b.hypotheses.is_empty() {
        out += &format!("\n## {}\n", l("cw-board"));
        for h in &b.hypotheses {
            out += &format!("\n### {}\n\n", line(&h.hypothesis.title));
            for (sign, claims) in [("+", &h.supports), ("−", &h.contradicts)] {
                for c in claims {
                    out += &format!(
                        "- {sign} {} ({}, {})\n",
                        line(&c.claim.title),
                        c.status.as_deref().unwrap_or("?"),
                        c.sources
                    );
                }
            }
        }
    }

    if !doc.pieces.is_empty() {
        out += &format!(
            "\n## {}\n\n| {} | SHA-256 | {} | {} |\n|---|---|---|---|\n",
            l("cw-md-pieces"),
            l("cw-md-piece"),
            l("cw-md-captured"),
            l("cw-md-source")
        );
        for p in &doc.pieces {
            let cell = |s: &Option<String>| line(s.as_deref().unwrap_or("")).replace('|', "\\|");
            out += &format!(
                "| {} | `{}` | {} | {} |\n",
                line(&p.title).replace('|', "\\|"),
                p.sha256.as_deref().unwrap_or(""),
                cell(&p.captured_at),
                cell(&p.source_url)
            );
        }
    }

    if let Some((_, reviewed)) = frozen.filter(|(_, r)| !r.is_empty()) {
        out += &format!("\n## {}\n\n", l("cw-detections"));
        for p in reviewed {
            let key = if p.status == "retained" {
                "cw-md-retained"
            } else {
                "cw-md-rejected"
            };
            let at = model::ms(&p.t_start)
                .map(|t| model::stamp(t, local))
                .unwrap_or_default();
            let why = p
                .reject_reason
                .as_deref()
                .map(|r| format!(" — {}", line(r)))
                .unwrap_or_default();
            out += &format!(
                "- {at} {} [{}] `{}`{why}\n",
                (w.proposal)(p),
                l(key),
                p.detector
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::ontology::api::{
        CaseAttachment, CaseBoard, CaseEvent, CaseWorkspace, ObjectRef, ObjectView, PieceEntry,
    };
    use serde_json::json;

    fn view(type_key: &str, title: &str, props: Value) -> ObjectView {
        serde_json::from_value(json!({
            "id": title, "type_key": type_key, "title": title, "native_id": "", "system": false,
            "properties": props, "version": 1, "valid_from": "2025-01-01T00:00:00Z",
            "created_at": "2025-01-01T00:00:00Z", "updated_at": "2025-01-01T00:00:00Z",
        }))
        .unwrap()
    }

    #[test]
    fn renders_the_timeline_pieces_and_fingerprint() {
        let ann = ObjectRef {
            id: "a".into(),
            type_key: "person".into(),
            title: "Ann\nB.".into(),
            native_id: String::new(),
        };
        let doc = CaseExport {
            schema: "pnex.case/1".into(),
            exported_at: "2025-06-01T00:00:00Z".into(),
            workspace: CaseWorkspace {
                case: view("case", "Leak", json!({"question": "Why?"})),
                events: vec![CaseEvent {
                    event: view(
                        "event",
                        "Leak seen",
                        json!({"occurred": {"from": "2025-05-02T09:00:00Z", "to": "2025-05-02T09:00:00Z", "precision": "instant"}, "note": "Wet floor"}),
                    ),
                    attachments: vec![CaseAttachment {
                        link_id: 1,
                        object: ann,
                        role: "actor".into(),
                        detail: Some("witness".into()),
                    }],
                }],
                undated: Vec::new(),
                zones: Vec::new(),
                places: Vec::new(),
                parents: Vec::new(),
                children: Vec::new(),
            },
            board: CaseBoard::default(),
            pieces: vec![PieceEntry {
                id: "p".into(),
                title: "Order | 12".into(),
                sha256: Some("ab".into()),
                ..Default::default()
            }],
            persons: 1,
        };
        let snap = CaseSnapshot {
            seq: 3,
            sha256: "f00".into(),
            ..Default::default()
        };
        let words = Words {
            label: &|k| k.to_string(),
            role: &|r| r.to_uppercase(),
            proposal: &|p| p.kind.clone(),
        };
        let md = render(&doc, Some((&snap, &[])), false, &words);
        assert!(md.starts_with("# Leak\n"), "{md}");
        assert!(md.contains("- **case-question**: Why?"), "{md}");
        assert!(
            md.contains("cw-md-snapshot 3") && md.contains("`f00`"),
            "{md}"
        );
        assert!(md.contains("> cw-md-persons"), "{md}");
        assert!(
            md.contains("### 2025-05-02 09:00 — Leak seen\n\nWet floor\n"),
            "{md}"
        );
        assert!(md.contains("- ACTOR: Ann B. (person) — witness"), "{md}");
        assert!(md.contains("| Order \\| 12 | `ab` |"), "{md}");
    }
}
