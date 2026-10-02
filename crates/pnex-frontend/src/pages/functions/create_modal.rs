use super::*;

/// Modal de création : nom + langage + description. Le code démarre du
/// squelette du langage (`handle` obligatoire) — la version 1 est créée par
/// le POST, l'éditeur s'ouvre directement dessus.
#[component]
pub(super) fn CreateFunctionModal(on_close: Callback<()>, on_created: Callback<i64>) -> Element {
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut language = use_signal(|| FunctionLanguage::Js);
    let mut template = use_signal(crate::components::function_templates::TemplateKind::default);
    let mut name_error = use_signal(|| false);
    let mut creating = use_signal(|| false);

    let submit = move |_| {
        let trimmed = name().trim().to_string();
        if trimmed.is_empty() {
            name_error.set(true);
            return;
        }
        creating.set(true);
        let lang = language();
        let params = CreateFunction {
            name: trimmed,
            language: lang,
            description: {
                let value = description().trim().to_string();
                (!value.is_empty()).then_some(value)
            },
            code: crate::components::function_templates::template_code(template(), lang),
            note: None,
        };
        spawn(async move {
            match api::functions::create(params).await {
                Ok(detail) => {
                    toasts::success("toast-function-created");
                    on_close.call(());
                    on_created.call(detail.id);
                }
                Err(err) => {
                    creating.set(false);
                    toasts::error(err);
                }
            }
        });
    };

    rsx! {
        FormDialog {
            title: t!("functions-create-title").to_string(),
            submit_label: t!("functions-new").to_string(),
            on_close,
            on_submit: submit,
            busy: creating(),
            // `valid` au défaut (true) : l'erreur inline name_error s'affiche
            // après clic — le bouton ne doit pas être verrouillé avant.
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("functions-field-name")}
                }
                input {
                    class: if name_error() { "w-full px-3 py-2 border border-red-400 bg-red-50 rounded-lg text-sm" } else { "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm" },
                    r#type: "text",
                    value: "{name}",
                    oninput: move |event| {
                        name.set(event.value());
                        name_error.set(false);
                    },
                }
                if name_error() {
                    span { class: "text-xs text-red-600 mt-1 block",
                        {t!("functions-field-name-required")}
                    }
                }
            }
            fieldset { class: "flex flex-col gap-2",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("functions-field-language")}
                }
                div { class: "grid grid-cols-1 sm:grid-cols-2 gap-2.5",
                    for lang_opt in [FunctionLanguage::Starlark, FunctionLanguage::Js] {
                        {
                            let (title, desc) = match lang_opt {
                                FunctionLanguage::Starlark => {
                                    (t!("functions-lang-starlark"), t!("functions-lang-starlark-desc"))
                                }
                                FunctionLanguage::Js => {
                                    (t!("functions-lang-js"), t!("functions-lang-js-desc"))
                                }
                            };
                            rsx! {
                                label { class: if language() == lang_opt { "flex gap-2.5 p-3 border-2 border-blue-700 bg-blue-50 rounded-xl cursor-pointer" } else { "flex gap-2.5 p-3 border border-gray-300 rounded-xl cursor-pointer hover:border-gray-400" },
                                    input {
                                        r#type: "radio",
                                        name: "fn-lang",
                                        class: "mt-1",
                                        checked: language() == lang_opt,
                                        onchange: move |_| language.set(lang_opt),
                                    }
                                    span { class: "flex flex-col gap-0.5",
                                        span { class: "font-semibold text-[15px]", "{title}" }
                                        span { class: "text-[13px] text-gray-600", "{desc}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            fieldset { class: "flex flex-col gap-2",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("functions-tpl-title")}
                }
                div { class: "grid grid-cols-1 sm:grid-cols-2 gap-2.5",
                    for kind in crate::components::function_templates::TemplateKind::all() {
                        {
                            let selected = template() == kind;
                            let title = t!(kind.title_key());
                            let sig = t!(kind.sig_key());
                            rsx! {
                                label { class: if selected { "flex flex-col gap-1.5 p-3 border-2 border-blue-700 bg-blue-50 rounded-xl cursor-pointer" } else { "flex flex-col gap-1.5 p-3 border border-gray-300 rounded-xl cursor-pointer hover:border-gray-400" },
                                    span { class: "flex items-center gap-2",
                                        input {
                                            r#type: "radio",
                                            name: "fn-tpl",
                                            class: "m-0",
                                            checked: selected,
                                            onchange: move |_| template.set(kind),
                                        }
                                        span { class: "font-semibold text-sm", "{title}" }
                                    }
                                    span { class: "font-mono text-xs text-gray-600", "{sig}" }
                                }
                            }
                        }
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block",
                    {t!("functions-field-description")}
                }
                input {
                    class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm",
                    r#type: "text",
                    value: "{description}",
                    oninput: move |event| description.set(event.value()),
                }
            }
        }
    }
}
