//! On-demand palette (principle 3) — a floating `+` button opening a
//! popover with search. Replaces the permanent left panels of the three
//! editors. **No backdrop**: dragging a model from the popover footer to
//! the canvas (dashboard, D41) must let its `pointermove/up` reach the
//! canvas — closing goes through a pick, the cross, Escape in the search
//! or a re-click on `+`.
//! One menu at a time and click-outside close: `state::ui::use_exclusive_menu`
//! + the dismiss listener of `EditorShell` (menu slots carry `data-floating`).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;
use crate::components::ui::INPUT;

/// Palette icons — flat enum (data), rendered by `IconView` via the
/// `icons.rs` set. Avoids any `Element`/closure in the items (trivial
/// `PartialEq` props) while giving the types a visual cue.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaletteIcon {
    Calculator,
    Thermometer,
    Snowflake,
    Eye,
    Braces,
    Bug,
    Puzzle,
    Database,
    Cpu,
    Activity,
    Bell,
    Globe,
    Spline,
    Gauge,
    Hash,
    LineChart,
    CheckCircle,
    Info,
    Layers,
    Image,
    Camera,
    Video,
    History,
    Shapes,
    ToggleRight,
    SlidersHorizontal,
    Pointer,
    /// Home icon catalog entry (D136), by id.
    Home(&'static str),
}

/// Rendering of a `PaletteIcon` (explicit match — never a constructed icon).
#[component]
pub fn IconView(icon: PaletteIcon, class: Option<String>) -> Element {
    let class = class.unwrap_or_else(|| "h-4 w-4".to_string());
    rsx! {
        {
            match icon {
                PaletteIcon::Calculator => rsx! {
                    icons::Calculator { class }
                },
                PaletteIcon::Thermometer => rsx! {
                    icons::Thermometer { class }
                },
                PaletteIcon::Snowflake => rsx! {
                    icons::Snowflake { class }
                },
                PaletteIcon::Eye => rsx! {
                    icons::Eye { class }
                },
                PaletteIcon::Braces => rsx! {
                    icons::Braces { class }
                },
                PaletteIcon::Bug => rsx! {
                    icons::Bug { class }
                },
                PaletteIcon::Puzzle => rsx! {
                    icons::Puzzle { class }
                },
                PaletteIcon::Database => rsx! {
                    icons::Database { class }
                },
                PaletteIcon::Cpu => rsx! {
                    icons::Cpu { class }
                },
                PaletteIcon::Activity => rsx! {
                    icons::Activity { class }
                },
                PaletteIcon::Bell => rsx! {
                    icons::Bell { class }
                },
                PaletteIcon::Globe => rsx! {
                    icons::Globe { class }
                },
                PaletteIcon::Spline => rsx! {
                    icons::Spline { class }
                },
                PaletteIcon::Gauge => rsx! {
                    icons::Gauge { class }
                },
                PaletteIcon::Hash => rsx! {
                    icons::Hash { class }
                },
                PaletteIcon::LineChart => rsx! {
                    icons::LineChart { class }
                },
                PaletteIcon::CheckCircle => rsx! {
                    icons::CheckCircle { class }
                },
                PaletteIcon::Info => rsx! {
                    icons::Info { class }
                },
                PaletteIcon::Layers => rsx! {
                    icons::Layers { class }
                },
                PaletteIcon::Image => rsx! {
                    icons::Image { class }
                },
                PaletteIcon::Camera => rsx! {
                    icons::Camera { class }
                },
                PaletteIcon::Video => rsx! {
                    icons::Video { class }
                },
                PaletteIcon::History => rsx! {
                    icons::History { class }
                },
                PaletteIcon::Shapes => rsx! {
                    icons::Shapes { class }
                },
                PaletteIcon::ToggleRight => rsx! {
                    icons::ToggleRight { class }
                },
                PaletteIcon::SlidersHorizontal => rsx! {
                    icons::SlidersHorizontal { class }
                },
                PaletteIcon::Pointer => rsx! {
                    icons::Pointer { class }
                },
                PaletteIcon::Home(id) => rsx! {
                    crate::components::home_icons::HomeIconView { id, class }
                },
            }
        }
    }
}

/// Palette entry — flat data: pick key, i18n labels resolved by the
/// caller, icon + tile classes (full literals, see the Tailwind
/// scan).
#[derive(Clone, PartialEq, Debug)]
pub struct PaletteItem {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub icon: Option<PaletteIcon>,
    /// Tailwind classes of the icon tile (background/text of the type's tone,
    /// e.g. "bg-emerald-50 text-emerald-600") — full literal.
    pub tile_class: Option<String>,
    /// Section header (already translated). Items sharing a group must be
    /// contiguous: the popover starts a new section whenever it changes.
    /// `None` everywhere = flat list (dashboard, studio).
    pub group: Option<String>,
}

impl PaletteItem {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            description: None,
            icon: None,
            tile_class: None,
            group: None,
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_icon(mut self, icon: PaletteIcon, tile_class: &'static str) -> Self {
        self.icon = Some(icon);
        self.tile_class = Some(tile_class.to_string());
        self
    }

    pub fn with_group(mut self, group: impl Into<String>) -> Self {
        self.group = Some(group.into());
        self
    }
}

/// Pairs each item with the section header to render before it (`Some` on
/// the first item of every group run, `None` otherwise). Groups emptied by
/// the search filter therefore vanish with their header.
fn with_headers(items: &[PaletteItem]) -> Vec<(Option<String>, PaletteItem)> {
    let mut previous: Option<&str> = None;
    items
        .iter()
        .map(|item| {
            let current = item.group.as_deref();
            let header = match current {
                Some(group) if previous != Some(group) => Some(group.to_string()),
                _ => None,
            };
            previous = current;
            (header, item.clone())
        })
        .collect()
}

