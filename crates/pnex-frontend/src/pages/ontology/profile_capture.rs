//! Profile capture of a person (crm.md D333) and its
//! prospecting objection (D341): one pasted profile address, captured once
//! by the caller's browser agent in its signed-in session, and where the
//! captures of the person stand. The same dialog asks the capture of a
//! company's employee list on LinkedIn from an organization (D779).

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::app::Route;
use crate::components::badges::date_label;
use crate::components::crud::form::FormDialog;
use crate::state::toasts;

/// `people`: `person_id` is an organization and the address its employee
/// list (D779); no objection there, it belongs to each person.
#[component]
pub fn ProfileCaptureDialog(
    person_id: String,
    #[props(default)] people: bool,
    on_close: EventHandler<()>,
) -> Element {
    let mut url = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut reload = use_signal(|| 0u32);
    // The objection is final: a first click asks to confirm.
    let mut confirming = use_signal(|| false);
    let pid = person_id.clone();
    let opposition = use_resource(move || {
        let id = pid.clone();
        async move {
            let _ = reload();
            if people {
                return None;
            }
            api::extension::opposition(&id).await.ok()
        }
    });
    let opposed = opposition
        .read()
        .clone()
        .flatten()
        .is_some_and(|o| o.opposed);
    let submit_id = person_id.clone();
    let oppose_id = person_id.clone();
    let title = if people {
        t!("people-capture-title")
    } else {
        t!("profile-capture-title")
    };
    let placeholder = if people {
        "https://www.linkedin.com/company/…/people/"
    } else {
        "https://www.linkedin.com/in/…"
    };

    rsx! {
        FormDialog {
            title: title.to_string(),
            submit_label: t!("profile-capture-submit").to_string(),
            on_close: move |_| on_close.call(()),
            busy: busy(),
            valid: !opposed && !url().trim().is_empty(),
            on_submit: move |_| {
                let id = submit_id.clone();
                let raw = url();
                let done = t!("profile-capture-requested").to_string();
                busy.set(true);
                spawn(async move {
                    let asked = if people {
                        api::extension::capture_people(&id, &raw).await
                    } else {
                        api::extension::capture_profile(&id, &raw).await
                    };
                    match asked {
                        Ok(_) => {
                            toasts::success(done);
                            on_close.call(());
                        }
                        Err(e) => toasts::error(e),
                    }
                    busy.set(false);
                });
            },
            BrowserNotice {}
            if opposed {
                p {
                    class: "rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-800",
                    "data-testid": "profile-opposed",
                    {t!("profile-capture-opposed")}
                }
            } else {
                label { class: "block text-sm font-medium text-gray-700",
                    if people {
                        {t!("people-capture-url")}
                    } else {
                        {t!("profile-capture-url")}
                    }
                    input {
                        class: "mt-1 w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                        r#type: "url",
                        "data-testid": "profile-url",
                        placeholder,
                        value: "{url}",
                        oninput: move |e| url.set(e.value()),
                    }
                }
                if people {
                    p { class: "text-xs text-gray-500", {t!("people-capture-help")} }
                } else {
                    p { class: "text-xs text-gray-500", {t!("profile-capture-help")} }
                }
                // D254: the platform's terms may forbid automated access.
                p { class: "rounded-lg bg-amber-50 px-3 py-2 text-xs text-amber-800",
                    {t!("profile-capture-terms-warning")}
                }
                button {
                    hidden: people,
                    class: "text-xs text-red-700 underline",
                    r#type: "button",
                    "data-testid": "profile-oppose",
                    disabled: busy(),
                    onclick: move |_| {
                        if !confirming() {
                            confirming.set(true);
                            return;
                        }
                        let id = oppose_id.clone();
                        let done = t!("profile-opposition-recorded").to_string();
                        spawn(async move {
                            match api::extension::oppose(&id).await {
                                Ok(_) => {
                                    toasts::success(done);
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(e) => toasts::error(e),
                            }
                        });
                    },
                    if confirming() {
                        {t!("profile-opposition-confirm")}
                    } else {
                        {t!("profile-opposition-record")}
                    }
                }
            }
        }
    }
}

/// Says when a capture of the caller would wait, like the server decides it:
/// their own browser if they have one, else a common one (D550).
#[component]
fn BrowserNotice() -> Element {
    let agents = use_resource(|| async { api::extension::agents().await.ok() });
    let Some(Some(agents)) = agents.read().clone() else {
        return rsx! {};
    };
    let own: Vec<_> = agents.iter().filter(|a| a.mine && !a.common).collect();
    let pool: Vec<_> = if own.is_empty() {
        agents.iter().filter(|a| a.mine || a.common).collect()
    } else {
        own
    };
    if pool.iter().any(|a| a.online) {
        return rsx! {};
    }
    let text = if pool.is_empty() {
        t!("profile-browser-none")
    } else {
        t!("profile-browser-offline")
    };
    rsx! {
        p {
            class: "rounded-lg bg-amber-50 px-3 py-2 text-sm text-amber-800",
            "data-testid": "profile-browser-notice",
            "{text} "
            Link { class: "underline", to: Route::BrowserAgents {}, {t!("browser-agents-title")} }
        }
    }
}

/// Captures asked for the person, on its page; empty until one is asked.
/// Refreshed while one is open: the agent runs it at its own pace.
#[component]
pub fn ProfileCaptures(
    person_id: String,
    #[props(default)] people: bool,
    reload: Signal<u32>,
) -> Element {
    let mut tick = use_signal(|| 0u32);
    let id = person_id.clone();
    let captures = use_resource(move || {
        let id = id.clone();
        async move {
            let _ = (reload(), tick());
            if people {
                api::extension::people_captures(&id).await.ok()
            } else {
                api::extension::profile_captures(&id).await.ok()
            }
        }
    });
    let rows = captures.read().clone().flatten().unwrap_or_default();
    let any_open = rows.iter().any(|c| c.open);
    use_future(move || async move {
        loop {
            crate::util::sleep(std::time::Duration::from_secs(15)).await;
            if captures
                .peek()
                .clone()
                .flatten()
                .is_some_and(|r| r.iter().any(|c| c.open))
            {
                tick.with_mut(|t| *t += 1);
            }
        }
    });
    if rows.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "mt-6 space-y-2", "data-testid": "profile-captures",
            h3 { class: "text-sm font-medium text-ink",
                if people {
                    {t!("people-captures-title")}
                } else {
                    {t!("profile-captures-title")}
                }
            }
            if any_open {
                BrowserNotice {}
            }
            ul { class: "space-y-1 text-sm",
                for c in rows {
                    li {
                        key: "{c.id}",
                        class: "flex flex-wrap items-baseline gap-x-3",
                        span { class: "break-all text-ink", "{c.url}" }
                        span { class: "text-gray-600", {capture_label(&c)} }
                        span { class: "text-xs text-gray-500", {date_label(&c.created_at)} }
                    }
                }
            }
        }
    }
}

