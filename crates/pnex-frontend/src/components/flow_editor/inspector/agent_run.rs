use std::collections::BTreeMap;

use super::helpers::*;
use super::*;

use super::site_recipe::RuleFields;
use crate::components::ui::FIELD_LABEL;
use pnex_core::agent_chains::{AgentRunFinishedConfig, AgentRunNodeConfig, ChainLimits};

// ─────────────── agent-run / agent-run-finished (agents.md D713) ───────────────

const INPUT: &str = "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm";

/// Agent run inspector: what it runs (an agent or a website rule, D786); for an agent,
/// the message path of each of its declared inputs, the subject path and the bounds of
/// the chain the node opens.
#[component]
pub(super) fn AgentRunForm(
    mut cx: EditorCx,
    initial: AgentRunNodeConfig,
    can_write: bool,
) -> Element {
    let agents = use_resource(|| async {
        api::hosted_agents::agents(200, 0)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });
    let mut rule = use_signal(|| !initial.recipe_id.is_empty());
    let mut agent_id = use_signal(|| initial.agent_id.clone());
    let mut inputs = use_signal(|| initial.inputs.clone());
    // Inputs read as a text of the message instead of an artifact id.
    let mut texts = use_signal(|| initial.texts.clone());
    let mut subject = use_signal(|| initial.subject_from.clone());
    let mut ticket = use_signal(|| initial.ticket_from.clone());
    let mut limits = use_signal(|| initial.limits);
    let mut save = move || {
        let next = AgentRunNodeConfig {
            agent_id: agent_id(),
            inputs: inputs(),
            texts: texts(),
            subject_from: subject(),
            ticket_from: ticket(),
            limits: limits(),
            ..Default::default()
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::AgentRun { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let list = agents.read().clone().unwrap_or_default();
    // The declared inputs of the picked agent, each with its path (default `payload.<name>`)
    // and whether the path holds a text rather than an artifact id.
    let declared: Vec<(String, String, bool)> = list
        .iter()
        .find(|a| a.id == agent_id())
        .map(|a| {
            a.spec
                .inputs
                .iter()
                .map(|name| {
                    let text = texts.read().get(name).cloned();
                    let path = text
                        .clone()
                        .or_else(|| inputs.read().get(name).cloned())
                        .unwrap_or_default();
                    (name.clone(), path, text.is_some())
                })
                .collect()
        })
        .unwrap_or_default();
    let on_pick = move |e: Event<FormData>| {
        let id = e.value();
        let names: Vec<String> = agents
            .read()
            .clone()
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.id == id)
            .map(|a| a.spec.inputs)
            .unwrap_or_default();
        let map: BTreeMap<String, String> = names
            .into_iter()
            .map(|n| (n.clone(), format!("payload.{n}")))
            .collect();
        agent_id.set(id);
        inputs.set(map);
        texts.set(BTreeMap::new());
        save();
    };
    // `which`: 0 runs, 1 model calls, 2 rounds.
    let limit_field = move |label: String, which: u8, value: i32| {
        rsx! {
            label { class: FIELD_LABEL, r#for: "agent-run-limit-{which}", "{label}" }
            input {
                id: "agent-run-limit-{which}",
                class: INPUT,
                r#type: "number",
                min: "1",
                disabled: !can_write,
                value: "{value}",
                onchange: move |e| {
                    let n = e.value().parse::<i32>().unwrap_or(1).max(1);
                    let mut l: ChainLimits = limits();
                    match which {
                        0 => l.runs_max = n,
                        1 => l.llm_calls_max = n,
                        _ => l.rounds_max = n,
                    }
                    limits.set(l);
                    save();
                },
            }
        }
    };
    let current = limits();
    let on_mode = move |e: Event<FormData>| {
        rule.set(e.value() == "rule");
        agent_id.set(String::new());
        inputs.set(BTreeMap::new());
        texts.set(BTreeMap::new());
        save();
    };

    rsx! {
        div { class: "space-y-3",
            label { class: FIELD_LABEL, r#for: "agent-run-mode", {t!("flows-agent-run-mode")} }
            select {
                id: "agent-run-mode",
                class: INPUT,
                "data-testid": "agent-run-mode",
                disabled: !can_write,
                onchange: on_mode,
                option { value: "agent", selected: !rule(), {t!("flows-agent-run-mode-agent")} }
                option { value: "rule", selected: rule(), {t!("flows-agent-run-mode-rule")} }
            }
            if rule() {
                RuleFields { cx, initial: initial.clone(), can_write }
            } else {
                p { class: "text-xs text-gray-500", {t!("flows-agent-run-help")} }
                label { class: FIELD_LABEL, r#for: "agent-run-agent", {t!("flows-agent-run-agent")} }
                select {
                    id: "agent-run-agent",
                    class: INPUT,
                    "data-testid": "agent-run-agent",
                    disabled: !can_write,
                    onchange: on_pick,
                    option { value: "", selected: agent_id().is_empty(), "—" }
                    for a in list {
                        option {
                            key: "{a.id}",
                            selected: a.id == agent_id(),
                            value: "{a.id}",
                            "{a.name}"
                        }
                    }
                }
                for (name, path, is_text) in declared {
                    label { key: "l-{name}", class: FIELD_LABEL, "{name}" }
                    input {
                        class: INPUT,
                        disabled: !can_write,
                        value: "{path}",
                        onchange: {
                            let name = name.clone();
                            move |e: Event<FormData>| {
                                let mut target = if is_text { texts } else { inputs };
                                target.with_mut(|m| m.insert(name.clone(), e.value()));
                                save();
                            }
                        },
                    }
                    label { class: "flex items-center gap-2 text-xs text-gray-600",
                        input {
                            r#type: "checkbox",
                            disabled: !can_write,
                            checked: is_text,
                            onchange: move |e| {
                                let (mut from, mut to) = if e.checked() {
                                    (inputs, texts)
                                } else {
                                    (texts, inputs)
                                };
                                let path = from.with_mut(|m| m.remove(&name)).unwrap_or_default();
                                to.with_mut(|m| m.insert(name.clone(), path));
                                save();
                            },
                        }
                        {t!("flows-agent-run-input-is-text")}
                    }
                }
                p { class: "text-xs text-gray-500", {t!("flows-agent-run-inputs-help")} }
                label { class: FIELD_LABEL, r#for: "agent-run-subject",
                    {t!("flows-agent-run-subject")}
                }
                input {
                    id: "agent-run-subject",
                    class: INPUT,
                    disabled: !can_write,
                    value: "{subject}",
                    oninput: move |e| subject.set(e.value()),
                    onchange: move |_| save(),
                }
                label { class: FIELD_LABEL, r#for: "agent-run-ticket", {t!("flows-agent-run-ticket")} }
                input {
                    id: "agent-run-ticket",
                    class: INPUT,
                    disabled: !can_write,
                    value: "{ticket}",
                    oninput: move |e| ticket.set(e.value()),
                    onchange: move |_| save(),
                }
                p { class: "text-xs text-gray-500", {t!("flows-agent-run-ticket-help")} }
                {limit_field(t!("flows-agent-run-runs-max").to_string(), 0, current.runs_max)}
                {
                    limit_field(
                        t!("flows-agent-run-llm-calls-max").to_string(),
                        1,
                        current.llm_calls_max,
                    )
                }
                {limit_field(t!("flows-agent-run-rounds-max").to_string(), 2, current.rounds_max)}
            }
        }
    }
}

/// Run-finished trigger inspector: one agent or any, chained runs only or all; one
/// website rule or any, and one outcome of it or any (D786).
#[component]
pub(super) fn AgentRunFinishedForm(
    mut cx: EditorCx,
    initial: AgentRunFinishedConfig,
    can_write: bool,
) -> Element {
    let agents = use_resource(|| async {
        api::hosted_agents::agents(200, 0)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });
    let recipes = use_resource(|| async { api::site_recipes::all().await.unwrap_or_default() });
    let mut agent_id = use_signal(|| initial.agent_id.clone());
    let mut chained = use_signal(|| initial.chained_only);
    let mut recipe_id = use_signal(|| initial.recipe_id.clone());
    let mut outcome = use_signal(|| initial.outcome.clone());
    let mut save = move || {
        let next = AgentRunFinishedConfig {
            agent_id: agent_id(),
            chained_only: chained(),
            recipe_id: recipe_id(),
            outcome: outcome(),
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::AgentRunFinished { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let list = agents.read().clone().unwrap_or_default();
    let rules = recipes.read().clone().unwrap_or_default();
    let outcomes = [
        (
            pnex_core::agent_chains::RULE_PASSED,
            t!("flows-agent-run-outcome-passed"),
        ),
        (
            pnex_core::agent_chains::RULE_FAILED,
            t!("flows-agent-run-outcome-failed"),
        ),
        (
            pnex_core::agent_chains::RULE_BROKE,
            t!("flows-agent-run-outcome-broke"),
        ),
    ];

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-agent-run-finished-help")} }
            label { class: FIELD_LABEL, r#for: "agent-run-finished-agent",
                {t!("flows-agent-run-agent")}
            }
            select {
                id: "agent-run-finished-agent",
                class: INPUT,
                "data-testid": "agent-run-finished-agent",
                disabled: !can_write,
                onchange: move |e| {
                    agent_id.set(e.value());
                    save();
                },
                option { value: "", selected: agent_id().is_empty(),
                    {t!("flows-agent-run-finished-any")}
                }
                for a in list {
                    option {
                        key: "{a.id}",
                        selected: a.id == agent_id(),
                        value: "{a.id}",
                        "{a.name}"
                    }
                }
            }
            label { class: "flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    disabled: !can_write,
                    checked: chained(),
                    onchange: move |e| {
                        chained.set(e.checked());
                        save();
                    },
                }
                {t!("flows-agent-run-finished-chained")}
            }
            label { class: FIELD_LABEL, r#for: "agent-run-finished-rule",
                {t!("flows-site-recipe-rule")}
            }
            select {
                id: "agent-run-finished-rule",
                class: INPUT,
                "data-testid": "agent-run-finished-rule",
                disabled: !can_write,
                onchange: move |e| {
                    recipe_id.set(e.value());
                    save();
                },
                option { value: "", selected: recipe_id().is_empty(),
                    {t!("flows-agent-run-finished-any-rule")}
                }
                for r in rules {
                    option {
                        key: "{r.id}",
                        selected: r.id == recipe_id(),
                        value: "{r.id}",
                        "{r.site} · {r.name}"
                    }
                }
            }
            label { class: FIELD_LABEL, r#for: "agent-run-finished-outcome",
                {t!("flows-agent-run-outcome")}
            }
            select {
                id: "agent-run-finished-outcome",
                class: INPUT,
                "data-testid": "agent-run-finished-outcome",
                disabled: !can_write,
                onchange: move |e| {
                    outcome.set(e.value());
                    save();
                },
                option { value: "", selected: outcome().is_empty(),
                    {t!("flows-agent-run-finished-any-outcome")}
                }
                for (value, label) in outcomes {
                    option {
                        key: "{value}",
                        selected: outcome() == value,
                        value: "{value}",
                        "{label}"
                    }
                }
            }
            p { class: "text-xs text-gray-500", {t!("flows-agent-run-outcome-help")} }
        }
    }
}
