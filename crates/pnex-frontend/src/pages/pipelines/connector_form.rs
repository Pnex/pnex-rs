//! The form of a connector (connectors.md D791): its kind first, then only the
//! fields that kind uses. Every right is checked again by the server.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::connectors::{
    Connector, ConnectorInput, ConnectorKind, RestAuth, RestSettings, WebhookSettings,
};

use crate::api;
use crate::api::error_i18n::localize;
use crate::components::crud::form::FormDialog;
use crate::components::secret_field::{can_manage_secrets, SecretDraft, SecretField};
use crate::components::ui::{FIELD_ERROR, FIELD_HELP, FIELD_LABEL, INPUT};

const KINDS: [ConnectorKind; 4] = [
    ConnectorKind::Postgres,
    ConnectorKind::Mysql,
    ConnectorKind::Rest,
    ConnectorKind::Webhook,
];

const AUTHS: [RestAuth; 5] = [
    RestAuth::None,
    RestAuth::Bearer,
    RestAuth::Header,
    RestAuth::Basic,
    RestAuth::Oauth2ClientCredentials,
];

pub(super) fn kind_label(k: ConnectorKind) -> String {
    match k {
        ConnectorKind::Postgres => t!("pipe-connector-kind-postgres"),
        ConnectorKind::Mysql => t!("pipe-connector-kind-mysql"),
        ConnectorKind::Rest => t!("pipe-connector-kind-rest"),
        ConnectorKind::Webhook => t!("pipe-connector-kind-webhook"),
    }
    .to_string()
}

fn kind_help(k: ConnectorKind) -> String {
    match k {
        ConnectorKind::Postgres => t!("pipe-connector-kind-postgres-help"),
        ConnectorKind::Mysql => t!("pipe-connector-kind-mysql-help"),
        ConnectorKind::Rest => t!("pipe-connector-kind-rest-help"),
        ConnectorKind::Webhook => t!("pipe-connector-kind-webhook-help"),
    }
    .to_string()
}

fn default_port(k: ConnectorKind) -> i32 {
    match k {
        ConnectorKind::Postgres => 5432,
        ConnectorKind::Mysql => 3306,
        ConnectorKind::Rest => 443,
        ConnectorKind::Webhook => 0,
    }
}

fn auth_label(a: RestAuth) -> String {
    match a {
        RestAuth::None => t!("pipe-connector-auth-none"),
        RestAuth::Bearer => t!("pipe-connector-auth-bearer"),
        RestAuth::Header => t!("pipe-connector-auth-header"),
        RestAuth::Basic => t!("pipe-connector-auth-basic"),
        RestAuth::Oauth2ClientCredentials => t!("pipe-connector-auth-oauth2"),
    }
    .to_string()
}

/// The name of the secret a connector holds, `None` when it holds none.
fn secret_label(k: ConnectorKind, auth: RestAuth) -> Option<String> {
    let label = match (k, auth) {
        (ConnectorKind::Webhook, _) => t!("pipe-connector-webhook-secret"),
        (ConnectorKind::Rest, RestAuth::None) => return None,
        (ConnectorKind::Rest, RestAuth::Bearer) => t!("pipe-connector-secret-token"),
        (ConnectorKind::Rest, RestAuth::Header) => t!("pipe-connector-secret-key"),
        (ConnectorKind::Rest, RestAuth::Oauth2ClientCredentials) => {
            t!("pipe-connector-secret-client")
        }
        _ => t!("pipe-connector-password"),
    };
    Some(label.to_string())
}

