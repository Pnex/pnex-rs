//! Editor of an environment (agent-environments.md D702, D221, D222): its Dockerfile and
//! context files as tabs, a save makes a new version on the latest one read, the common
//! version history, and the builds beside it. Building and activating stay human gestures.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;

use super::builds::BuildsPanel;
use crate::api;
use crate::api::error::ApiError;
use crate::components::icons;
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY_SM, INPUT, TAB, TAB_ACTIVE, TAB_BAR};
use crate::components::versions_drawer::{VersionRow, VersionsDrawer};
use crate::state::access;
use crate::state::toasts;
use pnex_core::environments::{
    Environment, EnvironmentSource, EnvironmentVersion, EnvironmentVersionInput,
};

/// The environment and its latest version, if any.
async fn load(id: &str) -> Result<(Environment, Option<EnvironmentVersion>), ApiError> {
    let env = api::hosted_agents::environment(id).await?;
    let version = match env.latest_version {
        Some(n) => Some(api::hosted_agents::version(id, n).await?),
        None => None,
    };
    Ok((env, version))
}

/// One text per version, for the diff of the history: the Dockerfile then each file.
fn source_text(s: &EnvironmentSource) -> String {
    let mut text = format!("# Dockerfile\n{}", s.dockerfile);
    for (path, content) in &s.files {
        text.push_str(&format!("\n# {path}\n{content}"));
    }
    text
}

