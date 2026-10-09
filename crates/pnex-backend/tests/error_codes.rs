//! Cross-check: every literal code passed to `ErrorDetail::new(` in the
//! backend controllers must be registered in `pnex_core::err_codes::ALL`
//! (or belong to the GENERIC allowlist below — legacy sites still awaiting
//! their specific code). This guarantees the frontend can resolve
//! `err-<kebab>` for any server error the UI may display, with verbatim
//! fallback otherwise (i18n chantier — see `crates/pnex-core/src/err_codes.rs`).

use std::path::PathBuf;

/// Existing dynamic-detail codes (message embeds the actual runtime/engine
/// error) — verbatim display is the contract until they are re-coined with
/// structured args. Do not grow this list with new sites: coin a specific
/// code instead.
const DYNAMIC_ALLOWED: &[&str] = &[
    "flow_runtime", // flows.rs, functions.rs — embeds the engine error
    "upstream",     // oauth2.rs — embeds the IdP error
    // "last_version" — pruned: media.rs 409 now coins media-last-version.
    // "render" — pruned: notify templates now coin notify-template-render.
    "coolprop_error", // thermo.rs — embeds the CoolProp error
];

#[test]
fn error_detail_codes_are_registered() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/controllers");
    let mut violations: Vec<String> = Vec::new();
    let mut files = 0;
    for path in rust_files(&src) {
        files += 1;
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for (line_no, line) in strip_comments(&text).lines().enumerate() {
            let Some(code) = literal_error_detail_code(line) else {
                continue;
            };
            if DYNAMIC_ALLOWED.contains(&code) {
                continue;
            }
            if !pnex_core::err_codes::exists(code) {
                violations.push(format!("{}:{} → \"{}\"", path.display(), line_no + 1, code));
            }
        }
    }
    if !violations.is_empty() {
        panic!(
            "Unregistered error codes (add the const to pnex_core::err_codes::ALL and the \
             err-<kebab> key to BOTH locales/*.ftl):\n{}",
            violations.join("\n")
        );
    }
    assert!(
        files > 15,
        "sterile sweep: only {files} controller files scanned"
    );
}

fn literal_error_detail_code(line: &str) -> Option<&str> {
    let rest = line.trim_start();
    let rest = rest
        .strip_prefix("loco_rs::controller::ErrorDetail::new(")
        .or_else(|| rest.strip_prefix("ErrorDetail::new("))?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Std-only recursive walker (no walkdir dependency).
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

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        if let Some(idx) = line.find("//") {
            out.push_str(&line[..idx]);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}
