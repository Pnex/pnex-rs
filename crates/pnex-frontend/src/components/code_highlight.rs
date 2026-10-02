//! Coloration syntaxique minimaliste (zéro dep) pour l'éditeur de code des
//! fonctions : tokenizer main-roulé JS/Starlark + éditeur « overlay » — un
//! `<pre>` coloré sous un textarea au texte transparent (astuce classique),
//! scroll synchronisé par transform. Texte rendu en nœuds texte (jamais
//! `dangerously_set_inner_html` — le code est du contenu utilisateur).

use crate::components::code_editing::{self, CodeLang};
use dioxus::prelude::*;

/// DOM handle of the editor's textarea — `web_sys` type on wasm, unit on
/// native (unit tests run natively; the handle is only used in-browser).
#[cfg(target_arch = "wasm32")]
pub type AreaHandle = web_sys::HtmlTextAreaElement;
#[cfg(not(target_arch = "wasm32"))]
pub type AreaHandle = ();

/// Classe de token — littéral chaîne `&'static str` (classe tailwind).
pub type TokClass = &'static str;

/// Token coloré (texte + classe tailwind ; classe vide = texte par défaut).
#[derive(Clone)]
pub struct Tok {
    pub class: TokClass,
    pub text: String,
}

const KW_JS: &[&str] = &[
    "function", "return", "const", "let", "var", "if", "else", "for", "while", "await", "async",
    "new", "typeof", "in", "of", "break", "continue",
];
const LIT_JS: &[&str] = &["true", "false", "null", "undefined"];
const KW_STARLARK: &[&str] = &[
    "def", "return", "if", "else", "elif", "for", "while", "in", "not", "and", "or", "pass",
    "break", "continue", "lambda",
];
const LIT_STARLARK: &[&str] = &["None", "True", "False"];
const KW_CPP: &[&str] = &[
    "if",
    "else",
    "for",
    "while",
    "do",
    "return",
    "break",
    "continue",
    "switch",
    "case",
    "default",
    "void",
    "int",
    "float",
    "double",
    "bool",
    "char",
    "long",
    "short",
    "unsigned",
    "signed",
    "const",
    "static",
    "volatile",
    "struct",
    "class",
    "enum",
    "typedef",
    "auto",
    "uint8_t",
    "uint16_t",
    "uint32_t",
    "uint64_t",
    "int8_t",
    "int16_t",
    "int32_t",
    "int64_t",
    "size_t",
    "String",
    "sizeof",
    "new",
    "delete",
    "namespace",
    "using",
    "template",
    "public",
    "private",
    "protected",
    "virtual",
    "inline",
    "extern",
];
const LIT_CPP: &[&str] = &[
    "true",
    "false",
    "nullptr",
    "NULL",
    "HIGH",
    "LOW",
    "INPUT",
    "OUTPUT",
    "INPUT_PULLUP",
];

fn keywords(lang: CodeLang) -> &'static [&'static str] {
    match lang {
        CodeLang::Js => KW_JS,
        CodeLang::Starlark => KW_STARLARK,
        CodeLang::Cpp => KW_CPP,
    }
}

fn literals(lang: CodeLang) -> &'static [&'static str] {
    match lang {
        CodeLang::Js => LIT_JS,
        CodeLang::Starlark => LIT_STARLARK,
        CodeLang::Cpp => LIT_CPP,
    }
}

/// Préfixe de commentaire canonique du langage (JS : `//` seulement — `#` est
/// une erreur de syntaxe ; Starlark : `#` seulement — `//` est une division
/// entière). Le parseur de directives pnex-core tolère les deux, la
/// coloration suit la règle du langage.
fn comment_prefix(lang: CodeLang) -> &'static str {
    match lang {
        CodeLang::Js | CodeLang::Cpp => "//",
        CodeLang::Starlark => "#",
    }
}