#[component]
pub(super) fn EnvironmentEditor(env_id: String, on_back: Callback<()>) -> Element {
    let can_write = access::can("environments:write");
    let can_build = access::can("environments:build");
    let mut generation = use_signal(|| 0u32);
    let load_id = env_id.clone();
    let detail = use_resource(move || {
        let id = load_id.clone();
        let gen = generation();
        async move { (gen, load(&id).await) }
    });

    let mut env = use_signal(|| Option::<Environment>::None);
    // Source the draft started from: the dirty check compares with it.
    let mut base = use_signal(|| Option::<EnvironmentSource>::None);
    let mut shown_version = use_signal(|| Option::<i32>::None);
    let mut dockerfile = use_signal(String::new);
    let mut files = use_signal(BTreeMap::<String, String>::new);
    let mut build_args = use_signal(BTreeMap::<String, String>::new);
    let mut init_done = use_signal(|| None::<u32>);
    use_effect(move || {
        let Some((gen, Ok((e, v)))) = &*detail.read() else {
            return;
        };
        let gen = *gen;
        if gen != generation() || init_done() == Some(gen) {
            return;
        }
        env.set(Some(e.clone()));
        let source = v.as_ref().map(|v| v.source.clone());
        dockerfile.set(
            source
                .as_ref()
                .map(|s| s.dockerfile.clone())
                .unwrap_or_default(),
        );
        files.set(source.as_ref().map(|s| s.files.clone()).unwrap_or_default());
        build_args.set(
            source
                .as_ref()
                .map(|s| s.build_args.clone())
                .unwrap_or_default(),
        );
        shown_version.set(v.as_ref().map(|v| v.summary.number));
        base.set(source);
        init_done.set(Some(gen));
    });
    let load_error = match &*detail.read() {
        Some((_, Err(e))) => Some(crate::api::error_i18n::localize(e)),
        _ => None,
    };

    // `None` = the Dockerfile tab.
    let mut tab = use_signal(|| Option::<String>::None);
    let new_file = use_signal(String::new);
    let mut renaming = use_signal(|| Option::<String>::None);
    let mut versions_open = use_signal(|| false);
    let mut builds_reload = use_signal(|| 0u32);

    let draft = move || EnvironmentSource {
        dockerfile: dockerfile(),
        files: files(),
        build_args: build_args(),
    };
    let dirty = match base() {
        Some(b) => b != draft(),
        None => !dockerfile().trim().is_empty() || !files().is_empty(),
    };

    let save_source = move |source: EnvironmentSource| {
        let Some(e) = env() else { return };
        spawn(async move {
            let input = EnvironmentVersionInput {
                source,
                expected_latest: e.latest_version,
            };
            match api::hosted_agents::save_version(&e.id, &input).await {
                Ok(v) => toasts::info(t!("agents-env-saved", v : v.number).to_string()),
                Err(err) if err.status == Some(409) => {
                    toasts::error(t!("agents-env-save-conflict").to_string());
                }
                Err(err) => {
                    toasts::error(err);
                    return;
                }
            }
            versions_open.set(false);
            generation.with_mut(|g| *g += 1);
        });
    };

    let build_id = env_id.clone();
    let start_build = move |_| {
        let Some(number) = shown_version() else {
            return;
        };
        let id = build_id.clone();
        spawn(async move {
            match api::hosted_agents::start_build(&id, number).await {
                Ok(_) => {
                    toasts::info(t!("agents-env-build-started", v : number).to_string());
                    builds_reload.with_mut(|r| *r += 1);
                }
                Err(e) => toasts::error(e),
            }
        });
    };

    let history_id = env_id.clone();
    let history = use_resource(move || {
        let id = history_id.clone();
        let open = versions_open();
        let _ = generation();
        async move {
            if !open {
                return None;
            }
            // shortcut: one request per version, the 20 latest; a list with sources would do it in one.
            let list = async {
                let summaries = api::hosted_agents::versions(&id).await?;
                let mut out = Vec::new();
                for s in summaries.results {
                    out.push(api::hosted_agents::version(&id, s.number).await?);
                }
                Ok::<_, ApiError>(out)
            };
            Some(list.await)
        }
    });
    let history_versions: Vec<EnvironmentVersion> = match &*history.value().read() {
        Some(Some(Ok(v))) => v.clone(),
        _ => Vec::new(),
    };
    let rows = match &*history.value().read() {
        Some(Some(Ok(v))) => Some(Ok(v
            .iter()
            .map(|v| VersionRow {
                version: v.summary.number,
                yaml: source_text(&v.source),
                created_at: v.summary.created_at.clone(),
                author: None,
            })
            .collect())),
        Some(Some(Err(e))) => Some(Err(e.clone())),
        _ => None,
    };
    let find_version = move |n: i32, list: &[EnvironmentVersion]| {
        list.iter().find(|v| v.summary.number == n).cloned()
    };

    let Some(current) = env() else {
        return rsx! {
            div { class: "space-y-3",
                button {
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| on_back.call(()),
                    icons::ArrowLeft { class: "h-4 w-4" }
                }
                if let Some(e) = load_error {
                    p { class: "text-sm text-alarm", "{e}" }
                }
            }
        };
    };
    let latest = current.latest_version.unwrap_or(0);
    let open_list = history_versions.clone();
    let restore_list = history_versions;
    let file_names: Vec<String> = files().keys().cloned().collect();
    let selected = tab();
    let text = match &selected {
        None => dockerfile(),
        Some(path) => files().get(path).cloned().unwrap_or_default(),
    };
    let new_name = new_file().trim().to_string();
    let can_add = !new_name.is_empty()
        && new_name != "Dockerfile"
        && !files().contains_key(&new_name)
        && renaming().is_none();

    rsx! {
        div { class: "space-y-4",
            div { class: "flex items-start justify-between flex-wrap gap-3",
                div { class: "flex items-start gap-3 min-w-0",
                    button {
                        class: "{BTN_SECONDARY_SM} shrink-0",
                        aria_label: t!("agents-env-back").to_string(),
                        onclick: move |_| on_back.call(()),
                        icons::ArrowLeft { class: "h-4 w-4" }
                    }
                    div { class: "min-w-0",
                        h2 { class: "text-xl font-bold text-ink", {current.name.clone()} }
                        div { class: "flex items-center gap-2 mt-1 flex-wrap text-sm text-slate",
                            if let Some(v) = shown_version() {
                                span { "v{v}" }
                                if v != latest {
                                    span { class: "text-xs text-amber-600",
                                        {t!("agents-env-old-version", latest : latest)}
                                    }
                                }
                            } else {
                                span { {t!("agents-env-no-version")} }
                            }
                            if dirty {
                                span { class: "text-xs text-amber-600", {t!("agents-env-dirty")} }
                            }
                            if !current.description.is_empty() {
                                span { {current.description.clone()} }
                            }
                        }
                    }
                }
                div { class: "flex items-center gap-2 shrink-0",
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| versions_open.set(true),
                        icons::History { class: "h-4 w-4 inline mr-1" }
                        {t!("agents-env-history")}
                    }
                    if can_build {
                        button {
                            class: BTN_SECONDARY_SM,
                            disabled: dirty || shown_version().is_none(),
                            title: t!("agents-env-build-hint").to_string(),
                            onclick: start_build,
                            {t!("agents-env-build")}
                        }
                    }
                    if can_write {
                        button {
                            class: BTN_PRIMARY,
                            disabled: !dirty,
                            onclick: move |_| save_source(draft()),
                            icons::Save { class: "h-4 w-4 inline mr-1" }
                            {t!("agents-env-save")}
                        }
                    }
                }
            }
            div { class: "grid grid-cols-1 xl:grid-cols-3 gap-4 items-start",
                div { class: "xl:col-span-2 space-y-2",
                    div { class: "{TAB_BAR} flex-wrap", role: "tablist",
                        button {
                            class: if selected.is_none() { TAB_ACTIVE } else { TAB },
                            r#type: "button",
                            role: "tab",
                            aria_selected: selected.is_none(),
                            onclick: move |_| tab.set(None),
                            "Dockerfile"
                        }
                        for name in file_names {
                            button {
                                key: "{name}",
                                class: if selected.as_deref() == Some(name.as_str()) { TAB_ACTIVE } else { TAB },
                                r#type: "button",
                                role: "tab",
                                aria_selected: selected.as_deref() == Some(name.as_str()),
                                onclick: {
                                    let name = name.clone();
                                    move |_| {
                                        renaming.set(None);
                                        tab.set(Some(name.clone()));
                                    }
                                },
                                span { class: "font-mono", {name.clone()} }
                            }
                        }
                    }
                    if can_write {
                        FileBar {
                            selected: selected.clone(),
                            files,
                            tab,
                            new_file,
                            renaming,
                            can_add,
                        }
                    }
                    textarea {
                        class: "{INPUT} w-full h-[28rem] font-mono text-xs leading-5",
                        spellcheck: "false",
                        aria_label: selected.clone().unwrap_or_else(|| "Dockerfile".to_string()),
                        readonly: !can_write,
                        value: "{text}",
                        oninput: move |e| {
                            let value = e.value();
                            match tab() {
                                None => dockerfile.set(value),
                                Some(path) => {
                                    files.write().insert(path, value);
                                }
                            }
                        },
                    }
                    p { class: "text-xs text-slate", {t!("agents-env-files-hint")} }
                }
                BuildsPanel {
                    env: current.clone(),
                    reload: builds_reload,
                    on_changed: move |_| generation.with_mut(|g| *g += 1),
                }
            }
            if versions_open() {
                VersionsDrawer {
                    rows,
                    active_version: latest,
                    can_write,
                    on_close: move |_| versions_open.set(false),
                    on_open: move |row: VersionRow| {
                        if let Some(v) = find_version(row.version, &open_list) {
                            dockerfile.set(v.source.dockerfile.clone());
                            files.set(v.source.files.clone());
                            build_args.set(v.source.build_args.clone());
                            shown_version.set(Some(v.summary.number));
                            base.set(Some(v.source));
                            tab.set(None);
                            versions_open.set(false);
                        }
                    },
                    on_restore: move |n: i32| {
                        if let Some(v) = find_version(n, &restore_list) {
                            save_source(v.source);
                        }
                    },
                }
            }
        }
    }
}

