//! The studio of a web site, laid out as the Vision Lab: in the shared
//! editor shell (D222), the selected step's screenshot fills the stage,
//! the shared player sits under it (D220), and a closable column on the
//! right holds the rule (blocks to drag, each edited in place), its code
//! (YAML) and what the run extracted. Versions in the shell (D221).

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::site_recipe::{
    new_step, to_yaml, validate, Recipe, RunInput, RunResume, RunStatus, SiteRecipe, SiteRecipeRun,
    SiteRecipeVersion, Violation, ACTIONS,
};

use super::blocks::{ActionStrip, Drag, RecipeSettings, StepBlocks};
use super::player::{Extracts, RunTimeline, Stage};
use super::versions::VersionsDrawer;
use super::{action_label, sleep_ms, status_label};
use crate::api;
use crate::api::error_i18n::{localize, localize_field};
use crate::app::Route;
use crate::components::editor_shell::{
    Chip, EditorShell, EditorStatus, PaletteIcon, PaletteItem, PalettePopover, StatusTone,
};
use crate::components::ontology::short_time;
use crate::components::ui::{
    BTN_DANGER, BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT, TAB, TAB_ACTIVE,
    TAB_BAR,
};
use crate::state::access::can;

fn starter(site: &str) -> Recipe {
    Recipe {
        name: "programme".into(),
        site: site.into(),
        allow_hosts: Vec::new(),
        params: Vec::new(),
        browser: None,
        captures: None,
        steps: ["open", "extract"]
            .iter()
            .filter_map(|a| new_step(a))
            .collect(),
    }
}

/// Which rule the editor holds: a saved one or a new one.
#[derive(Debug, Clone, PartialEq)]
enum Editing {
    Saved { id: String, version: i32 },
    New,
}

/// Tab of the side column.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Tab {
    Rule,
    Code,
    Extracts,
}

/// Client check of a structured rule (the same rules as the server).
fn check(r: &Recipe) -> Vec<Violation> {
    serde_json::to_value(r)
        .ok()
        .map(|v| validate(&v).err().unwrap_or_default())
        .unwrap_or_default()
}

fn violation_place(path: &str) -> String {
    match path
        .strip_prefix("steps.")
        .and_then(|n| n.parse::<usize>().ok())
    {
        Some(n) => t!("web-sites-step-n", n: n + 1).to_string(),
        None => path.to_string(),
    }
}

