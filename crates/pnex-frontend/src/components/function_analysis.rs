//! Client-side static analysis for the function editor — finds names used
//! in the code but not declared as `@input`/`@output` directives. Best-effort
//! single pass over a **masked** copy of the code (string contents and
//! comments blanked to spaces, same offsets as the source), supports both
//! function languages.

use pnex_core::{FunctionLanguage, FunctionSignature};

/// Kind of undeclared use (input read vs output produced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortKind {
    Input,
    Output,
}

/// One undeclared name with its kind and first use line (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredUse {
    pub name: String,
    pub kind: PortKind,
    pub first_line: u32,
}

/// Masked copy of the code: characters inside string literals and comments
/// become spaces (offsets unchanged). Quote delimiters are blanked too.
fn mask_strings_comments(code: &str, lang: FunctionLanguage) -> String {
    let mut out = String::with_capacity(code.len());
    let mut quote: char = ' ';
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        if quote != ' ' {
            match c {
                '\\' => {
                    out.push(' ');
                    chars.next();
                }
                q if q == quote => {
                    out.push(' ');
                    quote = ' ';
                }
                _ => out.push(' '),
            }
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                quote = c;
                out.push(' ');
            }
            '/' if lang == FunctionLanguage::Js && chars.peek() == Some(&'/') => {
                // Line comment: blank to end of line, keep the newline.
                out.push(' ');
                while let Some(c2) = chars.peek() {
                    if *c2 == '\n' {
                        break;
                    }
                    chars.next();
                    out.push(' ');
                }
            }
            '#' if lang == FunctionLanguage::Starlark => {
                out.push(' ');
                while let Some(c2) = chars.peek() {
                    if *c2 == '\n' {
                        break;
                    }
                    chars.next();
                    out.push(' ');
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Word character (identifier) for the scanner.
fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Analyzes the code for undeclared port uses: `inputs["k"]` / `inputs.k` /
/// `inputs.get("k"` → input reads; depth-1 keys of a `return { … }` group →
/// output writes. Names already declared are filtered out; dedup per
/// (kind, name), ordered inputs first then outputs (first appearance).
pub fn analyze_undeclared(
    code: &str,
    lang: FunctionLanguage,
    sig: &FunctionSignature,
) -> Vec<UndeclaredUse> {
    let masked = mask_strings_comments(code, lang);
    let mchars: Vec<char> = masked.chars().collect();
    let chars: Vec<char> = code.chars().collect();
    let mut inputs: Vec<(String, u32)> = Vec::new();
    let mut outputs: Vec<(String, u32)> = Vec::new();

    // ── Pass 1: `inputs` accesses ──
    let mut i = 0usize;
    while i < mchars.len() {
        let word = mchars[i..(i + 6).min(mchars.len())]
            .iter()
            .collect::<String>();
        let is_target = word == "inputs"
            && (i == 0 || !is_ident(mchars[i - 1]))
            && (i + 6 <= mchars.len() && !is_ident(*mchars.get(i + 6).unwrap_or(&' ')));
        if is_target {
            let mut j = i + 6;
            while j < mchars.len() && mchars[j].is_whitespace() {
                j += 1;
            }
            let k = j + 1;
            if mchars.get(j) == Some(&'[') {
                if let Some(name) = quoted_key(&chars, k) {
                    inputs.push((name, line_of(&masked, k)));
                }
            } else if mchars.get(j) == Some(&'.') {
                // `.get("k"` method form vs `.field` form.
                if let Some(name) = method_key(&mchars, &chars, k) {
                    inputs.push((name, line_of(&masked, k)));
                } else {
                    let start = k;
                    let mut k2 = k;
                    while k2 < mchars.len() && is_ident(mchars[k2]) {
                        k2 += 1;
                    }
                    if k2 > start {
                        let name: String = chars[start..k2].iter().collect();
                        inputs.push((name, line_of(&masked, start)));
                    }
                }
            }
        }
        i += 1;
    }

    // ── Pass 2: `return { … }` keys → outputs ──
    let mut i = 0usize;
    while i < mchars.len() {
        if i + 6 <= mchars.len()
            && mchars[i..i + 6].iter().collect::<String>() == "return"
            && (i == 0 || !is_ident(mchars[i - 1]))
            && (i + 6 == mchars.len() || !is_ident(mchars[i + 6]))
        {
            // First `{` after `return` (whitespace/newlines skipped).
            let mut j = i + 6;
            while j < mchars.len() && mchars[j] != '{' && mchars[j] != ';' && mchars[j] != '\n' {
                j += 1;
            }
            if j < mchars.len() && mchars[j] == '{' {
                collect_dict_keys(&mchars, &chars, j, &mut outputs);
            }
        }
        i += 1;
    }
    build_result(sig, inputs, outputs)
}

/// Extracts the string between `chars[k]` (expected quote) and its matching
/// closing quote. None if the quote never closes on the same slice.
fn quoted_key(chars: &[char], k: usize) -> Option<String> {
    // The opening quote may appear blanked in the masked copy — find it
    // within a few characters of k (inputs["k"] / get("k" shapes).
    let mut q = k;
    let mut window = 0usize;
    while q < chars.len() && window < 4 && chars[q] != '"' && chars[q] != '\'' {
        q += 1;
        window += 1;
    }
    if q >= chars.len() || window >= 4 {
        return None;
    }
    let quote = chars[q];
    let mut name = String::new();
    let mut j = q + 1;
    while j < chars.len() {
        let c = chars[j];
        if c == '\\' {
            j += 2;
            continue;
        }
        if c == quote {
            return Some(name);
        }
        name.push(c);
        j += 1;
    }
    None
}

/// `inputs.get("k"` method form: from the identifier at `k`, expects
/// `( quote key quote` shortly after.
fn method_key(mchars: &[char], chars: &[char], k: usize) -> Option<String> {
    let mut j = k;
    while j < mchars.len() && is_ident(mchars[j]) {
        j += 1;
    }
    while j < mchars.len() && mchars[j].is_whitespace() {
        j += 1;
    }
    if mchars.get(j) != Some(&'(') {
        return None;
    }
    // No whitespace skipping past `(`: the opening quote may be masked to a
    // space — quoted_key locates it within a small window.
    quoted_key(chars, j + 1)
}

/// Line number (1-based) of byte position `pos` in the masked text.
fn line_of(text: &str, pos: usize) -> u32 {
    text[..pos.min(text.len())].matches('\n').count() as u32 + 1
}

/// Collects the depth-1 keys of the `{` group starting at `open` (masked
/// position). Keys are read from the original `chars` (strings are masked).
fn collect_dict_keys(mchars: &[char], chars: &[char], open: usize, out: &mut Vec<(String, u32)>) {
    let mut depth = 0i32;
    let mut i = open;
    while i < mchars.len() {
        match mchars[i] {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            ':' if depth == 1 => {
                // Key = token immediately before the colon (original text).
                if let Some((name, pos)) = key_before(mchars, chars, i) {
                    out.push((name, line_of_masked_pos(mchars, pos)));
                }
            }
            _ => {}
        }
        i += 1;
    }
}

/// The key token before a depth-1 colon, read from the ORIGINAL text
/// (quotes are visible there, unlike in the masked copy).
fn key_before(_mchars: &[char], chars: &[char], colon: usize) -> Option<(String, usize)> {
    let mut j = colon;
    while j > 0 && chars[j - 1].is_whitespace() {
        j -= 1;
    }
    if j == 0 {
        return None;
    }
    // Quoted key: chars[j-1] is the closing quote — walk back to the opener.
    if chars[j - 1] == '"' || chars[j - 1] == '\'' {
        let close = j - 1;
        let mut k = close.saturating_sub(1);
        while k > 0 && chars[k] != chars[close] {
            k -= 1;
        }
        if close > k + 1 {
            return Some((chars[k + 1..close].iter().collect(), k));
        }
        return None;
    }
    // Bare identifier key (JS object shorthand `k:`).
    let end = j;
    while j > 0 && is_ident(chars[j - 1]) {
        j -= 1;
    }
    if j == end {
        return None;
    }
    Some((chars[j..end].iter().collect(), j))
}

/// Line (1-based) of a masked-char position.
fn line_of_masked_pos(mchars: &[char], pos: usize) -> u32 {
    1 + mchars[..pos.min(mchars.len())]
        .iter()
        .filter(|c| **c == '\n')
        .count() as u32
}

/// Dedup + declared-name filtering + ordering (inputs first).
fn build_result(
    sig: &FunctionSignature,
    inputs: Vec<(String, u32)>,
    outputs: Vec<(String, u32)>,
) -> Vec<UndeclaredUse> {
    let mut out: Vec<UndeclaredUse> = Vec::new();
    let push = |name: String, kind: PortKind, line: u32, out: &mut Vec<UndeclaredUse>| {
        if name.is_empty() {
            return;
        }
        // Directive names are lowercase snake_case; avoid case-sensitive
        // misses while not hiding genuinely different names.
        if sig.inputs.iter().any(|i| i.name == name) || sig.outputs.iter().any(|o| o.name == name) {
            return;
        }
        if out.iter().any(|u| u.name == name && u.kind == kind) {
            return;
        }
        out.push(UndeclaredUse {
            name,
            kind,
            first_line: line,
        });
    };
    for (name, line) in inputs {
        push(name, PortKind::Input, line, &mut out);
    }
    for (name, line) in outputs {
        push(name, PortKind::Output, line, &mut out);
    }
    out
}

/// Convenience: parse_directives + analyze in one call (None = directive
/// parse failed — the caller shows the directive error instead).
pub fn analyze(code: &str, lang: FunctionLanguage) -> Option<Vec<UndeclaredUse>> {
    let sig = pnex_core::parse_directives(code).ok()?;
    Some(analyze_undeclared(code, lang, &sig))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::FunctionLanguage as Lang;

    #[test]
    fn masking_preserves_offsets_and_blanks_strings() {
        let code = "x = \"abc\" # note\ny = 1";
        let m = mask_strings_comments(code, Lang::Starlark);
        assert_eq!(m.len(), code.len());
        assert_eq!(&m[4..7], "   ");
        assert!(!m.contains("abc"));
        assert!(!m.contains("note"));
        assert!(m.contains('\n'));
    }

    #[test]
    fn detects_inputs_reads_both_forms() {
        let code = "def handle(inputs, msg):\n    a = inputs[\"temperature\"]\n    b = inputs.threshold\n    c = inputs.get(\"offset\", 0)\n";
        let uses = analyze(code, Lang::Starlark).unwrap();
        let names: Vec<&str> = uses
            .iter()
            .filter(|u| u.kind == PortKind::Input)
            .map(|u| u.name.as_str())
            .collect();
        assert!(names.contains(&"temperature"));
        assert!(names.contains(&"threshold"));
        assert!(names.contains(&"offset"));
    }

    #[test]
    fn detects_return_dict_keys_as_outputs() {
        let code = "def handle(inputs, msg):\n    return {\"heating\": 1, \"delta\": 2}\n";
        let uses = analyze(code, Lang::Starlark).unwrap();
        let outs: Vec<&str> = uses
            .iter()
            .filter(|u| u.kind == PortKind::Output)
            .map(|u| u.name.as_str())
            .collect();
        assert_eq!(outs, vec!["heating", "delta"]);
    }

    #[test]
    fn js_bare_keys_and_multiline_return() {
        let code = "function handle(inputs, msg) {\n  if (inputs.temp > 0) {\n    return {\n      heat: 1,\n      mode: inputs[\"mode\"],\n    };\n  }\n  return { idle: true };\n}";
        let sig = pnex_core::parse_directives(code).unwrap();
        let uses = analyze_undeclared(code, Lang::Js, &sig);
        let outs: Vec<&str> = uses
            .iter()
            .filter(|u| u.kind == PortKind::Output)
            .map(|u| u.name.as_str())
            .collect();
        assert!(outs.contains(&"heat"));
        assert!(outs.contains(&"idle"));
        assert!(uses
            .iter()
            .any(|u| u.name == "mode" && u.kind == PortKind::Input));
        assert!(uses
            .iter()
            .any(|u| u.name == "temp" && u.kind == PortKind::Input));
    }

    #[test]
    fn declared_names_are_filtered() {
        let code = "// @input temperature number\n// @output heating number\ndef handle(inputs, msg):\n    return {\"heating\": inputs[\"temperature\"]}\n";
        assert!(analyze(code, Lang::Starlark).unwrap().is_empty());
    }

    #[test]
    fn strings_and_comments_are_not_scanned() {
        let code = "def handle(inputs, msg):\n    s = \"inputs[\\\"ghost\\\"]\"\n    # inputs[\"in_comment\"]\n    return inputs[\"real\"]\n";
        let uses = analyze(code, Lang::Starlark).unwrap();
        let names: Vec<String> = uses.iter().map(|u| u.name.clone()).collect();
        assert_eq!(names, vec!["real".to_string()]);
    }
}
