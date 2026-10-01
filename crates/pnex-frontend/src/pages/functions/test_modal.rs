use super::*;

/// Live-test modal (opened from the editor toolbar "Test run"): one typed
/// field per declared input, simulated incoming msg (JSON), result (pretty
/// outputs, console logs, duration). All signals live in the parent — the
/// modal body re-renders the result in place after each Run (dialog stays
/// open until cancelled).
#[component]
pub(super) fn FunctionTestModal(
    open: bool,
    on_close: Callback<()>,
    inputs: Vec<pnex_core::FunctionInput>,
    outputs: Vec<pnex_core::FunctionOutput>,
    mut test_inputs: Signal<BTreeMap<String, String>>,
    mut test_msg: Signal<String>,
    test_result: Signal<Option<FunctionTestResponse>>,
    test_running: Signal<bool>,
    run_test: EventHandler,
) -> Element {
    let result = test_result();
    let running = test_running();
    let msg_value = test_msg();

    // Lignes par port précalculées (pas de `let` dans rsx) : (libellé,
    // muet, valeur pretty). L'ordre = ordre des @output déclarés.
    let port_rows: Vec<(String, bool, String)> = match &result {
        Some(resp) if resp.ok => resp
            .outputs
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let label = match outputs.get(i) {
                    Some(o) => format!("{} · port {}", o.name, i),
                    None => format!("port {}", i),
                };
                match v {
                    serde_json::Value::Null => (label, true, String::new()),
                    other => (
                        label,
                        false,
                        serde_json::to_string_pretty(other).unwrap_or_default(),
                    ),
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    let has_port_rows = !port_rows.is_empty();

    if !open {
        return rsx! {};
    }
    rsx! {
        FormDialog {
            title: t!("functions-test-title").to_string(),
            submit_label: t!("functions-test-execute").to_string(),
            on_close: on_close,
            on_submit: move |_| run_test.call(()),
            busy: running,
            max_width: "max-w-2xl".to_string(),
            div { class: "space-y-3",
                p { class: "text-xs text-gray-400", {t!("functions-test-hint")} }

                if inputs.is_empty() {
                    p { class: "text-xs text-gray-400", {t!("functions-test-no-inputs")} }
                }
                div { class: "grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3",
                    for input in inputs.clone() {
                        {test_field(input, test_inputs)}
                    }
                }
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("functions-test-msg-field")} }
                    textarea {
                        class: "w-full h-16 px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono",
                        value: "{msg_value}",
                        spellcheck: false,
                        oninput: move |event| test_msg.set(event.value()),
                    }
                }

                match result {
                    Some(resp) if resp.ok => rsx! {
                        div { class: "space-y-2",
                            div { class: "flex items-center gap-3 text-xs text-gray-400",
                                span { {format!("{} {} ms", t!("functions-test-duration"), resp.duration_ms)} }
                            }
                            if !resp.logs.is_empty() {
                                div { class: "rounded-lg bg-gray-50 border border-gray-200 p-2",
                                    p { class: "text-xs font-semibold text-gray-400 uppercase tracking-wider mb-1", {t!("functions-test-logs")} }
                                    for line in resp.logs.clone() {
                                        p { class: "text-xs text-gray-600 font-mono", {line} }
                                    }
                                }
                            }
                            div {
                                p { class: "text-xs font-semibold text-gray-400 uppercase tracking-wider mb-1", {t!("functions-test-outputs")} }
                                if has_port_rows {
                                    div { class: "space-y-1.5",
                                        for (label, muted, pretty) in port_rows {
                                            div { class: "flex items-start gap-2 rounded-lg bg-gray-900 p-2",
                                                span { class: "shrink-0 w-40 text-[11px] text-cyan-300 font-mono pt-0.5 truncate", {label} }
                                                if muted {
                                                    span { class: "text-[11px] text-gray-500 italic", {t!("functions-test-port-muted")} }
                                                } else {
                                                    pre { class: "flex-1 text-xs text-gray-100 font-mono whitespace-pre-wrap break-all m-0", {pretty} }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    pre { class: "rounded-lg bg-gray-900 text-gray-100 p-3 text-xs font-mono overflow-x-auto",
                                        {serde_json::to_string_pretty(&resp.outputs).unwrap_or_default()}
                                    }
                                    p { class: "text-[11px] text-gray-500 italic mt-1", {t!("functions-test-no-outputs")} }
                                }
                            }
                        }
                    },
                    Some(resp) => rsx! {
                        div { class: "rounded-lg bg-red-50 border border-red-200 p-3",
                            p { class: "text-xs font-semibold text-red-700 mb-1", {t!("functions-test-error")} }
                            p { class: "text-xs text-red-700 font-mono whitespace-pre-wrap", {resp.error.clone().unwrap_or_default()} }
                            if !resp.logs.is_empty() {
                                for line in resp.logs.clone() {
                                    p { class: "text-xs text-red-600 font-mono", {line} }
                                }
                            }
                        }
                    },
                    None => rsx! {},
                }
            }
        }
    }
}

/// Champ typé d'un input déclaré : number → pas de texte, string → texte,
/// bool → case, any → JSON une ligne. Vide = champ absent de la requête.
fn test_field(
    input: pnex_core::FunctionInput,
    mut test_inputs: Signal<BTreeMap<String, String>>,
) -> Element {
    let name = input.name.clone();
    let raw = test_inputs.read().get(&name).cloned().unwrap_or_default();
    let label_desc = input.desc.clone().unwrap_or_default();
    let name_for_setter = name.clone();
    let mut setter = move |value: String| {
        test_inputs.with_mut(|m| {
            m.insert(name_for_setter.clone(), value);
        });
    };
    let name_for_label = name.clone();
    rsx! {
        label { class: "block",
            span { class: "text-xs font-medium text-gray-500 mb-1 block",
                {format!("{name_for_label} ({})", type_label(input.ty))}
            }
            match input.ty {
                FunctionType::Number => rsx! {
                    input {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                        r#type: "number",
                        step: "any",
                        value: "{raw}",
                        oninput: move |event| setter(event.value()),
                    }
                },
                FunctionType::String => rsx! {
                    input {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                        r#type: "text",
                        value: "{raw}",
                        oninput: move |event| setter(event.value()),
                    }
                },
                FunctionType::Bool => rsx! {
                    input {
                        class: "h-4 w-4 text-blue-600 border-gray-300 rounded",
                        r#type: "checkbox",
                        checked: raw == "true",
                        onchange: move |event| setter(event.checked().to_string()),
                    }
                },
                FunctionType::Any => rsx! {
                    input {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                        r#type: "text",
                        placeholder: { "{}" },
                        value: "{raw}",
                        oninput: move |event| setter(event.value()),
                    }
                },
            }
            if !label_desc.is_empty() {
                span { class: "text-[11px] text-gray-400 block mt-0.5", {label_desc} }
            }
        }
    }
}
