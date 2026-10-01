//! Edge agent UI (D95): install card (single-use enrollment code + install
//! one-liners, live connection wait) and the agent detail panel (machine
//! facts, distinct keys quota, discovered keys with the per-key OpenObserve
//! history toggle, ingestion examples).

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::badges::date_label;
use super::icons;
use crate::api;
use crate::state::toasts;
use crate::util::sleep;

#[derive(Clone, Copy, PartialEq)]
enum InstallTab {
    Linux,
    Windows,
    Manual,
}

/// One copyable command block.
#[component]
fn CommandBlock(command: String) -> Element {
    let mut copied = use_signal(|| false);
    let to_copy = command.clone();
    rsx! {
        div { class: "relative",
            pre { class: "whitespace-pre-wrap break-all rounded-lg bg-gray-900 p-3 pr-20 font-mono text-xs text-green-300",
                "{command}"
            }
            button {
                class: "absolute right-2 top-2 rounded bg-gray-700 px-2 py-1 text-xs text-white hover:bg-gray-600",
                r#type: "button",
                onclick: move |_| {
                    crate::util::copy_text(&to_copy);
                    copied.set(true);
                    spawn(async move {
                        sleep(Duration::from_secs(2)).await;
                        copied.set(false);
                    });
                },
                if copied() { {t!("agent-install-copied")} } else { {t!("agent-install-copy")} }
            }
        }
    }
}

/// Install card: generates a fresh code on mount (and on demand), shows the
/// per-platform commands, then waits for the first connection.
#[component]
pub fn AgentInstall(device_pk: i64) -> Element {
    let mut server = use_signal(api::config::api_base);
    let mut enrollment = use_signal(|| None::<pnex_core::agent::AgentEnrollmentCreated>);
    let mut generating = use_signal(|| false);
    let mut tab = use_signal(|| InstallTab::Linux);
    let mut connected = use_signal(|| false);

    let mut generate = move || {
        generating.set(true);
        spawn(async move {
            match api::agents::new_enrollment(device_pk).await {
                Ok(e) => enrollment.set(Some(e)),
                Err(err) => toasts::error(err),
            }
            generating.set(false);
        });
    };
    use_hook(move || generate());

    // Connection wait: poll the device detail until the agent is online.
    use_future(move || async move {
        loop {
            sleep(Duration::from_secs(3)).await;
            if let Ok(d) = api::devices::detail(device_pk).await {
                if d.connected && enrollment().is_some() {
                    connected.set(true);
                }
            }
        }
    });

    let Some(created) = enrollment() else {
        return rsx! {
            p { class: "text-sm text-gray-500", {t!("agent-install-generating")} }
        };
    };
    let commands =
        api::agents::install_commands(&server(), &created.code, created.ca_sha256.as_deref());
    let expires = date_label(&created.expires_at);
    let base = server().trim().trim_end_matches('/').to_string();

    rsx! {
        div { class: "space-y-4",
            p { class: "text-sm text-gray-600", {t!("agent-install-help", time: expires)} }
            div { class: "flex flex-wrap items-center gap-3",
                span { class: "rounded-lg bg-blue-50 px-3 py-2 font-mono text-lg font-semibold tracking-widest text-blue-800",
                    "{created.code}"
                }
                button {
                    class: "rounded-lg border border-gray-300 px-3 py-2 text-sm text-gray-600 hover:bg-gray-50",
                    r#type: "button",
                    disabled: generating(),
                    onclick: move |_| generate(),
                    icons::RefreshCw { class: "mr-1 inline h-4 w-4" }
                    {t!("agent-install-new-code")}
                }
            }
            label { class: "block text-xs font-semibold uppercase tracking-wider text-gray-500",
                {t!("agent-install-server")}
                input {
                    class: "mt-1 w-full rounded-lg border border-gray-300 px-3 py-2 font-mono text-sm normal-case",
                    r#type: "url",
                    value: "{server}",
                    oninput: move |e| server.set(e.value()),
                }
            }
            div { class: "flex gap-1 border-b border-gray-200",
                for (t_id, label) in [
                    (InstallTab::Linux, t!("agent-install-tab-linux")),
                    (InstallTab::Windows, t!("agent-install-tab-windows")),
                    (InstallTab::Manual, t!("agent-install-tab-manual")),
                ] {
                    button {
                        class: if tab() == t_id {
                            "border-b-2 border-blue-600 px-3 py-2 text-sm font-medium text-blue-700"
                        } else {
                            "px-3 py-2 text-sm text-gray-500 hover:text-gray-800"
                        },
                        r#type: "button",
                        onclick: move |_| tab.set(t_id),
                        "{label}"
                    }
                }
            }
            match tab() {
                InstallTab::Linux => rsx! { CommandBlock { command: commands.linux.clone() } },
                InstallTab::Windows => rsx! { CommandBlock { command: commands.windows.clone() } },
                InstallTab::Manual => rsx! {
                    div { class: "space-y-2",
                        p { class: "text-xs text-gray-500", {t!("agent-install-manual-help")} }
                        div { class: "flex flex-wrap gap-2",
                            for (target, _file) in pnex_core::agent::AGENT_TARGETS.iter() {
                                a {
                                    key: "{target}",
                                    class: "rounded-full bg-gray-100 px-3 py-1 font-mono text-xs text-gray-700 hover:bg-gray-200",
                                    href: "{base}/api/v1/agent/download/{target}",
                                    "{target}"
                                }
                            }
                        }
                        CommandBlock { command: commands.manual.clone() }
                    }
                },
            }
            if let Some(fp) = created.ca_sha256.clone() {
                p { class: "break-all text-[11px] text-gray-400",
                    {t!("agent-install-fingerprint")}
                    " "
                    code { "{fp}" }
                }
            }
            if connected() {
                div { class: "rounded-lg bg-green-50 px-3 py-2 text-sm font-medium text-green-800",
                    icons::Check { class: "mr-1 inline h-4 w-4" }
                    {t!("agent-install-connected")}
                }
            } else {
                div { class: "flex items-center gap-2 text-sm text-gray-500",
                    span { class: "inline-block h-3 w-3 animate-spin rounded-full border-b-2 border-blue-600" }
                    {t!("agent-install-waiting")}
                }
            }
        }
    }
}

