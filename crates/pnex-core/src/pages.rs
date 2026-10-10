//! Pages (`docs/architecture/pages.md`, decision #21): collaborative
//! documents. Shared types backend ↔ frontend, and the editor schema
//! allowlist.
//!
//! The content of a page is a Yjs document (yrs on the server, Yjs in the
//! editor bundle `js/doc.js`), bound to ProseMirror by `y-prosemirror`:
//! each ProseMirror node is a `Y.XmlElement` named after the node type,
//! each mark a formatting attribute of a `Y.XmlText`. The server rejects
//! any update whose resulting tree leaves this allowlist (P15: no raw
//! HTML, no unknown block, link schemes allowlisted).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Root `Y.XmlFragment` name used by `y-prosemirror`; the only root type a
/// page may hold.
pub const FRAGMENT: &str = "prosemirror";

/// ProseMirror node types and the attributes each may carry.
pub const NODES: &[(&str, &[&str])] = &[
    ("paragraph", &[]),
    ("heading", &["level"]),
    ("blockquote", &[]),
    ("horizontal_rule", &[]),
    ("code_block", &[]),
    ("hard_break", &[]),
    ("bullet_list", &[]),
    ("ordered_list", &["order"]),
    ("list_item", &[]),
];

/// ProseMirror mark types (formatting attributes of a `Y.XmlText`).
pub const MARKS: &[&str] = &["em", "strong", "code", "link"];

/// Attributes of the `link` mark.
pub const LINK_ATTRS: &[&str] = &["href", "title"];

/// URL schemes a link may use (R11–R13); relative app paths start with `/`.
pub const LINK_SCHEMES: &[&str] = &["http:", "https:", "mailto:"];

/// Largest single update accepted on the websocket.
pub const MAX_UPDATE_BYTES: usize = 256 * 1024;
/// Largest encoded page state accepted (all history included).
pub const MAX_DOC_BYTES: usize = 8 * 1024 * 1024;
/// Title length (column `docs.title`).
pub const MAX_TITLE_CHARS: usize = 255;
/// Named version label length (column `doc_versions.label`).
pub const MAX_VERSION_LABEL_CHARS: usize = 255;

/// Is `href` an allowed link target?
pub fn link_allowed(href: &str) -> bool {
    let h = href.trim();
    (h.starts_with('/') && !h.starts_with("//"))
        || LINK_SCHEMES
            .iter()
            .any(|s| h.len() > s.len() && h[..s.len()].eq_ignore_ascii_case(s))
}

/// Allowed attributes of node `name`, `None` for an unknown node.
pub fn node_attrs(name: &str) -> Option<&'static [&'static str]> {
    NODES.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}

/// One page in a list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocSummary {
    pub id: Uuid,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

/// `POST /api/v1/docs`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateDoc {
    pub title: String,
}

/// `PATCH /api/v1/docs/{id}` (rename; the content goes through the
/// websocket).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatchDoc {
    pub title: String,
}

/// One named version (P5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocVersionSummary {
    pub version_number: i64,
    pub label: String,
    /// Display name of the author, if still known.
    pub created_by: Option<String>,
    pub created_at: String,
}

/// `POST /api/v1/docs/{id}/versions`: freezes the current content.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateDocVersion {
    pub label: String,
}

/// `GET /api/v1/docs/{id}/versions/{n}`: frozen Yjs state (v1 encoding,
/// base64), opened read-only by the editor for viewing, diff and restore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocVersionState {
    pub version_number: i64,
    pub label: String,
    pub state_b64: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_allowlisted() {
        for ok in ["https://pnex.io", "HTTP://x", "mailto:a@b.c", "/devices/3"] {
            assert!(link_allowed(ok), "{ok}");
        }
        for ko in [
            "javascript:alert(1)",
            "data:text/html,x",
            "//evil.io",
            "https:",
            "vbscript:x",
            "",
        ] {
            assert!(!link_allowed(ko), "{ko}");
        }
    }

    #[test]
    fn node_lookup() {
        assert_eq!(node_attrs("heading"), Some(&["level"][..]));
        assert_eq!(node_attrs("iframe"), None);
    }
}
