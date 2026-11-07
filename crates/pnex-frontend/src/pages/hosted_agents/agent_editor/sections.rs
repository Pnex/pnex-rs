//! Lists of an agent's record: variables, outputs, repositories, test services, caches,
//! and its pinned skills (read only).

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use super::{CacheDraft, Draft, ServiceDraft};
use crate::api;
use crate::components::secret_field::{SecretDraft, SecretField};
use crate::components::ui::{BTN_GHOST, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::access;
use pnex_core::agent_runs::{ArtifactOutput, RunRepo, GENERATED};
use pnex_core::agent_skills::SkillPin;
use pnex_core::SecretFieldView;

const ROW: &str = "rounded-lg border border-line p-3 space-y-3";

/// A heading with its add button.
#[component]
fn SectionHead(title: String, on_add: EventHandler<()>) -> Element {
    rsx! {
        div { class: "flex items-center justify-between",
            h3 { class: "text-sm font-semibold text-ink", {title} }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                onclick: move |_| on_add.call(()),
                {t!("common-add")}
            }
        }
    }
}

#[component]
fn RemoveButton(on_remove: EventHandler<()>) -> Element {
    rsx! {
        button {
            class: BTN_GHOST,
            r#type: "button",
            onclick: move |_| on_remove.call(()),
            {t!("agents-edit-remove")}
        }
    }
}

/// Name/value rows; with `generated`, a row may ask for a generated password (D717).
#[component]
pub(super) fn KvList(
    rows: Vec<(String, String)>,
    generated: bool,
    on_change: EventHandler<Vec<(String, String)>>,
) -> Element {
    let add = rows.clone();
    rsx! {
        div { class: "space-y-2",
            for (i, (k, value)) in rows.iter().cloned().enumerate() {
                KvRow {
                    key: "{i}",
                    name: k,
                    value,
                    generated,
                    on_change: {
                        let rows = rows.clone();
                        move |pair: (String, String)| {
                            let mut next = rows.clone();
                            next[i] = pair;
                            on_change.call(next);
                        }
                    },
                    on_remove: {
                        let rows = rows.clone();
                        move |_| {
                            let mut next = rows.clone();
                            next.remove(i);
                            on_change.call(next);
                        }
                    },
                }
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                onclick: move |_| {
                    let mut next = add.clone();
                    next.push((String::new(), String::new()));
                    on_change.call(next);
                },
                {t!("common-add")}
            }
        }
    }
}

#[component]
fn KvRow(
    name: String,
    value: String,
    generated: bool,
    on_change: EventHandler<(String, String)>,
    on_remove: EventHandler<()>,
) -> Element {
    let is_generated = value == GENERATED;
    let (n1, n2, n3, v1, v2) = (
        name.clone(),
        name.clone(),
        name.clone(),
        value.clone(),
        value.clone(),
    );
    rsx! {
        div { class: "flex flex-wrap items-center gap-2",
            input {
                class: "{INPUT} font-mono text-xs flex-1 min-w-32",
                aria_label: t!("agents-edit-key").to_string(),
                placeholder: t!("agents-edit-key").to_string(),
                value: "{n1}",
                oninput: move |e| on_change.call((e.value(), v1.clone())),
            }
            input {
                class: "{INPUT} font-mono text-xs flex-1 min-w-32",
                aria_label: t!("agents-edit-value").to_string(),
                placeholder: t!("agents-edit-value").to_string(),
                disabled: is_generated,
                value: "{v2}",
                oninput: move |e| on_change.call((n2.clone(), e.value())),
            }
            if generated {
                label { class: "flex items-center gap-1 text-xs text-ink",
                    input {
                        r#type: "checkbox",
                        checked: is_generated,
                        onchange: move |e| {
                            let value = if e.checked() { GENERATED.to_string() } else { String::new() };
                            on_change.call((n3.clone(), value));
                        },
                    }
                    {t!("agents-edit-generated")}
                }
            }
            RemoveButton { on_remove }
        }
    }
}