/// Ingestion examples (code, not translated).
const EXAMPLE_CURL: &str = r#"curl -X POST http://127.0.0.1:7070/v1/points \
  -H 'content-type: application/json' \
  -d '{"key":"temperature","value":21.5,"unit":"°C"}'"#;
const EXAMPLE_PYTHON: &str = r#"import json, urllib.request

def push(key, value, unit=None, record=False):
    body = json.dumps({"key": key, "value": value, "unit": unit, "record": record}).encode()
    req = urllib.request.Request("http://127.0.0.1:7070/v1/points", body,
                                 {"content-type": "application/json"})
    urllib.request.urlopen(req, timeout=5)

push("temperature", 21.5, "°C")
push("status", {"state": "running", "load": 0.42})"#;
const EXAMPLE_TS: &str = r#"await fetch("http://127.0.0.1:7070/v1/points", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify([
    { key: "power", value: 1520, unit: "W" },
    { key: "door_open", value: false },
  ]),
});"#;
const EXAMPLE_PS: &str = r#"Invoke-RestMethod -Method Post -Uri http://127.0.0.1:7070/v1/points `
  -ContentType 'application/json' `
  -Body (@{ key = 'cpu_load'; value = 37.5; unit = '%' } | ConvertTo-Json)"#;

/// Agent detail panel (device page).
#[component]
pub fn AgentPanel(device_pk: i64, can_write: bool) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut show_install = use_signal(|| false);
    let mut quota_input = use_signal(String::new);
    let mut example = use_signal(|| 0usize);

    let info = use_resource(move || async move {
        let _ = reload();
        api::agents::info(device_pk).await
    });
    let keys = use_resource(move || async move {
        let _ = reload();
        api::agents::keys(device_pk).await
    });

    // Quota input follows the server value on (re)load.
    use_effect(move || {
        if let Some(Ok(i)) = &*info.read() {
            quota_input.set(i.max_keys.to_string());
        }
    });

    let save_quota = move |_| {
        let Ok(max) = quota_input().trim().parse::<i32>() else {
            toasts::error("err-agent-quota-invalid");
            return;
        };
        spawn(async move {
            match api::agents::set_max_keys(device_pk, max).await {
                Ok(_) => toasts::success("agent-panel-quota-saved"),
                Err(err) => toasts::error(err),
            }
            reload.with_mut(|r| *r += 1);
        });
    };

    let examples = [
        ("curl", EXAMPLE_CURL),
        ("Python", EXAMPLE_PYTHON),
        ("TypeScript", EXAMPLE_TS),
        ("PowerShell", EXAMPLE_PS),
    ];

    rsx! {
        div { class: "space-y-6 p-6",
            {agent_facts(&info.read())}
            if can_write {
                div { class: "space-y-2",
                    button {
                        class: "rounded-lg border border-gray-300 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50",
                        r#type: "button",
                        onclick: move |_| show_install.set(!show_install()),
                        icons::RefreshCw { class: "mr-1 inline h-4 w-4" }
                        {t!("agent-panel-reinstall")}
                    }
                    if show_install() {
                        p { class: "text-xs text-amber-700", {t!("agent-panel-reinstall-help")} }
                        AgentInstall { device_pk }
                    }
                }
                div { class: "flex items-end gap-2",
                    label { class: "text-xs font-semibold uppercase tracking-wider text-gray-500",
                        {t!("agent-panel-quota")}
                        input {
                            class: "mt-1 block w-32 rounded-lg border border-gray-300 px-3 py-1.5 text-sm",
                            r#type: "number",
                            min: "1",
                            value: "{quota_input}",
                            oninput: move |e| quota_input.set(e.value()),
                        }
                    }
                    button {
                        class: "rounded-lg bg-blue-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-blue-700",
                        r#type: "button",
                        onclick: save_quota,
                        {t!("agent-panel-save")}
                    }
                }
            }
            div { class: "space-y-2",
                h3 { class: "text-sm font-semibold uppercase tracking-wider text-gray-500", {t!("agent-keys-title")} }
                p { class: "text-xs text-gray-500", {t!("agent-keys-help")} }
                match &*keys.read() {
                    Some(Ok(list)) if list.is_empty() => rsx! {
                        p { class: "py-4 text-center text-sm text-gray-400", {t!("agent-keys-empty")} }
                    },
                    Some(Ok(list)) => rsx! {
                        KeysTable { device_pk, keys: list.clone(), can_write, on_changed: move |_| reload.with_mut(|r| *r += 1) }
                    },
                    Some(Err(err)) => rsx! {
                        p { class: "text-sm text-red-600", {err.message.clone()} }
                    },
                    None => rsx! {},
                }
            }
            div { class: "space-y-2",
                h3 { class: "text-sm font-semibold uppercase tracking-wider text-gray-500", {t!("agent-examples-title")} }
                p { class: "text-xs text-gray-500", {t!("agent-examples-help")} }
                div { class: "flex gap-1",
                    for (i, (name, _)) in examples.iter().enumerate() {
                        button {
                            key: "{name}",
                            class: if example() == i {
                                "rounded-full bg-blue-600 px-3 py-1 text-xs text-white"
                            } else {
                                "rounded-full bg-gray-100 px-3 py-1 text-xs text-gray-600 hover:bg-gray-200"
                            },
                            r#type: "button",
                            onclick: move |_| example.set(i),
                            "{name}"
                        }
                    }
                }
                CommandBlock { command: examples[example().min(examples.len() - 1)].1.to_string() }
            }
        }
    }
}

fn agent_facts(
    info: &Option<Result<pnex_core::agent::AgentInfo, api::error::ApiError>>,
) -> Element {
    let Some(Ok(i)) = info else {
        return rsx! {};
    };
    let host = match (&i.hostname, &i.os, &i.arch) {
        (Some(h), Some(o), Some(a)) => format!("{h} ({o}/{a})"),
        (Some(h), _, _) => h.clone(),
        _ => "—".to_string(),
    };
    let enrolled = i
        .enrolled_at
        .as_deref()
        .map(date_label)
        .unwrap_or_else(|| t!("agent-panel-never"));
    let version = i.agent_version.clone().unwrap_or_else(|| "—".into());
    rsx! {
        div { class: "grid grid-cols-1 gap-3 sm:grid-cols-3",
            for (label, value) in [
                (t!("agent-panel-host"), host),
                (t!("agent-panel-version"), version),
                (t!("agent-panel-enrolled"), enrolled),
            ] {
                div { key: "{label}",
                    div { class: "text-xs uppercase tracking-wider text-gray-400", "{label}" }
                    div { class: "font-mono text-sm text-gray-800", "{value}" }
                }
            }
        }
    }
}

#[component]
fn KeysTable(
    device_pk: i64,
    keys: Vec<pnex_core::agent::AgentKey>,
    can_write: bool,
    on_changed: Callback<()>,
) -> Element {
    rsx! {
        div { class: "overflow-x-auto rounded-lg border border-gray-200",
            table { class: "min-w-full text-sm",
                thead { class: "bg-gray-50 text-left text-xs uppercase tracking-wider text-gray-500",
                    tr {
                        th { class: "px-3 py-2", {t!("agent-keys-col-key")} }
                        th { class: "px-3 py-2", {t!("agent-keys-col-unit")} }
                        th { class: "px-3 py-2", {t!("agent-keys-col-kind")} }
                        th { class: "px-3 py-2", {t!("agent-keys-col-last")} }
                        th { class: "px-3 py-2", {t!("agent-keys-col-history")} }
                        th { class: "px-3 py-2" }
                    }
                }
                tbody {
                    for k in keys {
                        KeyRow { key: "{k.id}", device_pk, row: k, can_write, on_changed }
                    }
                }
            }
        }
    }
}

#[component]
fn KeyRow(
    device_pk: i64,
    row: pnex_core::agent::AgentKey,
    can_write: bool,
    on_changed: Callback<()>,
) -> Element {
    let key_id = row.id;
    let recorded = row.record_o2;
    let toggle = move |_| {
        spawn(async move {
            match api::agents::set_record(device_pk, key_id, !recorded).await {
                Ok(_) => toasts::success("toast-saved"),
                Err(err) => toasts::error(err),
            }
            on_changed.call(());
        });
    };
    let forget = move |_| {
        spawn(async move {
            match api::agents::forget_key(device_pk, key_id).await {
                Ok(()) => toasts::success("toast-saved"),
                Err(err) => toasts::error(err),
            }
            on_changed.call(());
        });
    };
    rsx! {
        tr { class: "border-t border-gray-100",
            td { class: "px-3 py-2 font-mono text-gray-900", "{row.key}" }
            td { class: "px-3 py-2 text-gray-600", {row.unit.clone().unwrap_or_default()} }
            td { class: "px-3 py-2 text-gray-500", "{row.kind}" }
            td { class: "px-3 py-2 text-xs text-gray-500", {date_label(&row.last_seen_at)} }
            td { class: "px-3 py-2",
                input {
                    r#type: "checkbox",
                    class: "h-4 w-4",
                    checked: recorded,
                    disabled: !can_write,
                    onchange: toggle,
                }
            }
            td { class: "px-3 py-2 text-right",
                if can_write {
                    button {
                        class: "text-xs text-gray-400 hover:text-red-600",
                        r#type: "button",
                        title: t!("agent-keys-forget"),
                        onclick: forget,
                        icons::Trash2 { class: "h-4 w-4" }
                    }
                }
            }
        }
    }
}
