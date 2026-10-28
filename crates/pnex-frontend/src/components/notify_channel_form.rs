//! Notification channel form — rendered **from the backend**
//! (`GET /api/v1/notify/kinds` → `field_spec`, single source of truth,
//! zero front/back drift). `ai_connector_form.rs` school (prefill,
//! secret "empty = keep", toasts).
//!
//! Secrets (D54): never rendered — `secrets_set` shows "set" and the
//! field stays empty; submitting sends `null` for secrets not typed
//! (server-side merge = unchanged).
//!
//! Vault (secrets.md D113, lot S4): each `secret` field is a
//! [`SecretField`] — type a value (owner/admin, stored in the channel's
//! dedicated secret) or pick a secret of the org; a set field shows
//! "set · <secret name>".

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{FieldSpec, FieldType, NotifyChannel, NotifyKindInfo};

use crate::api;
use crate::components::secret_field::{can_manage_secrets, SecretDraft, SecretField};
use crate::components::ui::BTN_PRIMARY;
use crate::components::ui::{FIELD_LABEL, INPUT};
use crate::state::toasts;

/// Form fields: text/secret/number values by field id.
/// A single signal-map (no `use_signal` per field in a loop,
/// dioxus pitfall #2784).
type FieldValues = std::collections::HashMap<String, String>;

/// Vault state of each `secret` field, by field id.
type SecretDrafts = std::collections::HashMap<String, SecretDraft>;

/// JSON config assembled from the field_spec: secret → its vault input
/// (`null` = unchanged), number → f64, text → string.
fn build_config(
    field_spec: &[FieldSpec],
    values: &FieldValues,
    secrets: &SecretDrafts,
) -> serde_json::Value {
    let mut cfg = serde_json::Map::new();
    for f in field_spec {
        let raw = values.get(&f.id).cloned().unwrap_or_default();
        let v = match f.r#type {
            FieldType::Secret => secrets
                .get(&f.id)
                .map(SecretDraft::to_json)
                .unwrap_or(serde_json::Value::Null),
            FieldType::Number => raw
                .trim()
                .parse::<f64>()
                .map(|n| serde_json::json!(n))
                .unwrap_or(serde_json::Value::Null),
            FieldType::Bool => serde_json::json!(raw == "true"),
            _ => serde_json::json!(raw.trim()),
        };
        cfg.insert(f.id.clone(), v);
    }
    serde_json::Value::Object(cfg)
}