#[component]
pub(super) fn OutputsSection(draft: Signal<Draft>) -> Element {
    let mut draft = draft;
    let outputs = draft().outputs;
    rsx! {
        div { class: "space-y-2",
            SectionHead {
                title: t!("agents-edit-outputs").to_string(),
                on_add: move |_| {
                    draft
                        .write()
                        .outputs
                        .push(ArtifactOutput {
                            name: String::new(),
                            required: true,
                        })
                },
            }
            for (i, o) in outputs.into_iter().enumerate() {
                div { key: "{i}", class: "flex flex-wrap items-center gap-2",
                    input {
                        class: "{INPUT} font-mono text-xs flex-1 min-w-32",
                        aria_label: t!("agents-edit-output-name").to_string(),
                        placeholder: t!("agents-edit-output-name").to_string(),
                        value: "{o.name}",
                        oninput: move |e| draft.write().outputs[i].name = e.value(),
                    }
                    label { class: "flex items-center gap-1 text-xs text-ink",
                        input {
                            r#type: "checkbox",
                            checked: o.required,
                            onchange: move |e| draft.write().outputs[i].required = e.checked(),
                        }
                        {t!("agents-edit-output-required")}
                    }
                    RemoveButton {
                        on_remove: move |_| {
                            draft.write().outputs.remove(i);
                        },
                    }
                }
            }
        }
    }
}

fn secret_draft(id: &Option<String>) -> SecretDraft {
    match id.as_deref().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(secret_id) => SecretDraft::Keep(SecretFieldView {
            secret_id,
            name: String::new(),
        }),
        None => SecretDraft::Empty,
    }
}

fn secret_id(d: &SecretDraft) -> Option<String> {
    match d {
        SecretDraft::Keep(v) | SecretDraft::Pick(v) => Some(v.secret_id.to_string()),
        _ => None,
    }
}

#[component]
pub(super) fn ReposSection(draft: Signal<Draft>) -> Element {
    let mut draft = draft;
    let repos = draft().repos;
    rsx! {
        div { class: "space-y-2",
            SectionHead {
                title: t!("agents-edit-repos").to_string(),
                on_add: move |_| {
                    draft
                        .write()
                        .repos
                        .push(RunRepo {
                            url: String::new(),
                            git_ref: "main".into(),
                            path: String::new(),
                            secret_id: None,
                            push_secret_id: None,
                        })
                },
            }
            p { class: "text-xs text-slate", {t!("agents-edit-repos-hint")} }
            for (i, repo) in repos.into_iter().enumerate() {
                RepoRow {
                    key: "{i}",
                    draft,
                    index: i,
                    repo,
                }
            }
        }
    }
}