/// One labelled text input bound to a signal.
#[component]
fn TextField(
    id: String,
    label: String,
    value: Signal<String>,
    #[props(default)] help: Option<String>,
) -> Element {
    let mut value = value;
    rsx! {
        div {
            label { class: FIELD_LABEL, r#for: "{id}", "{label}" }
            input {
                id: "{id}",
                class: INPUT,
                value: "{value}",
                oninput: move |e| value.set(e.value()),
            }
            if let Some(help) = help {
                p { class: FIELD_HELP, "{help}" }
            }
        }
    }
}

#[component]
pub(super) fn ConnectorForm(
    initial: Option<Connector>,
    on_close: EventHandler<()>,
    on_saved: EventHandler<Connector>,
) -> Element {
    let start = initial.clone();
    let field = |f: fn(&Connector) -> String| start.as_ref().map(f).unwrap_or_default();
    let rest0 = start
        .as_ref()
        .and_then(|c| c.rest.clone())
        .unwrap_or_default();
    let hook0 = start
        .as_ref()
        .and_then(|c| c.webhook.clone())
        .unwrap_or_default();
    let kind = use_signal(|| start.as_ref().map_or(ConnectorKind::Postgres, |c| c.kind));
    let name = use_signal(|| field(|c| c.name.clone()));
    let host = use_signal(|| field(|c| c.host.clone()));
    let port = use_signal(|| {
        let p = start
            .as_ref()
            .map_or(default_port(ConnectorKind::Postgres), |c| c.port);
        p.to_string()
    });
    let database = use_signal(|| field(|c| c.database.clone()));
    let username = use_signal(|| field(|c| c.username.clone()));
    let mut tls = use_signal(|| start.as_ref().is_none_or(|c| c.tls));
    let base_path = use_signal(|| rest0.base_path.clone());
    let auth = use_signal(|| rest0.auth);
    let header_name = use_signal(|| rest0.header_name.clone());
    let token_url = use_signal(|| rest0.token_url.clone());
    let scope = use_signal(|| rest0.scope.clone());
    let sig_header = use_signal(|| hook0.signature_header.clone());
    let sig_prefix = use_signal(|| hook0.signature_prefix.clone());
    let ts_header = use_signal(|| hook0.timestamp_header.clone());
    let password = use_signal(SecretDraft::default);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let id = initial.as_ref().map(|c| c.id.clone());
    let version = initial.as_ref().map(|c| c.version);
    let creating = initial.is_none();

    let input = move || {
        let k = kind();
        let hook = k == ConnectorKind::Webhook;
        ConnectorInput {
            name: name().trim().to_string(),
            kind: k,
            host: if hook {
                String::new()
            } else {
                host().trim().to_string()
            },
            port: if hook {
                0
            } else {
                port().trim().parse().unwrap_or(0)
            },
            database: database().trim().to_string(),
            username: username().trim().to_string(),
            tls: k == ConnectorKind::Mysql || tls(),
            rest: (k == ConnectorKind::Rest).then(|| RestSettings {
                base_path: base_path().trim().to_string(),
                auth: auth(),
                header_name: header_name().trim().to_string(),
                token_url: token_url().trim().to_string(),
                scope: scope().trim().to_string(),
            }),
            webhook: hook.then(|| WebhookSettings {
                signature_header: sig_header().trim().to_string(),
                signature_prefix: sig_prefix().trim().to_string(),
                timestamp_header: ts_header().trim().to_string(),
            }),
            password: password().to_input(),
            expected_version: version,
        }
    };
    let submit = move |_| {
        let id = id.clone();
        let body = input();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::pipelines::save_connector(id.as_deref(), &body).await {
                Ok(saved) => on_saved.call(saved),
                Err(e) => error.set(Some(localize(&e))),
            }
            busy.set(false);
        });
    };
    let title = if creating {
        t!("pipe-connector-add")
    } else {
        t!("pipe-connector-edit")
    };
    let k = kind();
    let database_kind = matches!(k, ConnectorKind::Postgres | ConnectorKind::Mysql);
    let reaches = k != ConnectorKind::Webhook;
    let secret = secret_label(k, auth());
    let user_label = match (k, auth()) {
        (ConnectorKind::Rest, RestAuth::Oauth2ClientCredentials) => t!("pipe-connector-client-id"),
        _ => t!("pipe-connector-username"),
    }
    .to_string();
    let rest_user = k == ConnectorKind::Rest
        && matches!(auth(), RestAuth::Basic | RestAuth::Oauth2ClientCredentials);
    let has_secret = initial.as_ref().is_some_and(|c| c.has_password);
    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: t!("pipe-save").to_string(),
            on_close: move |_| on_close.call(()),
            on_submit: submit,
            busy: busy(),
            valid: input().violation().is_none(),
            div { class: "space-y-4",
                if creating {
                    KindPicker { kind, port }
                } else {
                    div {
                        p { class: FIELD_LABEL, {t!("pipe-col-kind")} }
                        p { class: "text-sm text-ink", "{kind_label(k)}" }
                        p { class: FIELD_HELP, {t!("pipe-connector-kind-locked")} }
                    }
                }
                TextField {
                    id: "conn-name",
                    label: t!("pipe-col-name").to_string(),
                    value: name,
                }
                if reaches {
                    HostFields { host, port }
                }
                if database_kind {
                    TextField {
                        id: "conn-db",
                        label: t!("pipe-connector-database").to_string(),
                        value: database,
                    }
                }
                if k == ConnectorKind::Rest {
                    RestFields {
                        base_path,
                        auth,
                        header_name,
                        token_url,
                        scope,
                    }
                }
                if database_kind || rest_user {
                    TextField { id: "conn-user", label: user_label, value: username }
                }
                if k == ConnectorKind::Webhook {
                    WebhookFields { sig_header, sig_prefix, ts_header }
                }
                if let Some(label) = secret {
                    SecretField {
                        label,
                        draft: password,
                        can_manage: can_manage_secrets(),
                    }
                }
                if has_secret && reaches {
                    p { class: FIELD_HELP, {t!("pipe-connector-password-keep")} }
                }
                if k == ConnectorKind::Webhook {
                    p { class: FIELD_HELP, {t!("pipe-connector-webhook-secret-help")} }
                }
                if k == ConnectorKind::Mysql {
                    p { class: FIELD_HELP, {t!("pipe-connector-tls-required")} }
                } else if reaches {
                    label { class: "flex items-center gap-2 text-sm text-ink",
                        input {
                            r#type: "checkbox",
                            checked: tls(),
                            onchange: move |e| tls.set(e.checked()),
                        }
                        {t!("pipe-connector-tls")}
                    }
                    if !tls() {
                        p { class: FIELD_HELP, {t!("pipe-connector-tls-off")} }
                    }
                }
                if let Some(e) = error() {
                    p { class: FIELD_ERROR, role: "alert", "{e}" }
                }
            }
        }
    }
}

