//! Progressive-disclosure inspector (principle 4) — the configurator exists
//! only on click on an element: slides in from the right over the canvas,
//! closes with the cross or Escape. The form body stays owned by the editor
//! (`body: Element`); the shell provides the header (icon + name + ref +
//! cross) and the panel base.

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::palette::{IconView, PaletteIcon};
use crate::components::icons;

/// Inspector panel — `absolute inset-y-0 right-0` in the body of
/// `EditorShell`. Escape on the panel (or bubbling from a form field)
/// closes via `on_close`; deselecting through the canvas closes too
/// (None ⇒ no panel).
#[component]
pub fn InspectorPanel(
    /// Icon of the edited type (header tile).
    #[props(default)]
    icon: Option<PaletteIcon>,
    /// Element name (or the type label if the name is empty).
    title: String,
    /// Secondary reference ("#N1").
    #[props(default)]
    subtitle: Option<String>,
    /// Close (cross / Escape) — the editor deselects.
    on_close: Callback<()>,
    /// Form body, provided by the editor.
    body: Element,
) -> Element {
    let close_label = t!("eshell-close");
    // Phones: full width (a 320 px strip next to 70 px of canvas is of no
    // use); the close button brings the canvas back.
    rsx! {
        aside {
            class: "absolute inset-y-0 right-0 z-30 flex h-full w-full sm:w-80 flex-col border-l border-gray-200 bg-white shadow-xl",
            tabindex: "0",
            onkeydown: move |e: KeyboardEvent| {
                if e.key() == Key::Escape {
                    on_close.call(());
                }
            },
            div { class: "flex items-center gap-2.5 border-b border-gray-200 px-4 py-3",
                {
                    match icon {
                        Some(icon) => rsx! {
                            span { class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-signal-50 text-signal-600",
                                IconView { icon, class: "h-4 w-4" }
                            }
                        },
                        None => rsx! {},
                    }
                }
                div { class: "min-w-0",
                    div { class: "truncate text-sm font-semibold text-gray-900", {title} }
                    if let Some(sub) = subtitle {
                        div { class: "text-xs text-gray-500", {sub} }
                    }
                }
                div { class: "flex-1" }
                button {
                    class: "shrink-0 rounded-lg p-1.5 text-gray-500 hover:bg-gray-100 hover:text-gray-600",
                    title: "{close_label}",
                    onclick: move |_| on_close.call(()),
                    icons::X { class: "h-4 w-4" }
                }
            }
            div { class: "flex-1 space-y-3 overflow-y-auto p-4", {body} }
        }
    }
}
