//! Searchable reference modal for the two function languages — opens from
//! the editor toolbar ("Insérer un snippet" = insert mode: clicking a row
//! inserts its snippet at the caret). Content reflects what the runtimes
//! ACTUALLY expose: Starlark = Standard dialect + `json` module only (no
//! math), JS = QuickJS globals + the console shim.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::FunctionLanguage;

/// (signature shown, description i18n key suffix, snippet inserted on +)
/// Flat table: group title keys live in [`GROUPS`], one row per entry.
type ItemsTable = &'static [(&'static str, &'static str, &'static str)];
type GroupsTable = &'static [(&'static str, usize, usize)];

const STARLARK_ITEMS: ItemsTable = &[
    ("inputs[\"nom\"]", "in", "inputs[\"nom\"]"),
    ("msg", "msg", "msg"),
    ("return {\"out\": 1}", "return", "return {\"out\": 1}"),
    ("len(x)", "len", "len(x)"),
    ("min(a, b) / max(a, b)", "minmax", "max(a, b)"),
    ("abs(x)", "abs", "abs(x)"),
    (
        "int(x) / float(x) / str(x) / bool(x)",
        "convert",
        "float(x)",
    ),
    ("range(n) / range(a, b)", "range", "range(a, b)"),
    ("sorted(x)", "sorted", "sorted(x)"),
    ("any(x) / all(x)", "anyall", "all(x)"),
    ("d.get(k, defaut)", "dget", "d.get(k, defaut)"),
    ("s.split / strip / replace", "sstring", "s.strip()"),
    ("json.encode(x) / json.decode(s)", "json", "json.decode(s)"),
    ("print(...) / fail(message)", "failprint", "fail(message)"),
];

const JS_ITEMS: ItemsTable = &[
    ("inputs.nom / inputs[\"nom\"]", "in", "inputs[\"nom\"]"),
    ("msg", "msg", "msg"),
    ("return { out: 1 }", "return", "return { out: 1 }"),
    ("Math.round / floor / abs", "math", "Math.round(x)"),
    ("Math.min(...) / Math.max(...)", "minmax2", "Math.max(a, b)"),
    ("Number(x) / parseFloat(s)", "convert2", "parseFloat(s)"),
    (
        "Number.isFinite(x) / isNaN(x)",
        "finite",
        "Number.isFinite(x)",
    ),
    (
        "JSON.parse(s) / JSON.stringify(x)",
        "json2",
        "JSON.parse(s)",
    ),
    (
        "Object.keys(o) / Object.entries(o)",
        "object",
        "Object.keys(o)",
    ),
    ("arr.map / filter / find", "array", "arr.filter(f)"),
    ("s.split / trim / replace", "string", "s.trim()"),
    ("Date.now()", "date", "Date.now()"),
    ("console.log(...)", "log", "console.log(x)"),
];

/// (group title key, item index range in the flat table).
const STARLARK_GROUPS: GroupsTable = &[
    ("functions-ref-group-context", 0, 3),
    ("functions-ref-group-builtins", 3, 10),
    ("functions-ref-group-methods", 10, 12),
    ("functions-ref-group-modules", 12, 14),
];

const JS_GROUPS: GroupsTable = &[
    ("functions-ref-group-context", 0, 3),
    ("functions-ref-group-numbers", 3, 7),
    ("functions-ref-group-json", 7, 10),
    ("functions-ref-group-strings", 10, 13),
];

/// Reference content per language — adapted to the actual runtime surface.
pub fn reference_data(lang: FunctionLanguage) -> Vec<RefGroup> {
    let (items, groups): (ItemsTable, GroupsTable) = match lang {
        FunctionLanguage::Starlark => (STARLARK_ITEMS, STARLARK_GROUPS),
        FunctionLanguage::Js => (JS_ITEMS, JS_GROUPS),
    };
    groups
        .iter()
        .map(|(title, start, end)| RefGroup {
            title_key: title,
            items: items[*start..*end]
                .iter()
                .map(|(sig, key, snip)| RefItem {
                    sig,
                    desc_key: format!("functions-ref-desc-{key}"),
                    snippet: snip.to_string(),
                })
                .collect(),
        })
        .collect()
}

#[derive(Clone, PartialEq)]
pub struct RefItem {
    pub sig: &'static str,
    pub desc_key: String,
    pub snippet: String,
}

pub struct RefGroup {
    pub title_key: &'static str,
    pub items: Vec<RefItem>,
}

/// Reference "not available" section text (pre-line), per language.
pub fn unavailable_key(lang: FunctionLanguage) -> &'static str {
    match lang {
        FunctionLanguage::Starlark => "functions-ref-unavailable-starlark",
        FunctionLanguage::Js => "functions-ref-unavailable-js",
    }
}

