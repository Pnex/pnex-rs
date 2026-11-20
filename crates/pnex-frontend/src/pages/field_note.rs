//! Field note on an object (parts-docs.md PD1d): a text and photos, kept on the
//! device and sent as a proposal of the maintenance pack when the network is
//! there (`state::field_notes`). Shown when the org has the `field_note` type.

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::ui::{
    BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_ERROR, FIELD_HELP, INPUT, PANEL,
};
use crate::state::field_notes::{self, QUEUE};

const TITLE_MAX: usize = 80;

/// The note's title: its first line, cut at 80 characters.
fn title_of(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(TITLE_MAX)
        .collect()
}

#[component]
pub fn FieldNoteForm(object_id: String) -> Element {
    let types = use_resource(crate::api::ontology::types);
    let mut open = use_signal(|| false);
    let mut text = use_signal(String::new);
    let mut photos = use_signal(Vec::<(String, String, Vec<u8>)>::new);
    let mut too_big = use_signal(|| false);
    let has_type = matches!(
        &*types.read(), Some(Ok(t)) if t.iter().any(|t| t.def.key == "field_note")
    );
    if !has_type || !crate::state::access::can("proposals:file") {
        return rsx! {};
    }
    let waiting: Vec<_> = QUEUE
        .read()
        .iter()
        .filter(|n| n.object_id == object_id)
        .cloned()
        .collect();
    let pending = waiting.iter().filter(|n| n.error.is_none()).count();
    let refused: Vec<_> = waiting.into_iter().filter(|n| n.error.is_some()).collect();
    rsx! {
        div { class: "{PANEL} p-4 space-y-3", "data-testid": "field-note",
            div { class: "flex flex-wrap items-center gap-3",
                if !open() {
                    button {
                        r#type: "button",
                        class: BTN_SECONDARY_SM,
                        "data-testid": "field-note-open",
                        onclick: move |_| open.set(true),
                        {t!("field-note-write")}
                    }
                }
                if pending > 0 {
                    span {
                        class: "text-sm text-slate",
                        "data-testid": "field-note-pending",
                        {t!("field-note-pending", count : pending)}
                    }
                }
            }
            for note in refused {
                div { class: "flex flex-wrap items-center gap-2 text-sm",
                    span { class: "text-alarm",
                        {t!("field-note-refused", title : note.title.clone())}
                        " "
                        {
                            crate::api::error_i18n::localize_code(
                                note.error.as_deref().unwrap_or_default(),
                                None,
                                "",
                            )
                        }
                    }
                    button {
                        r#type: "button",
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| field_notes::drop_note(&note),
                        {t!("field-note-drop")}
                    }
                }
            }
            if open() {
                textarea {
                    class: INPUT,
                    rows: 4,
                    maxlength: 4000,
                    "data-testid": "field-note-text",
                    placeholder: t!("field-note-placeholder"),
                    value: "{text}",
                    oninput: move |e| text.set(e.value()),
                }
                input {
                    r#type: "file",
                    class: "text-sm",
                    accept: "image/*",
                    multiple: true,
                    "data-testid": "field-note-photos",
                    onchange: move |evt: FormEvent| async move {
                        let mut got = Vec::new();
                        for file in evt.files() {
                            let bytes = file.read_bytes().await.map(|b| b.to_vec()).unwrap_or_default();
                            let kind = file.content_type().unwrap_or_else(|| "image/jpeg".into());
                            got.push((file.name(), kind, bytes));
                        }
                        photos.set(got);
                    },
                }
                p { class: FIELD_HELP, {t!("field-note-help")} }
                if too_big() {
                    p { class: FIELD_ERROR, {t!("field-note-too-big")} }
                }
                div { class: "flex gap-2",
                    button {
                        r#type: "button",
                        class: BTN_PRIMARY_SM,
                        disabled: title_of(&text()).is_empty(),
                        "data-testid": "field-note-send",
                        onclick: {
                            let id = object_id.clone();
                            move |_| {
                                let body = text();
                                if field_notes::push(&id, &title_of(&body), body.trim(), photos()) {
                                    text.set(String::new());
                                    photos.set(Vec::new());
                                    too_big.set(false);
                                    open.set(false);
                                    crate::state::toasts::success(t!("field-note-kept"));
                                } else {
                                    too_big.set(true);
                                }
                            }
                        },
                        {t!("field-note-send")}
                    }
                    button {
                        r#type: "button",
                        class: BTN_SECONDARY_SM,
                        onclick: move |_| open.set(false),
                        {t!("common-cancel")}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::title_of;

    #[test]
    fn the_title_is_the_first_line_cut() {
        assert_eq!(title_of("  Bearing noise \nmore"), "Bearing noise");
        assert_eq!(title_of(&"x".repeat(200)).len(), 80);
        assert_eq!(title_of(""), "");
    }
}
