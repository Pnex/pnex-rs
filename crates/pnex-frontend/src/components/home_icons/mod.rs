//! Home icon catalog of the dashboard cards (D136): original PneX
//! drawings on a 24 × 24 grid, stroked with `currentColor` so a card
//! colours its icon through CSS `color`. Ids are `home-<name>` (validated
//! server side by `pnex_core::valid_symbol_id`); an unknown id renders
//! nothing (open enum, like widget types).

mod catalog;

use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use dioxus_i18n::t;

pub use catalog::CATEGORIES;

/// One catalog entry (static data, generated).
#[derive(Debug, PartialEq, Eq)]
pub struct HomeIcon {
    pub id: &'static str,
    pub category: &'static str,
    /// Canonical English name (fallback when `hicon-<id>` is missing).
    pub name: &'static str,
    pub body: &'static str,
}

pub fn all() -> impl Iterator<Item = &'static HomeIcon> {
    catalog::ICONS.iter()
}

pub fn find(id: &str) -> Option<&'static HomeIcon> {
    all().find(|i| i.id == id)
}

/// Localized icon name — render scope only, never panics.
pub fn icon_name(icon: &HomeIcon) -> String {
    i18n()
        .try_translate(&format!("hicon-{}", icon.id))
        .unwrap_or_else(|_| icon.name.to_string())
}

/// Localized category label — render scope only.
pub fn category_label(category: &str) -> String {
    i18n()
        .try_translate(&format!("hicon-cat-{category}"))
        .unwrap_or_else(|_| category.to_string())
}

/// Renders a catalog icon (nothing for an unknown id).
#[component]
pub fn HomeIconView(id: String, #[props(default)] class: Option<String>) -> Element {
    let Some(icon) = find(&id) else {
        return rsx! {};
    };
    let class = class.unwrap_or_else(|| "h-5 w-5".to_string());
    rsx! {
        svg {
            class: "{class}",
            xmlns: "http://www.w3.org/2000/svg",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            "stroke-width": "2",
            "stroke-linecap": "round",
            "stroke-linejoin": "round",
            dangerous_inner_html: icon.body,
        }
    }
}

/// Compact icon picker: search + grid grouped by category. `value` = the
/// selected id (`None` = no icon); `on_change(None)` clears it.
#[component]
pub fn HomeIconPicker(
    value: Option<String>,
    disabled: bool,
    on_change: EventHandler<Option<String>>,
    /// Id of the visible caption naming this picker (`aria-labelledby`).
    #[props(default)]
    labelledby: Option<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut query = use_signal(String::new);
    let current = value.clone().unwrap_or_default();
    let q = query.read().to_lowercase();
    let groups: Vec<(&'static str, Vec<&'static HomeIcon>)> = CATEGORIES
        .iter()
        .map(|cat| {
            let icons: Vec<&'static HomeIcon> = all()
                .filter(|i| i.category == *cat)
                .filter(|i| {
                    q.is_empty() || i.id.contains(&q) || icon_name(i).to_lowercase().contains(&q)
                })
                .collect();
            (*cat, icons)
        })
        .filter(|(_, icons)| !icons.is_empty())
        .collect();
    let group_role = labelledby.as_ref().map(|_| "group");
    rsx! {
        div {
            class: "space-y-1",
            role: group_role,
            aria_labelledby: labelledby,
            div { class: "flex items-center gap-2",
                button {
                    r#type: "button",
                    class: "flex h-8 w-8 items-center justify-center rounded border border-gray-300 bg-white text-gray-700 hover:bg-gray-50",
                    disabled,
                    title: t!("hicon-picker").to_string(),
                    onclick: move |_| {
                        let next = !open();
                        open.set(next);
                    },
                    if current.is_empty() {
                        span { class: "text-xs text-gray-400", "—" }
                    } else {
                        HomeIconView { id: current.clone(), class: "h-5 w-5" }
                    }
                }
                if !current.is_empty() && !disabled {
                    button {
                        r#type: "button",
                        class: "text-xs text-gray-400 hover:text-red-500",
                        onclick: move |_| on_change.call(None),
                        {t!("hicon-none")}
                    }
                }
            }
            if open() && !disabled {
                div { class: "rounded-lg border border-gray-200 bg-white p-2 shadow-sm",
                    input {
                        class: "mb-2 w-full rounded border border-gray-300 px-2 py-1 text-xs",
                        placeholder: t!("hicon-search").to_string(),
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                    div { class: "max-h-64 space-y-2 overflow-y-auto",
                        for (cat, icons) in groups {
                            div { key: "{cat}",
                                p { class: "mb-1 text-[10px] font-medium uppercase text-gray-400",
                                    {category_label(cat)}
                                }
                                div { class: "grid grid-cols-6 gap-1",
                                    for icon in icons {
                                        IconCell {
                                            key: "{icon.id}",
                                            id: icon.id.to_string(),
                                            title: icon_name(icon),
                                            selected: icon.id == current,
                                            on_pick: move |id: String| {
                                                on_change.call(Some(id));
                                                open.set(false);
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn IconCell(id: String, title: String, selected: bool, on_pick: EventHandler<String>) -> Element {
    let pick = id.clone();
    rsx! {
        button {
            r#type: "button",
            class: if selected { "flex h-8 items-center justify-center rounded bg-blue-600 text-white" } else { "flex h-8 items-center justify-center rounded text-gray-700 hover:bg-gray-100" },
            title: "{title}",
            onclick: move |_| on_pick.call(pick.clone()),
            HomeIconView { id, class: "h-5 w-5" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_ids_are_unique_valid_and_safe() {
        let mut seen = HashSet::new();
        for i in all() {
            assert!(pnex_core::valid_symbol_id(i.id), "{}", i.id);
            assert!(i.id.starts_with("home-"), "{}", i.id);
            assert!(seen.insert(i.id), "duplicate {}", i.id);
            assert!(CATEGORIES.contains(&i.category), "{}", i.id);
            assert!(!i.body.is_empty(), "{}", i.id);
            assert!(!i.body.contains("<script"), "{}", i.id);
            assert!(!i.body.contains(" on"), "{} has an event attribute", i.id);
        }
        assert!(seen.len() >= 60);
    }

    #[test]
    fn every_icon_and_category_is_translated() {
        for lang in ["en-US", "fr-FR"] {
            let path = format!("{}/locales/{lang}.ftl", env!("CARGO_MANIFEST_DIR"));
            let ftl = std::fs::read_to_string(path).unwrap();
            for i in all() {
                assert!(
                    ftl.contains(&format!("\nhicon-{} = ", i.id)),
                    "{lang}: {}",
                    i.id
                );
            }
            for c in CATEGORIES {
                assert!(ftl.contains(&format!("\nhicon-cat-{c} = ")), "{lang}: {c}");
            }
        }
    }
}
