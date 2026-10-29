//! Client-side static analysis for the function editor — finds names used
//! in the code but not declared in the function's signature (D497: the
//! ports live outside the code; this only warns). Best-effort
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
                    // Blank the escaped char too: offsets must stay aligned.
                    if chars.next().is_some() {
                        out.push(' ');
                    }
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
/// output writes, in named-ports mode only: in message mode a returned dict
/// is the message, never ports. Names already declared are filtered out; dedup per
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
            // `return {`: only a dict literal returned as is (a list or a call
            // holding a dict is a positional or computed return).
            let mut j = i + 6;
            while j < mchars.len() && mchars[j].is_whitespace() && mchars[j] != '\n' {
                j += 1;
            }
            // A helper's return builds a value, not the node's output.
            let in_helper = enclosing_fn(&mchars, i, lang).is_some_and(|f| f != "handle");
            if !in_helper && j < mchars.len() && mchars[j] == '{' {
                // Same rule as the runtime (`map_return`): routing by key
                // exists only once outputs are declared.
                if !sig.is_message() {
                    collect_dict_keys(&mchars, &chars, j, &mut outputs);
                }
            }
        }
        i += 1;
    }
    build_result(sig, inputs, outputs)
}

/// Name of the innermost named function enclosing masked position `pos`
/// (Starlark `def` by indentation, JS `function name(…) { … }` by braces).
/// None at top level. Anonymous JS closures are not names: they count as
/// their enclosing function.
fn enclosing_fn(mchars: &[char], pos: usize, lang: FunctionLanguage) -> Option<String> {
    // ASCII-only copy: byte offsets equal char offsets.
    let text: String = mchars
        .iter()
        .map(|c| if c.is_ascii() { *c } else { ' ' })
        .collect();
    let ident_after = |s: &str| -> String { s.chars().take_while(|c| is_ident(*c)).collect() };
    match lang {
        FunctionLanguage::Starlark => {
            let line_start = text[..pos].rfind('\n').map_or(0, |n| n + 1);
            let indent = |l: &str| l.len() - l.trim_start().len();
            let mut cur = indent(&text[line_start..]);
            for line in text[..line_start].lines().rev() {
                if line.trim().is_empty() || indent(line) >= cur {
                    continue;
                }
                if let Some(rest) = line.trim_start().strip_prefix("def ") {
                    return Some(ident_after(rest.trim_start()));
                }
                cur = indent(line);
            }
            None
        }
        FunctionLanguage::Js => {
            let mut best: Option<(usize, String)> = None;
            for (start, _) in text.match_indices("function") {
                let after = &text[start + 8..];
                if (start > 0 && is_ident(mchars[start - 1]))
                    || !after.starts_with(char::is_whitespace)
                {
                    continue;
                }
                let name = ident_after(after.trim_start());
                let Some(open) = text[start..].find('{').map(|o| start + o) else {
                    continue;
                };
                if name.is_empty() || open > pos {
                    continue;
                }
                let mut depth = 0i32;
                let close = (open..mchars.len()).find(|&k| {
                    match mchars[k] {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                });
                if close.is_none_or(|c| c > pos) && best.as_ref().is_none_or(|(o, _)| open > *o) {
                    best = Some((open, name));
                }
            }
            best.map(|(_, n)| n)
        }
    }
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
                if let Some((name, pos)) = key_before(chars, i) {
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
fn key_before(chars: &[char], colon: usize) -> Option<(String, usize)> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::FunctionLanguage as Lang;

    /// Named-ports mode with these outputs.
    fn ports(names: &[&str]) -> FunctionSignature {
        FunctionSignature {
            inputs: vec![],
            outputs: names
                .iter()
                .map(|n| pnex_core::FunctionOutput {
                    name: n.to_string(),
                    ty: pnex_core::FunctionType::Any,
                    desc: None,
                })
                .collect(),
        }
    }

    fn message() -> FunctionSignature {
        FunctionSignature::default()
    }

    fn names(uses: &[UndeclaredUse], kind: PortKind) -> Vec<&str> {
        uses.iter()
            .filter(|u| u.kind == kind)
            .map(|u| u.name.as_str())
            .collect()
    }

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
        let uses = analyze_undeclared(code, Lang::Starlark, &message());
        let ins = names(&uses, PortKind::Input);
        assert!(ins.contains(&"temperature"));
        assert!(ins.contains(&"threshold"));
        assert!(ins.contains(&"offset"));
    }

    #[test]
    fn return_dict_keys_are_outputs_in_ports_mode_only() {
        let code = "def handle(inputs, msg):\n    return {\"heating\": 1, \"delta\": 2}\n";
        let uses = analyze_undeclared(code, Lang::Starlark, &ports(&["heating"]));
        assert_eq!(names(&uses, PortKind::Output), vec!["delta"]);
        // Message mode (D497): the returned dict is the message.
        assert!(analyze_undeclared(code, Lang::Starlark, &message()).is_empty());
    }

    #[test]
    fn returned_message_dict_is_not_outputs() {
        // qw_finding shape: the dict is the message.
        let code = "def handle(inputs, msg):\n    e = msg.get(\"payload\", {}) or {}\n    if not e.get(\"props\"): return None\n    return {\"payload\": e[\"props\"], \"title\": e.get(\"title\", \"\"), \"link_targets\": e.get(\"link_targets\", {}),\n            \"object\": None, \"object_id\": None, \"links\": None}\n";
        assert!(analyze_undeclared(code, Lang::Starlark, &message()).is_empty());
        let js = "function handle(inputs, msg) {\n  return { payload: 1, topic: \"t\" };\n}";
        assert!(analyze_undeclared(js, Lang::Js, &message()).is_empty());
        // With declared outputs the runtime routes by key: still flagged.
        let routed = "def handle(inputs, msg):\n    return {\"a\": 1, \"payload\": 2}\n";
        let uses = analyze_undeclared(routed, Lang::Starlark, &ports(&["a"]));
        assert_eq!(names(&uses, PortKind::Output), vec!["payload"]);
    }

    #[test]
    fn helper_returns_are_not_outputs() {
        let star = "def finding(t):\n    if t:\n        return {\"props\": t, \"title\": t}\n    return None\n\ndef handle(inputs, msg):\n    return {\"out\": finding(1)}\n";
        let uses = analyze_undeclared(star, Lang::Starlark, &ports(&["x"]));
        assert_eq!(names(&uses, PortKind::Output), vec!["out"]);
        let js = "function helper(x) {\n  return { props: x };\n}\nfunction handle(inputs, msg) {\n  const f = (y) => { return { inner: y }; };\n  return { out: helper(1) };\n}";
        let uses = analyze_undeclared(js, Lang::Js, &ports(&["x"]));
        assert_eq!(names(&uses, PortKind::Output), vec!["inner", "out"]);
    }

    /// Pack functions, with the outputs their pack declares, show no phantom
    /// "undeclared" output.
    #[test]
    fn pack_functions_have_no_phantom_outputs() {
        let packs = [
            include_str!("../../../pnex-backend/packs/quali_watch.yaml"),
            include_str!("../../../pnex-backend/packs/web_presence.yaml"),
            include_str!("../../../pnex-backend/packs/crm.yaml"),
            include_str!("../../../pnex-backend/packs/prospecting_b2b.yaml"),
            include_str!("../../../pnex-backend/packs/prospecting_b2b_fr.yaml"),
        ];
        let mut checked = 0;
        for pack in packs {
            // Every pack function is Starlark: `outputs:` rows
            // (`- {name: x, …}`) then its `code: |` block indented 6.
            for function in pack.split("\n  - name: ").skip(1) {
                let Some((head, block)) = function.split_once("    code: |\n") else {
                    continue;
                };
                let declared: Vec<&str> = head
                    .lines()
                    .filter_map(|l| l.trim().strip_prefix("- {name: "))
                    .filter_map(|l| l.split(',').next())
                    .collect();
                let code = block
                    .lines()
                    .take_while(|l| l.is_empty() || l.starts_with("      "))
                    .map(|l| l.get(6..).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join("\n");
                let uses = analyze_undeclared(&code, Lang::Starlark, &ports(&declared));
                let outs = names(&uses, PortKind::Output);
                assert!(outs.is_empty(), "{outs:?} in:\n{code}");
                checked += 1;
            }
        }
        assert!(checked > 20);
    }

    #[test]
    fn js_bare_keys_and_multiline_return() {
        let code = "function handle(inputs, msg) {\n  if (inputs.temp > 0) {\n    return {\n      heat: 1,\n      mode: inputs[\"mode\"],\n    };\n  }\n  return { idle: true };\n}";
        let uses = analyze_undeclared(code, Lang::Js, &ports(&["other"]));
        let outs = names(&uses, PortKind::Output);
        assert!(outs.contains(&"heat"));
        assert!(outs.contains(&"idle"));
        let ins = names(&uses, PortKind::Input);
        assert!(ins.contains(&"mode") && ins.contains(&"temp"));
    }

    #[test]
    fn declared_names_are_filtered() {
        let code = "def handle(inputs, msg):\n    return {\"heating\": inputs[\"temperature\"]}\n";
        let mut sig = ports(&["heating"]);
        sig.inputs.push(pnex_core::FunctionInput {
            name: "temperature".into(),
            ty: pnex_core::FunctionType::Number,
            default: None,
            desc: None,
        });
        assert!(analyze_undeclared(code, Lang::Starlark, &sig).is_empty());
    }

    #[test]
    fn strings_and_comments_are_not_scanned() {
        let code = "def handle(inputs, msg):\n    s = \"inputs[\\\"ghost\\\"]\"\n    # inputs[\"in_comment\"]\n    return inputs[\"real\"]\n";
        let uses = analyze_undeclared(code, Lang::Starlark, &message());
        assert_eq!(names(&uses, PortKind::Input), vec!["real"]);
    }
}
