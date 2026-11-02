//! Canonical filter bar — the container and the two fields duplicated
//! verbatim in the historical pages (search + refresh). Page-specific
//! fields (the page's i18n selects) stay in the page rsx, placed inside the
//! `FilterBar`.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;
use crate::components::labels_editor::parse_entry;
use crate::components::ui::LabelTag;
use crate::state::toasts;
use pnex_core::resources::parse_label_filter;

/// Filter container — same class as the `filters` slot of
/// [`super::layout::ListLayout`], to place the bar in the page body
/// (filters specific to the list branch, hidden in the sub-views).
///
/// Below `sm` the search takes a full row of its own (first) and the page's
/// selects share the next rows evenly (min 8rem each, wrapping cleanly),
/// instead of wrapping at random with a truncated placeholder.
#[component]
pub fn FilterBar(children: Element) -> Element {
    rsx! {
        div { class: FILTER_BAR_CLASS, {children} }
    }
}

/// Container classes of [`FilterBar`] (what the `filters` slot of
/// [`super::layout::ListLayout`] expects).
pub const FILTER_BAR_CLASS: &str = "mb-4 md:mb-6 flex flex-wrap items-center gap-2 [&>select]:min-w-0 [&>select]:grow [&>select]:basis-32 sm:[&>select]:grow-0 sm:[&>select]:basis-auto sm:[&>select]:w-auto";

/// Server search field — **live input without refetch** (the signal is
/// enough); submission is explicit: Enter → `prevent_default` +
/// `on_submit` (page side: reset of the current page + reload). New
/// filters always reset the page to 0 before the refetch.
#[component]
pub fn SearchInput(
    /// Placeholder (i18n resolved by the caller).
    placeholder: String,
    /// Signal of the current query (input kept between two renders).
    mut value: Signal<String>,
    on_submit: Callback<()>,
    /// Current page of a list that refetches as you type: reset to 0 on each
    /// keystroke, otherwise a narrower query is read at a stale offset.
    page: Option<Signal<i64>>,
) -> Element {
    rsx! {
        input {
            class: "order-first w-full sm:order-none sm:w-auto sm:flex-1 sm:min-w-48 rounded-md border border-line-strong bg-surface px-3 py-2 text-sm text-ink placeholder:text-gray-500 focus:border-signal focus:outline-none focus:ring-3 focus:ring-signal-200",
            r#type: "search",
            placeholder: "{placeholder}",
            value: "{value}",
            oninput: move |event| {
                value.set(event.value());
                if let Some(mut page) = page {
                    page.set(0);
                }
            },
            onkeydown: move |event| {
                if event.key() == Key::Enter {
                    event.prevent_default();
                    on_submit.call(());
                }
            },
        }
    }
}

/// Label filter: one chip per term (`name` or `name:value`), all terms
/// required (AND). `value` holds the comma-joined terms sent as the `label`
/// list parameter (and accepted by `?label=` links). Enter adds the typed
/// term, × or Backspace on an empty field removes one; each change submits.
#[component]
pub fn LabelFilterInput(
    placeholder: String,
    value: Signal<String>,
    on_submit: Callback<()>,
) -> Element {
    let mut value = value;
    let mut draft = use_signal(String::new);
    let raw = value();
    // An unparsable value (hand-edited link) stays visible as one removable chip.
    let terms: Vec<(String, Option<String>)> = match parse_label_filter(&raw) {
        (Some(terms), _) => terms,
        _ if raw.trim().is_empty() => Vec::new(),
        _ => vec![(raw.trim().to_string(), None)],
    };
    let mut commit = move |terms: Vec<(String, Option<String>)>| {
        value.set(join_label_terms(&terms));
        on_submit.call(());
    };
    let for_keys = terms.clone();
    rsx! {
        crate::components::labels_editor::LabelSuggestions { id: "pnex-labels-filter" }
        div { class: "order-first w-full sm:order-none sm:w-auto sm:flex-1 sm:min-w-48 flex flex-wrap items-center gap-1 rounded-md border border-line-strong bg-surface px-2 py-1.5 focus-within:border-signal focus-within:ring-3 focus-within:ring-signal-200",
            for (i, (name, term_value)) in terms.iter().cloned().enumerate() {
                LabelTag { key: "{name}", name: name.clone(), value: term_value,
                    button {
                        class: "border-l border-line-strong px-1.5 text-slate hover:bg-red-50 hover:text-alarm",
                        r#type: "button",
                        aria_label: t!("resources-label-remove", name : name.clone()),
                        onclick: {
                            let terms = terms.clone();
                            move |_| {
                                let mut terms = terms.clone();
                                terms.remove(i);
                                commit(terms);
                            }
                        },
                        {"×"}
                    }
                }
            }
            input {
                class: "min-w-24 flex-1 border-0 bg-transparent p-0.5 text-sm text-ink placeholder:text-gray-500 focus:outline-none focus:ring-0",
                r#type: "search",
                list: "pnex-labels-filter",
                placeholder: "{placeholder}",
                aria_label: "{placeholder}",
                value: "{draft}",
                oninput: move |event| draft.set(event.value()),
                onkeydown: move |event| {
                    let mut terms = for_keys.clone();
                    match event.key() {
                        Key::Enter => {
                            event.prevent_default();
                            if draft.peek().trim().is_empty() {
                                on_submit.call(());
                                return;
                            }
                            let entry = parse_entry(&draft.peek());
                            match entry {
                                Ok((name, term_value)) => {
                                    // One value per name: a resource carries a single value per label name.
                                    terms.retain(|(n, _)| *n != name);
                                    terms.push((name, term_value));
                                    draft.set(String::new());
                                    commit(terms);
                                }
                                Err(msg) => toasts::error(msg),
                            }
                        }
                        Key::Backspace if draft.peek().is_empty() && !terms.is_empty() => {
                            terms.pop();
                            commit(terms);
                        }
                        _ => {}
                    }
                },
            }
        }
    }
}

/// Comma-joined filter terms, the `label` list parameter format.
fn join_label_terms(terms: &[(String, Option<String>)]) -> String {
    terms
        .iter()
        .map(|(n, v)| match v {
            Some(v) => format!("{n}:{v}"),
            None => n.clone(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Manual refresh button — icon-only everywhere (single idiom).
/// `inline-flex items-center`: an inline SVG sits on the text baseline and
/// otherwise inflates the line box by ~5 px (button taller than neighboring
/// selects). Icon `h-5 w-5` → same 38 px height as `py-2 text-sm` inputs.
#[component]
pub fn RefreshButton(on_click: Callback<()>) -> Element {
    rsx! {
        button {
            class: "inline-flex h-10 w-10 shrink-0 items-center justify-center rounded-md border border-line-strong bg-surface text-ink hover:bg-gray-50 transition-colors",
            r#type: "button",
            title: t!("common-refresh"),
            "aria-label": t!("common-refresh"),
            onclick: move |_| on_click.call(()),
            icons::RefreshCw { class: "h-5 w-5" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joined_terms_parse_back() {
        let terms = vec![
            ("source".to_string(), Some("recorder".to_string())),
            ("critique".to_string(), None),
        ];
        let joined = join_label_terms(&terms);
        assert_eq!(joined, "source:recorder,critique");
        assert_eq!(parse_label_filter(&joined).0, Some(terms));
    }
}