/// Tokenise le code entier (par ligne, retours ligne en tokens bruts).
pub fn highlight(code: &str, lang: impl Into<CodeLang>) -> Vec<Tok> {
    let lang = lang.into();
    let mut out: Vec<Tok> = Vec::new();
    for (i, line) in code.split('\n').enumerate() {
        if i > 0 {
            out.push(Tok {
                class: "",
                text: "\n".into(),
            });
        }
        highlight_line(line, lang, &mut out);
    }
    out
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_ident(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

/// Tokenise une ligne : chaînes, nombres, identifiants (keyword / littéral /
/// appel de fonction), commentaires (directives `@input`/`@output`
/// sub-colorées). Pragmatique : guillemets non fermés = fin de ligne.
fn highlight_line(line: &str, lang: CodeLang, out: &mut Vec<Tok>) {
    // C++ preprocessor line: `#directive` highlighted, `<header>` as a
    // string, the rest tokenized normally.
    if lang == CodeLang::Cpp {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let indent = &line[..line.len() - trimmed.len()];
            if !indent.is_empty() {
                push_plain(out, indent);
            }
            let word_len = rest.len() - rest.trim_start().len()
                + rest
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic() || *c == '_')
                    .count();
            out.push(Tok {
                class: "text-pink-600 font-semibold",
                text: format!("#{}", &rest[..word_len]),
            });
            let mut tail = &rest[word_len..];
            let spaces = tail.len() - tail.trim_start().len();
            if spaces > 0 {
                push_plain(out, &tail[..spaces]);
                tail = &tail[spaces..];
            }
            if tail.starts_with('<') {
                let end = tail.find('>').map(|p| p + 1).unwrap_or(tail.len());
                out.push(Tok {
                    class: "text-emerald-600",
                    text: tail[..end].to_string(),
                });
                tail = &tail[end..];
            }
            highlight_code(tail, lang, out);
            return;
        }
    }
    highlight_code(line, lang, out);
}

/// Tokenise a line fragment (no preprocessor handling).
fn highlight_code(line: &str, lang: CodeLang, out: &mut Vec<Tok>) {
    let comment = comment_prefix(lang);
    let kws = keywords(lang);
    let lits = literals(lang);
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        // Commentaire : du préfixe au bout de la ligne.
        if line[i..].starts_with(comment) {
            emit_comment(&line[i..], comment, out);
            return;
        }
        // Chaîne (' ou ").
        if c == '"' || c == '\'' {
            let quote = c;
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] as char != quote {
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // guillemet fermant
            }
            out.push(Tok {
                class: "text-emerald-600",
                text: line[start..i].to_string(),
            });
            continue;
        }
        // Nombre (décimal simple : chiffres, point, exposant).
        if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || bytes[i] == b'.'
                    || bytes[i] == b'_'
                    || ((bytes[i] == b'e' || bytes[i] == b'E')
                        && i + 1 < bytes.len()
                        && (bytes[i + 1].is_ascii_digit()
                            || bytes[i + 1] == b'+'
                            || bytes[i + 1] == b'-')))
            {
                i += 1;
                if i > start
                    && (bytes[i.saturating_sub(1)] == b'e' || bytes[i.saturating_sub(1)] == b'E')
                    && i < bytes.len()
                    && (bytes[i] == b'+' || bytes[i] == b'-')
                {
                    i += 1;
                }
            }
            out.push(Tok {
                class: "text-amber-600",
                text: line[start..i].to_string(),
            });
            continue;
        }
        // Identifiant / mot-clé / littéral / appel.
        if is_ident_start(c) {
            let start = i;
            while i < bytes.len() && is_ident(bytes[i] as char) {
                i += 1;
            }
            let word = &line[start..i];
            // Appel de fonction : ident suivi (après espaces) de `(`.
            let call = line[i..].trim_start().starts_with('(');
            let class = if kws.contains(&word) {
                "text-purple-600 font-semibold"
            } else if lits.contains(&word) {
                "text-orange-600 font-semibold"
            } else if call {
                "text-blue-600"
            } else {
                ""
            };
            out.push(Tok {
                class,
                text: word.to_string(),
            });
            continue;
        }
        // Tout le reste : caractère par caractère (ponctuation, espaces).
        let start = i;
        while i < bytes.len() {
            let b = bytes[i] as char;
            if b == '"'
                || b == '\''
                || b.is_ascii_digit()
                || is_ident_start(b)
                || line[i..].starts_with(comment)
            {
                break;
            }
            i += 1;
        }
        out.push(Tok {
            class: "",
            text: line[start..i].to_string(),
        });
    }
}

