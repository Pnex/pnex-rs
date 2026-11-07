//! Profile capture of a person (account-mapping.md D333) and its
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

/// Searches of the person's network profile (D788), on its page: the query
/// reached, each candidate with its score and what became of it. Refreshed
/// while one runs: the browser agent plays them at its own pace.
#[component]
pub fn PersonMatches(person_id: String, reload: Signal<u32>) -> Element {
    let mut tick = use_signal(|| 0u32);
    let id = person_id.clone();
    let matches = use_resource(move || {
        let id = id.clone();
        async move {
            let _ = (reload(), tick());
            api::extension::person_matches(&id).await.ok()
        }
    });
    use_future(move || async move {
        loop {
            crate::util::sleep(std::time::Duration::from_secs(15)).await;
            if matches
                .peek()
                .clone()
                .flatten()
                .is_some_and(|r| r.iter().any(|m| m.state.open()))
            {
                tick.with_mut(|t| *t += 1);
            }
        }
    });
    let rows = matches.read().clone().flatten().unwrap_or_default();
    if rows.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "mt-6 space-y-3", "data-testid": "person-matches",
            h3 { class: "text-sm font-medium text-ink", {t!("person-matches-title")} }
            for m in rows {
                div { key: "{m.id}", class: "space-y-1 text-sm",
                    div { class: "flex flex-wrap items-baseline gap-x-3",
                        span {
                            class: "font-medium text-ink",
                            "data-testid": "person-match-state",
                            {match_state_label(&m)}
                        }
                        span { class: "text-gray-600", {match_query_label(&m)} }
                        span { class: "text-xs text-gray-500", {date_label(&m.created_at)} }
                    }
                    ul { class: "space-y-1 pl-3",
                        for c in m.candidates.clone() {
                            li {
                                key: "{c.url}",
                                class: "flex flex-wrap items-baseline gap-x-2",
                                a {
                                    class: "break-all text-signal underline",
                                    href: "{c.url}",
                                    target: "_blank",
                                    rel: "noopener noreferrer",
                                    "{c.url}"
                                }
                                span { class: "text-gray-600", {candidate_label(&c)} }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn match_query_label(m: &pnex_core::person_match::PersonMatch) -> String {
    let n = (m.step as usize + 1).min(m.queries.len());
    t!("person-match-query", n : n, total : m.queries.len()).to_string()
}

fn match_state_label(m: &pnex_core::person_match::PersonMatch) -> String {
    use pnex_core::person_match::MatchState as S;
    let state = match m.state {
        S::Searching => t!("person-match-state-searching"),
        S::Matched => t!("person-match-state-matched"),
        S::Proposed => t!("person-match-state-proposed"),
        S::NotFound => t!("person-match-state-not_found"),
        S::Stopped => t!("person-match-state-stopped"),
    }
    .to_string();
    match m.state_code.as_deref() {
        // A registered error code, else a rule run reason (permission, sign-in wall…).
        Some(code) => {
            let reason = crate::api::error_i18n::resolve(&format!("web-sites-reason-{code}"), None);
            let text = crate::api::error_i18n::localize_code(code, None, &reason);
            format!("{state} · {text}")
        }
        None => state,
    }
}

fn candidate_label(c: &pnex_core::person_match::MatchCandidate) -> String {
    use pnex_core::person_match::Verdict;
    let mut parts = Vec::new();
    if let Some(score) = c.score {
        parts.push(t!("person-match-score", score : score).to_string());
    }
    if let Some(v) = c.verdict {
        parts.push(
            match v {
                Verdict::Write => t!("person-match-verdict-write"),
                Verdict::Propose => t!("person-match-verdict-propose"),
                Verdict::Drop => t!("person-match-verdict-drop"),
            }
            .to_string(),
        );
    }
    if c.left_company {
        parts.push(t!("person-match-left").to_string());
    }
    parts.join(" · ")
}