/// One repository; its two tokens are picked from the vault, never typed (D714, D775).
#[component]
fn RepoRow(draft: Signal<Draft>, index: usize, repo: RunRepo) -> Element {
    let mut draft = draft;
    let i = index;
    let mut read = use_signal(|| secret_draft(&repo.secret_id));
    let mut push = use_signal(|| secret_draft(&repo.push_secret_id));
    // A record replaced from outside (another version opened): adopt its tokens.
    let (read_id, push_id) = (repo.secret_id.clone(), repo.push_secret_id.clone());
    use_effect(use_reactive!(|read_id, push_id| {
        if secret_id(&read.peek()) != read_id {
            read.set(secret_draft(&read_id));
        }
        if secret_id(&push.peek()) != push_id {
            push.set(secret_draft(&push_id));
        }
    }));
    // A token picked here goes to the record.
    use_effect(move || {
        let (r, p) = (secret_id(&read()), secret_id(&push()));
        let same = draft
            .peek()
            .repos
            .get(i)
            .is_some_and(|x| x.secret_id == r && x.push_secret_id == p);
        if !same {
            if let Some(x) = draft.write().repos.get_mut(i) {
                x.secret_id = r;
                x.push_secret_id = p;
            }
        }
    });
    let can_pick = access::can("agents:write");
    rsx! {
        div { class: ROW,
            div { class: "grid grid-cols-1 md:grid-cols-4 gap-2",
                input {
                    class: "{INPUT} font-mono text-xs md:col-span-2",
                    aria_label: t!("agents-edit-repo-url").to_string(),
                    placeholder: "https://github.com/owner/repo.git",
                    value: "{repo.url}",
                    oninput: move |e| draft.write().repos[i].url = e.value(),
                }
                input {
                    class: "{INPUT} font-mono text-xs",
                    aria_label: t!("agents-edit-repo-ref").to_string(),
                    placeholder: t!("agents-edit-repo-ref").to_string(),
                    value: "{repo.git_ref}",
                    oninput: move |e| draft.write().repos[i].git_ref = e.value(),
                }
                input {
                    class: "{INPUT} font-mono text-xs",
                    aria_label: t!("agents-edit-repo-path").to_string(),
                    placeholder: t!("agents-edit-repo-path").to_string(),
                    value: "{repo.path}",
                    oninput: move |e| draft.write().repos[i].path = e.value(),
                }
            }
            div { class: "grid grid-cols-1 md:grid-cols-2 gap-3",
                SecretField {
                    label: t!("agents-edit-repo-read").to_string(),
                    draft: read,
                    can_manage: can_pick,
                    pick_only: true,
                }
                div {
                    SecretField {
                        label: t!("agents-edit-repo-push").to_string(),
                        draft: push,
                        can_manage: can_pick,
                        pick_only: true,
                    }
                    p { class: "text-xs text-slate mt-1", {t!("agents-edit-repo-push-hint")} }
                }
            }
            RemoveButton {
                on_remove: move |_| {
                    draft.write().repos.remove(i);
                },
            }
        }
    }
}

#[component]
pub(super) fn ServicesSection(draft: Signal<Draft>) -> Element {
    let mut draft = draft;
    let services = draft().services;
    rsx! {
        div { class: "space-y-2",
            SectionHead {
                title: t!("agents-edit-services").to_string(),
                on_add: move |_| draft.write().services.push(ServiceDraft::default()),
            }
            p { class: "text-xs text-slate", {t!("agents-edit-services-hint")} }
            for (i, s) in services.into_iter().enumerate() {
                ServiceRow {
                    key: "{i}",
                    draft,
                    index: i,
                    service: s,
                }
            }
        }
    }
}

#[component]
fn ServiceRow(draft: Signal<Draft>, index: usize, service: ServiceDraft) -> Element {
    let mut draft = draft;
    let i = index;
    rsx! {
        div { class: ROW,
            div { class: "grid grid-cols-1 md:grid-cols-3 gap-2",
                input {
                    class: "{INPUT} font-mono text-xs",
                    aria_label: t!("agents-edit-service-name").to_string(),
                    placeholder: t!("agents-edit-service-name").to_string(),
                    value: "{service.name}",
                    oninput: move |e| draft.write().services[i].name = e.value(),
                }
                input {
                    class: "{INPUT} font-mono text-xs md:col-span-2",
                    aria_label: t!("agents-edit-service-image").to_string(),
                    placeholder: "postgres@sha256:…",
                    value: "{service.image}",
                    oninput: move |e| draft.write().services[i].image = e.value(),
                }
            }
            div { class: "grid grid-cols-1 md:grid-cols-2 gap-2",
                div {
                    label { class: FIELD_LABEL, {t!("agents-edit-service-ready")} }
                    textarea {
                        class: "{INPUT} font-mono text-xs h-16",
                        aria_label: t!("agents-edit-service-ready").to_string(),
                        spellcheck: "false",
                        value: "{service.ready}",
                        oninput: move |e| draft.write().services[i].ready = e.value(),
                    }
                    p { class: "text-xs text-slate mt-1", {t!("agents-edit-command-hint")} }
                }
                div {
                    label { class: FIELD_LABEL, {t!("agents-edit-service-data")} }
                    input {
                        class: "{INPUT} font-mono text-xs",
                        aria_label: t!("agents-edit-service-data").to_string(),
                        value: "{service.data_paths}",
                        oninput: move |e| draft.write().services[i].data_paths = e.value(),
                    }
                }
            }
            div {
                label { class: FIELD_LABEL, {t!("agents-edit-service-env")} }
                KvList {
                    rows: service.env.clone(),
                    generated: true,
                    on_change: move |rows| draft.write().services[i].env = rows,
                }
            }
            RemoveButton {
                on_remove: move |_| {
                    draft.write().services.remove(i);
                },
            }
        }
    }
}

