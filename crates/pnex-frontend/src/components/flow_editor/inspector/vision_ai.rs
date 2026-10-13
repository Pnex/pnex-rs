use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::vision_ai::{
    LlmTextConfig, MediaFuseConfig, VisionCaptionConfig, CAPTION_PROMPT_MAX,
    DEFAULT_CAPTION_PROMPT, LLM_OUTPUT_CHARS, LLM_PROMPT_MAX,
};

// ─────────────── vision-caption / media-fuse (D195, D196) ───────────────

/// Description languages offered by the fusion form (`""` = automatic).
const FUSE_LANGUAGES: [&str; 8] = ["", "fr", "en", "es", "de", "it", "pt", "nl"];

fn patch_caption(cx: &mut EditorCx, f: impl FnOnce(&mut VisionCaptionConfig) + 'static) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::VisionCaption { config } = &mut node.kind {
            f(config);
        }
    });
}

fn patch_fuse(cx: &mut EditorCx, f: impl FnOnce(&mut MediaFuseConfig) + 'static) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::MediaFuse { config } = &mut node.kind {
            f(config);
        }
    });
}

/// Red frame on an invalid number field.
fn number_class(invalid: bool) -> &'static str {
    if invalid {
        "w-full px-2 py-1.5 border border-red-400 bg-red-50 rounded-lg text-sm"
    } else {
        "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm"
    }
}

/// Caption inspector: prompt, interval per camera, crop of the best
/// detection, and the reminder that the image goes to the org provider.
#[component]
pub(super) fn VisionCaptionForm(
    mut cx: EditorCx,
    initial: VisionCaptionConfig,
    can_write: bool,
) -> Element {
    let mut prompt = use_signal(|| initial.prompt.clone());
    let mut interval_raw = use_signal(|| v_to_string(initial.min_interval_secs));
    let interval_now = interval_raw();
    let interval_invalid = parse_secs(&interval_now).is_none_or(|v| !(1.0..=86_400.0).contains(&v));
    let prompt_len = prompt.read().chars().count();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-caption-help")} }
            p { class: "text-xs text-amber-700", {t!("flows-caption-privacy")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-caption-prompt")} }
                textarea {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                    rows: "3",
                    maxlength: "{CAPTION_PROMPT_MAX}",
                    placeholder: DEFAULT_CAPTION_PROMPT,
                    value: "{prompt}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let next = event.value();
                        prompt.set(next.clone());
                        patch_caption(&mut cx, move |c| c.prompt = next);
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block",
                    {t!("flows-caption-prompt-hint", count : prompt_len, max : CAPTION_PROMPT_MAX)}
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-caption-interval")} }
                input {
                    class: number_class(interval_invalid),
                    r#type: "number",
                    min: "1",
                    max: "86400",
                    step: "1",
                    value: "{interval_raw}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        interval_raw.set(raw.clone());
                        if let Some(v) = parse_secs(&raw).filter(|v| (1.0..=86_400.0).contains(v)) {
                            patch_caption(&mut cx, move |c| c.min_interval_secs = v);
                        }
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-caption-interval-hint")} }
            }
            label { class: "flex items-start gap-2 text-sm text-gray-700 select-none",
                input {
                    class: "h-4 w-4 mt-0.5 accent-signal-600",
                    r#type: "checkbox",
                    checked: initial.crop_best,
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = event.checked();
                        patch_caption(&mut cx, move |c| c.crop_best = next);
                    },
                }
                span {
                    span { class: "block", {t!("flows-caption-crop")} }
                    span { class: "text-xs text-gray-500 block", {t!("flows-caption-crop-hint")} }
                }
            }
        }
    }
}