fn capture_label(c: &pnex_core::browser_agent::ProfileCapture) -> String {
    if c.open {
        crate::pages::browser_agents::state_text(c.state, c.state_code.as_deref())
    } else if c.captured {
        t!("profile-capture-state-done").to_string()
    } else {
        t!("profile-capture-state-stopped").to_string()
    }
}

/// The caller's LinkedIn relationship with the person and the exploration
/// of their connections (D976, D977): each user declares their own state;
/// only a connected person can be explored, and only the people at the
/// organizations chosen in the prospecting pack are kept.
#[component]
pub fn PersonConnection(person_id: String, can_capture: bool) -> Element {
    use pnex_core::browser_agent::ConnectionState as S;
    let mut tick = use_signal(|| 0u32);
    let mut busy = use_signal(|| false);
    let id = person_id.clone();
    let state = use_resource(move || {
        let id = id.clone();
        async move {
            let _ = tick();
            api::extension::connection(&id).await.ok()
        }
    });
    let id = person_id.clone();
    let runs = use_resource(move || {
        let id = id.clone();
        async move {
            let _ = tick();
            api::extension::connections_captures(&id).await.ok()
        }
    });
    use_future(move || async move {
        loop {
            crate::util::sleep(std::time::Duration::from_secs(15)).await;
            if runs
                .peek()
                .clone()
                .flatten()
                .is_some_and(|r| r.iter().any(|c| c.open))
            {
                tick.with_mut(|t| *t += 1);
            }
        }
    });
    let Some(current) = state.read().clone().flatten() else {
        return rsx! {};
    };
    let rows = runs.read().clone().flatten().unwrap_or_default();
    let connected = current.state == S::Connected;
    let open = rows.iter().any(|c| c.open);
    let set_id = person_id.clone();
    let explore_id = person_id.clone();
    rsx! {
        div { class: "mt-6 space-y-2", "data-testid": "person-connection",
            h3 { class: "text-sm font-medium text-ink", {t!("connection-title")} }
            div { class: "flex flex-wrap items-center gap-3",
                select {
                    aria_label: t!("connection-state-label"),
                    class: "px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white",
                    disabled: !can_capture || busy(),
                    "data-testid": "connection-state",
                    onchange: move |e| {
                        let id = set_id.clone();
                        let new = S::parse(&e.value());
                        busy.set(true);
                        spawn(async move {
                            if let Err(e) = api::extension::set_connection(&id, new).await {
                                toasts::error(e);
                            }
                            busy.set(false);
                            tick.with_mut(|t| *t += 1);
                        });
                    },
                    for (value, label) in [
                        (S::None, t!("connection-state-none")),
                        (S::InvitationSent, t!("connection-state-invitation-sent")),
                        (S::Connected, t!("connection-state-connected")),
                    ]
                    {
                        option {
                            value: value.as_str(),
                            selected: current.state == value,
                            "{label}"
                        }
                    }
                }
                if can_capture {
                    button {
                        r#type: "button",
                        class: crate::components::ui::BTN_SECONDARY_SM,
                        disabled: !connected || open || busy(),
                        title: if !connected { t!("connection-explore-needs-connected") } else { t!("connection-explore-help") },
                        "data-testid": "connection-explore",
                        onclick: move |_| {
                            let id = explore_id.clone();
                            let done = t!("profile-capture-requested").to_string();
                            busy.set(true);
                            spawn(async move {
                                match api::extension::capture_connections(&id).await {
                                    Ok(_) => toasts::success(done),
                                    Err(e) => toasts::error(e),
                                }
                                busy.set(false);
                                tick.with_mut(|t| *t += 1);
                            });
                        },
                        {t!("connection-explore")}
                    }
                }
                if let Some(at) = current.explored_at.as_deref() {
                    span { class: "text-xs text-gray-500",
                        {t!("connection-explored-on", date : date_label(at))}
                    }
                }
            }
            if connected {
                p { class: "text-xs text-gray-500", {t!("connection-explore-help")} }
            }
            if open {
                BrowserNotice {}
            }
            if !rows.is_empty() {
                ul { class: "space-y-1 text-sm",
                    for c in rows {
                        li {
                            key: "{c.id}",
                            class: "flex flex-wrap items-baseline gap-x-3",
                            span { class: "text-gray-600", {exploration_label(&c)} }
                            span { class: "text-xs text-gray-500", {date_label(&c.created_at)} }
                        }
                    }
                }
            }
        }
    }
}

/// An exploration's state; a stopped one says why (hidden list…).
fn exploration_label(c: &pnex_core::browser_agent::ProfileCapture) -> String {
    match (c.captured, c.state_code.as_deref()) {
        (false, Some(code)) if !c.open => crate::api::error_i18n::localize_code(code, None, code),
        _ => capture_label(c),
    }
}