/// Commentaire : préfixe gris, directive `@input`/`@output` mise en valeur,
/// nom d'entrée/sortie en vert, reste gris.
fn emit_comment(text: &str, prefix: &str, out: &mut Vec<Tok>) {
    let body = text.strip_prefix(prefix).unwrap_or(text);
    out.push(Tok {
        class: "text-gray-400",
        text: prefix.to_string(),
    });
    let trimmed = body.trim_start();
    let is_directive = trimmed.starts_with("@input") || trimmed.starts_with("@output");
    if !is_directive {
        out.push(Tok {
            class: "text-gray-500",
            text: body.to_string(),
        });
        return;
    }
    // Espaces avant la directive.
    let lead = body.len() - trimmed.len();
    if lead > 0 {
        out.push(Tok {
            class: "",
            text: body[..lead].to_string(),
        });
    }
    let after_dir = trimmed
        .trim_start_matches("@input")
        .trim_start_matches("@output");
    let dir_word = &trimmed[..trimmed.len() - after_dir.len()];
    out.push(Tok {
        class: "text-sky-600 font-semibold",
        text: dir_word.to_string(),
    });
    let rest = after_dir;
    let rest_trim = rest.trim_start();
    let lead2 = rest.len() - rest_trim.len();
    if lead2 > 0 {
        out.push(Tok {
            class: "",
            text: rest[..lead2].to_string(),
        });
    }
    // Nom puis reste **avec l'espace séparateur conservé** (le découpage
    // splitn avale le séparateur — le roundtrip texte doit rester exact,
    // sinon l'overlay se désaligne : « temperaturenumber »).
    let split_at = rest_trim
        .find(char::is_whitespace)
        .unwrap_or(rest_trim.len());
    let name = &rest_trim[..split_at];
    let tail = &rest_trim[split_at..];
    if !name.is_empty() {
        out.push(Tok {
            class: "text-emerald-600 font-medium",
            text: name.to_string(),
        });
    }
    if !tail.is_empty() {
        out.push(Tok {
            class: "",
            text: tail.to_string(),
        });
    }
}

// ───────────────────────── Notification templates ─────────────────────────

/// Tokenize a notification template: `{{ … }}` blocks stand out (braces
/// included), plain text carries no class. An unterminated `{{` stays
/// plain so a typo does not repaint the rest of the text.
pub fn highlight_template(text: &str) -> Vec<Tok> {
    const VAR_CLASS: TokClass = "text-sky-700 font-semibold";
    let mut out: Vec<Tok> = Vec::new();
    let mut rest = text;
    loop {
        let Some(start) = rest.find("{{") else {
            push_plain(&mut out, rest);
            break;
        };
        let (before, after) = rest.split_at(start);
        push_plain(&mut out, before);
        match after.find("}}") {
            // `end` points at the opener of `}}`; keep the closing braces
            // in the block token (split_at is byte-safe on ASCII delims).
            Some(end) => {
                let (block, tail) = after.split_at(end + 2);
                out.push(Tok {
                    class: VAR_CLASS,
                    text: block.to_string(),
                });
                rest = tail;
            }
            None => {
                push_plain(&mut out, after);
                break;
            }
        }
    }
    out
}

/// Plain-text token, skipped when empty (fewer DOM nodes).
fn push_plain(out: &mut Vec<Tok>, text: &str) {
    if !text.is_empty() {
        out.push(Tok {
            class: "",
            text: text.to_string(),
        });
    }
}

// ─────────────────────────── Composant éditeur ───────────────────────────

/// Rend un token : classe vide = nœud texte brut (moins de nœuds DOM),
/// sinon `<span>` coloré.
fn tok_text(tok: Tok) -> Element {
    if tok.class.is_empty() {
        rsx! {
            {tok.text}
        }
    } else {
        rsx! {
            span { class: tok.class, {tok.text} }
        }
    }
}

/// Severity of a [`LineMark`] (gutter color + row background).
#[derive(Clone, Copy, PartialEq)]
pub enum MarkSeverity {
    Error,
    Warning,
}

/// A marked source line (1-based) — diagnostics overlay of the editor.
#[derive(Clone, Copy, PartialEq)]
pub struct LineMark {
    pub line: u32,
    pub severity: MarkSeverity,
}

/// Per-line highlighting for the marked editor: tokens of each line, in
/// order. The renderer joins the lines with newline text nodes — concatenating
/// everything reproduces the source byte-for-byte (same contract as
/// `highlight`).
pub fn highlight_lines(code: &str, lang: impl Into<CodeLang>) -> Vec<Vec<Tok>> {
    let lang = lang.into();
    code.split('\n').map(|line| highlight(line, lang)).collect()
}