/// Floating `+` button + popover (search + list + optional footer).
/// Positioned by the parent slot (`absolute left-4 top-4` in the body of
/// `EditorShell`) — the card anchors on this wrapper.
#[component]
pub fn PalettePopover(
    /// Tooltip/aria of the "+" button ("Add a node"…).
    add_title: String,
    /// Popover header ("Nodes", "Library"…).
    title: String,
    /// Search placeholder.
    search_placeholder: String,
    /// Offered types.
    items: Vec<PaletteItem>,
    /// Pick of an item (key) — the popover closes.
    on_pick: Callback<String>,
    /// Popover footer (dashboard: draggable server models).
    #[props(default)]
    footer: Option<Element>,
) -> Element {
    let mut open = use_signal(|| false);
    // One menu at a time; closes on a click outside (EditorShell).
    crate::state::ui::use_exclusive_menu(open);
    let mut search = use_signal(String::new);
    let close_label = t!("eshell-close");

    // In-memory filter: label or description contains the query
    // (case-insensitive). Computed at render — the component re-renders on `search`.
    let query = search().to_lowercase();
    let filtered: Vec<PaletteItem> = items
        .iter()
        .filter(|item| {
            if query.is_empty() {
                return true;
            }
            let label = item.label.to_lowercase();
            let description = item.description.as_deref().unwrap_or("").to_lowercase();
            let group = item.group.as_deref().unwrap_or("").to_lowercase();
            label.contains(&query) || description.contains(&query) || group.contains(&query)
        })
        .cloned()
        .collect();
    let rows = with_headers(&filtered);

    rsx! {
        div { // wrapper anchored by the parent slot


            button {
                class: "flex h-10 w-10 items-center justify-center rounded-full bg-ink text-white shadow-lg transition-colors hover:bg-gray-800",
                title: "{add_title}",
                "aria-label": "{add_title}",
                onclick: move |_| {
                    open.toggle();
                    search.set(String::new());
                },
                // One glyph: the plus turns 45° into the close cross.
                icons::Plus { class: if open() { "h-5 w-5 rotate-45 transition-transform duration-150 ease-out-ui" } else { "h-5 w-5 transition-transform duration-150 ease-out-ui" } }
            }
            if open() {
                div { class: "pnex-pop-in absolute left-0 top-12 z-30 flex w-72 flex-col overflow-hidden rounded-xl border border-gray-200 bg-white shadow-xl",
                    div { class: "flex items-center justify-between px-3 pt-3",
                        span { class: "text-sm font-semibold text-gray-900", {title} }
                        button {
                            class: "text-gray-500 hover:text-gray-600",
                            title: "{close_label}",
                            onclick: move |_| open.set(false),
                            icons::X { class: "h-4 w-4" }
                        }
                    }
                    div { class: "p-3 pb-2",
                        input {
                            class: "{INPUT} w-full",
                            placeholder: "{search_placeholder}",
                            value: "{search}",
                            autofocus: true,
                            oninput: move |e: Event<FormData>| search.set(e.value()),
                            onkeydown: move |e: KeyboardEvent| {
                                if e.key() == Key::Escape {
                                    open.set(false);
                                }
                            },
                        }
                    }
                    div { class: "max-h-96 overflow-y-auto px-2 pb-2",
                        for (header, item) in rows.into_iter() {
                            div { key: "{item.key}",
                                if let Some(header) = header {
                                    div { class: "px-2 pb-1 pt-3 text-xs font-semibold text-gray-500",
                                        {header}
                                    }
                                }
                                button {
                                    class: "flex w-full items-center gap-3 rounded-lg px-2 py-2 text-left hover:bg-gray-100",
                                    onclick: move |_| {
                                        on_pick.call(item.key.clone());
                                        open.set(false);
                                    },
                                    {
                                        match (item.icon, item.tile_class.as_deref()) {
                                            (Some(icon), Some(tile)) => rsx! {
                                                span { class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg {tile}",
                                                    IconView { icon }
                                                }
                                            },
                                            (Some(icon), None) => rsx! {
                                                span { class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-signal-50 text-signal-600",
                                                    IconView { icon }
                                                }
                                            },
                                            (None, _) => rsx! {},
                                        }
                                    }
                                    div { class: "min-w-0",
                                        div { class: "truncate text-sm font-medium text-gray-900",
                                            {item.label.clone()}
                                        }
                                        if let Some(description) = &item.description {
                                            div { class: "truncate text-xs text-gray-500",
                                                {description.clone()}
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if filtered.is_empty() {
                            p { class: "px-2 py-4 text-center text-sm text-gray-500",
                                {t!("eshell-no-result")}
                            }
                        }
                    }
                    if let Some(f) = footer {
                        div { class: "max-h-56 overflow-y-auto border-t border-gray-200 px-3 py-2",
                            {f}
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

    #[test]
    fn headers_only_on_group_changes() {
        let items = vec![
            PaletteItem::new("a", "A").with_group("G1"),
            PaletteItem::new("b", "B").with_group("G1"),
            PaletteItem::new("c", "C").with_group("G2"),
        ];
        let headers: Vec<Option<String>> =
            with_headers(&items).into_iter().map(|(h, _)| h).collect();
        assert_eq!(headers, vec![Some("G1".into()), None, Some("G2".into())]);
    }

    #[test]
    fn ungrouped_items_get_no_header() {
        let items = vec![PaletteItem::new("a", "A"), PaletteItem::new("b", "B")];
        assert!(with_headers(&items).iter().all(|(h, _)| h.is_none()));
    }
}
