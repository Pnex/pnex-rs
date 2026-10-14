//! Route inconnue — renvoyée par le routeur (`/:..route`).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::app::Route;
use crate::components::ui::BTN_PRIMARY;

/// Unknown route, rendered inside the shell as an empty state (O57).
#[component]
pub fn NotFound(route: Vec<String>) -> Element {
    rsx! {
        div { class: "mx-auto mt-12 flex max-w-prose flex-col items-center gap-2 rounded-lg border border-dashed border-line-strong px-6 py-12 text-center",
            h1 { class: "text-xl font-semibold text-ink", {t!("not-found-title")} }
            p { class: "break-all text-sm text-slate",
                {t!("not-found-hint", path : route.join("/"))}
            }
            Link { to: Route::Dashboard {}, class: "{BTN_PRIMARY} mt-4", {t!("nav-dashboard")} }
        }
    }
}