/// Notification template editor: `{{ … }}` blocks stand out — same overlay
/// technique as `CodeEditor` (transparent field above a colored pre, same
/// metrics, synced scroll). `multiline` = textarea (`height_class` carries
/// the height, e.g. `h-28`); otherwise a single-line field for the subject.
#[component]
pub fn TemplateEditor(
    value: String,
    oninput: EventHandler<String>,
    multiline: bool,
    placeholder: String,
    id: Option<String>,
    height_class: Option<String>,
) -> Element {
    // Field scroll → pre transform (vertical for the textarea, horizontal
    // for the single-line field when the text overflows).
    let mut scroll_y = use_signal(|| 0f64);
    let mut scroll_x = use_signal(|| 0f64);

    let tokens = highlight_template(&value);
    let shift = format!(
        "transform: translate(-{}px, -{}px);",
        scroll_x(),
        scroll_y()
    );
    let height_class = height_class.unwrap_or_default();

    let field_metrics = "px-3 py-2 text-sm font-mono leading-5";

    if multiline {
        rsx! {
            div { class: "relative {height_class} w-full rounded-lg border border-gray-300 overflow-hidden bg-white focus-within:border-blue-500",
                pre {
                    class: "absolute inset-0 {field_metrics} whitespace-pre-wrap break-words overflow-hidden pointer-events-none m-0",
                    "aria-hidden": "true",
                    style: "{shift}",
                    for tok in tokens {
                        {tok_text(tok)}
                    }
                    // Mirror of the trailing newline (keeps heights aligned).
                    "\n"
                }
                textarea {
                    id,
                    class: "absolute inset-0 w-full h-full {field_metrics} bg-transparent text-transparent caret-gray-900 resize-none outline-none",
                    value: "{value}",
                    placeholder: "{placeholder}",
                    spellcheck: false,
                    wrap: "soft",
                    oninput: move |event| oninput.call(event.value()),
                    onscroll: move |event| {
                        scroll_y.set(event.data().scroll_top());
                    },
                }
            }
        }
    } else {
        rsx! {
            div { class: "relative w-full rounded-lg border border-gray-300 overflow-hidden bg-white focus-within:border-blue-500",
                pre {
                    class: "absolute inset-0 {field_metrics} whitespace-pre overflow-hidden pointer-events-none m-0",
                    "aria-hidden": "true",
                    style: "{shift}",
                    for tok in tokens {
                        {tok_text(tok)}
                    }
                }
                input {
                    id,
                    class: "relative w-full {field_metrics} bg-transparent text-transparent caret-gray-900 outline-none",
                    value: "{value}",
                    placeholder: "{placeholder}",
                    spellcheck: false,
                    oninput: move |event| oninput.call(event.value()),
                    onscroll: move |event| {
                        scroll_x.set(event.data().scroll_left());
                    },
                }
            }
        }
    }
}

// ──────────────────────────── JSON documents ────────────────────────────

/// Tokenize a JSON document: object keys (a string directly followed by
/// `:`) sky-blue, string values emerald, numbers amber, `true`/`false`/
/// `null` orange, punctuation plain. Pragmatic school of the JS tokenizer:
/// an unterminated string swallows the rest of the line. Newlines are raw
/// tokens — concatenating every token reproduces the source byte-for-byte
/// (the colored overlay must stay aligned with the textarea).
pub fn highlight_json(code: &str) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    for (i, line) in code.split('\n').enumerate() {
        if i > 0 {
            out.push(Tok {
                class: "",
                text: "\n".into(),
            });
        }
        json_line(line, &mut out);
    }
    out
}

fn json_line(line: &str, out: &mut Vec<Tok>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        // String — object key when the next non-space char is `:`.
        if c == '"' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] as char != '"' {
                i += 1;
            }
            if i < bytes.len() {
                i += 1; // closing quote
            }
            let class = if line[i..].trim_start().starts_with(':') {
                "text-sky-700 font-semibold"
            } else {
                "text-emerald-600"
            };
            out.push(Tok {
                class,
                text: line[start..i].to_string(),
            });
            continue;
        }
        // Number (leading minus included; exponent chars swept loosely —
        // this is coloring, not a validator).
        if c.is_ascii_digit() || (c == '-' && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit()))
        {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_digit()
                    || matches!(bytes[i], b'.' | b'-' | b'+' | b'e' | b'E'))
            {
                i += 1;
            }
            out.push(Tok {
                class: "text-amber-600",
                text: line[start..i].to_string(),
            });
            continue;
        }
        // Literals.
        let lit = ["true", "false", "null"]
            .iter()
            .find(|l| line[i..].starts_with(*l));
        if let Some(lit) = lit {
            out.push(Tok {
                class: "text-orange-600 font-semibold",
                text: (*lit).to_string(),
            });
            i += lit.len();
            continue;
        }
        // Plain punctuation / whitespace, one char.
        out.push(Tok {
            class: "",
            text: c.to_string(),
        });
        i += 1;
    }
}

