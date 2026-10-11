//! Anti-regression guard of the design system (docs/architecture/
//! design-system.md): class literals must stay on the shared type scale
//! and in sentence case. Uppercase labels, letter-spaced caps and
//! arbitrary pixel font sizes are the drift that turned the UI into a
//! patchwork before the 2026-10 overhaul.

#![cfg(test)]

use std::path::{Path, PathBuf};

/// The guard itself names the forbidden tokens.
const EXCLUDED_FILES: &[&str] = &["design_guard.rs"];

/// Forbidden class token and the rule it breaks.
fn violation(token: &str) -> Option<&'static str> {
    if token == "uppercase" {
        return Some("labels are sentence case, never uppercase");
    }
    if token.starts_with("tracking-wide") {
        return Some("no letter-spaced caps");
    }
    let size = token.strip_prefix("text-[")?;
    if size.ends_with("px]") || size.ends_with("rem]") {
        return Some("font sizes come from the scale (text-xs … text-3xl)");
    }
    None
}

/// Every whitespace-separated token of every string literal of a line.
fn literal_tokens(line: &str) -> Vec<&str> {
    line.split('"')
        .skip(1)
        .step_by(2)
        .flat_map(str::split_whitespace)
        .collect()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn class_literals_follow_the_design_system() {
    let mut files = Vec::new();
    rust_files(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let mut findings = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if EXCLUDED_FILES.contains(&name) {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for (no, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for token in literal_tokens(line) {
                // Responsive and state variants carry the same rule.
                let base = token.rsplit(':').next().unwrap_or(token);
                if let Some(rule) = violation(base) {
                    findings.push(format!("{}:{} `{token}`: {rule}", path.display(), no + 1));
                }
            }
        }
    }
    assert!(
        findings.is_empty(),
        "design-system drift:\n{}",
        findings.join("\n")
    );
}

#[test]
fn violation_rules() {
    assert!(violation("uppercase").is_some());
    assert!(violation("tracking-wider").is_some());
    assert!(violation("text-[11px]").is_some());
    assert!(violation("text-xs").is_none());
    assert!(violation("text-[var(--x)]").is_none());
    assert_eq!(literal_tokens(r#"class: "a b", x = "c""#), ["a", "b", "c"]);
}
