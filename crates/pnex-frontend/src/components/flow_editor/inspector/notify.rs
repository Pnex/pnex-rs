use super::device::patch_selected_notify_and_rewire;
use super::helpers::*;
use super::*;

// ─────────────── pnex-notify (D49–D54) ───────────────

/// Lignes de vars éditables `(clé, valeur)` — littéral OU `{{ msg.x }}`.
type NotifyVarRows = Vec<(String, String)>;

/// Nil UUID of the "no template" option — also the template-pick sentinel.
const NIL_UUID: &str = "00000000-0000-0000-0000-000000000000";

#[component]
pub(super) fn NotifyForm(mut cx: EditorCx, initial: NotifyNodeConfig, can_write: bool) -> Element {
    // Copiés avant le move d'`initial` dans les signaux (école DebugForm).
    let strict = initial.strict;
    let anti = initial.anti_spam;
    let mut channel_ids =
        use_signal::<Vec<String>>(|| initial.channel_ids.iter().map(|u| u.to_string()).collect());
    let mut template_id = use_signal(|| initial.template_id.to_string());
    let mut vars = use_signal::<NotifyVarRows>(|| {
        initial
            .vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    });
    // Template vars already stamped into the node (canvas anchors) — the
    // self-heal reference: nodes saved before the stamping shipped carry an
    // empty `template_vars` while a template is picked.
    let mut stamped_vars = use_signal(|| initial.template_vars.clone());
    // Anti-spam : enable + max + fenêtre en minutes (fenêtre fixe « compteur
    // × durée », une phrase lisible — l'AntiSpamConfig porte des secondes).
    let mut anti_enabled = use_signal(move || anti.is_some());
    let mut anti_max = use_signal(move || anti.map_or("3".to_string(), |a| a.max_msgs.to_string()));
    let mut anti_window_min =
        use_signal(move || anti.map_or("10".to_string(), |a| (a.window_secs / 60).to_string()));

    let channels = use_resource(move || async move {
        api::notify::list_channels()
            .await
            .ok()
            .map(|p| p.results)
            .unwrap_or_default()
    });
    let templates = use_resource(move || async move {
        api::notify::list_templates()
            .await
            .ok()
            .map(|p| p.results)
            .unwrap_or_default()
    });

    // Template pick: the effect below (stamped vars ≠ template vars) does
    // the stamping + pruning — one single path for a pick and a self-heal.
    let mut commit_template = move |id: String| {
        template_id.set(id);
    };

    // Stamp the template vars into the node (wireable input anchors) and
    // prune the per-node values to the template's vars — surviving vars keep
    // their value. One single path for a template (re)pick and the self-heal
    // of nodes saved before the stamping shipped (template picked, anchors
    // never stamped — the canvas then shows a single unlabeled anchor).
    use_effect(move || {
        let tid = template_id();
        if tid == NIL_UUID {
            return;
        }
        let list = templates.read().clone().unwrap_or_default();
        let Some(new_vars) = list
            .iter()
            .find(|tpl| tpl.id.to_string() == tid)
            .map(|tpl| tpl.vars.iter().map(|v| v.name.clone()).collect::<Vec<_>>())
        else {
            return;
        };
        if stamped_vars() == new_vars {
            return;
        }
        // Surviving vars keep their value, gone ones are pruned (the template
        // drives the rows — no free-form rows anymore).
        let pruned: NotifyVarRows = vars
            .peek()
            .iter()
            .filter(|(k, _)| new_vars.contains(k))
            .cloned()
            .collect();
        vars.set(pruned.clone());
        stamped_vars.set(new_vars.clone());
        patch_selected_notify_and_rewire(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::PnexNotify { config } = &mut node.kind {
                config.template_id = tid.parse().unwrap_or_default();
                config.template_vars = new_vars.clone();
                config.vars = pruned.iter().cloned().collect();
            }
        });
    });

    // Commit anti-spam : clamp min côté form (max ≥ 1, fenêtre ≥ 1 min) —
    // validate_graph reste le garde-fou structurel.
    let mut commit_anti_spam = move |enabled: bool| {
        let rule = if enabled {
            Some(pnex_core::AntiSpamConfig {
                max_msgs: anti_max.read().parse::<u32>().unwrap_or(0).clamp(1, 1000),
                window_secs: anti_window_min
                    .read()
                    .parse::<u64>()
                    .unwrap_or(0)
                    .clamp(1, 525_600)
                    .saturating_mul(60),
            })
        } else {
            None
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::PnexNotify { config } = &mut node.kind {
                config.anti_spam = rule;
            }
        });
    };

    let channels_value: Vec<pnex_core::NotifyChannel> = channels.read().clone().unwrap_or_default();
    let templates_value: Vec<pnex_core::NotifyTemplate> =
        templates.read().clone().unwrap_or_default();
    let template_nil = template_id() == NIL_UUID;
    let channel_rows = channel_ids.read().clone();
    let selected_template = template_id();
    let template_options: Vec<(String, String, bool)> = templates_value
        .iter()
        .map(|tpl| {
            let id = tpl.id.to_string();
            let selected = id == selected_template;
            let label = tpl.name.clone();
            (id, label, selected)
        })
        .collect();
    // Vars du template sélectionné (colonne mergée « détectées + déclarées »)
    // — lues du live (resource), jamais du stamp graphique.
    let template_vars_now: Vec<String> = templates_value
        .iter()
        .find(|tpl| tpl.id.to_string() == selected_template)
        .map(|tpl| tpl.vars.iter().map(|v| v.name.clone()).collect())
        .unwrap_or_default();

    rsx! {
        div { class: "space-y-3",
            // ── Canaux (rows répétables, école DeviceForm) ──
            div { class: "space-y-1",
                span { class: "text-xs font-medium text-gray-500 block", {t!("flows-notify-channels")} }
                for (i, cid) in channel_rows.iter().enumerate() {
                    NotifyChannelRow {
                        key: "{i}-{cid}",
                        index: i,
                        channel_id: cid.clone(),
                        channels: channels_value.clone(),
                        can_write,
                        on_pick: move |(idx, id): (usize, String)| {
                            let mut ids = channel_ids.read().clone();
                            if idx < ids.len() {
                                ids[idx] = id;
                            }
                            channel_ids.set(ids.clone());
                            patch_selected(
                                &mut cx,
                                move |node: &mut FlowNode| {
                                    if let FlowNodeKind::PnexNotify { config } = &mut node.kind {
                                        config.channel_ids = ids
                                            .iter()
                                            .filter_map(|s| s.parse().ok())
                                            .collect();
                                    }
                                },
                            );
                        },
                    }
                }
                if can_write {
                    button {
                        class: "text-xs text-blue-600 hover:text-blue-700 disabled:opacity-50",
                        disabled: channels_value.is_empty(),
                        onclick: move |_| {
                            let mut ids = channel_ids.read().clone();
                            ids.push(String::new());
                            channel_ids.set(ids.clone());
                            patch_selected(
                                &mut cx,
                                move |node: &mut FlowNode| {
                                    if let FlowNodeKind::PnexNotify { config } = &mut node.kind {
                                        config.channel_ids = ids
                                            .iter()
                                            .filter_map(|s| s.parse().ok())
                                            .collect();
                                    }
                                },
                            );
                        },
                        "+ "
                        {t!("flows-notify-add-channel")}
                    }
                }
            }

            // ── Template ──
            div { class: "space-y-1",
                span { class: "text-xs font-medium text-gray-500 block", {t!("flows-notify-template")} }
                select {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm bg-white disabled:bg-gray-50",
                    disabled: !can_write,
                    value: "{selected_template}",
                    onchange: move |event| commit_template(event.value()),
                    option { value: NIL_UUID, selected: template_nil, {t!("flows-notify-no-template")} }
                    for (tpl_id, tpl_label, tpl_selected) in template_options {
                        option { value: tpl_id.clone(), selected: tpl_selected, {tpl_label} }
                    }
                }
                // Vars du template = ancres d'entrée nommées (stampées au
                // pick, routées au deploy par tagger topic = var).
                if !template_vars_now.is_empty() {
                    div { class: "space-y-1",
                        span { class: "text-xs text-gray-500 block",
                            {t!("flows-notify-template-vars-hint")}
                        }
                        div { class: "flex flex-wrap gap-1",
                            for var_name in &template_vars_now {
                                span {
                                    key: "{var_name}",
                                    class: "px-1.5 py-0.5 rounded bg-purple-50 text-purple-700 text-xs font-mono",
                                    {format!("{{{{ {var_name} }}}}")}
                                }
                            }
                        }
                    }
                }
                // The permanent `trigger` gate row: wire a logic function's
                // boolean output to decide whether the notification is sent.
                div { class: "space-y-1",
                    span { class: "text-xs text-gray-500 block", {t!("flows-notify-trigger-hint")} }
                }
            }

            // ── Variable values — one row per template var (literal or
            // `{{ msg.x }}` two-level render); the template drives the rows,
            // no free-form add ──
            if !template_vars_now.is_empty() {
                div { class: "space-y-1",
                    span { class: "text-xs font-medium text-gray-500", {t!("flows-notify-vars")} }
                    for var_name in &template_vars_now {
                        NotifyVarRow {
                            key: "{var_name}",
                            name: var_name.clone(),
                            value: var_value(&vars.read(), var_name),
                            can_write,
                            on_change: move |(name, value): (String, String)| {
                                upsert_var(&mut vars.write(), name, value);
                                let rows = vars.read().clone();
                                store_vars(&mut cx, &rows);
                            },
                        }
                    }
                }
            }

            // ── strict — plain checkbox + plain-language hint ──
            div { class: "space-y-1",
                label { class: "flex items-center gap-2 select-none",
                    input {
                        class: "h-4 w-4 accent-blue-600",
                        r#type: "checkbox",
                        checked: strict,
                        disabled: !can_write,
                        onchange: move |event| {
                            let checked = event.checked();
                            patch_selected(
                                &mut cx,
                                move |node: &mut FlowNode| {
                                    if let FlowNodeKind::PnexNotify { config } = &mut node.kind {
                                        config.strict = checked;
                                    }
                                },
                            );
                        },
                    }
                    span { class: "text-xs font-medium text-gray-500", {t!("flows-notify-strict")} }
                }
                p { class: "text-xs text-gray-400", {t!("flows-notify-strict-hint")} }
            }

            // ── Anti-spam (fixed window "count × duration") — plain checkbox,
            // revealed fields indented under it, fluent summary ──
            div { class: "space-y-1.5",
                label { class: "flex items-center gap-2 select-none",
                    input {
                        class: "h-4 w-4 accent-blue-600",
                        r#type: "checkbox",
                        checked: anti_enabled(),
                        disabled: !can_write,
                        onchange: move |event| {
                            let checked = event.checked();
                            anti_enabled.set(checked);
                            commit_anti_spam(checked);
                        },
                    }
                    span { class: "text-xs font-medium text-gray-500", {t!("flows-notify-antispam")} }
                }
                if anti_enabled() {
                    div { class: "grid grid-cols-2 gap-2 pl-6",
                        label { class: "text-xs text-gray-500 space-y-0.5 block",
                            {t!("flows-notify-antispam-max")}
                            input {
                                class: "w-full px-2 py-1 border border-gray-300 rounded text-sm bg-white disabled:bg-gray-50",
                                r#type: "number",
                                min: "1",
                                value: "{anti_max.read()}",
                                disabled: !can_write,
                                oninput: move |event| {
                                    anti_max.set(event.value());
                                    commit_anti_spam(true);
                                },
                            }
                        }
                        label { class: "text-xs text-gray-500 space-y-0.5 block",
                            {t!("flows-notify-antispam-window")}
                            input {
                                class: "w-full px-2 py-1 border border-gray-300 rounded text-sm bg-white disabled:bg-gray-50",
                                r#type: "number",
                                min: "1",
                                value: "{anti_window_min.read()}",
                                disabled: !can_write,
                                oninput: move |event| {
                                    anti_window_min.set(event.value());
                                    commit_anti_spam(true);
                                },
                            }
                        }
                    }
                    p { class: "text-xs text-gray-400 pl-6",
                        {
                            t!(
                                "flows-notify-antispam-hint", max : anti_max.read().parse::< u32 > ()
                                .unwrap_or(0).clamp(1, 1000) as i64, window : anti_window_min.read().parse::<
                                u64 > ().unwrap_or(0).clamp(1, 525_600) as i64
                            )
                        }
                    }
                }
            }
        }
    }
}

