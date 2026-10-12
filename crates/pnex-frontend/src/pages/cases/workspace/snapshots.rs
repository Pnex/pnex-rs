//! Frozen states of a case (cases-workspace.md K33), in the case frame:
//! freeze with an optional label, list them with their fingerprint, save
//! one as JSON (the fingerprinted bytes) or as Markdown.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{
    CaseExport, CaseProposal, CaseSnapshot, CaseSnapshotContent, CaseSnapshotInput,
};

use super::markdown::{render, Words};
use super::{model, proposal_title, Ws};
use crate::api;
use crate::components::ontology::add_to_case::role_label;
use crate::components::ui::{BTN_GHOST, BTN_SECONDARY_SM, FIELD_HELP, INPUT};
use crate::state::toasts;

/// The case as Markdown, in the reader's language.
pub(super) fn markdown(
    doc: &CaseExport,
    frozen: Option<(&CaseSnapshot, &[CaseProposal])>,
    local: bool,
) -> String {
    let label = |k: &str| t!(k).to_string();
    let words = Words {
        label: &label,
        role: &role_label,
        proposal: &proposal_title,
    };
    render(doc, frozen, local, &words)
}

/// A file name from the case title.
pub(super) fn file_name(title: &str, suffix: &str) -> String {
    format!("{}{suffix}", title.replace(['/', '\\'], "-"))
}

#[component]
pub(super) fn Snapshots(case_id: String, title: String) -> Element {
    let ws = use_context::<Ws>();
    let mut version = use_signal(|| 0u32);
    let mut label = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let id = case_id.clone();
    let list = use_resource(move || {
        let id = id.clone();
        let _ = version();
        async move { api::cases::snapshots(&id).await.unwrap_or_default() }
    });
    let id = case_id.clone();
    let freeze = move |_| {
        if busy() {
            return;
        }
        let input = CaseSnapshotInput {
            label: Some(label()).filter(|l| !l.trim().is_empty()),
        };
        let id = id.clone();
        busy.set(true);
        spawn(async move {
            match api::cases::freeze(&id, &input).await {
                Ok(s) => {
                    label.set(String::new());
                    version += 1;
                    toasts::success(t!("cw-snapshot-done", seq : s.seq).to_string());
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    // Saves frozen state `seq`, as fingerprinted (JSON) or as Markdown.
    let save = move |seq: i32, md: bool| {
        let (id, title) = (case_id.clone(), title.clone());
        let local = *ws.local_time.peek();
        spawn(async move {
            let full = match api::cases::snapshot(&id, seq).await {
                Ok(f) => f,
                Err(e) => return toasts::error(e),
            };
            if !md {
                let name = file_name(&title, &format!(".snapshot-{seq}.json"));
                return crate::util::save_blob(&name, full.content.as_bytes());
            }
            match serde_json::from_str::<CaseSnapshotContent>(&full.content) {
                Ok(c) => {
                    let text = markdown(&c.export, Some((&full.snapshot, &c.proposals)), local);
                    let name = file_name(&title, &format!(".snapshot-{seq}.md"));
                    crate::util::save_blob(&name, text.as_bytes());
                }
                Err(e) => toasts::error(e.to_string()),
            }
        });
    };
    let rows = list.read().clone().unwrap_or_default();
    let local = (ws.local_time)();
    rsx! {
        div { class: "space-y-2", "data-testid": "case-snapshots",
            h3 { class: "text-sm font-semibold text-ink", {t!("cw-snapshots")} }
            if ws.can_write {
                div { class: "flex gap-2",
                    input {
                        class: INPUT,
                        maxlength: 255,
                        placeholder: t!("cw-snapshot-label"),
                        "aria-label": t!("cw-snapshot-label"),
                        value: "{label}",
                        oninput: move |e| label.set(e.value()),
                    }
                    button {
                        class: "{BTN_SECONDARY_SM} whitespace-nowrap",
                        r#type: "button",
                        disabled: busy(),
                        "data-testid": "case-freeze",
                        onclick: freeze,
                        {t!("cw-freeze")}
                    }
                }
            }
            if rows.is_empty() {
                p { class: FIELD_HELP, {t!("cw-snapshots-none")} }
            }
            for s in rows {
                div {
                    key: "{s.seq}",
                    class: "flex items-start gap-2 border-b border-line py-1.5 text-xs",
                    "data-testid": "case-snapshot",
                    div { class: "min-w-0 flex-1",
                        div { class: "text-ink",
                            {t!("cw-snapshot-seq", seq : s.seq)}
                            if let Some(l) = s.label.clone() {
                                " · {l}"
                            }
                        }
                        div { class: "text-slate",
                            {model::ms(&s.taken_at).map(|t| model::stamp(t, local)).unwrap_or_default()}
                            if let Some(b) = s.taken_by.clone() {
                                " · {b}"
                            }
                        }
                        div {
                            class: "truncate font-mono text-slate",
                            title: "{s.sha256}",
                            "sha256 {s.sha256}"
                        }
                    }
                    button {
                        class: BTN_GHOST,
                        r#type: "button",
                        onclick: {
                            let seq = s.seq;
                            let save = save.clone();
                            move |_| save(seq, false)
                        },
                        "JSON"
                    }
                    button {
                        class: BTN_GHOST,
                        r#type: "button",
                        onclick: {
                            let seq = s.seq;
                            let save = save.clone();
                            move |_| save(seq, true)
                        },
                        "Markdown"
                    }
                }
            }
            p { class: FIELD_HELP, {t!("cw-snapshots-help")} }
        }
    }
}
