//! Toast container, top right corner: slides in and out through the right
//! edge (ported from the React `ToastContainer.tsx`).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;
use crate::state::toasts::{ToastKind, ToastMessage, TOASTS};

#[component]
pub fn ToastContainer() -> Element {
    rsx! {
        div { class: "fixed top-4 right-4 z-50 space-y-2",
            for toast in TOASTS.cloned() {
                ToastCard {
                    key: "{toast.id}",
                    id: toast.id,
                    kind: toast.kind,
                    message: toast.message,
                }
            }
        }
    }
}

/// One toast. Owns its auto-dismiss timer: the task lives in this card's
/// scope (mounted by the always-present container), so it survives the
/// unmount of the component that raised the toast — before, a toast raised
/// by a closing modal was never dismissed.
#[component]
fn ToastCard(id: u64, kind: ToastKind, message: ToastMessage) -> Element {
    let mut leaving = use_signal(|| false);
    // Exit leaves through the edge it came in from, then the card unmounts
    // (200 ms = `.pnex-toast-leaving`).
    let mut close = move || {
        if leaving() {
            return;
        }
        leaving.set(true);
        spawn(async move {
            crate::util::sleep(std::time::Duration::from_millis(200)).await;
            crate::state::toasts::dismiss(id);
        });
    };
    use_future(move || async move {
        crate::util::sleep(std::time::Duration::from_secs(
            crate::state::toasts::AUTO_DISMISS_SECS,
        ))
        .await;
        close();
    });
    // Conditional classes = full literals (Tailwind scan).
    let (colors, icon) = match kind {
        ToastKind::Success => (
            "bg-green-50 border-green-200",
            rsx! {
                icons::CheckCircle { class: "h-5 w-5 text-green-500" }
            },
        ),
        ToastKind::Error => (
            "bg-red-50 border-red-200",
            rsx! {
                icons::AlertTriangle { class: "h-5 w-5 text-red-500" }
            },
        ),
        ToastKind::Info => (
            "bg-signal-50 border-signal-200",
            rsx! {
                icons::Info { class: "h-5 w-5 text-signal-500" }
            },
        ),
    };
    let motion = if leaving() { "pnex-toast-leaving" } else { "" };
    rsx! {
        div { class: "pnex-toast {motion} border rounded-lg shadow-lg p-4 min-w-[320px] max-w-md flex items-start {colors}",
            {icon}
            p { class: "ml-3 flex-1 text-sm text-gray-800 break-words",
                {
                    match &message {
                        ToastMessage::Api(err) => crate::api::error_i18n::localize(err),
                        ToastMessage::Text(text) => text.clone(),
                        ToastMessage::Key(key) => t!(* key),
                    }
                }
            }
            button {
                class: "ml-4 flex-shrink-0 text-gray-500 hover:text-gray-600 transition-colors",
                onclick: move |_| close(),
                icons::X { class: "h-4 w-4" }
            }
        }
    }
}