/// Une ligne de sélection de canal — composant dédié (pas de `let` dans le
/// `for` rsx, piège dioxus). `on_pick((index, channel_id))` remonte le choix.
#[component]
fn NotifyChannelRow(
    index: usize,
    channel_id: String,
    channels: Vec<pnex_core::NotifyChannel>,
    can_write: bool,
    on_pick: EventHandler<(usize, String)>,
) -> Element {
    // Options pré-calculées (pas de `let` dans un `for` rsx).
    let options: Vec<(String, String, bool)> = channels
        .iter()
        .map(|ch| {
            let id = ch.id.to_string();
            let selected = id == channel_id;
            let label = format!("{} ({})", ch.name, ch.kind);
            (id, label, selected)
        })
        .collect();
    rsx! {
        div { class: "flex gap-1 items-center",
            select {
                class: "flex-1 px-2 py-1 border border-gray-300 rounded text-xs bg-white disabled:bg-gray-50",
                disabled: !can_write,
                value: "{channel_id}",
                onchange: move |event| on_pick.call((index, event.value())),
                option { value: "", selected: channel_id.is_empty(), {t!("flows-notify-pick-channel")} }
                for (opt_id, opt_label, opt_selected) in options {
                    option { value: opt_id.clone(), selected: opt_selected, {opt_label} }
                }
            }
        }
    }
}