/// JSON document editor — the `TemplateEditor` overlay technique applied to
/// [`highlight_json`]: a colored pre under a transparent textarea, same
/// metrics, scroll synced. `height_class` carries the height (e.g. `h-28`).
#[component]
pub fn JsonEditor(
    value: String,
    oninput: EventHandler<String>,
    readonly: bool,
    placeholder: String,
    height_class: Option<String>,
) -> Element {
    let mut scroll_y = use_signal(|| 0f64);
    let tokens = highlight_json(&value);
    let shift = format!("transform: translateY(-{}px);", scroll_y());
    let height_class = height_class.unwrap_or_else(|| "h-28".to_string());
    let field_metrics = "px-3 py-2 text-sm font-mono leading-5";

    rsx! {
        div { class: "relative {height_class} w-full rounded-lg border border-gray-300 overflow-hidden bg-white focus-within:border-blue-500",
            pre {
                class: "absolute inset-0 {field_metrics} whitespace-pre-wrap break-words overflow-hidden pointer-events-none m-0",
                "aria-hidden": "true",
                style: "{shift}",
                for tok in tokens {
                    {tok_text(tok)}
                }
                // Mirror of the trailing newline (keeps heights aligned).
                "\n"
            }
            textarea {
                class: "absolute inset-0 w-full h-full {field_metrics} bg-transparent text-transparent caret-gray-900 resize-none outline-none disabled:bg-gray-50/60",
                value: "{value}",
                placeholder: "{placeholder}",
                spellcheck: false,
                wrap: "soft",
                disabled: readonly,
                oninput: move |event| oninput.call(event.value()),
                onscroll: move |event| {
                    scroll_y.set(event.data().scroll_top());
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::FunctionLanguage;

    fn text_of(toks: &[Tok]) -> String {
        toks.iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn roundtrip_texte_integral() {
        let code = "// @input a number\ndef handle(inputs, msg):\n    return inputs[\"a\"]\n";
        assert_eq!(text_of(&highlight(code, FunctionLanguage::Starlark)), code);
        let js = "function handle(inputs, msg) {\n  return msg.payload; // commentaire\n}\n";
        assert_eq!(text_of(&highlight(js, FunctionLanguage::Js)), js);
    }

    #[test]
    fn json_roundtrip_integral() {
        let doc = "{\n  \"test\": true,\n  \"value\": 1.5,\n  \"note\": null,\n  \"list\": [1, \"a\", false]\n}\n";
        assert_eq!(text_of(&highlight_json(doc)), doc);
    }

    #[test]
    fn json_key_vs_value_classes() {
        let toks = highlight_json("{\"a\": \"b\", \"n\": -12e3, \"t\": true}");
        let class_of = |text: &str| {
            toks.iter()
                .find(|t| t.text == text)
                .map(|t| t.class)
                .unwrap_or("")
        };
        // Keys (followed by `:`) stand out from string values; numbers and
        // literals carry their own classes; punctuation stays plain.
        assert_eq!(class_of("\"a\""), "text-sky-700 font-semibold");
        assert_eq!(class_of("\"b\""), "text-emerald-600");
        assert_eq!(class_of("-12e3"), "text-amber-600");
        assert_eq!(class_of("true"), "text-orange-600 font-semibold");
        assert_eq!(class_of(":"), "");
        // An unterminated string stays swallowed-to-end-of-line but keeps
        // the round-trip intact.
        let toks = highlight_json("{\"a\": \"open");
        assert_eq!(
            toks.iter().map(|t| t.text.as_str()).collect::<String>(),
            "{\"a\": \"open"
        );
    }

    #[test]
    fn roundtrip_directives_espace_conserve() {
        // Régression (capture utilisateur) : le découpage nom/type avalait
        // l'espace → « temperaturenumber » et désalignement de l'overlay.
        let star = "# @input temperature number \"Température mesurée (°C)\"\n# @input threshold number=20 \"Seuil d'alerte\"\n# @output heating number \"1 si chauffage\"\n";
        assert_eq!(text_of(&highlight(star, FunctionLanguage::Starlark)), star);
        let js = "// @input temperature number \"T\"\n// @output heating number\nfunction handle(inputs, msg) { return 0; }\n";
        assert_eq!(text_of(&highlight(js, FunctionLanguage::Js)), js);
    }

    #[test]
    fn prefixe_commentaire_par_langage() {
        // En Starlark, `//` n'est PAS un commentaire (division entière) :
        // la ligne reste tokenisée comme code, pas avalée en commentaire.
        let toks = highlight("x = a // b\n", FunctionLanguage::Starlark);
        let joined = toks.iter().map(|t| t.text.as_str()).collect::<String>();
        assert_eq!(joined, "x = a // b\n");
        assert!(
            toks.iter().all(|t| t.class != "text-gray-500"),
            "aucun token commentaire attendu"
        );
        // En JS, `//` avale la fin de ligne en commentaire.
        let toks = highlight("let x = 1 // init\n", FunctionLanguage::Js);
        assert!(toks.iter().any(|t| t.class == "text-gray-500"));
    }

    #[test]
    fn directives_sub_colorees() {
        let toks = highlight(
            "// @input temperature number \"Température\"\n",
            FunctionLanguage::Js,
        );
        assert!(toks
            .iter()
            .any(|t| t.class.contains("sky") && t.text == "@input"));
        assert!(toks
            .iter()
            .any(|t| t.class.contains("emerald") && t.text == "temperature"));
    }

    #[test]
    fn classes_basiques() {
        let toks = highlight(
            "function handle(inputs, msg) { return 42; }",
            FunctionLanguage::Js,
        );
        assert!(toks
            .iter()
            .any(|t| t.class.contains("purple") && t.text == "function"));
        assert!(toks
            .iter()
            .any(|t| t.class.contains("purple") && t.text == "return"));
        assert!(toks
            .iter()
            .any(|t| t.class.contains("blue") && t.text == "handle"));
        assert!(toks
            .iter()
            .any(|t| t.class.contains("amber") && t.text == "42"));
    }

    #[test]
    fn cpp_preprocessor_and_keywords() {
        let code = "#include <Pnex.h>\n  #define N 3 // n\nvoid loop() { return; }";
        let toks = highlight(code, CodeLang::Cpp);
        assert_eq!(text_of(&toks), code);
        assert!(toks
            .iter()
            .any(|t| t.text == "#include" && t.class.contains("pink")));
        assert!(toks
            .iter()
            .any(|t| t.text == "<Pnex.h>" && t.class.contains("emerald")));
        assert!(toks.iter().any(|t| t.text == "#define"));
        assert!(toks
            .iter()
            .any(|t| t.text == "void" && t.class.contains("purple")));
        assert!(toks
            .iter()
            .any(|t| t.text.starts_with("//") || t.class.contains("gray")));
    }

    #[test]
    fn template_blocks_roundtrip_and_stand_out() {
        let t = "[{{ device }}] seuil {{ value }} dépassé\n";
        assert_eq!(text_of(&highlight_template(t)), t);
        let toks = highlight_template(t);
        assert!(toks
            .iter()
            .any(|x| x.class.contains("sky") && x.text == "{{ device }}"));
        assert!(toks
            .iter()
            .any(|x| x.class.contains("sky") && x.text == "{{ value }}"));
    }

    #[test]
    fn template_unterminated_stays_plain() {
        let toks = highlight_template("body {{ broken");
        assert!(toks.iter().all(|x| x.class.is_empty()));
        // A closer without opener stays plain too.
        let toks = highlight_template("plain }} text");
        assert!(toks.iter().all(|x| x.class.is_empty()));
    }

    #[test]
    fn template_multiline_block_roundtrip() {
        let t = "line1 {{\nvar }} line2";
        assert_eq!(text_of(&highlight_template(t)), t);
        assert!(highlight_template(t)
            .iter()
            .any(|x| x.class.contains("sky")));
    }
}

// ─────────────────────── Éditeur fonction v2 ───────────────────────

/// Éditeur de code fonction avancé : gouttière de numéros, marques de
/// diagnostic par ligne (fond + numéro coloré), indentation auto
/// (Tab/Shift+Tab/Enter via `code_editing`), insertion à la demande
/// (snippets, quick-fix). Même technique overlay que `CodeEditor` (pre
/// coloré sous textarea transparent, métriques identiques) — la stack code
/// est décalée à droite de la gouttière, les deux couches bougeant
/// identiquement. Limite documentée : soft-wrap (lignes plus larges que
/// l'éditeur) décale la gouttière — toléré pour les fonctions.
#[component]
pub fn FunctionCodeEditor(
    value: String,
    language: CodeLang,
    readonly: bool,
    oninput: EventHandler<String>,
    /// Marked lines (1-based): colored gutter number + row background.
    #[props(default)]
    marks: Vec<LineMark>,
    /// Imperative insert-at-caret bridge: `(generation, text)` — each new
    /// generation splices `text` at the current caret.
    #[props(default)]
    insert_request: Signal<Option<(u64, String)>>,
    /// Textarea element reference (onmounted capture) for the parent's
    /// caret logic.
    #[props(default)]
    area: Signal<Option<AreaHandle>>,
) -> Element {
    let mut scroll_y = use_signal(|| 0f64);
    // Caret to restore after the controlled rerender (dioxus re-assigning
    // the `value` attribute moves the caret to the end — restore post-DOM).
    let mut pending_caret: Signal<Option<(u32, Option<u32>)>> = use_signal(|| None);

    let lines = highlight_lines(&value, language);
    let line_count = lines.len().max(1) as u32;
    let marked: std::collections::HashMap<u32, MarkSeverity> =
        marks.iter().map(|m| (m.line, m.severity)).collect();
    let shift = format!("transform: translateY(-{}px);", scroll_y());

    // Clones for the effect closures (`value` stays available for the rsx).
    let value_fx = value.clone();

    // Caret restoration after the controlled rerender: dioxus re-assigns
    // the `value` attribute (caret → end); restore ours once, post-DOM.
    use_effect(move || {
        let Some((caret, sel_end)) = pending_caret() else {
            return;
        };
        pending_caret.set(None);
        let Some(el) = area() else { return };
        let c16 = code_editing::byte_to_utf16(&value_fx, caret);
        let e16 = sel_end.map(|e| code_editing::byte_to_utf16(&value_fx, e));
        dom::set_selection(&el, c16, e16.unwrap_or(c16));
    });

    // Imperative insert-at-caret: each new `(gen, text)` splices at the
    // current caret and queues the caret restore after the spliced text.
    let value_fx2 = value.clone();
    use_effect(move || {
        let Some((gen, text)) = insert_request() else {
            return;
        };
        let _ = gen;
        let Some(el) = area() else {
            return;
        };
        let old = dom::value_of(&el);
        let (s16, _) = dom::selection(&el);
        let caret = code_editing::utf16_to_byte(&old, s16);
        let mut next = String::with_capacity(old.len() + text.len());
        next.push_str(&old[..caret as usize]);
        next.push_str(&text);
        next.push_str(&old[caret as usize..]);
        pending_caret.set(Some((caret + text.len() as u32, None)));
        oninput.call(next);
        let _ = &value_fx2;
    });

    rsx! {
        div { class: "relative h-[560px] rounded-lg border border-gray-300 overflow-hidden bg-white",
            // Gutter: numbers share the exact code metrics (leading-5).
            div { class: "absolute inset-y-0 left-0 w-10 overflow-hidden border-r border-gray-100 bg-gray-50",
                pre {
                    class: "m-0 py-2 text-right text-sm font-mono leading-5 text-gray-400 whitespace-pre overflow-hidden pointer-events-none",
                    style: "{shift}",
                    for i in 1..=line_count {
                        if let Some(sev) = marked.get(&i) {
                            span {
                                class: match sev {
                                    MarkSeverity::Error => "text-red-600 font-semibold",
                                    MarkSeverity::Warning => "text-amber-600 font-semibold",
                                },
                                "{i}"
                            }
                        } else {
                            "{i}"
                        }
                        "\n"
                    }
                }
            }
            // Code stack (colored pre + transparent textarea), shifted right
            // of the gutter — both layers move identically on scroll.
            // The colored pre takes its content height (top-anchored, no
            // own overflow clip): clipped at the editor height, the
            // translated layer showed blank lines past ~28 lines. The
            // container clips instead.
            div { class: "absolute inset-y-0 left-10 right-0 overflow-hidden",
                pre {
                    class: "absolute inset-x-0 top-0 px-3 py-2 text-sm font-mono leading-5 whitespace-pre-wrap break-words pointer-events-none m-0",
                    "aria-hidden": "true",
                    style: "{shift}",
                    for (idx, line_toks) in lines.iter().enumerate() {
                        if idx > 0 {
                            "\n"
                        }
                        if marked.contains_key(&((idx + 1) as u32)) {
                            span {
                                class: match marked.get(&((idx + 1) as u32)).copied() {
                                    Some(MarkSeverity::Error) => "bg-red-50",
                                    _ => "bg-amber-50",
                                },
                                for tok in line_toks {
                                    {tok_text(tok.clone())}
                                }
                            }
                        } else {
                            for tok in line_toks {
                                {tok_text(tok.clone())}
                            }
                        }
                    }
                    // Mirror of the trailing newline (keeps heights aligned).
                    "\n"
                }
                textarea {
                    class: "absolute inset-0 w-full h-full px-3 py-2 text-sm font-mono leading-5 bg-transparent text-transparent caret-gray-900 resize-none outline-none disabled:bg-gray-50/60",
                    value: "{value}",
                    spellcheck: false,
                    disabled: readonly,
                    wrap: "soft",
                    oninput: move |event| oninput.call(event.value()),
                    onscroll: move |event| {
                        scroll_y.set(event.data().scroll_top());
                    },
                    onmounted: move |evt| async move {
                        if let Some(a) = dom::capture(evt) {
                            area.set(Some(a));
                        }
                    },
                    onkeydown: move |event: Event<KeyboardData>| {
                        let key = event.key();
                        // DOM selection offsets are UTF-16 units → byte
                        // offsets for the pure engine (and back for restore).
                        let to_byte = |p: Option<u32>| {
                            code_editing::utf16_to_byte(&value, p.unwrap_or(0))
                        };
                        let (sel_start, sel_end) = area
                            .read()
                            .as_ref()
                            .map(dom::selection)
                            .unwrap_or((0, 0));
                        let (sel_start, sel_end) = (to_byte(Some(sel_start)), to_byte(Some(sel_end)));
                        let edit = match key {
                            Key::Tab => {
                                Some(
                                    code_editing::tab_indent(
                                        &value,
                                        sel_start,
                                        sel_end,
                                        language,
                                        event.modifiers().shift(),
                                    ),
                                )
                            }
                            Key::Enter if !readonly => {
                                Some(code_editing::enter_smart(&value, sel_start, language))
                            }
                            _ => None,
                        };
                        let Some(edit) = edit else { return };
                        event.prevent_default();
                        pending_caret.set(Some((edit.caret, edit.sel_end)));
                        oninput.call(edit.value);
                    },
                }
            }
        }
    }
}

// ─────────────────────── DOM caret helpers ───────────────────────

/// DOM caret ops for the editor. Real `web_sys` calls on wasm; inert stubs
/// on native (unit tests run natively — the browser is the only consumer).
#[cfg(target_arch = "wasm32")]
mod dom {
    use web_sys::wasm_bindgen::JsCast;

    pub fn capture(
        evt: dioxus::prelude::Event<dioxus::prelude::MountedData>,
    ) -> Option<web_sys::HtmlTextAreaElement> {
        let el = evt.data().downcast::<web_sys::Element>()?.clone();
        el.dyn_into::<web_sys::HtmlTextAreaElement>().ok()
    }

    /// (selection_start, selection_end) in UTF-16 units.
    pub fn selection(el: &web_sys::HtmlTextAreaElement) -> (u32, u32) {
        (
            el.selection_start().ok().flatten().unwrap_or(0),
            el.selection_end().ok().flatten().unwrap_or(0),
        )
    }

    pub fn set_selection(el: &web_sys::HtmlTextAreaElement, start: u32, end: u32) {
        let _ = el.set_selection_range(start, end);
    }

    pub fn value_of(el: &web_sys::HtmlTextAreaElement) -> String {
        el.value()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod dom {
    use super::AreaHandle;

    pub fn capture(_: dioxus::prelude::Event<dioxus::prelude::MountedData>) -> Option<AreaHandle> {
        None
    }

    pub fn selection(_: &AreaHandle) -> (u32, u32) {
        (0, 0)
    }

    pub fn set_selection(_: &AreaHandle, _start: u32, _end: u32) {}

    pub fn value_of(_: &AreaHandle) -> String {
        String::new()
    }
}