/// Fusion inspector: window length and description language.
#[component]
pub(super) fn MediaFuseForm(
    mut cx: EditorCx,
    initial: MediaFuseConfig,
    can_write: bool,
) -> Element {
    let mut window_raw = use_signal(|| v_to_string(initial.window_secs));
    let window_now = window_raw();
    let window_invalid = parse_secs(&window_now).is_none_or(|v| !(5.0..=3600.0).contains(&v));
    let current = initial.language.clone();
    let custom = !FUSE_LANGUAGES.contains(&current.as_str());

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-fuse-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-fuse-window")} }
                input {
                    class: number_class(window_invalid),
                    r#type: "number",
                    min: "5",
                    max: "3600",
                    step: "5",
                    value: "{window_raw}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        window_raw.set(raw.clone());
                        if let Some(v) = parse_secs(&raw).filter(|v| (5.0..=3600.0).contains(v)) {
                            patch_fuse(&mut cx, move |c| c.window_secs = v);
                        }
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-fuse-window-hint")} }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-fuse-language")} }
                select {
                    class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = event.value();
                        patch_fuse(&mut cx, move |c| c.language = next);
                    },
                    for code in FUSE_LANGUAGES {
                        option {
                            key: "{code}",
                            value: "{code}",
                            selected: code == current,
                            if code.is_empty() {
                                {t!("flows-fuse-language-auto")}
                            } else {
                                "{code}"
                            }
                        }
                    }
                    if custom {
                        option { value: "{current}", selected: true, "{current}" }
                    }
                }
            }
            p { class: "text-xs text-gray-500", {t!("flows-fuse-trust")} }
        }
    }
}

fn patch_llm(cx: &mut EditorCx, f: impl FnOnce(&mut LlmTextConfig) + 'static) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::LlmText { config } = &mut node.kind {
            f(config);
        }
    });
}

/// `llm_text` inspector (D208): instruction, data path, language, length.
#[component]
pub(super) fn LlmTextForm(mut cx: EditorCx, initial: LlmTextConfig, can_write: bool) -> Element {
    let mut max_raw = use_signal(|| initial.max_chars.to_string());
    let max_now = max_raw();
    let (lo, hi) = LLM_OUTPUT_CHARS;
    let max_invalid = max_now
        .trim()
        .parse::<u32>()
        .map_or(true, |v| !(lo..=hi).contains(&v));
    let current = initial.language.clone();
    let custom = !FUSE_LANGUAGES.contains(&current.as_str());
    let prompt_max = LLM_PROMPT_MAX.to_string();
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-llm-help")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-llm-prompt")} }
                textarea {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                    rows: "6",
                    maxlength: "{prompt_max}",
                    disabled: !can_write,
                    placeholder: t!("flows-llm-prompt-placeholder").to_string(),
                    value: "{initial.prompt}",
                    oninput: move |event| {
                        let next = event.value();
                        patch_llm(&mut cx, move |c| c.prompt = next);
                    },
                }
            }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-llm-data-field")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                    r#type: "text",
                    value: "{initial.data_field}",
                    placeholder: "payload",
                    disabled: !can_write,
                    oninput: move |event| {
                        let next = event.value();
                        patch_llm(&mut cx, move |c| c.data_field = next);
                    },
                }
                span { class: "text-xs text-gray-500 mt-1 block", {t!("flows-llm-data-field-hint")} }
            }
            div { class: "grid grid-cols-2 gap-2",
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("flows-fuse-language")} }
                    select {
                        class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                        disabled: !can_write,
                        onchange: move |event| {
                            let next = event.value();
                            patch_llm(&mut cx, move |c| c.language = next);
                        },
                        for code in FUSE_LANGUAGES {
                            option {
                                key: "{code}",
                                value: "{code}",
                                selected: code == current,
                                if code.is_empty() {
                                    {t!("flows-fuse-language-auto")}
                                } else {
                                    "{code}"
                                }
                            }
                        }
                        if custom {
                            option { value: "{current}", selected: true, "{current}" }
                        }
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("flows-llm-max-chars")} }
                    input {
                        class: number_class(max_invalid),
                        r#type: "number",
                        min: "{lo}",
                        max: "{hi}",
                        step: "100",
                        value: "{max_raw}",
                        disabled: !can_write,
                        oninput: move |event| {
                            let raw = event.value();
                            max_raw.set(raw.clone());
                            if let Some(v) = raw.trim().parse::<u32>().ok().filter(|v| (lo..=hi).contains(v))
                            {
                                patch_llm(&mut cx, move |c| c.max_chars = v);
                            }
                        },
                    }
                }
            }
            p { class: "text-xs text-gray-500", {t!("flows-llm-trust")} }
        }
    }
}
