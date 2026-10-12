use super::helpers::*;
use super::*;

use crate::components::ui::{BTN_SECONDARY_SM, FIELD_LABEL};
use pnex_core::decide::{
    DecideQuestion, QuestionKind, TextDecideConfig, DECIDE_INSTRUCTIONS_MAX, DECIDE_MAX_QUESTIONS,
};

// ─────────────── text-decide (media-vision-studio.md D197) ───────────────

fn patch_decide(cx: &mut EditorCx, f: impl FnOnce(&mut TextDecideConfig) + 'static) {
    patch_selected(cx, move |node: &mut FlowNode| {
        if let FlowNodeKind::TextDecide { config } = &mut node.kind {
            f(config);
        }
    });
}

/// One option per line; for a choice `label: description` (the description
/// helps the model, the label is what the flow routes on).
fn parse_options(raw: &str, kind: QuestionKind) -> (Vec<String>, Vec<String>) {
    let mut options = Vec::new();
    let mut descriptions = Vec::new();
    for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match line
            .split_once(':')
            .filter(|_| kind == QuestionKind::Choice)
        {
            Some((label, desc)) => {
                options.push(label.trim().to_string());
                descriptions.push(desc.trim().to_string());
            }
            None => {
                options.push(line.to_string());
                descriptions.push(String::new());
            }
        }
    }
    // Trailing empty descriptions are dropped (shorter config).
    while descriptions.last().is_some_and(String::is_empty) {
        descriptions.pop();
    }
    (options, descriptions)
}

/// Inverse of [`parse_options`] for the editing buffer.
fn options_text(q: &DecideQuestion) -> String {
    q.options
        .iter()
        .enumerate()
        .map(
            |(i, o)| match q.descriptions.get(i).filter(|d| !d.is_empty()) {
                Some(d) => format!("{o}: {d}"),
                None => o.clone(),
            },
        )
        .collect::<Vec<_>>()
        .join("\n")
}

/// Select value → kind.
fn kind_of(value: &str) -> QuestionKind {
    if value == "score" {
        QuestionKind::Score
    } else {
        QuestionKind::Choice
    }
}

/// First free `qN` id.
fn next_id(questions: &[DecideQuestion]) -> String {
    (1..)
        .map(|n| format!("q{n}"))
        .find(|id| !questions.iter().any(|q| &q.id == id))
        .unwrap_or_default()
}

/// One question: id, kind, instructions, options (one per line).
#[component]
fn QuestionCard(
    mut cx: EditorCx,
    index: usize,
    question: DecideQuestion,
    can_write: bool,
) -> Element {
    let mut id = use_signal(|| question.id.clone());
    let mut instructions = use_signal(|| question.instructions.clone());
    let mut options_raw = use_signal(|| options_text(&question));
    let kind = question.kind;
    let count = question.options.len();
    let (options_placeholder, options_hint) = if kind == QuestionKind::Score {
        (
            t!("flows-decide-levels-placeholder").to_string(),
            t!("flows-decide-levels-hint", count: count).to_string(),
        )
    } else {
        (
            t!("flows-decide-options-placeholder").to_string(),
            t!("flows-decide-options-hint", count: count).to_string(),
        )
    };
    let instructions_placeholder = t!("flows-decide-instructions-placeholder").to_string();

    rsx! {
        div { class: "border border-gray-200 rounded-lg p-2 space-y-2",
            div { class: "flex items-center gap-2",
                input {
                    class: "flex-1 min-w-0 px-2 py-1 border border-gray-300 rounded-lg text-sm font-mono",
                    r#type: "text",
                    maxlength: "32",
                    value: "{id}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let next = event.value().trim().to_lowercase();
                        id.set(next.clone());
                        patch_decide(
                            &mut cx,
                            move |c| {
                                if let Some(q) = c.questions.get_mut(index) {
                                    q.id = next;
                                }
                            },
                        );
                    },
                }
                select {
                    class: "px-2 py-1 border border-gray-300 rounded-lg text-sm",
                    disabled: !can_write,
                    onchange: move |event| {
                        let next = kind_of(&event.value());
                        let (options, descriptions) = parse_options(&options_raw.peek(), next);
                        patch_decide(
                            &mut cx,
                            move |c| {
                                if let Some(q) = c.questions.get_mut(index) {
                                    q.kind = next;
                                    q.options = options;
                                    q.descriptions = descriptions;
                                }
                            },
                        );
                    },
                    option {
                        value: "choice",
                        selected: kind == QuestionKind::Choice,
                        {t!("flows-decide-kind-choice")}
                    }
                    option { value: "score", selected: kind == QuestionKind::Score,
                        {t!("flows-decide-kind-score")}
                    }
                }
                if can_write {
                    button {
                        r#type: "button",
                        class: "text-xs text-red-600 hover:underline",
                        onclick: move |_| {
                            patch_decide(
                                &mut cx,
                                move |c| {
                                    if index < c.questions.len() {
                                        c.questions.remove(index);
                                    }
                                },
                            );
                        },
                        {t!("flows-decide-remove")}
                    }
                }
            }
            textarea {
                class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm",
                rows: "2",
                maxlength: "{DECIDE_INSTRUCTIONS_MAX}",
                placeholder: "{instructions_placeholder}",
                value: "{instructions}",
                disabled: !can_write,
                oninput: move |event| {
                    let next = event.value();
                    instructions.set(next.clone());
                    patch_decide(
                        &mut cx,
                        move |c| {
                            if let Some(q) = c.questions.get_mut(index) {
                                q.instructions = next;
                            }
                        },
                    );
                },
            }
            textarea {
                class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                rows: "4",
                placeholder: "{options_placeholder}",
                value: "{options_raw}",
                disabled: !can_write,
                oninput: move |event| {
                    let raw = event.value();
                    options_raw.set(raw.clone());
                    let (options, descriptions) = parse_options(&raw, kind);
                    patch_decide(
                        &mut cx,
                        move |c| {
                            if let Some(q) = c.questions.get_mut(index) {
                                q.options = options;
                                q.descriptions = descriptions;
                            }
                        },
                    );
                },
            }
            span { class: "text-xs text-gray-500 block", "{options_hint}" }
        }
    }
}

