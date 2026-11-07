use std::collections::BTreeMap;

use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::agent_chains::{AgentRunFinishedConfig, AgentRunNodeConfig, ChainLimits};

// ─────────────── agent-run / agent-run-finished (agent-environments.md D713) ───────────────

const INPUT: &str = "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm";

/// Agent run inspector: the agent, the message path of each of its declared inputs, the
/// subject path and the bounds of the chain the node opens.
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
    let mut agent_id = use_signal(|| initial.agent_id.clone());
    let mut inputs = use_signal(|| initial.inputs.clone());
    let mut subject = use_signal(|| initial.subject_from.clone());
    let mut limits = use_signal(|| initial.limits);
    let mut save = move || {
        let next = AgentRunNodeConfig {
            agent_id: agent_id(),
            inputs: inputs(),
            subject_from: subject(),
            limits: limits(),
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::AgentRun { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let list = agents.read().clone().unwrap_or_default();
    // The declared inputs of the picked agent, each with its path (default `payload.<name>`).
    let declared: Vec<(String, String)> = list
        .iter()
        .find(|a| a.id == agent_id())
        .map(|a| {
            a.spec
                .inputs
                .iter()
                .map(|name| {
                    let path = inputs.read().get(name).cloned().unwrap_or_default();
                    (name.clone(), path)
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

    rsx! {
        div { class: "space-y-3",
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
            for (name, path) in declared {
                label { key: "l-{name}", class: FIELD_LABEL, "{name}" }
                input {
                    key: "i-{name}",
                    class: INPUT,
                    disabled: !can_write,
                    value: "{path}",
                    onchange: move |e| {
                        inputs.with_mut(|m| m.insert(name.clone(), e.value()));
                        save();
                    },
                }
            }
            p { class: "text-xs text-gray-500", {t!("flows-agent-run-inputs-help")} }
            label { class: FIELD_LABEL, r#for: "agent-run-subject", {t!("flows-agent-run-subject")} }
            input {
                id: "agent-run-subject",
                class: INPUT,
                disabled: !can_write,
                value: "{subject}",
                oninput: move |e| subject.set(e.value()),
                onchange: move |_| save(),
            }
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

/// Run-finished trigger inspector: one agent or any, chained runs only or all.
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
    let mut agent_id = use_signal(|| initial.agent_id.clone());
    let mut chained = use_signal(|| initial.chained_only);
    let mut save = move || {
        let next = AgentRunFinishedConfig {
            agent_id: agent_id(),
            chained_only: chained(),
        };
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::AgentRunFinished { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let list = agents.read().clone().unwrap_or_default();

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
        }
    }
}
