//! Anti-regression guard for HTML injection in the hand-written JS glue
//! (`js/*.js`, docs/architecture/security.md R11, SEC-6). User strings
//! (labels, names, notes) reach these files and render on the app origin,
//! where the session tokens live:
//! - every pannellum viewer is created with `escapeHTML: true` (pannellum
//!   renders hotspot text as raw HTML otherwise);
//! - no HTML sink other than clearing a node (`innerHTML = ''`): text goes
//!   through `textContent` / DOM nodes.

#![cfg(test)]

use std::path::PathBuf;

fn js_sources() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("js");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("js/ directory") {
        let path = entry.expect("js entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("js") {
            let src = std::fs::read_to_string(&path).expect("read js source");
            out.push((path.display().to_string(), src));
        }
    }
    assert!(!out.is_empty(), "no js source scanned");
    out
}

/// Lines that are pure comments are ignored by the sink scan.
fn code_lines(src: &str) -> impl Iterator<Item = (usize, &str)> {
    src.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.starts_with("//") && !l.starts_with('*') && !l.starts_with("/*"))
}

#[test]
fn every_pannellum_viewer_escapes_html() {
    for (path, src) in js_sources() {
        for (start, _) in src.match_indices("pannellumLib.viewer(") {
            let config = &src[start..];
            let end = config.find("});").unwrap_or(config.len());
            assert!(
                config[..end].contains("escapeHTML: true"),
                "{path}: pannellumLib.viewer without `escapeHTML: true` at byte {start}"
            );
        }
    }
}

#[test]
fn no_html_sinks_in_js_glue() {
    const SINKS: &[&str] = &[
        "insertAdjacentHTML",
        "outerHTML",
        "document.write",
        "setHTML(",
        "createContextualFragment",
    ];
    for (path, src) in js_sources() {
        for (n, line) in code_lines(&src) {
            for sink in SINKS {
                assert!(!line.contains(sink), "{path}:{n}: HTML sink `{sink}`");
            }
            if let Some(pos) = line.find("innerHTML") {
                let rest = line[pos + "innerHTML".len()..].trim_start();
                assert!(
                    rest.starts_with("= ''") || rest.starts_with("= \"\""),
                    "{path}:{n}: innerHTML may only be cleared, use textContent"
                );
            }
        }
    }
}
