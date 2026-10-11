//! Bridge to the Pages editor (pages.md #21) — `assets/doc.js` (esbuild
//! IIFE of ProseMirror + Yjs, loaded by `main.rs`) exposes `window.pnexDoc`.
//!
//! One code path for web and native: `document::eval` (tron.rs school), no
//! closure crossing the bridge. Events come back by polling [`state`]
//! (`window.__pnexDoc[host]`). Failures (script missing, host not found)
//! return `false`/`None`: the page shows a badge, never panics.

use std::time::Duration;

use serde::Deserialize;

/// Live state of a mounted editor.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DocState {
    /// `connecting` | `synced` | `offline` | `refused` | `closed`.
    pub status: String,
    pub close_code: Option<u16>,
    /// Bumped on every disconnect that deserves a new ticket.
    pub need_ticket: u64,
    /// Other people on the page.
    pub peers: u32,
    /// Link or comment asked from the selection bubble.
    pub request: Option<Request>,
    /// Bumped when the server says the comments changed.
    pub comments_seq: u64,
    /// Comment whose highlighted text was clicked.
    pub active_comment: Option<ActiveComment>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ActiveComment {
    pub seq: u64,
    pub id: i64,
}

/// Anchor of a selection, for a new comment thread.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SelectionAnchor {
    pub anchor: String,
    pub quote: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Request {
    pub seq: u64,
    /// `link` | `comment`.
    pub kind: String,
}

/// One block of a version diff.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DiffBlock {
    /// `eq` | `add` | `del`.
    pub op: String,
    /// ProseMirror node type (`paragraph`, `heading`…).
    pub kind: String,
    pub text: String,
}

