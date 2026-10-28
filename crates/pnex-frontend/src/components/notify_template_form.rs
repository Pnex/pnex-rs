//! Notification template editor (D52) — name, optional subject,
//! body, declared vars `{name, example}` (addable rows), **Preview**
//! button (preview API, render only) and **Test send** (chosen channel,
//! rendered template — single pnex-notify path).
//!
//! Preview and Test also work on an unsaved draft: the
//! draft is created server-side on the first click (dated name generated when
//! the name field is empty, default-names-of-new-items pattern) and
//! the editor switches to update mode.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{NotifyTemplate, NotifyTemplateInput};

use crate::api;
use crate::api::error::ApiError;
use crate::components::ui::BTN_PRIMARY;
use crate::components::ui::{FIELD_LABEL, INPUT};
use crate::state::toasts;

/// Editable var rows `(name, example)`.
type VarRows = Vec<(String, String)>;

#[component]
pub fn NotifyTemplateForm(
    existing: Option<NotifyTemplate>,
    on_close: Callback<()>,
    on_changed: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let is_edit = existing.is_some();
    // Template id as known by the server: the existing one's, or
    // the draft's created on the first Preview/Test (auto-save).
    let saved_id = use_signal(|| existing.as_ref().map(|c| c.id.to_string()));

    let mut name = use_signal(|| {
        existing
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default()
    });
    let mut subject = use_signal(|| {
        existing
            .as_ref()
            .and_then(|c| c.subject.clone())
            .unwrap_or_default()
    });
    let mut body = use_signal(|| {
        existing
            .as_ref()
            .map(|c| c.body.clone())
            .unwrap_or_default()
    });
    let mut vars = use_signal::<VarRows>(|| {
        existing
            .as_ref()
            .map(|c| {
                c.vars
                    .iter()
                    .map(|v| (v.name.clone(), v.example.clone()))
                    .collect()
            })
            .unwrap_or_default()
    });

    let mut preview = use_signal(|| None::<pnex_core::PreviewResult>);
    // Errors are kept as `ApiError` and localized at render time (machine
    // code → `err-<kebab>`, e.g. `notify-template-render`).
    let mut preview_error = use_signal(|| None::<ApiError>);
    let mut body_error = use_signal(|| None::<ApiError>);
    let mut busy = use_signal(|| false);
    // Test send: channel selection (None or empty = not started).
    let mut test_target = use_signal(|| None::<String>);
    let test_channels = use_resource(move || async move { api::notify::list_channels().await });

    // Preview (render only): auto-saves the draft if needed.
    let do_preview = move |_| {
        if busy() {
            return;
        }
        preview_error.set(None);
        busy.set(true);
        let vars_map = collect_vars(vars.cloned());
        spawn(async move {
            let template_id =
                match ensure_saved(saved_id, name, subject, body, vars, on_changed).await {
                    Ok(id) => id,
                    Err(err) => {
                        preview_error.set(Some(err));
                        busy.set(false);
                        return;
                    }
                };
            match api::notify::preview_template(&template_id, vars_map, serde_json::json!({})).await
            {
                Ok(result) => preview.set(Some(result)),
                Err(err) => preview_error.set(Some(err)),
            }
            busy.set(false);
        });
    };

    let do_save = move |_| {
        if busy() {
            return;
        }
        let name_value = name.cloned();
        let name_value = name_value.trim().to_string();
        if name_value.is_empty() {
            body_error.set(Some(ApiError::new(
                t!("notify-error-name-required").to_string(),
            )));
            return;
        }
        busy.set(true);
        body_error.set(None);
        let current_id = saved_id.cloned();
        let input = NotifyTemplateInput {
            name: name_value,
            subject: {
                let s = subject.cloned();
                let s = s.trim().to_string();
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            },
            body: body.cloned(),
            vars: api::notify::vars_from_rows(vars.cloned()),
        };
        spawn(async move {
            let result = match current_id.as_deref() {
                None => api::notify::create_template(input).await,
                Some(id) => api::notify::update_template(id, input).await,
            };
            match result {
                Ok(_) => {
                    toasts::success("toast-notify-template-saved");
                    on_saved.call(());
                }
                Err(err) => body_error.set(Some(err)),
            }
            busy.set(false);
        });
    };

    // Inserting a variable: auto name `tpl_N` (incremented), declared
    // row added, `{{ tpl_N }}` tag inserted at the cursor position
    // in the body (via the DOM, document::eval pattern).
    let insert_variable = move |_| {
        let var_name = {
            let rows = vars.read();
            next_var_name(&rows)
        };
        vars.with_mut(|rows| rows.push((var_name.clone(), String::new())));
        let tag = format!("{{{{ {var_name} }}}}");
        let tag_json = serde_json::to_string(&tag).unwrap_or_default();
        let js = format!(
            r#"(() => {{
  const el = document.getElementById("notify-template-body-input");
  if (!el) {{ return null; }}
  const pos = typeof el.selectionStart === "number" ? el.selectionStart : el.value.length;
  el.focus();
  el.setRangeText({tag_json}, pos, pos, "end");
  return el.value;
}})()"#
        );
        spawn(async move {
            let inserted = match dioxus::document::eval(&js).await {
                Ok(value) => value.as_str().map(str::to_string),
                Err(_) => None,
            };
            match inserted {
                Some(new_body) => body.set(new_body),
                None => {
                    // DOM unavailable (edge case): append at the end.
                    let current = body.cloned();
                    body.set(format!("{current}{tag}"));
                }
            }
        });
    };

    // Test send on the chosen channel: auto-saves the draft then
    // sends the rendered template (not the channels' built-in message).
    let do_test = move |_| {
        let Some(channel_id) = test_target.cloned() else {
            return;
        };
        if channel_id.is_empty() || busy() {
            return;
        }
        busy.set(true);
        let vars_map = collect_vars(vars.cloned());
        spawn(async move {
            match ensure_saved(saved_id, name, subject, body, vars, on_changed).await {
                Err(err) => toasts::error(err),
                Ok(template_id) => {
                    match api::notify::test_channel(&channel_id, Some(&template_id), vars_map).await
                    {
                        Ok(()) => toasts::success("toast-notify-test-sent"),
                        Err(err) => toasts::error(err),
                    }
                }
            }
            busy.set(false);
        });
    };

    let saved = saved_id.cloned().is_some();

    rsx! {
        crate::components::modal::Modal {
            title: if is_edit || saved { t!("notify-edit-template-title").to_string() } else { t!("notify-new-template-title").to_string() },
            max_width: "max-w-2xl".to_string(),
            on_close,
            div { class: "space-y-4",
                div {
                    label {
                        r#for: "notify-template-form-field-1",
                        class: FIELD_LABEL,
                        {t!("notify-field-template-name")}
                    }
                    input {
                        id: "notify-template-form-field-1",
                        class: "{INPUT} w-full",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                    if saved && !is_edit {
                        p { class: "mt-1 text-xs text-amber-700",
                            {t!("notify-template-autosaved-note")}
                        }
                    }
                }
                div {
                    label {
                        r#for: "notify-template-subject-input",
                        class: FIELD_LABEL,
                        {t!("notify-template-subject")}
                    }
                    crate::components::code_highlight::TemplateEditor {
                        multiline: false,
                        id: "notify-template-subject-input".to_string(),
                        placeholder: t!("notify-template-subject-placeholder").to_string(),
                        value: "{subject}",
                        oninput: move |e: String| subject.set(e),
                    }
                }
                div {
                    div { class: "flex items-center justify-between",
                        label {
                            r#for: "notify-template-body-input",
                            class: FIELD_LABEL,
                            {t!("notify-template-body")}
                            span { class: "text-red-500 ml-1", "*" }
                        }
                        button {
                            r#type: "button",
                            class: "text-sm text-signal-600 hover:text-signal-700",
                            onclick: insert_variable,
                            {t!("notify-vars-insert")}
                        }
                    }
                    crate::components::code_highlight::TemplateEditor {
                        multiline: true,
                        id: "notify-template-body-input".to_string(),
                        height_class: "h-28".to_string(),
                        placeholder: t!("notify-template-body-placeholder").to_string(),
                        value: "{body}",
                        oninput: move |e: String| body.set(e),
                    }
                }

                // Declared vars ({name, example} rows).
                div { class: "space-y-2",
                    div { class: "flex items-center justify-between",
                        p {
                            id: "notify-template-vars-label",
                            class: "block text-sm font-medium text-gray-700",
                            {t!("notify-template-vars")}
                        }
                        button {
                            class: "text-sm text-signal-600 hover:text-signal-700",
                            onclick: move |_| vars.with_mut(|rows| rows.push((String::new(), String::new()))),
                            {t!("notify-vars-add")}
                        }
                    }
                    p { class: "text-xs text-gray-500", {t!("notify-template-vars-hint")} }
                    for (i, (var_name, example)) in vars.read().clone().into_iter().enumerate() {
                        div {
                            class: "flex gap-2 items-center",
                            key: "{i}",
                            role: "group",
                            aria_labelledby: "notify-template-vars-label",
                            input {
                                class: "w-40 px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                                placeholder: t!("notify-vars-name-placeholder"),
                                value: "{var_name}",
                                oninput: move |e| vars.with_mut(|rows| rows[i].0 = e.value()),
                            }
                            input {
                                class: "flex-1 px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                                placeholder: t!("notify-vars-example-placeholder"),
                                value: "{example}",
                                oninput: move |e| vars.with_mut(|rows| rows[i].1 = e.value()),
                            }
                            button {
                                class: "text-red-500 hover:text-red-700 text-sm",
                                onclick: move |_| {
                                    vars
                                        .with_mut(|rows| {
                                        rows.remove(i);
                                    })
                                },
                                "✕"
                            }
                        }
                    }
                }

                if let Some(err) = body_error() {
                    div { class: "rounded-lg bg-red-50 border border-red-200 p-3 text-sm text-red-700",
                        {api::error_i18n::localize(&err)}
                    }
                }

                // Preview (render only).
                div { class: "space-y-2",
                    button {
                        class: "px-3 py-2 text-sm border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-50",
                        disabled: busy(),
                        onclick: do_preview,
                        {t!("notify-preview")}
                    }
                    if let Some(err) = preview_error() {
                        p { class: "text-sm text-amber-700", {api::error_i18n::localize(&err)} }
                    }
                    if let Some(p) = &*preview.read() {
                        div { class: "rounded-lg bg-gray-50 border border-gray-200 p-3 text-sm space-y-1",
                            if let Some(s) = &p.subject {
                                p { class: "font-medium text-gray-900", {s.clone()} }
                            }
                            p { class: "text-gray-700 whitespace-pre-wrap", {p.body.clone()} }
                        }
                    }
                }

                // Test send (chosen channel) — also available on a
                // draft (auto-saved on click).
                div { class: "border-t border-gray-200 pt-3 space-y-2",
                    label {
                        r#for: "notify-template-test-channel",
                        class: FIELD_LABEL,
                        {t!("notify-test-send-title")}
                    }
                    match &*test_channels.value().read() {
                        Some(Ok(channels)) if !channels.is_empty() => rsx! {
                            div { class: "flex gap-2",
                                select {
                                    id: "notify-template-test-channel",
                                    class: "{INPUT} flex-1",
                                    onchange: move |e| test_target.set(Some(e.value())),
                                    option {
                                        value: "",
                                        selected: test_target.cloned().map(|v| v.is_empty()).unwrap_or(true),
                                        {t!("notify-test-pick-channel")}
                                    }
                                    for ch in channels.clone() {
                                        option {
                                            value: "{ch.id}",
                                            selected: test_target.cloned() == Some(ch.id.to_string()),
                                            "{ch.name} ({ch.kind})"
                                        }
                                    }
                                }
                                button {
                                    class: "px-3 py-2 text-sm border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-50",
                                    disabled: busy() || test_target.cloned().map(|v| v.is_empty()).unwrap_or(true),
                                    onclick: do_test,
                                    {t!("notify-test")}
                                }
                            }
                        },
                        Some(Ok(_)) | Some(Err(_)) | None => rsx! {
                            p { class: "text-sm text-gray-500", {t!("notify-test-no-channels")} }
                        },
                    }
                }

                div { class: crate::components::modal::MODAL_FOOTER,
                    button {
                        class: BTN_PRIMARY,
                        disabled: busy(),
                        onclick: do_save,
                        // Stable label: a Preview/Test auto-saves the draft (O49).
                        {t!("common-save")}
                    }
                }
            }
        }
    }
}