/// (i18n key, host shown, href) of the external doc links.
pub fn doc_links(lang: FunctionLanguage) -> Vec<(&'static str, &'static str, &'static str)> {
    match lang {
        FunctionLanguage::Starlark => vec![
            (
                "functions-ref-link-starlark-spec",
                "github.com/bazelbuild/starlark",
                "https://github.com/bazelbuild/starlark/blob/master/spec.md",
            ),
            (
                "functions-ref-link-mdn",
                "developer.mozilla.org",
                "https://developer.mozilla.org/fr/docs/Web/JavaScript/Reference",
            ),
            (
                "functions-ref-link-quickjs",
                "bellard.org",
                "https://bellard.org/quickjs/",
            ),
        ],
        FunctionLanguage::Js => vec![
            (
                "functions-ref-link-mdn",
                "developer.mozilla.org",
                "https://developer.mozilla.org/fr/docs/Web/JavaScript/Reference",
            ),
            (
                "functions-ref-link-mdn-objects",
                "developer.mozilla.org",
                "https://developer.mozilla.org/fr/docs/Web/JavaScript/Reference/Global_Objects",
            ),
            (
                "functions-ref-link-quickjs",
                "bellard.org",
                "https://bellard.org/quickjs/",
            ),
        ],
    }
}

/// Right drawer reference modal (mockup "Référence"): client-side search,
/// groups, "not available" section, doc links, and a `+` insert button per
/// row. Single language — always the edited function's own (the selector
/// would be redundant: the language is fixed at creation).
#[component]
pub fn FunctionsReferenceModal(
    open: bool,
    lang: FunctionLanguage,
    oninsert: EventHandler<String>,
    onclose: EventHandler,
) -> Element {
    let mut search = use_signal(String::new);
    if !open {
        return rsx! {};
    }
    let groups = reference_data(lang);
    let unavailable = unavailable_key(lang);
    let links: Vec<(String, &str, &str)> = doc_links(lang)
        .into_iter()
        .map(|(k, h, u)| (k.to_string(), h, u))
        .collect();
    let unavailable_str: &str = unavailable;
    let q = search.read().to_lowercase();
    rsx! {
        div { class: "fixed inset-0 z-50 flex justify-end bg-black/30",
            div { class: "h-full w-[480px] max-w-full bg-white shadow-2xl flex flex-col",
                div { class: "flex items-center gap-2 px-5 py-3.5 border-b border-gray-200",
                    h2 { class: "text-lg font-semibold flex-grow", {t!("functions-ref-title")} }
                    button {
                        class: "text-gray-500 hover:text-gray-800 text-2xl leading-none",
                        onclick: move |_| onclose.call(()),
                        "×"
                    }
                }
                div { class: "px-5 pt-3",
                    input {
                        class: "w-full h-9 rounded-lg border border-gray-300 px-3 text-sm",
                        placeholder: t!("functions-ref-search-placeholder"),
                        value: "{search}",
                        oninput: move |ev| search.set(ev.value()),
                    }
                }
                div { class: "flex-1 overflow-y-auto px-5 py-3 space-y-4",
                    for group in &groups {
                        RefGroupView {
                            title_key: group.title_key.to_string(),
                            items: group.items.clone(),
                            q: q.clone(),
                            oninsert,
                        }
                    }
                    div { class: "rounded-lg bg-red-50 border border-red-100 p-3",
                        h3 { class: "text-[13px] font-semibold text-red-900 uppercase tracking-wide",
                            {t!("functions-ref-unavailable-title")}
                        }
                        p { class: "text-[13px] text-red-800 whitespace-pre-line",
                            {t!(unavailable_str)}
                        }
                    }
                    div { class: "pt-2",
                        h3 { class: "text-[13px] font-semibold text-gray-500 uppercase tracking-wide",
                            {t!("functions-ref-docs-title")}
                        }
                        for (key, host, href) in &links {
                            a {
                                href: "{href}",
                                target: "_blank",
                                rel: "noopener",
                                class: "flex items-center justify-between py-1.5 text-sm",
                                span { class: "font-medium text-blue-700", {t!(key.as_str())} }
                                span { class: "text-xs text-gray-500", "{host}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One group with its (filtered) rows.
#[component]
fn RefGroupView(
    title_key: String,
    items: Vec<RefItem>,
    q: String,
    oninsert: EventHandler<String>,
) -> Element {
    let lower = q.to_lowercase();
    let matches: Vec<(String, String, String)> = items
        .iter()
        .filter(|it| {
            lower.is_empty()
                || it.sig.to_lowercase().contains(&lower)
                || it.desc_key.to_lowercase().contains(&lower)
        })
        .map(|it| (it.sig.to_string(), it.desc_key.clone(), it.snippet.clone()))
        .collect();
    rsx! {
        div {
            h3 { class: "text-[13px] font-semibold text-gray-500 uppercase tracking-wide",
                {t!(title_key.as_str())}
            }
            div { class: "mt-1.5 divide-y divide-gray-100",
                for (sig, desc, snippet) in matches {
                    div { class: "flex items-center gap-2 py-1.5",
                        div { class: "flex-grow min-w-0",
                            div { class: "font-mono text-[13px]", "{sig}" }
                            div { class: "text-[13px] text-gray-600", {t!(desc.as_str())} }
                        }
                        button {
                            class: "h-6 w-6 rounded-md border border-gray-300 text-sm text-blue-700 hover:bg-blue-50 shrink-0",
                            title: "insert",
                            onclick: move |_| oninsert.call(snippet.clone()),
                            "+"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference tables must be non-empty for BOTH languages.
    #[test]
    fn reference_tables_non_empty() {
        for lang in [FunctionLanguage::Js, FunctionLanguage::Starlark] {
            let data = reference_data(lang);
            assert!(!data.is_empty());
            assert!(data.iter().map(|g| g.items.len()).sum::<usize>() >= 10);
        }
    }
}
