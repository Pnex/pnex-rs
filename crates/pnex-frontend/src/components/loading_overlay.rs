//! Light, reusable loading overlay — the minimal variant of the login
//! screen animation (no animated lines: a translucent veil + a spinner +
//! a short message). Place it in a `relative` parent to cover a card/area;
//! meant to serve on every page (operations waiting on the server: flow
//! delete, deployments, builds…).

use crate::components::gerbe::Gerbe;
use dioxus::prelude::*;

/// Translucent veil + centred spinner, optional message. Full literal
/// classes (Tailwind scan) — opacity in v4 slash syntax.
#[component]
pub fn LoadingOverlay(
    /// Message shown next to the spinner (empty = spinner only).
    #[props(default)]
    message: String,
) -> Element {
    rsx! {
        div { class: "absolute inset-0 z-40 flex items-center justify-center bg-white/60 backdrop-blur-[2px] rounded-lg",
            div { class: "flex flex-col items-center gap-2",
                Gerbe { size: 36, delayed: Some(false) }
                if !message.is_empty() {
                    span { class: "text-sm text-gray-600", {message} }
                }
            }
        }
    }
}
