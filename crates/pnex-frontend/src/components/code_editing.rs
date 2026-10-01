//! Pure (DOM-free) caret/edit operations for the function code editor:
//! Tab/Shift+Tab indent-dedent, smart Enter and whole-buffer reindent
//! ("Formater"). Consumed by `CodeEditor` (code_highlight.rs); unit-tested
//! here — the DOM layer stays thin.
//!
//! All offsets in this module are **byte offsets into `code`**. The DOM
//! layer converts from/to the textarea's UTF-16 selection offsets once per
//! operation (see `utf16_to_byte` / `byte_to_utf16`).

use pnex_core::FunctionLanguage;

/// Language of an editor buffer: the function languages plus C++ for the
/// custom firmware IDE (custom-firmware.md D93).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeLang {
    Js,
    Starlark,
    Cpp,
}

impl From<FunctionLanguage> for CodeLang {
    fn from(lang: FunctionLanguage) -> Self {
        match lang {
            FunctionLanguage::Js => CodeLang::Js,
            FunctionLanguage::Starlark => CodeLang::Starlark,
        }
    }
}

/// Indentation unit per language — mirrors the skeletons (JS 2, Starlark 4,
/// C++ 4 like the PneX sketches).
pub fn indent_unit(lang: impl Into<CodeLang>) -> &'static str {
    match lang.into() {
        CodeLang::Js => "  ",
        CodeLang::Starlark | CodeLang::Cpp => "    ",
    }
}

/// Result of a caret edit: new buffer + new selection (byte offsets).
#[derive(Debug, PartialEq, Eq)]
pub struct CaretEdit {
    pub value: String,
    pub caret: u32,
    /// Selection end when the edit should leave a selection (indent/dedent
    /// of a multi-line selection keeps it selected). None = caret only.
    pub sel_end: Option<u32>,
}

/// JS selection offsets are UTF-16 code units; Rust needs byte offsets.
pub fn utf16_to_byte(code: &str, pos16: u32) -> u32 {
    let mut units = 0u32;
    for (offset, ch) in code.char_indices() {
        if units + (ch.len_utf16() as u32) > pos16 {
            return offset as u32;
        }
        units += ch.len_utf16() as u32;
    }
    code.len() as u32
}

/// Inverse of [`utf16_to_byte`].
pub fn byte_to_utf16(code: &str, pos: u32) -> u32 {
    let pos = (pos as usize).min(code.len());
    let mut units = 0u32;
    for (offset, ch) in code.char_indices() {
        if offset >= pos {
            break;
        }
        units += ch.len_utf16() as u32;
    }
    units
}

// ─────────────────────── Shared line scanner ───────────────────────

/// Quote the line starts inside (`' '` = none); tracks multi-line strings
/// across lines. Triple-quoted strings are best-effort (single-quote rule).
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ScanState {
    quote: char,
}

/// One-line scan result (code part only — strings blanked, comments cut).
pub(crate) struct Scanned {
    /// Net bracket delta of the code part.
    net: i32,
    /// Last significant code character (for the `:` trigger).
    last: Option<char>,
    /// State at end of line.
    end: ScanState,
}