/// One template-var row: read-only mono name + value input — a literal or a
/// `{{ msg.x }}` expression pre-rendered against `{msg, meta}`. Dedicated
/// component (no `let` inside `for` rsx — dioxus pitfall).
#[component]
fn NotifyVarRow(
    name: String,
    value: String,
    can_write: bool,
    on_change: EventHandler<(String, String)>,
) -> Element {
    rsx! {
        div { class: "flex gap-1 items-center",
            span { class: "w-28 shrink-0 truncate text-xs font-mono text-gray-600", "{name}" }
            input {
                class: "flex-1 px-2 py-1 border border-gray-300 rounded text-xs font-mono",
                placeholder: t!("flows-inspector-var-value-placeholder"),
                value: "{value}",
                disabled: !can_write,
                oninput: move |event| {
                    // Value-only edit: the name is fixed by the template.
                    let new_value = event.value();
                    on_change.call((name.clone(), new_value));
                },
            }
        }
    }
}

/// Current value of a template variable (empty when unset).
fn var_value(rows: &[(String, String)], name: &str) -> String {
    rows.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// Sets a template variable, appending it when absent.
fn upsert_var(rows: &mut NotifyVarRows, name: String, value: String) {
    if let Some(row) = rows.iter_mut().find(|(k, _)| *k == name) {
        row.1 = value;
    } else {
        rows.push((name, value));
    }
}

/// Writes the variable rows into the selected notify node.
fn store_vars(cx: &mut EditorCx, rows: &NotifyVarRows) {
    let map: std::collections::BTreeMap<String, String> = rows.iter().cloned().collect();
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::PnexNotify { config } = &mut node.kind {
            config.vars = map;
        }
    });
}
