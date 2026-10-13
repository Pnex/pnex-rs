use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::pages::{PagePdfConfig, PagePdfSource, PageWriteConfig, PageWriteMode};

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

fn patch_pdf(cx: &mut EditorCx, f: impl FnOnce(&mut PagePdfConfig) + 'static) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::PagePdf { config } = &mut node.kind {
            f(config);
        }
    });
}

/// Page PDF inspector (D209, D210): which version, approval, language.
#[component]
pub(super) fn PagePdfForm(mut cx: EditorCx, initial: PagePdfConfig, can_write: bool) -> Element {
    let pages = use_resource(|| async { api::docs::list().await.unwrap_or_default() });
    let mut source = use_signal(move || initial.source);
    let mut approval = use_signal(move || initial.require_approval);
    let list = pages.read().clone().unwrap_or_default();
    let picked = initial.page_id.clone();
    let latest = source() == PagePdfSource::LatestSendable;
    let lang = initial.language.clone();
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-pdf-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-pdf-source")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = if event.value() == "latest_sendable" {
                            PagePdfSource::LatestSendable
                        } else {
                            PagePdfSource::Message
                        };
                        source.set(next);
                        patch_pdf(&mut cx, move |c| c.source = next);
                    },
                    option { value: "message", selected: !latest, {t!("flows-pdf-source-message")} }
                    option { value: "latest_sendable", selected: latest,
                        {t!("flows-pdf-source-latest")}
                    }
                }
            }
            if latest {
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("flows-page-target")} }
                    select {
                        class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                        disabled: !can_write,
                        onchange: move |event| {
                            let id = event.value();
                            patch_pdf(&mut cx, move |c| c.page_id = id);
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
            }
            label { class: "flex items-start gap-2 text-sm text-gray-700 select-none",
                input {
                    class: "h-4 w-4 mt-0.5 accent-signal-600",
                    r#type: "checkbox",
                    checked: approval(),
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = event.checked();
                        approval.set(next);
                        patch_pdf(&mut cx, move |c| c.require_approval = next);
                    },
                }
                span {
                    span { class: "block", {t!("flows-pdf-approval")} }
                    span { class: "text-xs text-gray-500 block", {t!("flows-pdf-approval-hint")} }
                }
            }
            if !approval() {
                p { class: "text-xs text-amber-700", {t!("flows-pdf-no-approval-warning")} }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-pdf-language")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = event.value();
                        patch_pdf(&mut cx, move |c| c.language = next);
                    },
                    option { value: "", selected: lang.is_empty() || lang == "en",
                        {t!("flows-pdf-language-en")}
                    }
                    option { value: "fr", selected: lang == "fr", {t!("flows-pdf-language-fr")} }
                }
            }
        }
    }
}