#[component]
pub(super) fn CachesSection(draft: Signal<Draft>) -> Element {
    let mut draft = draft;
    let caches = draft().caches;
    rsx! {
        div { class: "space-y-2",
            SectionHead {
                title: t!("agents-edit-caches").to_string(),
                on_add: move |_| draft.write().caches.push(CacheDraft::default()),
            }
            p { class: "text-xs text-slate", {t!("agents-edit-caches-hint")} }
            for (i, c) in caches.into_iter().enumerate() {
                div { key: "{i}", class: "flex flex-wrap items-center gap-2",
                    input {
                        class: "{INPUT} font-mono text-xs flex-1 min-w-24",
                        aria_label: t!("agents-edit-cache-name").to_string(),
                        placeholder: t!("agents-edit-cache-name").to_string(),
                        value: "{c.name}",
                        oninput: move |e| draft.write().caches[i].name = e.value(),
                    }
                    input {
                        class: "{INPUT} font-mono text-xs flex-1 min-w-32",
                        aria_label: t!("agents-edit-cache-path").to_string(),
                        placeholder: t!("agents-edit-cache-path").to_string(),
                        value: "{c.path}",
                        oninput: move |e| draft.write().caches[i].path = e.value(),
                    }
                    input {
                        class: "{INPUT} font-mono text-xs flex-1 min-w-32",
                        aria_label: t!("agents-edit-cache-keys").to_string(),
                        placeholder: t!("agents-edit-cache-keys").to_string(),
                        value: "{c.key_files}",
                        oninput: move |e| draft.write().caches[i].key_files = e.value(),
                    }
                    RemoveButton {
                        on_remove: move |_| {
                            draft.write().caches.remove(i);
                        },
                    }
                }
            }
        }
    }
}

/// Skills the record pins, read only: they are pinned from the skill editor (D711, D773).
#[component]
pub(super) fn SkillsSection(pins: Vec<SkillPin>, on_skills: Callback<()>) -> Element {
    let skills = use_resource(move || async move {
        api::agent_skills::list(100, 0)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });
    let shown: Vec<(String, i32)> = pins
        .iter()
        .map(|p| {
            let name = skills
                .value()
                .read()
                .as_ref()
                .and_then(|list| list.iter().find(|s| s.id == p.skill_id))
                .map(|s| s.name.clone())
                .unwrap_or_else(|| p.skill_id.clone());
            (name, p.version)
        })
        .collect();
    rsx! {
        div { class: "space-y-2",
            div { class: "flex items-center justify-between",
                h3 { class: "text-sm font-semibold text-ink", {t!("agents-edit-skills")} }
                button {
                    class: BTN_GHOST,
                    r#type: "button",
                    onclick: move |_| on_skills.call(()),
                    {t!("agents-edit-skills-open")}
                }
            }
            if shown.is_empty() {
                p { class: "text-sm text-slate", {t!("agents-edit-skills-none")} }
            } else {
                ul { class: "text-sm text-ink space-y-1",
                    for (name, v) in shown {
                        li { key: "{name}",
                            span { class: "font-mono", {name} }
                            " v{v}"
                        }
                    }
                }
            }
        }
    }
}
