use super::helpers::*;
use super::*;

use pnex_core::site_crawl::SiteCrawlConfig;

// ─────────────── site crawl (network-check.md D407) ───────────────

/// Site crawl inspector: page cap, proofs, changed pages only. The page
/// record and excluded paths are kept as they are (set by a pack flow).
#[component]
pub(super) fn SiteCrawlForm(
    mut cx: EditorCx,
    initial: SiteCrawlConfig,
    can_write: bool,
) -> Element {
    let mut config = use_signal(move || initial.clone());
    let current = config();
    let mut apply = move |next: SiteCrawlConfig| {
        config.set(next.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::SiteCrawl { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let on_pages = move |e: Event<FormData>| {
        if let Ok(n) = e.value().trim().parse::<u32>() {
            apply(SiteCrawlConfig {
                max_pages: n.clamp(1, pnex_core::browser_agent::CRAWL_PAGES_MAX),
                ..config()
            })
        }
    };
    let on_proofs = move |e: Event<FormData>| {
        apply(SiteCrawlConfig {
            proofs: e.checked(),
            ..config()
        })
    };
    let on_changed = move |e: Event<FormData>| {
        apply(SiteCrawlConfig {
            only_changed: e.checked(),
            ..config()
        })
    };
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-site-crawl-help")} }
            label { class: "block text-sm text-gray-700",
                {t!("flows-site-crawl-max-pages")}
                input {
                    r#type: "number",
                    class: "mt-1 w-full rounded border border-gray-300 px-2 py-1 text-sm",
                    min: "1",
                    max: "2000",
                    disabled: !can_write,
                    value: "{current.max_pages}",
                    onchange: on_pages,
                }
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: current.proofs,
                    onchange: on_proofs,
                }
                {t!("flows-site-crawl-proofs")}
            }
            label { class: "flex items-center gap-2 text-sm text-gray-700",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: current.only_changed,
                    onchange: on_changed,
                }
                {t!("flows-site-crawl-only-changed")}
            }
        }
    }
}
