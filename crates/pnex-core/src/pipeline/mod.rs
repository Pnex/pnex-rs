//! The pipeline engine (pipeline-engine.md D934–D953): one document for every
//! source and every transform, closed steps over named working tables. Pure,
//! wasm-safe: the backend parses the YAML and runs the steps.

pub mod doc;
pub mod periods;
pub mod template;
pub mod track;
#[cfg(feature = "tz")]
pub mod tz;

pub use doc::{Document, Step, Violation};

/// Glob over a file name: `*` (any run), `?` (one character), the rest
/// literal, case-insensitive. A pattern without `/` matches the base name.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let name = if pattern.contains('/') {
        name
    } else {
        name.rsplit('/').next().unwrap_or(name)
    };
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    // Iterative wildcard match with one backtrack point.
    let (mut i, mut j, mut star, mut mark) = (0, 0, None, 0);
    while j < n.len() {
        if i < p.len() && (p[i] == '?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == '*')
}
