use super::helpers::*;
use super::*;
use crate::components::secret_field::{can_manage_secrets, SecretSlotField};
use pnex_core::SecretSlot;

/// Mutation ciblée de la config `http_fetch` du nœud sélectionné.
fn patch_http_fetch(cx: &mut EditorCx, f: impl FnOnce(&mut HttpFetchNodeConfig) + 'static) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::HttpFetch { config } = &mut node.kind {
            f(config);
        }
    });
}

/// Formulaire du nœud `pnex-http-fetch` (C1a) — requête HTTP « type curl ».
/// Écoles : CoolPropForm (signal local par champ texte), champ `name` de
/// l'Inspector (sous-champs auth/proxy pilotés par le graphe), NotifyVarRow
/// (lignes headers répétables, composant dédié — pas de `let` dans le for).
#[component]
pub(super) fn HttpFetchForm(
    mut cx: EditorCx,
    initial: HttpFetchNodeConfig,
    can_write: bool,
) -> Element {
    let mut url = use_signal(move || initial.url.clone());
    let mut timeout_raw = use_signal(move || initial.timeout_secs.to_string());
    let mut body_raw = use_signal(move || initial.body.clone().unwrap_or_default());
    // Headers : signal unique `Vec<(name, value)>`, synchronisé au graphe à
    // chaque mutation (add/change/remove).
    let mut headers = use_signal(move || {
        let rows: Vec<(String, String)> = initial
            .headers
            .iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect();
        rows
    });

    // Valeurs d'affichage des sous-champs auth/proxy : dérivées des props à
    // chaque rendu (le graphe est la source de vérité — pas de signal local
    // qui se périmerait au changement de mode).
    // Secret fields are vault slots (secrets.md S5): typed values become
    // the node's dedicated secret on save, picks are references.
    let (auth_username, auth_name, auth_secret) = match &initial.auth {
        HttpFetchAuth::Basic { username, password } => {
            (username.clone(), String::new(), password.clone())
        }
        HttpFetchAuth::Bearer { token } => (String::new(), String::new(), token.clone()),
        HttpFetchAuth::Header { name, value } => (String::new(), name.clone(), value.clone()),
        HttpFetchAuth::None => (String::new(), String::new(), SecretSlot::Unset),
    };
    let (proxy_url, proxy_username, proxy_password) = match &initial.proxy {
        HttpFetchProxy::Custom {
            url,
            username,
            password,
        } => (
            url.clone(),
            username.clone().unwrap_or_default(),
            password.clone(),
        ),
        HttpFetchProxy::None => (String::new(), String::new(), SecretSlot::Unset),
    };
    let can_manage = can_write && can_manage_secrets();

    let is_post = matches!(initial.method, HttpFetchMethod::Post);

    rsx! {
        div { class: "space-y-3",
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-http-fetch-url")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                    r#type: "text",
                    placeholder: t!("flows-http-fetch-url-placeholder"),
                    value: "{url}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let raw = event.value();
                        url.set(raw.clone());
                        patch_http_fetch(&mut cx, move |cfg| cfg.url = raw);
                    },
                }
            }
            div { class: "grid grid-cols-2 gap-2",
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-http-fetch-method")} }
                    select {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white disabled:bg-gray-50",
                        disabled: !can_write,
                        onchange: move |event| {
                            let raw = event.value();
                            patch_http_fetch(&mut cx, move |cfg| {
                                cfg.method = if raw == "post" { HttpFetchMethod::Post } else { HttpFetchMethod::Get };
                            });
                        },
                        option { value: "get", selected: !is_post, {t!("flows-http-fetch-method-get")} }
                        option { value: "post", selected: is_post, {t!("flows-http-fetch-method-post")} }
                    }
                }
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-http-fetch-timeout")} }
                    input {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                        r#type: "text",
                        value: "{timeout_raw}",
                        disabled: !can_write,
                        oninput: move |event| {
                            let raw = event.value();
                            timeout_raw.set(raw.clone());
                            patch_http_fetch(&mut cx, move |cfg| {
                                cfg.timeout_secs = raw.trim().parse::<u64>().unwrap_or(30);
                            });
                        },
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-http-fetch-auth")} }
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white disabled:bg-gray-50",
                    disabled: !can_write,
                    onchange: move |event| {
                        let raw = event.value();
                        patch_http_fetch(&mut cx, move |cfg| {
                            cfg.auth = match raw.as_str() {
                                "basic" => HttpFetchAuth::Basic { username: String::new(), password: SecretSlot::Unset },
                                "bearer" => HttpFetchAuth::Bearer { token: SecretSlot::Unset },
                                "header" => HttpFetchAuth::Header { name: String::new(), value: SecretSlot::Unset },
                                _ => HttpFetchAuth::None,
                            };
                        });
                    },
                    option { value: "none", selected: matches!(initial.auth, HttpFetchAuth::None), {t!("flows-http-fetch-auth-none")} }
                    option { value: "basic", selected: matches!(initial.auth, HttpFetchAuth::Basic { .. }), {t!("flows-http-fetch-auth-basic")} }
                    option { value: "bearer", selected: matches!(initial.auth, HttpFetchAuth::Bearer { .. }), {t!("flows-http-fetch-auth-bearer")} }
                    option { value: "header", selected: matches!(initial.auth, HttpFetchAuth::Header { .. }), {t!("flows-http-fetch-auth-header")} }
                }
            }
            match &initial.auth {
                HttpFetchAuth::Basic { .. } => rsx! {
                    div { class: "grid grid-cols-2 gap-2",
                        input {
                            class: "px-2 py-1 border border-gray-300 rounded text-xs",
                            placeholder: t!("flows-http-fetch-username"),
                            value: "{auth_username}",
                            disabled: !can_write,
                            oninput: move |event| {
                                let raw = event.value();
                                patch_http_fetch(&mut cx, move |cfg| {
                                    if let HttpFetchAuth::Basic { username, .. } = &mut cfg.auth {
                                        *username = raw;
                                    }
                                });
                            },
                        }
                    }
                    SecretSlotField {
                        label: t!("flows-http-fetch-password").to_string(),
                        slot: auth_secret.clone(),
                        can_manage,
                        read_only: !can_write,
                        on_change: move |next: SecretSlot| {
                            patch_http_fetch(&mut cx, move |cfg| {
                                if let HttpFetchAuth::Basic { password, .. } = &mut cfg.auth {
                                    *password = next;
                                }
                            });
                        },
                    }
                },
                HttpFetchAuth::Bearer { .. } => rsx! {
                    SecretSlotField {
                        label: t!("flows-http-fetch-token").to_string(),
                        slot: auth_secret.clone(),
                        can_manage,
                        read_only: !can_write,
                        on_change: move |next: SecretSlot| {
                            patch_http_fetch(&mut cx, move |cfg| {
                                if let HttpFetchAuth::Bearer { token } = &mut cfg.auth {
                                    *token = next;
                                }
                            });
                        },
                    }
                },
                HttpFetchAuth::Header { .. } => rsx! {
                    div { class: "grid grid-cols-2 gap-2",
                        input {
                            class: "px-2 py-1 border border-gray-300 rounded text-xs font-mono",
                            placeholder: t!("flows-http-fetch-header-name"),
                            value: "{auth_name}",
                            disabled: !can_write,
                            oninput: move |event| {
                                let raw = event.value();
                                patch_http_fetch(&mut cx, move |cfg| {
                                    if let HttpFetchAuth::Header { name, .. } = &mut cfg.auth {
                                        *name = raw;
                                    }
                                });
                            },
                        }
                    }
                    SecretSlotField {
                        label: t!("flows-http-fetch-header-value").to_string(),
                        slot: auth_secret.clone(),
                        can_manage,
                        read_only: !can_write,
                        on_change: move |next: SecretSlot| {
                            patch_http_fetch(&mut cx, move |cfg| {
                                if let HttpFetchAuth::Header { value, .. } = &mut cfg.auth {
                                    *value = next;
                                }
                            });
                        },
                    }
                },
                HttpFetchAuth::None => rsx! {},
            }
            div { class: "space-y-1",
                span { class: "text-xs font-medium text-gray-500 block", {t!("flows-http-fetch-headers")} }
                for (index, (name, value)) in headers.read().iter().enumerate() {
                    HttpFetchHeaderRow {
                        key: "{index}-{name}",
                        index,
                        name: name.clone(),
                        value: value.clone(),
                        can_write,
                        on_change: move |(i, new_name, new_value)| {
                            let mut rows = headers.read().clone();
                            if let Some(row) = rows.get_mut(i) {
                                *row = (new_name, new_value);
                            }
                            let synced = rows.clone();
                            patch_http_fetch(&mut cx, move |cfg| {
                                cfg.headers = synced.iter().map(|(n, v)| HttpFetchHeader { name: n.clone(), value: v.clone() }).collect();
                            });
                        },
                        on_remove: move |i| {
                            let mut rows = headers.read().clone();
                            if i < rows.len() {
                                rows.remove(i);
                            }
                            let synced = rows.clone();
                            headers.set(rows);
                            patch_http_fetch(&mut cx, move |cfg| {
                                cfg.headers = synced.iter().map(|(n, v)| HttpFetchHeader { name: n.clone(), value: v.clone() }).collect();
                            });
                        },
                    }
                }
                if can_write {
                    button {
                        class: "text-xs text-blue-600 hover:text-blue-700",
                        onclick: move |_| {
                            let mut rows = headers.read().clone();
                            rows.push((String::new(), String::new()));
                            headers.set(rows);
                            patch_http_fetch(&mut cx, |cfg| cfg.headers.push(HttpFetchHeader { name: String::new(), value: String::new() }));
                        },
                        {t!("flows-http-fetch-add-header")}
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-http-fetch-proxy")} }
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white disabled:bg-gray-50",
                    disabled: !can_write,
                    onchange: move |event| {
                        let raw = event.value();
                        patch_http_fetch(&mut cx, move |cfg| {
                            cfg.proxy = if raw == "custom" {
                                HttpFetchProxy::Custom { url: String::new(), username: None, password: SecretSlot::Unset }
                            } else {
                                HttpFetchProxy::None
                            };
                        });
                    },
                    option { value: "none", selected: matches!(initial.proxy, HttpFetchProxy::None), {t!("flows-http-fetch-proxy-none")} }
                    option { value: "custom", selected: matches!(initial.proxy, HttpFetchProxy::Custom { .. }), {t!("flows-http-fetch-proxy-custom")} }
                }
            }
            if matches!(initial.proxy, HttpFetchProxy::Custom { .. }) {
                div { class: "space-y-1",
                    div { class: "grid grid-cols-2 gap-2",
                        input {
                            class: "px-2 py-1 border border-gray-300 rounded text-xs font-mono",
                            placeholder: t!("flows-http-fetch-proxy-url"),
                            value: "{proxy_url}",
                            disabled: !can_write,
                            oninput: move |event| {
                                let raw = event.value();
                                patch_http_fetch(&mut cx, move |cfg| {
                                    if let HttpFetchProxy::Custom { url, .. } = &mut cfg.proxy {
                                        *url = raw;
                                    }
                                });
                            },
                        }
                            input {
                                class: "px-2 py-1 border border-gray-300 rounded text-xs",
                                placeholder: t!("flows-http-fetch-proxy-username"),
                                value: "{proxy_username}",
                                disabled: !can_write,
                                oninput: move |event| {
                                    let raw = event.value();
                                    patch_http_fetch(&mut cx, move |cfg| {
                                        if let HttpFetchProxy::Custom { username, .. } = &mut cfg.proxy {
                                            *username = if raw.is_empty() { None } else { Some(raw) };
                                        }
                                    });
                                },
                            }
                        }
                        SecretSlotField {
                            label: t!("flows-http-fetch-proxy-password").to_string(),
                            slot: proxy_password.clone(),
                            can_manage,
                            read_only: !can_write,
                            on_change: move |next: SecretSlot| {
                                patch_http_fetch(&mut cx, move |cfg| {
                                    if let HttpFetchProxy::Custom { password, .. } = &mut cfg.proxy {
                                        *password = next;
                                    }
                                });
                            },
                        }
                        p { class: "text-xs text-gray-400", {t!("flows-http-fetch-proxy-hint")} }
                    }
            }
            if is_post {
                label { class: "block",
                    span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-http-fetch-body")} }
                    textarea {
                        class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-xs font-mono",
                        rows: "4",
                        placeholder: t!("flows-http-fetch-body-placeholder"),
                        value: "{body_raw}",
                        disabled: !can_write,
                        oninput: move |event| {
                            let raw = event.value();
                            body_raw.set(raw.clone());
                            patch_http_fetch(&mut cx, move |cfg| {
                                cfg.body = if raw.is_empty() { None } else { Some(raw) };
                            });
                        },
                    }
                }
            }
            label { class: "block",
                span { class: "text-xs font-medium text-gray-500 mb-1 block", {t!("flows-http-fetch-on-error")} }
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white disabled:bg-gray-50",
                    disabled: !can_write,
                    onchange: move |event| {
                        let raw = event.value();
                        patch_http_fetch(&mut cx, move |cfg| {
                            cfg.on_error = if raw == "passthrough" {
                                HttpFetchOnError::Passthrough
                            } else {
                                HttpFetchOnError::Reject
                            };
                        });
                    },
                    option { value: "reject", selected: matches!(initial.on_error, HttpFetchOnError::Reject), {t!("flows-http-fetch-on-error-reject")} }
                    option { value: "passthrough", selected: matches!(initial.on_error, HttpFetchOnError::Passthrough), {t!("flows-http-fetch-on-error-passthrough")} }
                }
            }
        }
    }
}