fn violations_of(e: &api::error::ApiError) -> Vec<Violation> {
    e.violations()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

#[component]
pub fn WebSiteStudio(site: String) -> Element {
    let site = use_signal(|| site);
    let mut reload = use_signal(|| 0u32);
    let mut editing = use_signal(|| None::<Editing>);
    let mut recipe = use_signal(|| starter(&site.peek()));
    let mut yaml = use_signal(String::new);
    let mut tab = use_signal(|| Tab::Rule);
    let mut column = use_signal(|| true);
    let mut violations = use_signal(Vec::<Violation>::new);
    let mut dirty = use_signal(|| false);
    let mut opened = use_signal(|| None::<i32>);
    let mut selected = use_signal(|| None::<usize>);
    let mut params = use_signal(BTreeMap::<String, String>::new);
    let mut run = use_signal(|| None::<SiteRecipeRun>);
    let mut step_index = use_signal(|| 0usize);
    let mut message = use_signal(|| None::<String>);
    let mut versions_open = use_signal(|| false);
    let mut confirm_delete = use_signal(|| false);
    let mut gen = use_signal(|| 0u64);
    let drag = use_signal(|| None::<Drag>);
    let navigator = use_navigator();
    let can_write = can("site-recipes:write");

    let recipes = use_resource(move || async move {
        let _ = reload();
        api::site_recipes::list(&site()).await
    });
    let journal = use_resource(move || async move {
        let _ = reload();
        api::site_recipes::runs(&site(), 50).await
    });

    let mut open_recipe = move |r: SiteRecipe| {
        if let Some(parsed) = r.recipe.clone() {
            recipe.set(parsed);
        }
        yaml.set(r.yaml.clone());
        violations.set(Vec::new());
        dirty.set(false);
        opened.set(None);
        selected.set(None);
        editing.set(Some(Editing::Saved {
            id: r.id.clone(),
            version: r.version,
        }));
    };

    // A rule proposed by the assistant opens as an unsaved draft.
    use_effect(move || {
        let Some(p) = super::PROPOSED.read().clone() else {
            return;
        };
        if p.site != *site.peek() {
            return;
        }
        let Some(Ok(list)) = &*recipes.read() else {
            return;
        };
        *super::PROPOSED.write() = None;
        let saved = p
            .recipe_id
            .as_ref()
            .and_then(|id| list.iter().find(|r| &r.id == id));
        editing.set(Some(match saved {
            Some(r) => Editing::Saved {
                id: r.id.clone(),
                version: r.version,
            },
            None => Editing::New,
        }));
        recipe.set(p.recipe);
        yaml.set(p.yaml);
        violations.set(Vec::new());
        opened.set(None);
        selected.set(None);
        dirty.set(true);
    });

    // First rule opened on arrival; a starter rule on an empty site.
    use_effect(move || {
        if editing.peek().is_some() || super::PROPOSED.peek().is_some() {
            return;
        }
        if let Some(Ok(list)) = &*recipes.read() {
            match list.first() {
                Some(r) => open_recipe(r.clone()),
                None => {
                    let r = starter(&site.peek());
                    yaml.set(to_yaml(&r));
                    recipe.set(r);
                    editing.set(Some(Editing::New));
                    // A new rule is unsaved until saved, even untouched.
                    dirty.set(true);
                }
            }
        }
    });

    // Last run of the site shown on arrival.
    use_effect(move || {
        if run.peek().is_some() {
            return;
        }
        if let Some(Ok(page)) = &*journal.read() {
            if let Some(last) = page.results.first() {
                run.set(Some(last.clone()));
            }
        }
    });

    // Block edits: checked at once, on the client.
    let mut changed = move |_| {
        dirty.set(true);
        violations.set(check(&recipe.peek()));
    };

    // Code edits: parsed by the server shortly after the last keystroke.
    let mut code_edit = move |text: String| {
        yaml.set(text.clone());
        dirty.set(true);
        gen.with_mut(|g| *g += 1);
        let mine = *gen.peek();
        spawn(async move {
            sleep_ms(400).await;
            if *gen.peek() != mine {
                return;
            }
            match api::site_recipes::validate(&text).await {
                Ok(r) => {
                    violations.set(Vec::new());
                    recipe.set(r);
                }
                Err(e) => violations.set(violations_of(&e)),
            }
        });
    };

    // Opening the code writes it from the blocks; leaving it needs a code
    // the server accepts.
    let mut open_tab = move |to: Tab| {
        let from = tab();
        if to == from {
            return;
        }
        if to == Tab::Code {
            yaml.set(to_yaml(&recipe.read()));
        }
        if from == Tab::Code && !violations().is_empty() {
            message.set(Some(t!("web-sites-yaml-fix-first").to_string()));
            return;
        }
        message.set(None);
        tab.set(to);
    };

    // The rule as it stands, as code.
    let current_yaml = move || {
        if tab() == Tab::Code {
            yaml()
        } else {
            to_yaml(&recipe.read())
        }
    };

    // Follows a run until it ends; the player stays on its last step.
    let follow = move |id: String| {
        spawn(async move {
            loop {
                sleep_ms(800).await;
                match api::site_recipes::run(&id).await {
                    Ok(r) => {
                        let done = r.status != RunStatus::Running;
                        step_index.set(r.steps.len().saturating_sub(1));
                        run.set(Some(r));
                        if done {
                            reload.with_mut(|n| *n += 1);
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    };

    let mut start = move |resume: Option<RunResume>| {
        let input = RunInput {
            recipe_id: match editing() {
                Some(Editing::Saved { id, .. }) => Some(id),
                _ => None,
            },
            yaml: Some(current_yaml()),
            params: params(),
            resume,
        };
        message.set(None);
        spawn(async move {
            match api::site_recipes::play(&input).await {
                Ok(r) => {
                    let id = r.id.clone();
                    step_index.set(0);
                    run.set(Some(r));
                    follow(id);
                }
                Err(e) => message.set(Some(localize(&e))),
            }
        });
    };
    let play = move |_| start(None);
    let resume = move |step: u32| {
        let Some(r) = run() else {
            return;
        };
        start(Some(RunResume { run_id: r.id, step }));
    };

    let save = move |_| {
        message.set(None);
        let text = current_yaml();
        let target = editing();
        spawn(async move {
            let res = match target {
                Some(Editing::Saved { id, version }) => {
                    api::site_recipes::update(&id, &text, version).await
                }
                _ => api::site_recipes::create(&text).await,
            };
            match res {
                Ok(r) => {
                    let moved = r.site != *site.peek();
                    let to = r.site.clone();
                    open_recipe(r);
                    reload.with_mut(|n| *n += 1);
                    if moved {
                        navigator.push(Route::WebSiteStudio { site: to });
                    }
                }
                Err(e) => message.set(Some(localize(&e))),
            }
        });
    };

    let open_version = move |v: SiteRecipeVersion| {
        versions_open.set(false);
        spawn(async move {
            match api::site_recipes::validate(&v.yaml).await {
                Ok(r) => {
                    recipe.set(r);
                    yaml.set(v.yaml.clone());
                    violations.set(Vec::new());
                    opened.set(Some(v.version));
                    dirty.set(true);
                }
                Err(e) => message.set(Some(localize(&e))),
            }
        });
    };

    let mut add_step = move |action: &'static str| {
        let Some(step) = new_step(action) else {
            return;
        };
        let at = selected().map_or(recipe.read().steps.len(), |i| i + 1);
        let at = at.min(recipe.read().steps.len());
        recipe.write().steps.insert(at, step);
        selected.set(Some(at));
        column.set(true);
        tab.set(Tab::Rule);
        changed(());
    };

    let delete_step = move |i: usize| {
        let mut r = recipe.write();
        if i < r.steps.len() {
            r.steps.remove(i);
        }
        drop(r);
        selected.set(None);
        changed(());
    };

    let select_step = move |i: usize| {
        selected.set(if selected() == Some(i) { None } else { Some(i) });
        if run.read().as_ref().is_some_and(|x| i < x.steps.len()) {
            step_index.set(i);
        }
    };

    let pick_run = move |id: String| {
        spawn(async move {
            if let Ok(x) = api::site_recipes::run(&id).await {
                let live = x.status == RunStatus::Running;
                step_index.set(x.failed_step.map(|f| f as usize).unwrap_or(0));
                run.set(Some(x));
                if live {
                    follow(id);
                }
            }
        });
    };

    let new_rule = move |_| {
        let r = starter(&site());
        yaml.set(to_yaml(&r));
        recipe.set(r);
        editing.set(Some(Editing::New));
        dirty.set(true);
        selected.set(None);
        tab.set(Tab::Rule);
        column.set(true);
    };

    let delete_rule = move |_| {
        confirm_delete.set(false);
        if let Some(Editing::Saved { id, .. }) = editing() {
            spawn(async move {
                match api::site_recipes::delete(&id).await {
                    Ok(()) => {
                        editing.set(None);
                        reload.with_mut(|n| *n += 1);
                    }
                    Err(e) => message.set(Some(localize(&e))),
                }
            });
        }
    };

    // ── derived view ──
    let r = recipe.read().clone();
    let has_errors = !violations().is_empty();
    let running = run().is_some_and(|x| x.status == RunStatus::Running);
    let missing_param = r
        .params
        .iter()
        .any(|p| params().get(p).is_none_or(|v| v.trim().is_empty()));
    let version = match editing() {
        Some(Editing::Saved { version, .. }) => Some(version),
        _ => None,
    };
    let status = match (editing(), run()) {
        (Some(Editing::New), _) => EditorStatus::new(StatusTone::Slate, t!("web-sites-new-recipe")),
        (_, Some(x)) => EditorStatus::new(
            match x.status {
                RunStatus::Ok => StatusTone::Green,
                RunStatus::Failed => StatusTone::Red,
                RunStatus::Running => StatusTone::Amber,
            },
            status_label(x.status),
        ),
        _ => EditorStatus::new(StatusTone::Slate, t!("web-sites-status-not-played")),
    };
    let played = run().map(|x| x.steps).unwrap_or_default();
    let runs = match &*journal.read() {
        Some(Ok(page)) => page.results.clone(),
        _ => Vec::new(),
    };
    let palette_items: Vec<PaletteItem> = ACTIONS
        .iter()
        .map(|a| PaletteItem {
            key: a.to_string(),
            label: action_label(a),
            description: None,
            icon: Some(PaletteIcon::Pointer),
            tile_class: None,
            group: None,
        })
        .collect();
    let can_delete = can("site-recipes:delete") && matches!(editing(), Some(Editing::Saved { .. }));
    let rule_button = if column() {
        format!("{BTN_SECONDARY_SM} border-signal text-signal")
    } else {
        BTN_SECONDARY_SM.to_string()
    };
    let play_title = if missing_param {
        t!("web-sites-param-needed").to_string()
    } else {
        String::new()
    };

    rsx! {
        EditorShell {
            on_back: move |_| {
                navigator.push(Route::WebSites {});
            },
            title: r.name.clone(),
            subtitle: Some(site()),
            status,
            version: version.map(i64::from),
            extra_chips: rsx! {
                if dirty() {
                    Chip { tone: StatusTone::Amber, label: t!("web-sites-unsaved").to_string() }
                }
                if let Some(v) = opened() {
                    Chip {
                        tone: StatusTone::Purple,
                        label: t!("web-sites-version-opened", v : v).to_string(),
                    }
                }
            },
            actions: rsx! {
                button {
                    class: "{rule_button}",
                    "aria-pressed": column(),
                    onclick: move |_| column.toggle(),
                    {t!("web-sites-rule")}
                }
                if matches!(editing(), Some(Editing::Saved { .. })) {
                    button { class: BTN_SECONDARY_SM, onclick: move |_| versions_open.set(true),
                        {t!("web-sites-versions")}
                    }
                }
                button {
                    class: BTN_SECONDARY_SM,
                    disabled: !can_write || has_errors || missing_param || running,
                    title: "{play_title}",
                    onclick: play,
                    if running {
                        {t!("web-sites-playing")}
                    } else {
                        {t!("web-sites-play")}
                    }
                }
                button {
                    class: BTN_PRIMARY_SM,
                    disabled: !can_write || has_errors || !dirty(),
                    onclick: save,
                    {t!("web-sites-save")}
                }
            },
            palette: if can_write { Some(rsx! {
                PalettePopover {
                    add_title: t!("web-sites-add-step").to_string(),
                    title: t!("web-sites-actions").to_string(),
                    search_placeholder: t!("web-sites-search-action").to_string(),
                    items: palette_items,
                    on_pick: move |key: String| {
                        if let Some(a) = ACTIONS.iter().find(|a| **a == key) {
                            add_step(a);
                        }
                    },
                }
            }) } else { None },
            canvas: rsx! {
                div { class: "flex h-full w-full bg-canvas",
                    div { class: "flex min-w-0 flex-1 flex-col gap-3 p-3 pt-16",
                        div { class: "min-h-0 flex-1",
                            Stage { run: run(), index: step_index() }
                        }
                        RunTimeline {
                            run: run(),
                            runs,
                            index: step_index,
                            on_pick: pick_run,
                            on_resume: can_write.then_some(EventHandler::new(resume)),
                        }
                    }
                    if column() {
                        aside { class: "flex w-full max-w-md shrink-0 flex-col border-l border-line bg-surface sm:w-[26rem]",
                            div { class: "{TAB_BAR} px-3 pt-2",
                                button {
                                    class: if tab() == Tab::Rule { TAB_ACTIVE } else { TAB },
                                    onclick: move |_| open_tab(Tab::Rule),
                                    {t!("web-sites-rule")}
                                }
                                button {
                                    class: if tab() == Tab::Code { TAB_ACTIVE } else { TAB },
                                    onclick: move |_| open_tab(Tab::Code),
                                    {t!("web-sites-code")}
                                }
                                button {
                                    class: if tab() == Tab::Extracts { TAB_ACTIVE } else { TAB },
                                    onclick: move |_| open_tab(Tab::Extracts),
                                    {t!("web-sites-extracts")}
                                }
                            }
                            div { class: "min-h-0 flex-1 space-y-3 overflow-y-auto p-3",
                                if tab() != Tab::Extracts {
                                    RuleTabs {
                                        recipes,
                                        editing,
                                        on_open: move |x| open_recipe(x),
                                        on_new: new_rule,
                                        can_write,
                                    }
                                    for p in r.params.clone() {
                                        div { key: "{p}",
                                            label { class: FIELD_LABEL, "{p}" }
                                            input {
                                                class: INPUT,
                                                value: "{params().get(&p).cloned().unwrap_or_default()}",
                                                oninput: {
                                                    let p = p.clone();
                                                    move |e: FormEvent| {
                                                        params.write().insert(p.clone(), e.value());
                                                    }
                                                },
                                            }
                                        }
                                    }
                                    if !violations().is_empty() {
                                        ul { class: "space-y-1",
                                            for (i, v) in violations().iter().enumerate() {
                                                li { key: "{i}", class: "text-sm text-alarm",
                                                    "{violation_place(&v.path)} : {localize_field(&v.path, &v.token)}"
                                                }
                                            }
                                        }
                                    }
                                    if let Some(m) = message() {
                                        p { class: "text-sm text-alarm", "{m}" }
                                    }
                                }
                                if tab() == Tab::Rule {
                                    ActionStrip {
                                        can_write,
                                        drag,
                                        on_add: move |a| add_step(a),
                                    }
                                    StepBlocks {
                                        recipe,
                                        selected: selected(),
                                        on_select: select_step,
                                        played: played.clone(),
                                        can_write,
                                        drag,
                                        on_change: changed,
                                        on_delete: delete_step,
                                    }
                                    details { class: "rounded-md border border-line",
                                        summary { class: "cursor-pointer px-3 py-2 text-sm font-medium text-ink",
                                            {t!("web-sites-settings")}
                                        }
                                        RecipeSettings { recipe, can_write, on_change: changed }
                                        if can_delete {
                                            div { class: "px-4 pb-4",
                                                button {
                                                    class: BTN_DANGER,
                                                    onclick: move |_| confirm_delete.set(true),
                                                    {t!("web-sites-delete-recipe")}
                                                }
                                            }
                                        }
                                    }
                                    p { class: FIELD_HELP, {t!("web-sites-play-help")} }
                                    if let Some(Editing::Saved { id, .. }) = editing() {
                                        ScheduleNote { recipe_id: id }
                                    }
                                }
                                if tab() == Tab::Code {
                                    textarea {
                                        class: "{INPUT} min-h-96 font-mono text-xs",
                                        spellcheck: "false",
                                        disabled: !can_write,
                                        value: "{yaml}",
                                        oninput: move |e| code_edit(e.value()),
                                    }
                                    p { class: FIELD_HELP, {t!("web-sites-code-help")} }
                                }
                                if tab() == Tab::Extracts {
                                    Extracts { run: run() }
                                }
                            }
                        }
                    }
                }
            },
        }
        if confirm_delete() {
            crate::components::confirm::ConfirmDialog {
                title: t!("web-sites-delete-recipe"),
                message: t!("web-sites-delete-confirm"),
                confirm_label: t!("web-sites-delete-recipe"),
                on_confirm: delete_rule,
                on_cancel: move |_| confirm_delete.set(false),
            }
        }
        if versions_open() {
            if let Some(Editing::Saved { id, version }) = editing() {
                VersionsDrawer {
                    recipe_id: id,
                    active_version: version,
                    can_write,
                    on_close: move |_| versions_open.set(false),
                    on_open: open_version,
                    on_restored: move |x| {
                        versions_open.set(false);
                        open_recipe(x);
                        reload.with_mut(|n| *n += 1);
                    },
                }
            }
        }
    }
}

/// The rules of the site, as tabs, and « New rule ».
#[component]
fn RuleTabs(
    recipes: Resource<Result<Vec<SiteRecipe>, api::error::ApiError>>,
    editing: Signal<Option<Editing>>,
    on_open: EventHandler<SiteRecipe>,
    on_new: EventHandler<()>,
    can_write: bool,
) -> Element {
    let current = match editing() {
        Some(Editing::Saved { id, .. }) => Some(id),
        _ => None,
    };
    let list = match &*recipes.read() {
        Some(Ok(l)) => l.clone(),
        _ => Vec::new(),
    };
    rsx! {
        div { class: "flex flex-wrap items-center gap-2",
            for x in list {
                button {
                    key: "{x.id}",
                    class: if current.as_deref() == Some(x.id.as_str()) { BTN_PRIMARY_SM } else { BTN_SECONDARY_SM },
                    onclick: {
                        let x = x.clone();
                        move |_| on_open.call(x.clone())
                    },
                    "{x.name}"
                }
            }
            if editing() == Some(Editing::New) {
                span { class: BTN_PRIMARY_SM, {t!("web-sites-new-recipe")} }
            }
            if can_write {
                button {
                    class: "{BTN_SECONDARY_SM} ml-auto",
                    onclick: move |_| on_new.call(()),
                    "+ "
                    {t!("web-sites-new-recipe")}
                }
            }
        }
    }
}

/// When the saved rule plays next: the deployed flows that trigger it
/// (D326, no scheduler of its own).
#[component]
fn ScheduleNote(recipe_id: String) -> Element {
    let id = recipe_id.clone();
    let schedule = use_resource(use_reactive!(|id| async move {
        api::site_recipes::schedule(&id).await
    }));
    let Some(Ok(s)) = &*schedule.read() else {
        return rsx! {};
    };
    if s.flows.is_empty() {
        return rsx! {
            p { class: FIELD_HELP, {t!("web-sites-schedule-none")} }
        };
    }
    let next: Vec<String> = s.next.iter().map(|t| short_time(t)).collect();
    rsx! {
        div { class: "space-y-1 rounded-md border border-line px-3 py-2 text-sm",
            p { class: "font-medium text-ink", {t!("web-sites-schedule")} }
            for f in s.flows.clone() {
                p { key: "{f.flow_id}", class: "text-slate",
                    Link {
                        class: "text-signal-700 hover:underline",
                        to: Route::Flows {
                            id: f.flow_id.to_string(),
                        },
                        "{f.name}"
                    }
                    for c in f.cron.iter() {
                        " · {c}"
                    }
                    for secs in f.every_secs.iter() {
                        " · "
                        {t!("web-sites-schedule-every", minutes : (secs / 60.0).round() as i64)}
                    }
                }
            }
            if !next.is_empty() {
                p { class: "text-xs text-slate",
                    {t!("web-sites-schedule-next", times : next.join(", "))}
                }
            }
        }
    }
}
