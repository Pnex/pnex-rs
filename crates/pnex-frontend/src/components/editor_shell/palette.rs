//! Palette à la demande (principe 3) — un bouton `+` flottant ouvrant un
//! popover avec recherche. Remplace les panneaux permanents de gauche des
//! trois éditeurs. **Sans backdrop** : le drag d'un modèle du pied de
//! popover vers le canvas (dashboard, D41) doit voir ses `pointermove/up`
//! atteindre le canvas — la fermeture passe par un pick, la croix, Échap
//! dans la recherche ou un re-clic sur `+`.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;

/// Icônes de la palette — énum plate (data), rendue par `IconView` via le
/// jeu `icons.rs`. Évite tout `Element`/closure dans les items (props
/// `PartialEq` triviales) tout en donnant aux types un repère visuel.
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
}

/// Rendu d'une `PaletteIcon` (match explicite — jamais d'icône construite).
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
            }
        }
    }
}

/// Entrée de palette — data plate : clé de pick, libellés i18n résolus par
/// l'appelant, icône + classes de tuile (littéraux complets, cf. scan
/// Tailwind).
#[derive(Clone, PartialEq, Debug)]
pub struct PaletteItem {
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub icon: Option<PaletteIcon>,
    /// Classes Tailwind de la tuile d'icône (fond/texte de tonalité du type,
    /// ex. « bg-emerald-50 text-emerald-600 ») — littéral complet.
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

/// Bouton `+` flottant + popover (recherche + liste + pied optionnel).
/// Positionné par le slot parent (`absolute left-4 top-4` dans le corps de
/// `EditorShell`) — la carte s'ancre sur ce wrapper.
#[component]
pub fn PalettePopover(
    /// Infobulle/aria du bouton « + » (« Ajouter un nœud »…).
    add_title: String,
    /// En-tête du popover (« Nœuds », « Bibliothèque »…).
    title: String,
    /// Placeholder de la recherche.
    search_placeholder: String,
    /// Types proposés.
    items: Vec<PaletteItem>,
    /// Pick d'un item (clé) — le popover se ferme.
    on_pick: Callback<String>,
    /// Pied du popover (dashboard : modèles serveur draggables).
    #[props(default)]
    footer: Option<Element>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut search = use_signal(String::new);
    let close_label = t!("eshell-close");

    // Filtre mémoire : label ou description contient la requête (insensible
    // à la casse). Calculé au rendu — le composant se re-render sur `search`.
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
        div { // wrapper ancré par le slot parent


            button {
                class: "flex h-10 w-10 items-center justify-center rounded-full bg-blue-600 text-white shadow-lg transition-colors hover:bg-blue-700",
                title: "{add_title}",
                "aria-label": "{add_title}",
                onclick: move |_| {
                    open.toggle();
                    search.set(String::new());
                },
                if open() {
                    icons::X { class: "h-5 w-5" }
                } else {
                    icons::Plus { class: "h-5 w-5" }
                }
            }
            if open() {
                div { class: "absolute left-0 top-12 z-30 flex w-72 flex-col overflow-hidden rounded-xl border border-gray-200 bg-white shadow-xl",
                    div { class: "flex items-center justify-between px-3 pt-3",
                        span { class: "text-sm font-semibold text-gray-900", {title} }
                        button {
                            class: "text-gray-400 hover:text-gray-600",
                            title: "{close_label}",
                            onclick: move |_| open.set(false),
                            icons::X { class: "h-4 w-4" }
                        }
                    }
                    div { class: "p-3 pb-2",
                        input {
                            class: "w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500",
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
                                    div { class: "px-2 pb-1 pt-3 text-[11px] font-semibold uppercase tracking-wide text-gray-400",
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
                                                span { class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-blue-50 text-blue-600",
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
                            p { class: "px-2 py-4 text-center text-sm text-gray-400",
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
