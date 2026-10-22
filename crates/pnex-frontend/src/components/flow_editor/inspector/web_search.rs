use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::web_search::{WebSearchConfig, LIMIT_MAX};

// ─────────────── web search (web-presence.md D361) ───────────────

const INPUT: &str = "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm";

/// Web search inspector: the org provider (default when empty), how many
/// results, country and language, and the excluded domains (one per line).
#[component]
pub(super) fn WebSearchForm(
    mut cx: EditorCx,
    initial: WebSearchConfig,
    can_write: bool,
) -> Element {
    let providers_res =
        use_resource(|| async { api::web_search::providers().await.unwrap_or_default() });
    let mut config = use_signal(move || initial.clone());
    let providers = providers_res.value().read().clone().unwrap_or_default();
    let loaded = providers_res.value().read().is_some();
    let current = config();

    let mut set = move |next: WebSearchConfig| {
        config.set(next.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::WebSearch { config } = &mut node.kind {
                *config = next;
            }
        });
    };

    let on_provider = move |e: Event<FormData>| {
        set(WebSearchConfig {
            provider: e.value(),
            ..config()
        })
    };
    let on_limit = move |e: Event<FormData>| {
        set(WebSearchConfig {
            limit: e.value().trim().parse().unwrap_or(0),
            ..config()
        })
    };
    let on_country = move |e: Event<FormData>| {
        set(WebSearchConfig {
            country: e.value().trim().to_string(),
            ..config()
        })
    };
    let on_lang = move |e: Event<FormData>| {
        set(WebSearchConfig {
            lang: e.value().trim().to_string(),
            ..config()
        })
    };
    let on_exclude = move |e: Event<FormData>| {
        let exclude_domains = e
            .value()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        set(WebSearchConfig {
            exclude_domains,
            ..config()
        })
    };
    let excluded = current.exclude_domains.join("\n");

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-web-search-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-web-search-provider")} }
                if loaded && providers.is_empty() {
                    p { class: "text-xs text-amber-700", {t!("flows-web-search-none")} }
                }
                select {
                    class: INPUT,
                    disabled: !can_write,
                    onchange: on_provider,
                    option { value: "", selected: current.provider.is_empty(),
                        {t!("flows-web-search-default")}
                    }
                    for p in providers.iter() {
                        option {
                            key: "{p.id}",
                            value: "{p.name}",
                            selected: p.name == current.provider,
                            "{p.name} · {p.kind.as_str()}"
                        }
                    }
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-web-search-limit")} }
                input {
                    class: INPUT,
                    r#type: "number",
                    min: "1",
                    max: "{LIMIT_MAX}",
                    disabled: !can_write,
                    value: "{current.limit}",
                    oninput: on_limit,
                }
            }
            div { class: "grid grid-cols-2 gap-2",
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("flows-web-search-country")} }
                    input {
                        class: INPUT,
                        placeholder: "fr",
                        disabled: !can_write,
                        value: "{current.country}",
                        oninput: on_country,
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("flows-web-search-lang")} }
                    input {
                        class: INPUT,
                        placeholder: "fr",
                        disabled: !can_write,
                        value: "{current.lang}",
                        oninput: on_lang,
                    }
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-web-search-exclude")} }
                textarea {
                    class: "{INPUT} font-mono",
                    rows: "6",
                    disabled: !can_write,
                    value: "{excluded}",
                    oninput: on_exclude,
                }
            }
        }
    }
}