/// JS literal of a Rust string.
fn lit(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

async fn eval(js: &str) -> Option<serde_json::Value> {
    dioxus::document::eval(js).await.ok()
}

async fn eval_bool(js: &str) -> bool {
    eval(js).await.and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Mounts the live editor in `host` (retries 100 ms × 50: the bundle may
/// finish loading after the first render). `labels`: the texts of the "/"
/// menu, bubble and hints, resolved by the caller in render scope.
pub async fn mount(
    host: &str,
    ws_url: &str,
    editable: bool,
    user_name: &str,
    color: &str,
    labels: &serde_json::Value,
) -> bool {
    let opts = serde_json::json!({
        "wsUrl": ws_url,
        "editable": editable,
        "user": { "name": user_name, "color": color },
        "labels": labels,
    });
    let js = format!(
        "return window.pnexDoc?.mount({}, {opts}) === true",
        lit(host)
    );
    for _ in 0..50 {
        if eval_bool(&js).await {
            return true;
        }
        crate::util::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Mounts a frozen version (base64 Yjs state), read-only.
pub async fn mount_static(host: &str, state_b64: &str) -> bool {
    let js = format!(
        "return window.pnexDoc?.mountStatic({}, {}) === true",
        lit(host),
        lit(state_b64)
    );
    for _ in 0..50 {
        if eval_bool(&js).await {
            return true;
        }
        crate::util::sleep(Duration::from_millis(100)).await;
    }
    false
}

pub async fn reconnect(host: &str, ws_url: &str) {
    eval(&format!(
        "window.pnexDoc?.reconnect({}, {})",
        lit(host),
        lit(ws_url)
    ))
    .await;
}

/// Fire-and-forget unmount (component drop).
pub fn unmount(host: &str) {
    let js = format!("window.pnexDoc?.unmount({})", lit(host));
    dioxus::dioxus_core::spawn_forever(async move {
        eval(&js).await;
    });
}

pub async fn state(host: &str) -> Option<DocState> {
    let v = eval(&format!(
        "return (window.__pnexDoc || {{}})[{}] || null",
        lit(host)
    ))
    .await?;
    serde_json::from_value(v).ok()
}

/// Header command (`undo`, `redo`, see doc.js).
pub async fn command(host: &str, name: &str) -> bool {
    eval_bool(&format!(
        "return window.pnexDoc?.command({}, {}) === true",
        lit(host),
        lit(name)
    ))
    .await
}

/// Link on the selection; `false` when refused (scheme, empty selection).
pub async fn link(host: &str, href: &str) -> bool {
    eval_bool(&format!(
        "return window.pnexDoc?.link({}, {}) === true",
        lit(host),
        lit(href)
    ))
    .await
}

pub async fn selection_anchor(host: &str) -> Option<SelectionAnchor> {
    let v = eval(&format!(
        "return window.pnexDoc?.selectionAnchor({}) ?? null",
        lit(host)
    ))
    .await?;
    serde_json::from_value(v).ok()
}

/// Comments to highlight: `(id, anchor, resolved)`.
pub async fn set_comments(host: &str, comments: &[(i64, Option<String>, bool)]) -> bool {
    let list: Vec<serde_json::Value> = comments
        .iter()
        .map(|(id, anchor, resolved)| serde_json::json!({ "id": id, "anchor": anchor, "resolved": resolved }))
        .collect();
    eval_bool(&format!(
        "return window.pnexDoc?.setComments({}, {}) === true",
        lit(host),
        serde_json::Value::Array(list)
    ))
    .await
}

/// Selects the text of an anchor; `false` when that text is gone.
pub async fn focus_anchor(host: &str, anchor: &str) -> bool {
    eval_bool(&format!(
        "return window.pnexDoc?.focusAnchor({}, {}) === true",
        lit(host),
        lit(anchor)
    ))
    .await
}

pub async fn export_markdown(host: &str) -> Option<String> {
    eval(&format!(
        "return window.pnexDoc?.exportMarkdown({}) ?? null",
        lit(host)
    ))
    .await
    .and_then(|v| v.as_str().map(str::to_string))
}

pub async fn import_markdown(host: &str, md: &str) -> bool {
    eval_bool(&format!(
        "return window.pnexDoc?.importMarkdown({}, {}) === true",
        lit(host),
        lit(md)
    ))
    .await
}

/// Replaces the live content with a frozen version.
pub async fn restore(host: &str, state_b64: &str) -> bool {
    eval_bool(&format!(
        "return window.pnexDoc?.restore({}, {}) === true",
        lit(host),
        lit(state_b64)
    ))
    .await
}

/// Waits until the server has persisted every local edit of the live page
/// in `host`; false when offline, refused or after 10 s.
pub async fn flush(host: &str) -> bool {
    eval_bool(&format!(
        "return (await window.pnexDoc?.flush({})) === true",
        lit(host)
    ))
    .await
}

/// Block diff between two frozen versions (`a` older, `b` newer).
pub async fn diff(a_b64: &str, b_b64: &str) -> Option<Vec<DiffBlock>> {
    let v = eval(&format!(
        "return window.pnexDoc?.diff({}, {}) ?? null",
        lit(a_b64),
        lit(b_b64)
    ))
    .await?;
    serde_json::from_value(v).ok()
}

#[cfg(test)]
mod tests {
    /// The editor schema (js/doc.js) and the server allowlist
    /// (`pnex_core::pages`) must agree: a node or scheme known to one side
    /// only means refused edits or unchecked content.
    #[test]
    fn js_schema_matches_server_allowlist() {
        let js = include_str!("../js/doc.js");
        let header = js
            .split("// Nodes:")
            .nth(1)
            .and_then(|rest| rest.split("\n\n").next())
            .expect("node list comment in doc.js");
        for (node, _) in pnex_core::pages::NODES {
            assert!(header.contains(node), "node `{node}` missing from doc.js");
        }
        let schemes = js
            .split("const LINK_SCHEMES = [")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .expect("LINK_SCHEMES in doc.js");
        let listed: Vec<&str> = schemes
            .split(',')
            .map(|s| s.trim().trim_matches('\''))
            .collect();
        assert_eq!(listed, pnex_core::pages::LINK_SCHEMES);
        for mark in pnex_core::pages::MARKS {
            assert!(
                js.contains(&format!("schema.marks.{mark}")),
                "mark `{mark}`"
            );
        }
    }
}
