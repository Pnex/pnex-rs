use super::helpers::*;
use super::*;

use pnex_core::web_visit::WebVisitConfig;

// ─────────────── website visit (web-presence.md D364) ───────────────

/// Website visit inspector: whether the legal notice is read too, and
/// whether the page is captured.
#[component]
pub(super) fn WebVisitForm(mut cx: EditorCx, initial: WebVisitConfig, can_write: bool) -> Element {
    let mut config = use_signal(move || initial.clone());
    let current = config();
    let mut apply = move |next: WebVisitConfig| {
        config.set(next.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::WebVisit { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let on_legal = move |e: Event<FormData>| {
        apply(WebVisitConfig {
            legal_page: e.checked(),
            ..config()
        })
    };
    let on_shot = move |e: Event<FormData>| {
        apply(WebVisitConfig {
            screenshot: e.checked(),
            ..config()
        })
    };
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-web-visit-help")} }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: current.legal_page,
                    onchange: on_legal,
                }
                {t!("flows-web-visit-legal")}
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: current.screenshot,
                    onchange: on_shot,
                }
                {t!("flows-web-visit-screenshot")}
            }
        }
    }
}