/// Add a file, rename or remove the selected one. Paths are checked by the server.
#[component]
fn FileBar(
    selected: Option<String>,
    files: Signal<BTreeMap<String, String>>,
    tab: Signal<Option<String>>,
    new_file: Signal<String>,
    renaming: Signal<Option<String>>,
    can_add: bool,
) -> Element {
    let mut files = files;
    let mut tab = tab;
    let mut new_file = new_file;
    let mut renaming = renaming;
    let rename_to = renaming().unwrap_or_default().trim().to_string();
    let can_rename =
        !rename_to.is_empty() && rename_to != "Dockerfile" && !files().contains_key(&rename_to);
    rsx! {
        div { class: "flex flex-wrap items-center gap-2",
            input {
                class: "{INPUT} font-mono text-xs",
                placeholder: t!("agents-env-file-placeholder").to_string(),
                aria_label: t!("agents-env-file-new").to_string(),
                value: "{new_file}",
                oninput: move |e| new_file.set(e.value()),
            }
            button {
                class: BTN_SECONDARY_SM,
                disabled: !can_add,
                onclick: move |_| {
                    let name = new_file().trim().to_string();
                    files.write().insert(name.clone(), String::new());
                    tab.set(Some(name));
                    new_file.set(String::new());
                },
                {t!("agents-env-file-add")}
            }
            if let Some(path) = selected {
                span { class: "flex-grow" }
                if renaming().is_some() {
                    input {
                        class: "{INPUT} font-mono text-xs",
                        aria_label: t!("agents-env-file-rename").to_string(),
                        value: "{rename_to}",
                        oninput: move |e| renaming.set(Some(e.value())),
                    }
                    button {
                        class: BTN_SECONDARY_SM,
                        disabled: !can_rename,
                        onclick: {
                            let path = path.clone();
                            move |_| {
                                let to = renaming().unwrap_or_default().trim().to_string();
                                let content = files.write().remove(&path);
                                if let Some(content) = content {
                                    files.write().insert(to.clone(), content);
                                }
                                tab.set(Some(to));
                                renaming.set(None);
                            }
                        },
                        {t!("common-save")}
                    }
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| renaming.set(None),
                        {t!("common-cancel")}
                    }
                } else {
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: {
                            let path = path.clone();
                            move |_| renaming.set(Some(path.clone()))
                        },
                        {t!("agents-env-file-rename")}
                    }
                    button {
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| {
                            files.write().remove(&path);
                            tab.set(None);
                        },
                        {t!("agents-env-file-remove")}
                    }
                }
            }
        }
    }
}