/// The kind, chosen once at creation: it decides every other field.
#[component]
fn KindPicker(kind: Signal<ConnectorKind>, port: Signal<String>) -> Element {
    let mut kind = kind;
    let mut port = port;
    rsx! {
        fieldset { class: "space-y-2",
            legend { class: FIELD_LABEL, {t!("pipe-col-kind")} }
            for k in KINDS {
                label {
                    key: "{k.wire()}",
                    class: "flex items-start gap-3 rounded-md border border-line px-3 py-2 cursor-pointer has-[:checked]:border-signal has-[:checked]:bg-signal-50",
                    input {
                        r#type: "radio",
                        name: "conn-kind",
                        class: "mt-1",
                        checked: kind() == k,
                        onchange: move |_| {
                            // A port left at the old default follows the new kind.
                            if port().trim() == default_port(kind()).to_string() {
                                port.set(default_port(k).to_string());
                            }
                            kind.set(k);
                        },
                    }
                    span {
                        span { class: "block text-sm font-medium text-ink", "{kind_label(k)}" }
                        span { class: "block text-sm text-gray-600", "{kind_help(k)}" }
                    }
                }
            }
        }
    }
}

/// Host and port side by side.
#[component]
fn HostFields(host: Signal<String>, port: Signal<String>) -> Element {
    rsx! {
        div { class: "grid grid-cols-[1fr_6.5rem] gap-3",
            TextField {
                id: "conn-host",
                label: t!("pipe-connector-host").to_string(),
                value: host,
            }
            TextField {
                id: "conn-port",
                label: t!("pipe-connector-port").to_string(),
                value: port,
            }
        }
    }
}

/// Base path and authentication of a REST connector.
#[component]
fn RestFields(
    base_path: Signal<String>,
    auth: Signal<RestAuth>,
    header_name: Signal<String>,
    token_url: Signal<String>,
    scope: Signal<String>,
) -> Element {
    let mut auth = auth;
    let current = auth();
    rsx! {
        TextField {
            id: "conn-base",
            label: t!("pipe-connector-base-path").to_string(),
            value: base_path,
        }
        div {
            label { class: FIELD_LABEL, r#for: "conn-auth", {t!("pipe-connector-auth")} }
            select {
                id: "conn-auth",
                class: INPUT,
                onchange: move |e| {
                    let picked = AUTHS.into_iter().nth(e.value().parse().unwrap_or(0));
                    auth.set(picked.unwrap_or_default());
                },
                // `selected` per option: a select value set before its options
                // exist would show the first one whatever the state.
                for (i, a) in AUTHS.into_iter().enumerate() {
                    option { key: "{i}", value: "{i}", selected: a == current, "{auth_label(a)}" }
                }
            }
        }
        if current == RestAuth::Header {
            TextField {
                id: "conn-header",
                label: t!("pipe-connector-header-name").to_string(),
                value: header_name,
            }
        }
        if current == RestAuth::Oauth2ClientCredentials {
            TextField {
                id: "conn-token-url",
                label: t!("pipe-connector-token-url").to_string(),
                value: token_url,
            }
            TextField {
                id: "conn-scope",
                label: t!("pipe-connector-scope").to_string(),
                value: scope,
            }
        }
    }
}

/// How the sender of a webhook signs its calls.
#[component]
fn WebhookFields(
    sig_header: Signal<String>,
    sig_prefix: Signal<String>,
    ts_header: Signal<String>,
) -> Element {
    rsx! {
        div { class: "grid grid-cols-[1fr_8rem] gap-3",
            TextField {
                id: "conn-sig-header",
                label: t!("pipe-connector-signature-header").to_string(),
                value: sig_header,
            }
            TextField {
                id: "conn-sig-prefix",
                label: t!("pipe-connector-signature-prefix").to_string(),
                value: sig_prefix,
            }
        }
        TextField {
            id: "conn-ts-header",
            label: t!("pipe-connector-timestamp-header").to_string(),
            value: ts_header,
            help: t!("pipe-connector-timestamp-help").to_string(),
        }
    }
}