#[component]
pub fn NotifyChannelForm(
    kind_info: NotifyKindInfo,
    existing: Option<NotifyChannel>,
    on_close: Callback<()>,
    on_saved: Callback<()>,
) -> Element {
    let field_spec = kind_info.field_spec.clone();
    let kind = kind_info.kind.clone();
    let edit_id: Option<String> = existing.as_ref().map(|c| c.id.to_string());
    let is_edit = existing.is_some();
    let can_manage = can_manage_secrets();
    let secret_drafts = use_signal::<SecretDrafts>(|| {
        existing
            .as_ref()
            .map(|c| {
                c.secrets
                    .iter()
                    .map(|(field, view)| {
                        (field.clone(), SecretDraft::from_view(Some(view.clone())))
                    })
                    .collect()
            })
            .unwrap_or_default()
    });

    let mut name = use_signal(|| {
        existing
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default()
    });
    let mut enabled = use_signal(|| existing.as_ref().map(|c| c.enabled).unwrap_or(true));
    // Initial values = masked config (secrets are null/empty).
    let values = use_signal::<FieldValues>(|| {
        let mut map = FieldValues::new();
        if let Some(ch) = &existing {
            for f in &field_spec {
                let raw = ch.config.get(&f.id).cloned().unwrap_or_default();
                let v = raw.as_str().map(str::to_string).unwrap_or_default();
                map.insert(f.id.clone(), v);
            }
        }
        map
    });
    let mut test_error = use_signal(|| None::<String>);
    let mut testing = use_signal(|| false);
    let mut saving = use_signal(|| false);

    let field_spec_save = field_spec.clone();
    let kind_save = kind.clone();
    let edit_id_save = edit_id.clone();
    let do_save = move |_| {
        if saving() {
            return;
        }
        let name_value = name().trim().to_string();
        if name_value.is_empty() {
            test_error.set(Some(t!("notify-error-name-required").to_string()));
            return;
        }
        saving.set(true);
        let config = build_config(
            &field_spec_save,
            &values.read().clone(),
            &secret_drafts.read().clone(),
        );
        let kind = kind_save.clone();
        let edit_id = edit_id_save.clone();
        spawn(async move {
            let result = match edit_id.as_deref() {
                None => api::notify::create_channel(&kind, &name_value, enabled(), config).await,
                Some(id) => {
                    api::notify::update_channel(id, &kind, &name_value, enabled(), config).await
                }
            };
            match result {
                Ok(_) => {
                    toasts::success(if is_edit {
                        "toast-notify-channel-updated"
                    } else {
                        "toast-notify-channel-created"
                    });
                    on_saved.call(());
                }
                Err(err) => toasts::error(err),
            }
            saving.set(false);
        });
    };

    let field_spec_test = field_spec.clone();
    let kind_test = kind.clone();
    let edit_id_test = edit_id.clone();
    let do_test = move |_| {
        if testing() {
            return;
        }
        testing.set(true);
        test_error.set(None);
        let config = build_config(
            &field_spec_test,
            &values.read().clone(),
            &secret_drafts.read().clone(),
        );
        let kind = kind_test.clone();
        let channel_id = edit_id_test.clone();
        spawn(async move {
            match api::notify::test_draft(&kind, name().trim(), config, channel_id.as_deref()).await
            {
                Ok(()) => toasts::success("toast-notify-test-sent"),
                Err(err) => toasts::error(err),
            }
            testing.set(false);
        });
    };

    rsx! {
        crate::components::modal::Modal {
            title: if is_edit { t!("notify-edit-channel-title").to_string() } else { t!("notify-new-channel-title").to_string() },
            max_width: "max-w-lg".to_string(),
            on_close,
            div { class: "space-y-4",
                div {
                    label {
                        class: FIELD_LABEL,
                        r#for: "notify-field-channel-name",
                        {t!("notify-field-name")}
                    }
                    input {
                        id: "notify-field-channel-name",
                        class: "{INPUT} w-full",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                }

                // Form generated from the backend field_spec.
                for f in field_spec.clone() {
                    if f.r#type == FieldType::Secret {
                        SecretSlot {
                            key: "{f.id}",
                            spec: f,
                            drafts: secret_drafts,
                            can_manage,
                        }
                    } else {
                        FieldInput { key: "{f.id}", spec: f, values }
                    }
                }

                // Websocket channel: no field, an informative block only.
                if field_spec.is_empty() {
                    div { class: "rounded-lg bg-signal-50 border border-signal-200 p-3 text-sm text-signal-800",
                        {t!("notify-kind-in-app-noconfig")}
                    }
                }

                label { class: "flex items-center gap-2 text-sm text-gray-700",
                    input {
                        r#type: "checkbox",
                        checked: enabled(),
                        onchange: move |e| enabled.set(e.checked()),
                    }
                    {t!("notify-field-enabled")}
                }

                if let Some(msg) = test_error() {
                    div { class: "rounded-lg bg-red-50 border border-red-200 p-3 text-sm text-red-700",
                        {msg}
                    }
                }

                div { class: "flex justify-between gap-2 pt-2",
                    button {
                        class: "px-3 py-2 text-sm border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-50",
                        disabled: testing(),
                        onclick: do_test,
                        {
                            if testing() {
                                t!("notify-testing").to_string()
                            } else {
                                t!("notify-test").to_string()
                            }
                        }
                    }
                    button {
                        class: BTN_PRIMARY,
                        disabled: saving(),
                        onclick: do_save,
                        {
                            if is_edit {
                                t!("common-save").to_string()
                            } else {
                                t!("common-create").to_string()
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A `secret` field of the field_spec → [`SecretField`]. Owns the field's
/// draft signal (no `use_signal` in the parent's loop) and mirrors it into
/// the form's shared map.
#[component]
fn SecretSlot(spec: FieldSpec, mut drafts: Signal<SecretDrafts>, can_manage: bool) -> Element {
    let id = spec.id.clone();
    let draft = use_signal(|| drafts.peek().get(&id).cloned().unwrap_or_default());
    let mirror_id = id.clone();
    use_effect(move || {
        let current = draft();
        drafts.write().insert(mirror_id.clone(), current);
    });
    let label_key: &'static str = Box::leak(spec.label_i18n.clone().into_boxed_str());
    let label = if spec.required {
        format!("{} *", t!(label_key))
    } else {
        t!(label_key).to_string()
    };
    let help_key: Option<&'static str> = spec.help_i18n.as_ref().map(|h| {
        let leaked: &'static str = Box::leak(h.clone().into_boxed_str());
        leaked
    });
    rsx! {
        div {
            SecretField { label, draft, can_manage }
            if let Some(help) = help_key {
                p { class: "text-xs text-gray-500 mt-1", {t!(help)} }
            }
        }
    }
}

/// One field_spec field → its input (text, number, select, bool).
/// Dedicated component (no `use_signal` per field in a loop): reads and
/// writes the form's shared map.
#[component]
fn FieldInput(spec: FieldSpec, mut values: Signal<FieldValues>) -> Element {
    let id = spec.id.clone();
    let value = values.read().get(&id).cloned().unwrap_or_default();
    let options: Vec<(String, bool)> = spec
        .options
        .iter()
        .map(|o| (o.clone(), value == *o))
        .collect();
    let label_key: &'static str = Box::leak(spec.label_i18n.clone().into_boxed_str());
    let help_key: Option<&'static str> = spec.help_i18n.as_ref().map(|h| {
        let leaked: &'static str = Box::leak(h.clone().into_boxed_str());
        leaked
    });
    // Pairs the label with its control (accessible name).
    let control_id = format!("notify-field-{id}");
    rsx! {
        div {
            label { class: FIELD_LABEL, r#for: "{control_id}",
                {t!(label_key)}
                if spec.required {
                    span { class: "text-red-500 ml-1", "*" }
                }
            }
            match spec.r#type {
                FieldType::Number => rsx! {
                    input {
                        id: "{control_id}",
                        class: "{INPUT} w-full",
                        r#type: "number",
                        value: "{value}",
                        oninput: move |e| {
                            values.write().insert(id.clone(), e.value());
                        },
                    }
                },
                FieldType::Bool => rsx! {
                    input {
                        id: "{control_id}",
                        r#type: "checkbox",
                        checked: value == "true",
                        onchange: move |e| {
                            values
                                .write()
                                .insert(
                                    id.clone(),
                                    if e.checked() { "true".into() } else { "false".into() },
                                );
                        },
                    }
                },
                FieldType::Select if !spec.options.is_empty() => rsx! {
                    select {
                        id: "{control_id}",
                        class: "{INPUT} w-full",
                        onchange: move |e| {
                            values.write().insert(id.clone(), e.value());
                        },
                        for (opt, is_selected) in options {
                            option { value: opt.clone(), selected: is_selected, {opt.clone()} }
                        }
                    }
                },
                _ => rsx! {
                    input {
                        id: "{control_id}",
                        class: "{INPUT} w-full",
                        placeholder: spec.placeholder.clone().unwrap_or_default(),
                        value: "{value}",
                        oninput: move |e| {
                            values.write().insert(id.clone(), e.value());
                        },
                    }
                },
            }
            if let Some(help) = help_key {
                p { class: "text-xs text-gray-500 mt-1", {t!(help)} }
            }
        }
    }
}
