use super::*;

/// Éditeur d'une fonction : métadonnées, code, aperçu de signature (extraction
/// wasm des directives), test en live, historique. Remonté par `key` sur
/// `fn_id` ; le compteur `generation` re-charge le détail après save (et
/// ré-initialise les champs — la vérité serveur remplace l'état local).
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

    // Champs édités localement, initialisés depuis le détail (une fois par
    // génération — l'effet ne re-crashe pas les saisies entre deux reloads).
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut code = use_signal(String::new);
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
        loaded.set(Some(d.clone()));
        init_done.set(Some(gen));
    });

    // Aperçu de signature : la même extraction que le save serveur, en wasm.
    let signature = parse_directives(&code());
    let mut versions_open = use_signal(|| false);
    let mut test_open = use_signal(|| false);

    // Validation live (compile-only, débounced) + référence/snippets.
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

    // Débounce 700 ms : la dernière génération seule écrit ; les erreurs
    // d'infra (503) dégradent en silence (barre vidée, pas de toast).
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

    // Panneau de test : valeurs brutes par input (converties à l'exécution),
    // msg entrant simulé, résultat.
    let test_inputs = use_signal(BTreeMap::<String, String>::new);
    let test_msg = use_signal(|| "{}".to_string());
    let mut test_result = use_signal(|| None::<FunctionTestResponse>);
    let mut test_running = use_signal(|| false);

    // Dirty : comparaison à la dernière valeur chargée (jamais un signal
    // dirty tenu à jour — leçon brick0 « zéro set en render »).
    let dirty = loaded().is_some_and(|d| {
        code() != d.code
            || name().trim() != d.name
            || description().trim() != d.description.clone().unwrap_or_default()
    });

    let save = move |_| {
        let Some(base) = loaded() else { return };
        let mut params_code = None;
        if code() != base.code {
            params_code = Some(code());
        }
        let mut params_name = None;
        if name().trim() != base.name {
            params_name = Some(name().trim().to_string());
        }
        let mut params_description = None;
        if description().trim() != base.description.clone().unwrap_or_default() {
            let value = description().trim().to_string();
            params_description = Some(if value.is_empty() { None } else { Some(value) });
        }
        if params_code.is_none() && params_name.is_none() && params_description.is_none() {
            return;
        }
        let expected = base.current_version_number;
        spawn(async move {
            let params = pnex_core::SaveFunctionVersion {
                expected_version_number: expected,
                code: params_code,
                name: params_name,
                // Some(vide) = réinitialisation de la description côté serveur.
                description: params_description.map(|v| v.unwrap_or_default()),
                note: None,
            };
            match api::functions::save_version(fn_id, params).await {
                Ok(_) => {
                    toasts::success("toast-function-saved");
                    // Reload du détail → l'effet ré-initialise les champs et
                    // `current_version_number` est à jour (nouvelle version).
                    generation.with_mut(|g| *g += 1);
                }
                Err(err) if err.status == Some(409) => {
                    // Version périmée : quelqu'un a sauvé avant nous —
                    // rechargement simple + toast (pas de modal overwrite).
                    toasts::error(t!("functions-save-conflict").to_string());
                    generation.with_mut(|g| *g += 1);
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    // Exécution du test : conversion typée des valeurs brutes, code ad-hoc =
    // contenu courant du textarea (testable avant save).
    let run_test = move |_| {
        let Ok(sig) = parse_directives(&code()) else {
            toasts::error(t!("functions-test-directives-invalid").to_string());
            return;
        };
        let mut inputs = BTreeMap::new();
        for input in &sig.inputs {
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
        spawn(async move {
            let params = FunctionTestRequest {
                version_number: None,
                ad_hoc: Some(FunctionTestAdHoc {
                    language,
                    code: code(),
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

    // "Formater" : réindentation complète du buffer (idempotent).
    let format_code = move |_| {
        let current = code();
        let formatted = crate::components::code_editing::reindent(&current, detail_language);
        if formatted != current {
            code.set(formatted);
        }
    };

    // Insertion des directives d'un port non déclaré (chips + tout ajouter).
    let declare_ports = |names: Vec<(String, bool)>, base: String| {
        move |_| {
            let mut next = base.clone();
            for (name, is_input) in names.iter() {
                let fields = crate::components::function_ports::DirectiveFields {
                    name: name.clone(),
                    ty: FunctionType::Any,
                    default: None,
                    desc: None,
                };
                next =
                    crate::components::function_ports::insert_directive(&next, *is_input, &fields);
            }
            code.set(next);
        }
    };
    let detail_version = loaded().map(|d| d.current_version_number).unwrap_or(0);
    let description_value = description();
    let code_value = code();
    let signature_inputs = match &signature {
        Ok(sig) => sig.inputs.clone(),
        Err(_) => Vec::new(),
    };
    let signature_outputs = match &signature {
        Ok(sig) => sig.outputs.clone(),
        Err(_) => Vec::new(),
    };
    let signature_error = signature.as_ref().err().map(|e| e.to_string());
    let sig_ok = signature.as_ref().ok().cloned().unwrap_or_default();
    let undeclared = crate::components::function_analysis::analyze(&code_value, detail_language)
        .unwrap_or_default();
    let marks: Vec<LineMark> = if signature_error.is_some() {
        Vec::new()
    } else {
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
                        class: "px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors shrink-0",
                        onclick: move |_| on_back.call(()),
                        icons::ArrowLeft { class: "h-4 w-4" }
                    }
                    div { class: "flex-grow min-w-0",
                        input {
                            class: "w-full text-xl font-bold text-gray-900 bg-transparent rounded-lg px-2 py-0.5 -ml-2 border border-transparent hover:border-gray-300 focus:border-blue-500 focus:bg-white outline-none",
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
                                class: "flex-grow min-w-[220px] text-sm text-gray-600 bg-transparent rounded-lg px-2 py-0.5 border border-transparent hover:border-gray-300 focus:border-blue-500 focus:bg-white outline-none placeholder:text-gray-400",
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
                        class: "px-3 py-2 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                        onclick: move |_| versions_open.set(true),
                        icons::History { class: "h-4 w-4 inline mr-1" }
                        {t!("functions-history")}
                    }
                    if can_write {
                        button {
                            class: "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed",
                            disabled: !dirty,
                            onclick: save,
                            icons::Save { class: "h-4 w-4 inline mr-1" }
                            {t!("functions-save")}
                        }
                    }
                }
            }

            // ── Code + aside (aperçu nœud, détectés, ports) ──
            div { class: "grid grid-cols-1 xl:grid-cols-3 gap-4 items-start",
                div { class: "xl:col-span-2 space-y-0",
                    div { class: "flex items-center gap-2 px-3.5 py-2 rounded-t-xl border border-b-0 border-gray-200 bg-white",
                        span { class: "text-[13px] font-semibold font-mono", "handle(inputs, msg)" }
                        span { class: "text-[11px] text-gray-500 truncate hidden sm:inline",
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
                                class: "h-8 px-2.5 rounded-md border border-gray-300 bg-white text-[13px] text-gray-700 hover:bg-gray-50",
                                onclick: format_code,
                                {t!("functions-format")}
                            }
                            button {
                                class: "h-8 px-2.5 rounded-md border border-gray-300 bg-white text-[13px] text-gray-700 hover:bg-gray-50",
                                onclick: move |_| ref_open.set(true),
                                {t!("functions-snippet")}
                            }
                            button {
                                class: "h-8 px-2.5 rounded-md border border-gray-300 bg-white text-[13px] text-gray-700 hover:bg-gray-50",
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
                    if let Some(err) = &signature_error {
                        div { class: "flex items-center gap-2 px-3.5 py-2 rounded-b-xl border border-t-0 border-red-200 bg-red-50 text-[13px] text-red-800",
                            {err.clone()}
                        }
                    } else if !server_diags().is_empty() {
                        for d in server_diags() {
                            div { class: "flex items-center gap-2 px-3.5 py-2 rounded-b-xl border border-t-0 border-red-200 bg-red-50 text-[13px] text-red-800",
                                if let Some(line) = d.line {
                                    b { class: "font-mono", {format!("{line}")} }
                                }
                                span { class: "font-mono truncate", "{d.message}" }
                            }
                        }
                    } else if !undeclared.is_empty() {
                        div { class: "flex items-center gap-2 px-3.5 py-2 rounded-b-xl border border-t-0 border-amber-200 bg-amber-50 text-[13px] text-amber-800",
                            span { class: "truncate",
                                for u in undeclared.iter().take(3) {
                                    span { class: "font-mono font-semibold", "{u.name} " }
                                }
                            }
                            span { class: "flex-grow" }
                            if can_write {
                                button {
                                    class: "text-blue-700 font-semibold hover:underline shrink-0",
                                    onclick: declare_ports(
                                        undeclared.iter().map(|u| (u.name.clone(), u.kind == PortKind::Input)).collect(),
                                        code_value.clone(),
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
                    if !undeclared.is_empty() && signature_error.is_none() {
                        div { class: "rounded-xl border border-amber-200 bg-amber-50 p-3",
                            div { class: "flex items-center gap-2",
                                span { class: "text-sm text-amber-900 flex-grow",
                                    {t!("functions-detected-title")}
                                }
                                if can_write {
                                    button {
                                        class: "text-sm font-semibold text-blue-700 hover:underline",
                                        onclick: declare_ports(
                                            undeclared.iter().map(|u| (u.name.clone(), u.kind == PortKind::Input)).collect(),
                                            code_value.clone(),
                                        ),
                                        {t!("functions-add-all")}
                                    }
                                }
                            }
                            div { class: "flex flex-wrap gap-2 mt-2",
                                for u in undeclared.clone() {
                                    button {
                                        class: if u.kind == PortKind::Input { "h-8 px-3 rounded-full border border-teal-700 bg-teal-50 text-teal-800 font-mono text-[13px]" } else { "h-8 px-3 rounded-full border border-amber-600 bg-amber-100 text-amber-900 font-mono text-[13px]" },
                                        onclick: declare_ports(vec![(u.name.clone(), u.kind == PortKind::Input)], code_value.clone()),
                                        "+ {u.name}"
                                    }
                                }
                            }
                        }
                    }
                    FunctionPortsPanel {
                        code: code_value.clone(),
                        sig: sig_ok,
                        readonly: !can_write,
                        onedit: move |v: String| code.set(v),
                    }
                }
            }

            // ── Modale Référence (snippets + lecture) ──
            FunctionsReferenceModal {
                open: ref_open(),
                lang: detail_language,
                oninsert: move |snippet: String| insert_at_caret(snippet),
                onclose: move |_| ref_open.set(false),
            }

            // ── Live test modal (toolbar "Test run") ──
            FunctionTestModal {
                open: test_open(),
                on_close: move |_| test_open.set(false),
                inputs: signature_inputs,
                outputs: signature_outputs,
                test_inputs,
                test_msg,
                test_result,
                test_running,
                run_test,
            }

            // ── Drawer d'historique ──
            if versions_open() {
                FunctionVersionsDrawer {
                    fn_id,
                    current_version: detail_version,
                    on_close: move |_| versions_open.set(false),
                    on_load: move |loaded_code: String| {
                        code.set(loaded_code);
                        versions_open.set(false);
                    },
                }
            }
        }
    }
}
