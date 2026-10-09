use super::helpers::*;
use super::*;

/// Formulaire du nœud `json-split` : les clés déclarées = **ports de sortie
/// nommés** (une clé = un port, la valeur de `payload[key]` sort sur son
/// port). Le fil suit sa clé (rename/remove via `rewire_split_keys`) ;
/// « importer les clés » pré-remplit depuis un Value amont statique-objet
/// (one-shot, pas d'auto-sync). Zéro clé = comportement legacy (1 port).
#[component]
pub(super) fn JsonSplitForm(
    mut cx: EditorCx,
    initial: JsonSplitConfig,
    can_write: bool,
) -> Element {
    let mut keys = use_signal(move || initial.keys.clone());

    // Amont « clés connues » → clés importables (pré-calcul hors rsx) :
    // Value statique-objet (clés du payload) ou Merge (ses entrées nommées
    // = les clés de l'objet accumulé — pattern merge → split).
    let node_id = cx.selected_node.peek().clone();
    let import_keys: Option<Vec<String>> = node_id.and_then(|id| {
        let graph = cx.graph.cloned();
        let upstream = graph.nodes.iter().find(|n| {
            n.outputs.iter().any(|w| w.targets.iter().any(|t| t == &id))
                && match &n.kind {
                    FlowNodeKind::Value { config } => config.value.as_object().is_some(),
                    FlowNodeKind::JsonMerge { config } => {
                        config.inputs.iter().any(|i| !i.trim().is_empty())
                    }
                    _ => false,
                }
        });
        match upstream.map(|n| &n.kind) {
            Some(FlowNodeKind::Value { config }) => config
                .value
                .as_object()
                .map(|o| o.keys().cloned().collect()),
            Some(FlowNodeKind::JsonMerge { config }) => Some(
                config
                    .inputs
                    .iter()
                    .filter(|i| !i.trim().is_empty())
                    .cloned()
                    .collect(),
            ),
            _ => None,
        }
    });
    let importable = can_write && import_keys.as_ref().is_some_and(|k| !k.is_empty());

    let key_rows = keys.read().clone();
    let import_for_click = import_keys.clone();
    let auto = initial.auto;
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-json-split-hint")} }
            label { class: "flex items-center gap-2 text-xs text-gray-600",
                input {
                    r#type: "checkbox",
                    checked: auto,
                    disabled: !can_write,
                    onchange: move |_| {
                        // Bascule auto↔manuel : le funnel update_graph
                        // relance la synchro — repasser en auto régénère les
                        // clés à la volée.
                        patch_selected(
                            &mut cx,
                            |node| {
                                if let FlowNodeKind::JsonSplit { config } = &mut node.kind {
                                    config.auto = !config.auto;
                                }
                            },
                        );
                    },
                }
                {t!("flows-json-split-auto")}
            }
            div { class: "space-y-1",
                span { class: "text-xs font-medium text-gray-500 block",
                    {t!("flows-json-split-keys-label")}
                }
                if auto {
                    // Ports suivis automatiquement : liste en lecture seule.
                    for k in key_rows.iter() {
                        div { class: "text-xs font-mono text-gray-600 pl-4", "• {k}" }
                    }
                    if key_rows.is_empty() {
                        p { class: "text-xs text-gray-400", {t!("flows-json-split-auto-empty")} }
                    }
                }
                if !auto {
                    // Renommer une clé ne re-câble rien : le port i
                    // = keys[i], l'index ne bouge pas (seules les
                    // suppressions/import décalent les ports).
                    for (index, key) in key_rows.iter().enumerate() {
                        JsonNameRow {
                            key: "{index}",
                            index,
                            value: key.clone(),
                            placeholder: t!("flows-json-split-key-placeholder"),
                            can_write,
                            on_change: move |(i, new_key)| {
                                let old = keys.read().clone();
                                let mut new = old.clone();
                                if let Some(row) = new.get_mut(i) {
                                    *row = new_key;
                                }
                                keys.set(new.clone());
                                patch_selected(
                                    &mut cx,
                                    move |node| {
                                        if let FlowNodeKind::JsonSplit { config } = &mut node.kind {
                                            config.keys = new;
                                        }
                                    },
                                );
                            },
                            on_remove: move |i| {
                                let old = keys.read().clone();
                                let mut new = old.clone();
                                if i < new.len() {
                                    new.remove(i);
                                }
                                keys.set(new.clone());
                                let Some(id) = cx.selected_node.peek().clone() else {
                                    return;
                                };
                                cx.update_graph(move |graph| {
                                    state::rewire_split_keys(graph, &id, &old, &new);
                                    if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
                                        if let FlowNodeKind::JsonSplit { config } = &mut node.kind {
                                            config.keys = new;
                                        }
                                    }
                                });
                            },
                        }
                    }
                    if can_write {
                        button {
                            class: "text-xs text-blue-600 hover:text-blue-700",
                            onclick: move |_| {
                                let mut new = keys.read().clone();
                                let mut i = new.len() + 1;
                                let mut name = format!("key{i}");
                                while new.iter().any(|c| *c == name) {
                                    i += 1;
                                    name = format!("key{i}");
                                }
                                new.push(name);
                                keys.set(new.clone());
                                patch_selected(
                                    &mut cx,
                                    move |node| {
                                        if let FlowNodeKind::JsonSplit { config } = &mut node.kind {
                                            config.keys = new;
                                        }
                                    },
                                );
                            },
                            {t!("flows-json-split-add-key")}
                        }
                    }
                    // Nom auto : key1, key2… unique parmi les clés —
                    // une clé vide n'est jamais un port utilisable,
                    // elle n'a pas de raison d'exister.
                    if importable {
                        button {
                            class: "text-xs text-blue-600 hover:text-blue-700 ml-2",
                            onclick: move |_| {
                                let Some(imported) = import_for_click.clone() else {
                                    return;
                                };
                                let old = keys.read().clone();
                                keys.set(imported.clone());
                                let Some(id) = cx.selected_node.peek().clone() else {
                                    return;
                                };
                                cx.update_graph(move |graph| {
                                    state::rewire_split_keys(graph, &id, &old, &imported);
                                    if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
                                        if let FlowNodeKind::JsonSplit { config } = &mut node.kind {
                                            config.keys = imported;
                                        }
                                    }
                                });
                            },
                            {t!("flows-json-split-import-keys")}
                        }
                    }
                }
            }
        }
    }
}