/// String/comment-aware scan of one line.
pub(crate) fn scan_line(line: &str, lang: impl Into<CodeLang>, state: ScanState) -> Scanned {
    let lang = lang.into();
    let mut st = state;
    let mut net = 0i32;
    let mut last = None::<char>;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if st.quote != ' ' {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == st.quote {
                st.quote = ' ';
            }
            i += 1;
            continue;
        }
        match c {
            '"' | '\'' | '`' => {
                st.quote = c;
                i += 1;
            }
            '/' if lang != CodeLang::Starlark && chars.get(i + 1) == Some(&'/') => break,
            '#' if lang == CodeLang::Starlark => break,
            '(' | '[' | '{' => {
                net += 1;
                last = Some(c);
                i += 1;
            }
            ')' | ']' | '}' => {
                net -= 1;
                last = Some(c);
                i += 1;
            }
            c if !c.is_whitespace() => {
                last = Some(c);
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }
    Scanned { net, last, end: st }
}
// ───────────────────────── Tab / Shift+Tab ─────────────────────────

/// Tab (indent) / Shift+Tab (dedent). Single caret → insert/remove one
/// indent unit at the caret. Selection → indent/dedent every touched line
/// and keep the selection. Always returns an edit (Tab must never move
/// focus out of the editor); a dedent with nothing to remove is a no-op.
pub fn tab_indent(
    code: &str,
    sel_start: u32,
    sel_end: u32,
    lang: impl Into<CodeLang>,
    outdent: bool,
) -> CaretEdit {
    let lang = lang.into();
    let (start, end) = if sel_start <= sel_end {
        (sel_start as usize, sel_end as usize)
    } else {
        (sel_end as usize, sel_start as usize)
    };
    let start = start.min(code.len());
    let end = end.max(start).min(code.len());
    let unit = indent_unit(lang);

    // Single caret (no selection): insert / remove one unit at the caret.
    if start == end {
        let line_start = code[..start].rfind('\n').map(|p| p + 1).unwrap_or(0);
        if outdent {
            let ws: usize = code[line_start..start.max(line_start)]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .map(|c| c.len_utf8())
                .sum();
            let remove = ws.min(unit.len());
            if remove == 0 {
                // Nothing to remove — no-op (Tab must never move focus).
                return CaretEdit {
                    value: code.to_string(),
                    caret: sel_start,
                    sel_end: None,
                };
            }
            let mut value = String::with_capacity(code.len());
            value.push_str(&code[..line_start]);
            value.push_str(&code[line_start + remove..]);
            return CaretEdit {
                value,
                caret: (start - remove).max(line_start) as u32,
                sel_end: None,
            };
        }
        let mut value = String::with_capacity(code.len() + unit.len());
        value.push_str(&code[..start]);
        value.push_str(unit);
        value.push_str(&code[start..]);
        return CaretEdit {
            value,
            caret: (start + unit.len()) as u32,
            sel_end: None,
        };
    }

    // Selection touching one or more lines: indent/dedent each touched line.
    let first_line = code[..start].matches('\n').count();
    let mut last_line = code[..end].matches('\n').count();
    // A selection ending exactly at a column-0 line start leaves that line
    // untouched (same rule as a trailing newline at the end of the buffer).
    if end > start && line_start_of(code, last_line) == end {
        last_line = last_line.saturating_sub(1).max(first_line);
    }

    let mut out = String::with_capacity(code.len() + unit.len());
    let mut pos = 0usize;
    let mut caret = start;
    let mut sel_end_pos = end;
    for (idx, line) in code.split('\n').enumerate() {
        let orig_start = pos;
        if idx > 0 {
            out.push('\n');
            pos += 1;
        }
        if idx < first_line || idx > last_line {
            out.push_str(line);
            pos += line.len();
            continue;
        }
        if outdent {
            let remove: usize = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .map(|c| c.len_utf8())
                .sum::<usize>()
                .min(unit.len());
            out.push_str(&line[remove..]);
            pos += line.len() - remove;
            // Selection corners after the removed whitespace shift back.
            if caret > orig_start {
                caret = (caret - remove).max(orig_start);
            }
            if sel_end_pos > orig_start {
                sel_end_pos = (sel_end_pos - remove).max(orig_start);
            }
        } else {
            if line.trim().is_empty() {
                out.push_str(line);
                pos += line.len();
            } else {
                out.push_str(unit);
                out.push_str(line);
                pos += unit.len() + line.len();
                if caret > orig_start || (caret == orig_start && line.is_empty()) {
                    caret += unit.len();
                }
                if sel_end_pos > orig_start {
                    sel_end_pos += unit.len();
                }
                let _ = orig_start;
            }
        }
    }
    CaretEdit {
        value: out,
        caret: caret as u32,
        sel_end: Some(sel_end_pos as u32),
    }
}

/// Byte offset of the start of `line_index` (0-based).
fn line_start_of(code: &str, line_index: usize) -> usize {
    let mut pos = 0usize;
    for _ in 0..line_index {
        match code[pos..].find('\n') {
            Some(nl) => pos += nl + 1,
            None => break,
        }
    }
    pos
}

// ───────────────────────── Smart Enter ─────────────────────────

/// Enter key: keep the current line's indentation, add one unit when the
/// code before the caret ends with a block opener (`:` for Starlark,
/// `{`/`(`/`[` for both languages — string/comment aware). When the same
/// line continues after the caret with a matching closer (or a Starlark
/// dedent keyword), the classic split applies: the closer moves to its own
/// line at the parent indent. `None` = nothing special (plain newline).
pub fn enter_smart(code: &str, caret: u32, lang: impl Into<CodeLang>) -> CaretEdit {
    let lang = lang.into();
    let caret = (caret as usize).min(code.len());
    let line_start = code[..caret].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end = code[caret..]
        .find('\n')
        .map(|p| caret + p)
        .unwrap_or(code.len());

    // Indent of the line the caret sits on.
    let base_len = code[line_start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| c.len_utf8())
        .sum::<usize>();
    let base = &code[line_start..line_start + base_len];

    // Code before the caret (minus trailing spaces) via the scanner state.
    let before = &code[line_start..caret];
    let scanned = scan_line(before, lang, ScanState { quote: ' ' });
    let opener = match scanned.last {
        Some(':') if lang == CodeLang::Starlark => true,
        Some('{') | Some('(') | Some('[') => true,
        _ => false,
    };

    let deeper = if opener {
        format!("{base}{}", indent_unit(lang))
    } else {
        base.to_string()
    };

    // Remainder after the caret on the same line.
    let after = &code[caret..line_end];
    let after_trim = after.trim_start();
    let closes = after_trim.starts_with(')')
        || after_trim.starts_with(']')
        || after_trim.starts_with('}')
        || ((lang == CodeLang::Starlark)
            && (after_trim.starts_with("else ")
                || after_trim.starts_with("else:")
                || after_trim.starts_with("elif ")
                || after_trim.starts_with("elif")));

    // In split mode the prefix line's trailing whitespace is dropped
    // (editors never keep the stray space before the closer).
    let prefix_end = if closes {
        line_start + code[line_start..caret].trim_end().len()
    } else {
        caret
    };
    let mut value = String::with_capacity(code.len() + deeper.len() * 2 + 2);
    value.push_str(&code[..prefix_end]);
    value.push('\n');
    value.push_str(&deeper);
    let new_caret = value.len();
    if closes {
        // Split: remainder moves to its own line (at the deeper indent), the
        // closer/`else` continues on the next line at the base indent.
        value.push_str(after_trim);
        if !code[line_end..].is_empty() {
            value.push('\n');
        }
        value.push_str(base);
        value.push_str(&code[line_end..]);
    } else {
        value.push_str(&code[line_end..]);
    }
    CaretEdit {
        value,
        caret: new_caret as u32,
        sel_end: None,
    }
}

// ───────────────────────── Whole-buffer reindent ─────────────────────────

/// "Formater": reindent the whole buffer. Bracket depth (string/comment
/// aware) for both languages plus the Starlark colon rule; `else`/`elif`
/// and lines starting with a closer dedent first. Blank lines are emptied;
/// content lines keep their bytes (only leading whitespace is rewritten).
pub fn reindent(code: &str, lang: impl Into<CodeLang>) -> String {
    let lang = lang.into();
    let unit_len = indent_unit(lang).len();
    let mut extra: i32 = 0;
    let mut state = ScanState { quote: ' ' };
    let mut out: Vec<String> = Vec::new();
    for line in code.split('\n') {
        // Lines starting inside a multi-line string are left untouched.
        if state.quote != ' ' {
            let scanned = scan_line(line, lang, state);
            state = scanned.end;
            out.push(line.to_string());
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            out.push(String::new());
            continue;
        }
        // Dedent triggers for this line's own indent.
        let starts_closer =
            trimmed.starts_with(')') || trimmed.starts_with(']') || trimmed.starts_with('}');
        let starts_else = lang == CodeLang::Starlark
            && (trimmed.starts_with("else ")
                || trimmed.starts_with("else:")
                || trimmed.starts_with("elif"));
        let mut line_extra = extra;
        if starts_closer || starts_else {
            line_extra -= 1;
        }
        let indent_units = line_extra.max(0) as usize;
        let mut rendered = String::with_capacity(line.len() + indent_units * unit_len);
        for _ in 0..indent_units {
            rendered.push_str(indent_unit(lang));
        }
        rendered.push_str(trimmed);
        out.push(rendered);

        // Update the depth for the following lines: build on `line_extra`
        // (the dedent applied for this line) so `else:` keeps the block it
        // belongs to at the right depth.
        let scanned = scan_line(line, lang, state);
        // `else`'s dedent is not in `net` (zero-sum line) → build on the
        // dedented level; closers ARE in `net` → build on the raw level.
        extra = if starts_else { line_extra } else { extra } + scanned.net;
        if lang == CodeLang::Starlark && scanned.last == Some(':') {
            extra += 1;
        }
        state = scanned.end;
    }
    out.join("\n")
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_roundtrip() {
        let code = "let s = \"Température °C\";\nlet x = 1;";
        for pos in [0u32, 8, 12, 26, 30] {
            let b = byte_to_utf16(code, pos);
            assert_eq!(utf16_to_byte(code, b), pos.min(code.len() as u32));
        }
    }

    #[test]
    fn tab_single_caret_inserts_at_caret() {
        // Tab at end of a body line inserts one unit at the caret.
        let st = "if a:\n    return 1\n";
        let e = tab_indent(st, 18, 18, FunctionLanguage::Starlark, false);
        assert_eq!(e.value, "if a:\n    return 1    \n");
        assert_eq!(e.caret, 22);
    }

    #[test]
    fn shift_tab_dedent_and_noop() {
        let e = tab_indent("    return 1", 8, 8, FunctionLanguage::Starlark, true);
        assert_eq!(e.value, "return 1");
        assert_eq!(e.caret, 4);
        // Nothing to remove: no-op edit (Tab must never move focus).
        let e = tab_indent("return 1", 3, 3, FunctionLanguage::Starlark, true);
        assert_eq!(e.value, "return 1");
        assert_eq!(e.caret, 3);
    }

    #[test]
    fn tab_multi_line_selection() {
        let st = "if a:\nreturn 1\nreturn 2\n";
        let e = tab_indent(st, 6, 23, FunctionLanguage::Starlark, false);
        assert_eq!(e.value, "if a:\n    return 1\n    return 2\n");
        assert_eq!(e.sel_end, Some(31));
        let e = tab_indent(
            &e.value,
            e.caret,
            e.sel_end.unwrap(),
            FunctionLanguage::Starlark,
            true,
        );
        assert_eq!(e.value, st);
    }

    #[test]
    fn enter_after_colon_and_brace() {
        let st = "if a:";
        let e = enter_smart(st, 5, FunctionLanguage::Starlark);
        assert_eq!(e.value, "if a:\n    ");
        assert_eq!(e.caret, 10);
    }

    #[test]
    fn enter_after_opening_brace_js() {
        let js = "function f() {";
        let e = enter_smart(js, 14, FunctionLanguage::Js);
        assert_eq!(e.value, "function f() {\n  ");
        assert_eq!(e.caret, 17);
    }

    #[test]
    fn enter_keeps_indent_without_trigger() {
        let st = "    x = 1";
        let e = enter_smart(st, 9, FunctionLanguage::Starlark);
        assert_eq!(e.value, "    x = 1\n    ");
    }

    #[test]
    fn enter_colon_inside_string_is_not_a_trigger() {
        let st = "    s = \"abc:\"";
        let e = enter_smart(st, st.len() as u32, FunctionLanguage::Starlark);
        assert_eq!(e.value, "    s = \"abc:\"\n    ");
    }

    #[test]
    fn enter_split_before_closing_brace() {
        // Caret right after `{ ` with a matching closer ahead: the classic
        // split — deeper body line, closer on its own line at base indent.
        let js = "function f() { }";
        let caret = 15u32; // after the space, before `}`
        let e = enter_smart(js, caret, FunctionLanguage::Js);
        assert_eq!(e.value, "function f() {\n  }");
        assert_eq!(e.caret, 17);
    }

    #[test]
    fn reindent_starlark_fixture() {
        let messy = "# @input t number\ndef handle(inputs, msg):\nx = inputs[\"t\"]\nif x:\nx = x + 1\nelse:\nx = 0\nreturn {\"v\": x}\n";
        let formatted = reindent(messy, FunctionLanguage::Starlark);
        let expected = "# @input t number\ndef handle(inputs, msg):\n    x = inputs[\"t\"]\n    if x:\n        x = x + 1\n    else:\n        x = 0\n        return {\"v\": x}\n";
        assert_eq!(formatted, expected);
        // Idempotent.
        assert_eq!(reindent(&formatted, FunctionLanguage::Starlark), formatted);
    }

    #[test]
    fn cpp_indent_and_comments() {
        assert_eq!(indent_unit(CodeLang::Cpp), "    ");
        // `#include` is not a comment in C++, `//` is.
        let formatted = reindent(
            "#include <Pnex.h>\nvoid setup() {\nif (x) { // {\ny();\n}\n}",
            CodeLang::Cpp,
        );
        assert_eq!(
            formatted,
            "#include <Pnex.h>\nvoid setup() {\n    if (x) { // {\n        y();\n    }\n}"
        );
        let e = enter_smart("void loop() {}", 13, CodeLang::Cpp);
        assert_eq!(e.value, "void loop() {\n    }");
    }

    #[test]
    fn reindent_js_fixture_and_idempotence() {
        let messy = "function handle(inputs, msg) {\nconst a = inputs[\"x\"];\nif (a) {\nreturn a;\n}\nreturn {\nout: a,\n};\n}";
        let formatted = reindent(messy, FunctionLanguage::Js);
        let expected = "function handle(inputs, msg) {\n  const a = inputs[\"x\"];\n  if (a) {\n    return a;\n  }\n  return {\n    out: a,\n  };\n}";
        assert_eq!(formatted, expected);
        assert_eq!(reindent(&formatted, FunctionLanguage::Js), formatted);
        // Strings are byte-preserved.
        assert!(formatted.contains("inputs[\"x\"]"));
    }
}
