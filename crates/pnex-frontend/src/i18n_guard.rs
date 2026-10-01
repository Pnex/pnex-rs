//! Anti-regression guard for hardcoded French UI strings (i18n chantier).
//! Scan of frontend sources: string literals with French markers (accents
//! or stop-words) outside logs/tests/allowlist fail the test. Complements
//! the `t!` sweep (which only checks keys of existing `t!` calls) — this
//! one catches strings that bypass `t!` entirely.

#![cfg(test)]

use std::path::PathBuf;

/// Files exempted: the dev-only `/_showcase` page (documented exception —
/// hardcoded French labels assumed out-of-product) and the guard itself
/// (its stop-word constants would self-flag).
const EXCLUDED_FILES: &[&str] = &["showcase.rs", "i18n_guard.rs"];

/// Line-level exclusions: log macros + expect/panic (developer-only
/// output), and the take360 device-side filelog.
const LOG_PREFIXES: &[&str] = &[
    "eprintln!",
    "println!",
    "tracing::",
    "debug!",
    "warn!",
    "error!",
    "info!",
    "log::",
    "filelog::log",
    "expect(",
    "panic!(",
];

/// Stop-words: high-signal French markers inside string literals.
const STOP_WORDS: &[&str] = &[
    "introuvable",
    "indisponible",
    "échec",
    "impossible",
    "réussi",
    "requise",
    "requis",
    "invalide",
    "obligatoire",
];

/// Accent markers (near zero false positives).
const ACCENTS: &str = "àâäçéèêëîïôöùûüÿœ«»";

/// Allowlist: (path substring, literal substring) with justification.
/// Each entry documents why the French text is data, not UI chrome.
const ALLOWLIST: &[(&str, &str)] = &[
    // Starter-code data (user-editable generated code, not UI chrome).
    ("pages/functions.rs", "@input temperature"),
    // Language endonyms are shown in their own language (standard i18n
    // practice) — the locale picker labels.
    ("pages/login.rs", "Français"),
    ("pages/profile.rs", "Français"),
];

fn scan() -> Result<(usize, usize), Vec<String>> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut findings: Vec<String> = Vec::new();
    let mut files = 0;
    let mut literals = 0;
    for path in rust_files(&src) {
        files += 1;
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if EXCLUDED_FILES.contains(&name) {
            continue;
        }
        // A whole `tests.rs` file is a `mod tests;` submodule by convention
        // (test-only content, `#[cfg(test)]` lives on the declaration), so
        // scan it like an in-file `mod tests` block: skip it entirely.
        if name == "tests.rs" {
            continue;
        }
        // Skip whole `#[cfg(test)] mod tests { … }` blocks (brace-depth
        // state machine — attribute line + `mod` line both re-arm).
        let mut depth: usize = 0;
        let mut test_resume_at: Option<usize> = None;
        // Open parentheses of a log call still spanning the next lines
        // (rustfmt wraps long `log(&format!(…))` calls): its arguments stay
        // excluded until the call closes.
        let mut log_parens: usize = 0;
        for (line_no, raw_line) in text.lines().enumerate() {
            let line = strip_comments(raw_line);
            // Blank literal interiors so braces inside strings (raw JSON
            // fixtures, fluent placeholders) do not affect the depth
            // accounting.
            let spans = string_literals(&line);
            let mut blanked: Vec<u8> = line.as_bytes().to_vec();
            for (start, end) in &spans {
                for byte in blanked.iter_mut().skip(*start).take(end - start) {
                    *byte = b' ';
                }
            }
            let blank_line = String::from_utf8_lossy(&blanked);
            let opens = blank_line.matches('{').count();
            let closes = blank_line.matches('}').count();
            let paren_opens = blank_line.matches('(').count();
            let paren_closes = blank_line.matches(')').count();
            let trimmed = blank_line.trim_start();
            // `#[cfg(test)] mod tests {` spans two lines: only the `mod`
            // line carries the opening brace, so it is the arming point
            // (resume when the mod block closes).
            if test_resume_at.is_none() && trimmed.starts_with("mod tests") {
                test_resume_at = Some(depth + opens);
            }
            if let Some(resume) = test_resume_at {
                depth += opens;
                depth = depth.saturating_sub(closes);
                if depth < resume {
                    test_resume_at = None;
                }
                continue;
            }
            if log_parens > 0 || LOG_PREFIXES.iter().any(|p| line.contains(p)) {
                log_parens = (log_parens + paren_opens).saturating_sub(paren_closes);
                depth += opens;
                depth = depth.saturating_sub(closes);
                continue;
            }
            for (start, end) in spans.clone() {
                literals += 1;
                let lit = &line[start..end];
                let path_str = path.to_string_lossy();
                let allowed = ALLOWLIST
                    .iter()
                    .any(|(f, l)| path_str.contains(f) && lit.contains(l));
                if is_french(lit) && !allowed {
                    findings.push(format!("{}:{} → \"{}\"", path.display(), line_no + 1, lit));
                }
            }
            depth += opens;
            depth = depth.saturating_sub(closes);
        }
    }
    if findings.is_empty() {
        Ok((files, literals))
    } else {
        Err(findings)
    }
}

#[test]
fn no_hardcoded_french_ui_strings() {
    match scan() {
        Ok((files, literals)) => {
            assert!(files > 60, "sterile sweep: only {files} files scanned");
            assert!(literals > 1000, "sterile sweep: only {literals} literals");
        }
        Err(findings) => panic!(
            "Hardcoded French UI strings (move to fluent: add the key to BOTH locales/*.ftl \
             and render via t!/error_i18n, or allowlist with a justification):\n{}",
            findings.join("\n")
        ),
    }
}

/// High-signal French detection: accented characters or stop-words.
fn is_french(lit: &str) -> bool {
    if lit.chars().any(|c| ACCENTS.contains(c)) {
        return true;
    }
    let lower = lit.to_lowercase();
    STOP_WORDS.iter().any(|w| lower.contains(w))
}

/// Byte spans of double-quoted string literals in one line (no escape
/// handling beyond \" — enough for the source style of this crate).
fn string_literals(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 1;
            }
            b'"' => match start {
                Some(s) => {
                    out.push((s, i));
                    start = None;
                }
                None => start = Some(i + 1),
            },
            _ => {}
        }
        i += 1;
    }
    out
}

fn strip_comments(line: &str) -> &str {
    line.find("//").map_or(line, |idx| &line[..idx])
}

/// Std-only recursive walker.
fn rust_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    out
}
