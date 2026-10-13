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

/// Largest single update accepted on the websocket (a version restore or
/// a Markdown import of a large page is one update).
pub const MAX_UPDATE_BYTES: usize = 2 * 1024 * 1024;
/// Largest encoded page state accepted (all history included).
pub const MAX_DOC_BYTES: usize = 8 * 1024 * 1024;
/// Title length (column `docs.title`).
pub const MAX_TITLE_CHARS: usize = 255;
/// Named version label length (column `doc_versions.label`).
pub const MAX_VERSION_LABEL_CHARS: usize = 255;

/// Comment body length (P12).
pub const MAX_COMMENT_CHARS: usize = 4000;
/// Encoded anchor (two Yjs relative positions, base64) length.
pub const MAX_ANCHOR_CHARS: usize = 1024;
/// Quoted text kept with a thread (longer selections are cut).
pub const MAX_QUOTE_CHARS: usize = 300;

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
    /// Holds AI-generated text (D210).
    #[serde(default)]
    pub ai_generated: bool,
    /// Flow that wrote it (`page_write`, D209).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<i64>,
    /// Display name of who approved it, once approved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
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

/// One comment (P12). A thread root has `parent_id = None` and carries the
/// anchor; replies point to their root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocComment {
    pub id: i64,
    pub parent_id: Option<i64>,
    /// Opaque anchor written by the editor (Yjs relative positions).
    pub anchor: Option<String>,
    pub quote: Option<String>,
    pub body: String,
    /// Display name of the author, if still known.
    pub author: Option<String>,
    /// The caller wrote it (may edit and delete it).
    pub mine: bool,
    pub resolved: bool,
    /// Named version current when the comment was posted.
    pub version_number: Option<i64>,
    pub created_at: String,
}

/// `POST /api/v1/docs/{id}/comments`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateDocComment {
    pub body: String,
    #[serde(default)]
    pub parent_id: Option<i64>,
    #[serde(default)]
    pub anchor: Option<String>,
    #[serde(default)]
    pub quote: Option<String>,
}

/// `PATCH /api/v1/docs/{id}/comments/{cid}`: new body (author only) and/or
/// resolved state (thread roots).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatchDocComment {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub resolved: Option<bool>,
}

/// How a `page_write` node writes (D209).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageWriteMode {
    /// A new page per message, titled from the template.
    #[default]
    NewPage,
    /// Appended at the end of one existing page.
    Append,
}

/// Config of the `page_write` flow node (media-vision-studio.md D209).
/// Templates (`title`, `version_label`) accept `{date}`, `{time}`,
/// `{week}` (UTC at write time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageWriteConfig {
    #[serde(default)]
    pub mode: PageWriteMode,
    /// Title of each new page (`new_page`).
    #[serde(default)]
    pub title: String,
    /// The page appended to (`append`).
    #[serde(default)]
    pub page_id: String,
    /// Dotted path of the Markdown inside `msg.payload`.
    #[serde(default = "default_text_field")]
    pub text_field: String,
    /// Label of the named version created after each write.
    #[serde(default = "default_version_label")]
    pub version_label: String,
}

fn default_text_field() -> String {
    "text".into()
}

fn default_version_label() -> String {
    "{date}".into()
}

impl Default for PageWriteConfig {
    fn default() -> Self {
        Self {
            mode: PageWriteMode::NewPage,
            title: String::new(),
            page_id: String::new(),
            text_field: default_text_field(),
            version_label: default_version_label(),
        }
    }
}

impl PageWriteConfig {
    pub fn check(&self) -> Option<(&'static str, String)> {
        match self.mode {
            PageWriteMode::NewPage => {
                let n = self.title.trim().chars().count();
                if n == 0 || n > MAX_TITLE_CHARS {
                    return Some((
                        "page_write_title_invalid",
                        format!("write a title of 1 to {MAX_TITLE_CHARS} characters"),
                    ));
                }
            }
            PageWriteMode::Append => {
                if Uuid::parse_str(self.page_id.trim()).is_err() {
                    return Some(("page_write_page_missing", "pick a page".into()));
                }
            }
        }
        let path_ok = !self.text_field.trim().is_empty()
            && self.text_field.len() <= 128
            && self
                .text_field
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        let label_n = self.version_label.trim().chars().count();
        if !path_ok || label_n == 0 || label_n > MAX_VERSION_LABEL_CHARS {
            return Some((
                "page_write_options_invalid",
                "text path made of letters, digits, _ - . and a version label of 1 to 255 characters".into(),
            ));
        }
        None
    }
}

/// Largest Markdown a flow writes in one message, chars.
pub const MAX_FLOW_MARKDOWN: usize = 100_000;

/// `POST /internal/flow/page-write` (D209).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageWriteRequest {
    pub org_id: i64,
    pub flow_id: i64,
    pub mode: PageWriteMode,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub page_id: String,
    pub markdown: String,
    pub version_label: String,
    /// The Markdown holds AI-generated text (`payload.ai_generated`).
    #[serde(default)]
    pub ai_generated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageWriteResponse {
    pub page_id: String,
    pub title: String,
    /// Number of the named version created.
    pub version: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_write_check() {
        let mut c = PageWriteConfig::default();
        assert_eq!(c.check().unwrap().0, "page_write_title_invalid");
        c.title = "Weekly report {date}".into();
        assert!(c.check().is_none());
        c.mode = PageWriteMode::Append;
        assert_eq!(c.check().unwrap().0, "page_write_page_missing");
        c.page_id = "00000000-0000-0000-0000-000000000001".into();
        c.text_field = String::new();
        assert_eq!(c.check().unwrap().0, "page_write_options_invalid");
    }

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