/// Formulaire du nœud `json-merge` (« Merge to JSON ») : la clé sous
/// laquelle un payload sans `msg.topic` est rangé, et les **entrées
/// nommées** (une ligne = une variable rangée sous son nom dans l'objet
/// accumulé). Rename = l'annotation suit ; remove = prune (le fil casse
/// si plus aucune ligne ne le partage).
#[component]
pub(super) fn JsonMergeForm(
    mut cx: EditorCx,
    initial: JsonMergeConfig,
    can_write: bool,
) -> Element {
    let mut default_key = use_signal(move || initial.default_key.clone());
    let mut inputs = use_signal(move || initial.inputs.clone());

    let input_rows = inputs.read().clone();
    // La clé par défaut n'a de sens qu'en mode legacy (aucune entrée
    // nommée) : avec des lignes, tout fil est taggué par sa ligne et le
    // câblage nu est refusé à la sauvegarde — le champ serait mort et
    // incompréhensible affiché.
    let legacy_mode = input_rows.is_empty();
    rsx! {
        div { class: "space-y-3",
            if legacy_mode {
                {
                    text_field(
                        t!("flows-json-merge-key"),
                        default_key,
                        !can_write,
                        move |event| {
                            let raw = event.value();
                            default_key.set(raw.clone());
                            patch_selected(
                                &mut cx,
                                move |node: &mut FlowNode| {
                                    if let FlowNodeKind::JsonMerge { config } = &mut node.kind {
                                        config.default_key = raw;
                                    }
                                },
                            );
                        },
                    )
                }
            }
            div { class: "space-y-1",
                span { class: "text-xs font-medium text-gray-500 block",
                    {t!("flows-json-merge-inputs-label")}
                }
                for (index, name) in input_rows.iter().enumerate() {
                    JsonNameRow {
                        key: "{index}",
                        index,
                        value: name.clone(),
                        placeholder: t!("flows-json-merge-input-placeholder"),
                        can_write,
                        on_change: move |(i, new_name)| {
                            let old = inputs.read().clone();
                            let mut new = old.clone();
                            if let Some(row) = new.get_mut(i) {
                                *row = new_name;
                            }
                            let old_name: String = old.get(i).cloned().unwrap_or_default();
                            let renamed: String = new.get(i).cloned().unwrap_or_default();
                            inputs.set(new.clone());
                            let Some(id) = cx.selected_node.peek().clone() else {
                                return;
                            };
                            cx.update_graph(move |graph| {
                                state::rename_merge_input(graph, &id, &old_name, &renamed);
                                if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
                                    if let FlowNodeKind::JsonMerge { config } = &mut node.kind {
                                        config.inputs = new;
                                    }
                                }
                            });
                        },
                        on_remove: move |i| {
                            let old = inputs.read().clone();
                            let mut new = old.clone();
                            if i < new.len() {
                                new.remove(i);
                            }
                            inputs.set(new.clone());
                            let Some(id) = cx.selected_node.peek().clone() else {
                                return;
                            };
                            cx.update_graph(move |graph| {
                                state::prune_merge_inputs(graph, &id, &new);
                                if let Some(node) = graph.nodes.iter_mut().find(|n| n.id == id) {
                                    if let FlowNodeKind::JsonMerge { config } = &mut node.kind {
                                        config.inputs = new;
                                    }
                                }
                            });
                        },
                    }
                }
                if can_write {
                    button {
                        class: "text-xs text-blue-600 hover:text-blue-700",
                        onclick: move |_| {
                            // Nom auto : value1, value2… unique parmi les
                            // entrées — une ligne vide n'est pas câblable,
                            // elle n'a pas de raison d'exister.
                            let mut new = inputs.read().clone();
                            let mut i = new.len() + 1;
                            let mut name = format!("value{i}");
                            while new.iter().any(|c| *c == name) {
                                i += 1;
                                name = format!("value{i}");
                            }
                            new.push(name);
                            inputs.set(new.clone());
                            patch_selected(
                                &mut cx,
                                move |node| {
                                    if let FlowNodeKind::JsonMerge { config } = &mut node.kind {
                                        config.inputs = new;
                                    }
                                },
                            );
                        },
                        {t!("flows-json-merge-add-input")}
                    }
                }
            }
            p { class: "text-xs text-gray-400", {t!("flows-json-merge-hint")} }
        }
    }
}

/// Une ligne nom réutilisable (clé de split / entrée de merge) — composant
/// dédié (pas de `let` dans le for rsx, piège dioxus).
/// `on_change((index, value))` remonte la saisie.
#[component]
fn JsonNameRow(
    index: usize,
    value: String,
    placeholder: String,
    can_write: bool,
    on_change: EventHandler<(usize, String)>,
    on_remove: EventHandler<usize>,
) -> Element {
    rsx! {
        div { class: "flex gap-1 items-center",
            input {
                class: "flex-1 px-2 py-1 border border-gray-300 rounded text-xs font-mono",
                placeholder: "{placeholder}",
                value: "{value}",
                disabled: !can_write,
                oninput: move |event| {
                    let new_value = event.value();
                    on_change.call((index, new_value));
                },
            }
            if can_write {
                button {
                    class: "text-xs text-red-500 hover:text-red-700",
                    onclick: move |_| on_remove.call(index),
                    "✕"
                }
            }
        }
    }
}
