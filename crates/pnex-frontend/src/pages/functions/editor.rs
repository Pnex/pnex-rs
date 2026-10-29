use super::*;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY_SM};

/// Editor of a function: metadata, code, interface (ports panel, D497), live
/// test, history. Remounted by `key` on
/// `fn_id`; the `generation` counter reloads the detail after save (and
/// resets the fields — the server truth replaces the local state).
#[component]
pub(super) fn FunctionEditor(fn_id: i64, can_write: bool, on_back: Callback<()>) -> Element {
    let mut generation = use_signal(|| 0u32);
    // Tagged with the generation it was fetched for: right after a bump the
    // resource still holds the previous detail, which must not be taken
    // for the reloaded one.
    let detail = use_resource(move || {
        let gen = generation();
        async move { (gen, api::functions::detail(fn_id).await) }
    });

    // Locally edited fields, initialised from the detail (once per
    // generation — the effect does not clobber input between two reloads).
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut code = use_signal(String::new);
    let mut sig = use_signal(FunctionSignature::default);
    let mut loaded = use_signal(|| None::<FunctionDetail>);
    let mut init_done = use_signal(|| None::<u32>);
    use_effect(move || {
        let Some((gen, Ok(d))) = &*detail.read() else {
            return;
        };
        let gen = *gen;
        if gen != generation() || init_done() == Some(gen) {
            return;
        }
        name.set(d.name.clone());
        description.set(d.description.clone().unwrap_or_default());
        code.set(d.code.clone());
        sig.set(signature_of(d));
        loaded.set(Some(d.clone()));
        init_done.set(Some(gen));
    });

    let mut versions_open = use_signal(|| false);
    let mut test_open = use_signal(|| false);

    // Live validation (compile-only, debounced) + reference/snippets.
    let mut server_diags: Signal<Vec<pnex_core::FunctionDiagnostic>> = use_signal(Vec::new);
    let mut validate_gen = use_signal(|| 0u64);
    let mut ref_open = use_signal(|| false);
    let mut insert_req: Signal<Option<(u64, String)>> = use_signal(|| None);
    let editor_area: Signal<Option<AreaHandle>> = use_signal(|| None);
    let mut insert_gen = use_signal(|| 0u64);
    let mut insert_at_caret = move |text: String| {
        insert_gen.with_mut(|g| *g += 1);
        insert_req.set(Some((insert_gen(), text)));
    };

    // 700 ms debounce: only the latest generation writes; infra
    // errors (503) degrade silently (bar cleared, no toast).
    use_effect(move || {
        let current = code();
        validate_gen.with_mut(|g| *g += 1);
        let gen = validate_gen();
        let language = loaded().map(|d| d.language).unwrap_or_default();
        spawn(async move {
            util::sleep(std::time::Duration::from_millis(700)).await;
            if gen != validate_gen() {
                return;
            }
            match api::functions::validate(pnex_core::FunctionValidateRequest {
                language,
                code: current,
            })
            .await
            {
                Ok(resp) if gen == validate_gen() => server_diags.set(resp.diagnostics),
                _ => server_diags.set(Vec::new()),
            }
        });
    });

    // Test panel: raw values per input (converted at execution),
    // simulated incoming msg, result.
    let test_inputs = use_signal(BTreeMap::<String, String>::new);
    let test_msg = use_signal(|| "{}".to_string());
    let mut test_result = use_signal(|| None::<FunctionTestResponse>);
    let mut test_running = use_signal(|| false);

    // Dirty: comparison with the last loaded value (never a dirty signal
    // kept up to date — brick0 lesson "zero set in render").
    let dirty = loaded().is_some_and(|d| {
        code() != d.code
            || sig() != signature_of(&d)
            || name().trim() != d.name
            || description().trim() != d.description.clone().unwrap_or_default()
    });

    let save = move |_| {
        let Some(base) = loaded() else { return };
        let mut params_code = None;
        if code() != base.code {
            params_code = Some(code());
        }
        let current = sig();
        let ports_changed = current != signature_of(&base);
        let mut params_name = None;
        if name().trim() != base.name {
            params_name = Some(name().trim().to_string());
        }
        let mut params_description = None;
        if description().trim() != base.description.clone().unwrap_or_default() {
            let value = description().trim().to_string();
            params_description = Some(if value.is_empty() { None } else { Some(value) });
        }
        if params_code.is_none()
            && !ports_changed
            && params_name.is_none()
            && params_description.is_none()
        {
            return;
        }
        let expected = base.current_version_number;
        spawn(async move {
            let params = pnex_core::SaveFunctionVersion {
                expected_version_number: expected,
                code: params_code,
                inputs: ports_changed.then(|| current.inputs.clone()),
                outputs: ports_changed.then(|| current.outputs.clone()),
                name: params_name,
                // Some(empty) = reset of the description on the server.
                description: params_description.map(|v| v.unwrap_or_default()),
                note: None,
            };
            match api::functions::save_version(fn_id, params).await {
                Ok(_) => {
                    toasts::success("toast-function-saved");
                    // Reload of the detail → the effect resets the fields and
                    // `current_version_number` is up to date (new version).
                    generation.with_mut(|g| *g += 1);
                }
                Err(err) if err.status == Some(409) => {
                    // Stale version: someone saved before us —
                    // simple reload + toast (no overwrite modal).
                    toasts::error(t!("functions-save-conflict").to_string());
                    generation.with_mut(|g| *g += 1);
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    // Test run: typed conversion of the raw values, ad-hoc code =
    // current content of the textarea (testable before save).
    let run_test = move |_| {
        let declared = sig();
        let mut inputs = BTreeMap::new();
        for input in &declared.inputs {
            let raw = test_inputs
                .read()
                .get(&input.name)
                .cloned()
                .unwrap_or_default();
            if raw.trim().is_empty() {
                continue;
            }
            let value = match input.ty {
                FunctionType::Number => match raw.trim().parse::<f64>() {
                    Ok(v) => serde_json::json!(v),
                    Err(_) => {
                        toasts::error(format!("{} : {raw}", t!("functions-test-number-invalid")));
                        return;
                    }
                },
                FunctionType::String => serde_json::json!(raw),
                FunctionType::Bool => serde_json::json!(raw.trim() == "true"),
                FunctionType::Any => match serde_json::from_str::<serde_json::Value>(raw.trim()) {
                    Ok(v) => v,
                    Err(_) => {
                        toasts::error(format!("{} : {raw}", t!("functions-test-json-invalid")));
                        return;
                    }
                },
            };
            inputs.insert(input.name.clone(), value);
        }
        let msg: serde_json::Value = match serde_json::from_str::<serde_json::Value>(&test_msg()) {
            Ok(v) if v.is_object() => v,
            Ok(_) => {
                toasts::error(t!("functions-test-msg-invalid").to_string());
                return;
            }
            Err(_) => {
                toasts::error(t!("functions-test-msg-invalid").to_string());
                return;
            }
        };
        let language = loaded().map(|d| d.language).unwrap_or_default();
        test_running.set(true);
        // The previous result must not pass for this run's while it executes.
        test_result.set(None);
        spawn(async move {
            let params = FunctionTestRequest {
                version_number: None,
                ad_hoc: Some(FunctionTestAdHoc {
                    language,
                    code: code(),
                    inputs: declared.inputs,
                    outputs: declared.outputs,
                }),
                inputs,
                msg: Some(msg),
            };
            match api::functions::test(fn_id, params).await {
                Ok(resp) => test_result.set(Some(resp)),
                Err(err) => toasts::error(err),
            }
            test_running.set(false);
        });
    };

    let detail_language = loaded().map(|d| d.language).unwrap_or(FunctionLanguage::Js);

    // "Format": full reindentation of the buffer (idempotent).
    let format_code = move |_| {
        let current = code();
        let formatted = crate::components::code_editing::reindent(&current, detail_language);
        if formatted != current {
            code.set(formatted);
        }
    };

    // Declaration of undeclared ports (chips + add all), type `any`.
    let declare_ports = move |names: Vec<(String, bool)>| {
        move |_| {
            let mut next = sig();
            for (name, is_input) in names.iter() {
                next = crate::components::function_ports::add_port(
                    &next,
                    *is_input,
                    name.clone(),
                    FunctionType::Any,
                );
            }
            sig.set(next);
        }
    };
    let detail_version = loaded().map(|d| d.current_version_number).unwrap_or(0);
    let description_value = description();
    let code_value = code();
    let sig_value = sig();
    let signature_inputs = sig_value.inputs.clone();
    let signature_outputs = sig_value.outputs.clone();
    let undeclared = crate::components::function_analysis::analyze_undeclared(
        &code_value,
        detail_language,
        &sig_value,
    );
    let marks: Vec<LineMark> = {
        let mut m: Vec<LineMark> = Vec::new();
        for d in server_diags() {
            if let Some(line) = d.line {
                m.push(LineMark {
                    line,
                    severity: MarkSeverity::Error,
                });
            }
        }
        for u in &undeclared {
            m.push(LineMark {
                line: u.first_line,
                severity: MarkSeverity::Warning,
            });
        }
        m
    };

    rsx! {
        div { class: "space-y-6",
            // ── Header: back, inline name/description, actions ──
            div { class: "flex items-start justify-between flex-wrap gap-3",
                div { class: "flex items-start gap-3 flex-grow min-w-0",
                    button {
                        class: "{BTN_SECONDARY_SM} shrink-0",
                        onclick: move |_| on_back.call(()),
                        icons::ArrowLeft { class: "h-4 w-4" }
                    }
                    div { class: "flex-grow min-w-0",
                        input {
                            class: "w-full text-xl font-bold text-gray-900 bg-transparent rounded-lg px-2 py-0.5 -ml-2 border border-transparent hover:border-gray-300 focus:border-signal-500 focus:bg-white outline-none",
                            value: "{name}",
                            disabled: !can_write,
                            oninput: move |event| name.set(event.value()),
                        }
                        div { class: "flex items-center gap-2 mt-1 flex-wrap",
                            span { class: "inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium {language_badge(detail_language)} w-fit",
                                {language_label(detail_language)}
                            }
                            span { class: "text-sm text-gray-500", {format!("v{detail_version}")} }
                            if dirty {
                                span { class: "text-xs text-amber-600", {t!("functions-dirty")} }
                            }
                            input {
                                class: "flex-grow min-w-[220px] text-sm text-gray-600 bg-transparent rounded-lg px-2 py-0.5 border border-transparent hover:border-gray-300 focus:border-signal-500 focus:bg-white outline-none placeholder:text-gray-400",
                                placeholder: t!("functions-description-placeholder").to_string(),
                                value: "{description_value}",
                                disabled: !can_write,
                                oninput: move |event| description.set(event.value()),
                            }
                        }
                    }
                }
                div { class: "flex items-center gap-2 shrink-0",
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| versions_open.set(true),
                        icons::History { class: "h-4 w-4 inline mr-1" }
                        {t!("functions-history")}
                    }
                    if can_write {
                        button {
                            class: BTN_PRIMARY,
                            disabled: !dirty,
                            onclick: save,
                            icons::Save { class: "h-4 w-4 inline mr-1" }
                            {t!("functions-save")}
                        }
                    }
                }
            }

            // ── Code + aside (node preview, detected, ports) ──
            div { class: "grid grid-cols-1 xl:grid-cols-3 gap-4 items-start",
                div { class: "xl:col-span-2 space-y-0",
                    div { class: "flex items-center gap-2 px-3.5 py-2 rounded-t-xl border border-b-0 border-gray-200 bg-white",
                        span { class: "text-sm font-semibold font-mono", "handle(inputs, msg)" }
                        span { class: "text-xs text-gray-500 truncate hidden sm:inline",
                            {
                                t!(
                                    "functions-signature-pill", inputs : signature_inputs.len(), outputs :
                                    signature_outputs.len()
                                )
                                    .to_string()
                            }
                        }
                        span { class: "flex-grow" }
                        if can_write {
                            button {
                                class: "{BTN_SECONDARY_SM} h-8",
                                onclick: format_code,
                                {t!("functions-format")}
                            }
                            button {
                                class: "{BTN_SECONDARY_SM} h-8",
                                onclick: move |_| ref_open.set(true),
                                {t!("functions-snippet")}
                            }
                            button {
                                class: "{BTN_SECONDARY_SM} h-8",
                                onclick: move |_| test_open.set(true),
                                icons::Zap { class: "h-3.5 w-3.5 inline mr-1" }
                                {t!("functions-test-open")}
                            }
                        }
                    }
                    FunctionCodeEditor {
                        value: code_value.clone(),
                        language: detail_language.into(),
                        readonly: !can_write,
                        oninput: move |v: String| code.set(v),
                        marks,
                        insert_request: insert_req,
                        area: editor_area,
                    }
                    if !server_diags().is_empty() {
                        for d in server_diags() {
                            div { class: "flex items-center gap-2 px-3.5 py-2 rounded-b-xl border border-t-0 border-red-200 bg-red-50 text-sm text-red-800",
                                if let Some(line) = d.line {
                                    b { class: "font-mono", {format!("{line}")} }
                                }
                                span { class: "font-mono truncate", "{d.message}" }
                            }
                        }
                    } else if !undeclared.is_empty() {
                        div { class: "flex items-center gap-2 px-3.5 py-2 rounded-b-xl border border-t-0 border-amber-200 bg-amber-50 text-sm text-amber-800",
                            span { class: "truncate",
                                for u in undeclared.iter().take(3) {
                                    span { class: "font-mono font-semibold", "{u.name} " }
                                }
                            }
                            span { class: "flex-grow" }
                            if can_write {
                                button {
                                    class: "text-signal-700 font-semibold hover:underline shrink-0",
                                    onclick: declare_ports(
                                        undeclared.iter().map(|u| (u.name.clone(), u.kind == PortKind::Input)).collect(),
                                    ),
                                    {t!("functions-fix-all", count : undeclared.len())}
                                }
                            }
                        }
                    }
                }
                div { class: "space-y-4",
                    FunctionNodePreview {
                        name: name(),
                        language: detail_language,
                        version: detail_version,
                        inputs: signature_inputs.clone(),
                        outputs: signature_outputs.clone(),
                    }
                    if !undeclared.is_empty() {
                        div { class: "rounded-xl border border-amber-200 bg-amber-50 p-3",
                            div { class: "flex items-center gap-2",
                                span { class: "text-sm text-amber-900 flex-grow",
                                    {t!("functions-detected-title")}
                                }
                                if can_write {
                                    button {
                                        class: "text-sm font-semibold text-signal-700 hover:underline",
                                        onclick: declare_ports(
                                            undeclared.iter().map(|u| (u.name.clone(), u.kind == PortKind::Input)).collect(),
                                        ),
                                        {t!("functions-add-all")}
                                    }
                                }
                            }
                            div { class: "flex flex-wrap gap-2 mt-2",
                                for u in undeclared.clone() {
                                    button {
                                        class: if u.kind == PortKind::Input { "h-8 px-3 rounded-full border border-teal-700 bg-teal-50 text-teal-800 font-mono text-sm" } else { "h-8 px-3 rounded-full border border-amber-600 bg-amber-100 text-amber-900 font-mono text-sm" },
                                        onclick: declare_ports(vec![(u.name.clone(), u.kind == PortKind::Input)]),
                                        "+ {u.name}"
                                    }
                                }
                            }
                        }
                    }
                    FunctionPortsPanel {
                        sig: sig_value,
                        readonly: !can_write,
                        onedit: move |v: FunctionSignature| sig.set(v),
                    }
                }
            }

            // ── Reference modal (snippets + reading) ──
            FunctionsReferenceModal {
                open: ref_open(),
                lang: detail_language,
                oninsert: move |snippet: String| insert_at_caret(snippet),
                onclose: move |_| ref_open.set(false),
            }

            // ── Live test modal (toolbar "Test run") ──
            FunctionTestModal {
                open: test_open(),
                on_close: move |_| {
                    test_open.set(false);
                    test_result.set(None);
                },
                inputs: signature_inputs,
                outputs: signature_outputs,
                test_inputs,
                test_msg,
                test_result,
                test_running,
                run_test,
            }

            // ── History drawer ──
            if versions_open() {
                FunctionVersionsDrawer {
                    fn_id,
                    current_version: detail_version,
                    on_close: move |_| versions_open.set(false),
                    on_load: move |(loaded_code, loaded_sig): (String, FunctionSignature)| {
                        code.set(loaded_code);
                        sig.set(loaded_sig);
                        versions_open.set(false);
                    },
                }
            }
        }
    }
}

/// Declared interface of the loaded version.
fn signature_of(d: &FunctionDetail) -> FunctionSignature {
    FunctionSignature {
        inputs: d.inputs.clone(),
        outputs: d.outputs.clone(),
    }
}
