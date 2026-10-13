use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::pages::{PageWriteConfig, PageWriteMode};

// ─────────────── page-write (media-vision-studio.md D209) ───────────────

fn patch_page(cx: &mut EditorCx, f: impl FnOnce(&mut PageWriteConfig) + 'static) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::PageWrite { config } = &mut node.kind {
            f(config);
        }
    });
}

/// Page writer inspector: new page per message (title template) or append
/// to a page of the org, Markdown path and version label.
#[component]
pub(super) fn PageWriteForm(
    mut cx: EditorCx,
    initial: PageWriteConfig,
    can_write: bool,
) -> Element {
    let pages = use_resource(|| async { api::docs::list().await.unwrap_or_default() });
    let mut mode = use_signal(move || initial.mode);
    let list = pages.read().clone().unwrap_or_default();
    let picked = initial.page_id.clone();
    let appending = mode() == PageWriteMode::Append;
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-page-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-page-mode")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = if event.value() == "append" {
                            PageWriteMode::Append
                        } else {
                            PageWriteMode::NewPage
                        };
                        mode.set(next);
                        patch_page(&mut cx, move |c| c.mode = next);
                    },
                    option { value: "new_page", selected: !appending, {t!("flows-page-mode-new")} }
                    option { value: "append", selected: appending, {t!("flows-page-mode-append")} }
                }
            }
            if appending {
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("flows-page-target")} }
                    select {
                        class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                        disabled: !can_write,
                        onchange: move |event| {
                            let id = event.value();
                            patch_page(&mut cx, move |c| c.page_id = id);
                        },
                        option { value: "", selected: picked.is_empty(), "—" }
                        for d in list {
                            option {
                                key: "{d.id}",
                                value: "{d.id}",
                                selected: d.id.to_string() == picked,
                                "{d.title}"
                            }
                        }
                    }
                }
            } else {
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("flows-page-title")} }
                    input {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                        r#type: "text",
                        maxlength: "255",
                        value: "{initial.title}",
                        placeholder: t!("flows-page-title-placeholder").to_string(),
                        disabled: !can_write,
                        oninput: move |event| {
                            let next = event.value();
                            patch_page(&mut cx, move |c| c.title = next);
                        },
                    }
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-page-text-field")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                    r#type: "text",
                    value: "{initial.text_field}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let next = event.value();
                        patch_page(&mut cx, move |c| c.text_field = next);
                    },
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-page-version-label")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                    r#type: "text",
                    maxlength: "255",
                    value: "{initial.version_label}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let next = event.value();
                        patch_page(&mut cx, move |c| c.version_label = next);
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-page-placeholders")} }
            }
            p { class: "text-xs text-gray-500", {t!("flows-page-review")} }
        }
    }
}
