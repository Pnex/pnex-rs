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
    if token.contains("blue-") {
        return Some("the signal cyan is `signal-*`, not the stock blue scale");
    }
    let size = token.strip_prefix("text-[")?;
    if size.ends_with("px]") || size.ends_with("rem]") {
        return Some("font sizes come from the scale (text-xs … text-3xl)");
    }
    None
}

/// `dark:` on a scale shade: dark.css already mirrors every scale, so the
/// variant flips it back to a light shade on the dark theme (O53).
fn dark_violation(token: &str) -> Option<&'static str> {
    let rest = token.strip_prefix("dark:")?;
    let shade = rest.split('/').next()?.rsplit('-').next()?;
    shade
        .parse::<u16>()
        .is_ok()
        .then_some("dark.css mirrors the scales already: no `dark:` on a shade")
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
                if let Some(rule) = violation(base).or_else(|| dark_violation(token)) {
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
    assert!(dark_violation("dark:bg-gray-950").is_some());
    assert!(dark_violation("dark:bg-red-900/40").is_some());
    assert!(dark_violation("dark:bg-black/60").is_none());
    assert!(dark_violation("dark:hidden").is_none());
    assert_eq!(literal_tokens(r#"class: "a b", x = "c""#), ["a", "b", "c"]);
}

/// CRUD lists alike (design-system.md rule 8, D218): a list filtered by
/// label also has the name search, and the `filters` slot of `ListLayout`
/// holds a `FilterBar` (the slot adds no wrapper of its own).
#[test]
fn list_filter_bars_are_complete() {
    let mut files = Vec::new();
    rust_files(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pages"),
        &mut files,
    );
    // The ontology explorer searches inside its own type picker.
    const NO_SEARCH_OK: &[&str] = &[];
    let mut failures = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).expect("source");
        let name = path.display().to_string();
        if text.contains("LabelFilterInput {")
            && !text.contains("SearchInput {")
            && !NO_SEARCH_OK.iter().any(|f| name.ends_with(f))
        {
            failures.push(format!("{name}: label filter without the name search"));
        }
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("filters: rsx! {") {
                let next = text.lines().nth(i + 1).unwrap_or_default().trim_start();
                if !next.starts_with("FilterBar {") {
                    failures.push(format!(
                        "{name}:{}: `filters` slot without a FilterBar",
                        i + 1
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// One loading mark (D224): the gerbe (components/gerbe.rs), never a
/// hand-made `animate-spin` spinner.
#[test]
fn no_hand_made_spinner() {
    let mut files = Vec::new();
    rust_files(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    let banned = ["animate-", "spin", "pnex-spinner-", "delayed"];
    let (spin, delayed) = (
        banned[0].to_string() + banned[1],
        banned[2].to_string() + banned[3],
    );
    let offenders: Vec<String> = files
        .iter()
        .filter(|p| !p.ends_with("design_guard.rs"))
        .filter(|p| {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            text.contains(&spin) || text.contains(&delayed)
        })
        .map(|p| p.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "use components::gerbe::Gerbe: {offenders:?}"
    );
}