/// Text decide inspector: text field, question cards, calibration note.
#[component]
pub(super) fn TextDecideForm(
    mut cx: EditorCx,
    initial: TextDecideConfig,
    can_write: bool,
) -> Element {
    let mut field = use_signal(|| initial.text_field.clone());
    let questions = initial.questions.clone();
    let n = questions.len();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-decide-help")} }
            p { class: "text-xs text-amber-700", {t!("flows-decide-calibration")} }
            label { class: "block",
                span { class: FIELD_LABEL, {t!("flows-decide-text-field")} }
                input {
                    class: "w-full px-2 py-1.5 border border-gray-300 rounded-lg text-sm font-mono",
                    r#type: "text",
                    placeholder: "text",
                    value: "{field}",
                    disabled: !can_write,
                    oninput: move |event| {
                        let next = event.value();
                        field.set(next.clone());
                        patch_decide(&mut cx, move |c| c.text_field = next.trim().to_string());
                    },
                }
            }
            if questions.is_empty() {
                p { class: "text-xs text-amber-700", {t!("flows-decide-none")} }
            }
            for (i, q) in questions.into_iter().enumerate() {
                // Cards remount when the count changes (indices shift).
                QuestionCard {
                    key: "{i}-{n}",
                    cx,
                    index: i,
                    question: q,
                    can_write,
                }
            }
            if can_write && n < DECIDE_MAX_QUESTIONS {
                button {
                    r#type: "button",
                    class: BTN_SECONDARY_SM,
                    onclick: move |_| {
                        patch_decide(
                            &mut cx,
                            move |c| {
                                let id = next_id(&c.questions);
                                c.questions
                                    .push(DecideQuestion {
                                        id,
                                        ..Default::default()
                                    });
                            },
                        );
                    },
                    {t!("flows-decide-add")}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_round_trip() {
        let (o, d) = parse_options(
            "incident: someone at risk\n\n routine \n",
            QuestionKind::Choice,
        );
        assert_eq!(o, vec!["incident", "routine"]);
        assert_eq!(d, vec!["someone at risk"]);
        let q = DecideQuestion {
            options: o,
            descriptions: d,
            ..Default::default()
        };
        assert_eq!(options_text(&q), "incident: someone at risk\nroutine");
        let (o, d) = parse_options("low: calm\nhigh", QuestionKind::Score);
        assert_eq!(o, vec!["low: calm", "high"]);
        assert!(d.is_empty());
        assert_eq!(next_id(&[q]), "q1");
    }
}