/// Une ligne header nom/valeur — composant dédié (pas de `let` dans le for
/// rsx, piège dioxus). `on_change((index, name, value))` remonte la saisie.
#[component]
fn HttpFetchHeaderRow(
    index: usize,
    name: String,
    value: String,
    can_write: bool,
    on_change: EventHandler<(usize, String, String)>,
    on_remove: EventHandler<usize>,
) -> Element {
    let name_edit = name.clone();
    let value_edit = value.clone();
    rsx! {
        div { class: "flex gap-1 items-center",
            input {
                class: "w-28 px-2 py-1 border border-gray-300 rounded text-xs font-mono",
                placeholder: t!("flows-http-fetch-header-name"),
                value: "{name}",
                disabled: !can_write,
                oninput: move |event| {
                    let new_name = event.value();
                    on_change.call((index, new_name, value_edit.clone()));
                },
            }
            input {
                class: "flex-1 px-2 py-1 border border-gray-300 rounded text-xs font-mono",
                placeholder: t!("flows-http-fetch-header-value"),
                value: "{value}",
                disabled: !can_write,
                oninput: move |event| {
                    let new_value = event.value();
                    on_change.call((index, name_edit.clone(), new_value));
                },
            }
            if can_write {
                button {
                    class: "text-xs text-red-500 hover:text-red-700",
                    onclick: move |_| on_remove.call(index),
                    "✕"
                }
            }
        }
    }
}