/// First Preview/Test on a draft: the render and send APIs
/// work on a saved row — the draft is created server-side
/// (dated name when the field is empty, `-N` suffix on conflict),
/// the generated name is copied into the field and the editor switches to
/// update mode. `on_changed` refreshes the list behind the modal.
async fn ensure_saved(
    mut saved_id: Signal<Option<String>>,
    mut name: Signal<String>,
    subject: Signal<String>,
    body: Signal<String>,
    vars: Signal<VarRows>,
    on_changed: Callback<()>,
) -> Result<String, ApiError> {
    if let Some(id) = saved_id.cloned() {
        return Ok(id);
    }
    let base = {
        let current = name.cloned();
        let trimmed = current.trim();
        // Datetime name — data, not UI text: language-neutral, second
        // precision keeps same-minute collisions unlikely (and the `-N`
        // retry below covers the rest).
        if trimmed.is_empty() {
            format!("template {}", crate::util::now_label_secs())
        } else {
            trimmed.to_string()
        }
    };
    let build_input = |template_name: String| NotifyTemplateInput {
        name: template_name,
        subject: {
            let s = subject.cloned();
            let s = s.trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        },
        body: body.cloned(),
        vars: api::notify::vars_from_rows(vars.cloned()),
    };
    let mut last_conflict: Option<ApiError> = None;
    for suffix in ["", "-2", "-3", "-4"] {
        let attempt = if suffix.is_empty() {
            base.clone()
        } else {
            format!("{base}{suffix}")
        };
        match api::notify::create_template(build_input(attempt.clone())).await {
            Ok(created) => {
                name.set(attempt);
                saved_id.set(Some(created.id.to_string()));
                on_changed.call(());
                return Ok(created.id.to_string());
            }
            Err(err) if err.code.as_deref() == Some("notify-template-name-conflict") => {
                last_conflict = Some(err);
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_conflict.expect("at least one name conflict recorded in the retry loop"))
}

/// Next auto variable name: `tpl_1`, `tpl_2`, … — the highest declared
/// `tpl_N` + 1, skipping names already taken (manual rows may occupy any
/// slot).
fn next_var_name(rows: &[(String, String)]) -> String {
    let mut n: u32 = rows
        .iter()
        .filter_map(|(name, _)| name.strip_prefix("tpl_")?.parse().ok())
        .max()
        .unwrap_or(0);
    loop {
        n += 1;
        let candidate = format!("tpl_{n}");
        if !rows.iter().any(|(name, _)| name == &candidate) {
            return candidate;
        }
    }
}

/// Editable rows → vars map (trim; empty names ignored).
fn collect_vars(rows: VarRows) -> std::collections::BTreeMap<String, String> {
    rows.into_iter()
        .filter(|(name, _)| !name.trim().is_empty())
        .map(|(name, example)| (name.trim().to_string(), example))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_var_name_increments_and_skips_taken() {
        assert_eq!(next_var_name(&[]), "tpl_1");
        let rows = vec![
            ("tpl_1".to_string(), String::new()),
            ("tpl_3".to_string(), String::new()),
        ];
        assert_eq!(next_var_name(&rows), "tpl_4");
        // A manual higher slot is honoured, gaps included.
        let rows = vec![("tpl_2".to_string(), String::new())];
        assert_eq!(next_var_name(&rows), "tpl_3");
        // Custom (non tpl_) names never collide with the auto sequence.
        let rows = vec![
            ("seuil".to_string(), String::new()),
            ("tpl_1".to_string(), String::new()),
        ];
        assert_eq!(next_var_name(&rows), "tpl_2");
    }

    #[test]
    fn collect_vars_trims_and_skips_empty_names() {
        let map = collect_vars(vec![
            (" seuil ".to_string(), "80".to_string()),
            ("  ".to_string(), "x".to_string()),
            ("device".to_string(), "{{ msg.device }}".to_string()),
        ]);
        assert_eq!(map.len(), 2);
        assert_eq!(map["seuil"], "80");
        assert_eq!(map["device"], "{{ msg.device }}");
    }
}
