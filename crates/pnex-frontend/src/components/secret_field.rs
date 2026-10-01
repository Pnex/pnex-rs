//! `SecretField` — the single secret input of every functional form
//! (notification channel, HTTP node, WiFi, LLM provider; secrets.md D113).
//!
//! Two modes, no templating: type a value (owner/admin, creates or
//! replaces the field's dedicated secret server-side) or pick an existing
//! secret of the org. A defined field shows "set · <name>", never a value.

use dioxus::prelude::*;
use dioxus_i18n::t;
use uuid::Uuid;

use crate::api;
use crate::components::icons;
use pnex_core::{SecretFieldInput, SecretFieldView, SecretSlot};

/// Form-side state of a secret field.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum SecretDraft {
    /// No secret (new consumer).
    #[default]
    Empty,
    /// Reference loaded from the server, unchanged.
    Keep(SecretFieldView),
    /// Another existing secret picked.
    Pick(SecretFieldView),
    /// Value typed in the form.
    Value(String),
}

impl SecretDraft {
    /// Initial draft from a consumer's read model.
    pub fn from_view(view: Option<SecretFieldView>) -> Self {
        view.map(Self::Keep).unwrap_or_default()
    }

    /// Wire input of the field; `None` = no secret set.
    pub fn to_input(&self) -> Option<SecretFieldInput> {
        match self {
            Self::Empty => None,
            Self::Keep(v) | Self::Pick(v) => Some(SecretFieldInput::Pick {
                secret_id: v.secret_id,
            }),
            Self::Value(value) if value.is_empty() => None,
            Self::Value(value) => Some(SecretFieldInput::Value {
                value: value.clone(),
            }),
        }
    }

    /// Draft of a graph slot (the name of a reference is looked up by
    /// [`SecretField`]).
    pub fn from_slot(slot: &SecretSlot) -> Self {
        match slot {
            SecretSlot::Unset => Self::Empty,
            SecretSlot::Ref(id) => Self::Keep(SecretFieldView {
                secret_id: *id,
                name: String::new(),
            }),
            SecretSlot::Value(v) => Self::Value(v.clone()),
        }
    }

    /// Graph slot of the draft.
    pub fn to_slot(&self) -> SecretSlot {
        match self {
            Self::Empty => SecretSlot::Unset,
            Self::Keep(v) | Self::Pick(v) => SecretSlot::Ref(v.secret_id),
            Self::Value(v) => SecretSlot::from(v.clone()),
        }
    }

    /// Wire value of the field: the input, or `null` (unchanged / none).
    pub fn to_json(&self) -> serde_json::Value {
        self.to_input()
            .and_then(|i| serde_json::to_value(i).ok())
            .unwrap_or(serde_json::Value::Null)
    }
}

/// Vault writes (typing a value) are owner/admin only (D117).
pub fn can_manage_secrets() -> bool {
    let (Some(user), Some(org_id)) = (crate::state::session::user(), crate::state::org::current())
    else {
        return false;
    };
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .is_some_and(|m| crate::state::org::role_can_administer(&m.role))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Show,
    Type,
    Pick,
}

const TAB: &str = "px-3 py-1 text-xs rounded-md";
const TAB_ON: &str = "px-3 py-1 text-xs rounded-md bg-white shadow text-gray-900";

#[component]
pub fn SecretField(
    label: String,
    draft: Signal<SecretDraft>,
    /// Owner/admin: may type a value (D117). Others may only pick.
    can_manage: bool,
    /// Viewer: shows the state, no action.
    #[props(default)]
    read_only: bool,
) -> Element {
    let mut draft = draft;
    // A reference loaded from a graph has no name yet: look it up (re-run
    // on draft changes, network only while a name is missing).
    let lookup = use_resource(move || async move {
        let unnamed = matches!(
            &*draft.read(),
            SecretDraft::Keep(v) | SecretDraft::Pick(v) if v.name.is_empty()
        );
        if !unnamed {
            return Vec::new();
        }
        api::secrets::list(None, 100, 0)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });
    let initial = match &*draft.peek() {
        SecretDraft::Keep(_) | SecretDraft::Pick(_) => Mode::Show,
        _ if can_manage => Mode::Type,
        _ => Mode::Pick,
    };
    let mut mode = use_signal(|| initial);
    // A reference set from outside (a save stored the typed value) shows
    // as "set".
    use_effect(move || {
        if matches!(draft(), SecretDraft::Keep(_)) {
            mode.set(Mode::Show);
        }
    });
    let options = use_resource(move || async move {
        if mode() != Mode::Pick {
            return Vec::new();
        }
        api::secrets::list(None, 100, 0)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });

    let current = draft();
    let shown = match &current {
        SecretDraft::Keep(v) | SecretDraft::Pick(v) if !v.name.is_empty() => Some(v.name.clone()),
        SecretDraft::Keep(v) | SecretDraft::Pick(v) => lookup
            .value()
            .read()
            .as_ref()
            .and_then(|list| list.iter().find(|s| s.id == v.secret_id))
            .map(|s| s.name.clone()),
        _ => None,
    };
    let typed = match &current {
        SecretDraft::Value(v) => v.clone(),
        _ => String::new(),
    };
    let picked: Option<Uuid> = match &current {
        SecretDraft::Keep(v) | SecretDraft::Pick(v) => Some(v.secret_id),
        _ => None,
    };
    let choices: Vec<(Uuid, String)> = options
        .value()
        .read()
        .as_ref()
        .map(|list| list.iter().map(|s| (s.id, s.name.clone())).collect())
        .unwrap_or_default();

    rsx! {
        div { class: "space-y-2",
            label { class: "block text-sm font-medium text-gray-700", {label} }
            if mode() == Mode::Show {
                div { class: "flex items-center gap-2",
                    span { class: "inline-flex items-center gap-1 rounded bg-green-50 border border-green-200 px-2 py-1 text-xs text-green-800",
                        icons::Key { class: "h-3.5 w-3.5" }
                        {t!("secret-field-set")}
                        if let Some(name) = shown {
                            span { class: "font-mono", "· {name}" }
                        }
                    }
                    if !read_only {
                        button {
                            r#type: "button",
                            class: "px-2 py-1 text-xs border border-gray-300 rounded-lg hover:bg-gray-50",
                            onclick: move |_| mode.set(if can_manage { Mode::Type } else { Mode::Pick }),
                            {t!("secret-field-change")}
                        }
                    }
                }
            } else if read_only {
                span { class: "text-xs text-gray-400", {t!("secret-field-unset")} }
            } else {
                div { class: "inline-flex rounded-lg bg-gray-100 p-0.5",
                    if can_manage {
                        button {
                            r#type: "button",
                            class: if mode() == Mode::Type { TAB_ON } else { TAB },
                            onclick: move |_| mode.set(Mode::Type),
                            {t!("secret-field-type")}
                        }
                    }
                    button {
                        r#type: "button",
                        class: if mode() == Mode::Pick { TAB_ON } else { TAB },
                        onclick: move |_| mode.set(Mode::Pick),
                        {t!("secret-field-pick")}
                    }
                }
                if mode() == Mode::Type {
                    input {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm font-mono",
                        r#type: "password",
                        autocomplete: "new-password",
                        placeholder: t!("secret-field-type-placeholder"),
                        value: "{typed}",
                        oninput: move |e| draft.set(SecretDraft::Value(e.value())),
                    }
                } else {
                    select {
                        class: "w-full px-3 py-2 border border-gray-300 rounded-lg text-sm bg-white",
                        onchange: move |e| {
                            let id = e.value().parse::<Uuid>().ok();
                            let found = id.and_then(|id| {
                                options
                                    .value()
                                    .read()
                                    .as_ref()
                                    .and_then(|list| list.iter().find(|s| s.id == id).cloned())
                            });
                            draft.set(match found {
                                Some(s) => SecretDraft::Pick(SecretFieldView { secret_id: s.id, name: s.name }),
                                None => SecretDraft::Empty,
                            });
                        },
                        option { value: "", selected: picked.is_none(), {t!("secret-field-pick-placeholder")} }
                        for (id, name) in choices {
                            option { value: "{id}", selected: picked == Some(id), {name} }
                        }
                    }
                }
            }
        }
    }
}

/// [`SecretField`] bound to a secret slot of a flow graph node (lot S5):
/// reports every change of the slot through `on_change`.
#[component]
pub fn SecretSlotField(
    label: String,
    slot: SecretSlot,
    can_manage: bool,
    #[props(default)] read_only: bool,
    on_change: EventHandler<SecretSlot>,
) -> Element {
    let mut draft = use_signal(|| SecretDraft::from_slot(&slot));
    let mut last = use_signal(|| slot.clone());
    // Outside change of the slot (a save turned the typed value into a
    // reference): adopt it without reporting it back.
    use_effect(use_reactive!(|slot| {
        if slot.secret_id().is_some() && *last.peek() != slot {
            last.set(slot.clone());
            draft.set(SecretDraft::from_slot(&slot));
        }
    }));
    use_effect(move || {
        let next = draft().to_slot();
        if *last.peek() != next {
            last.set(next.clone());
            on_change.call(next);
        }
    });
    rsx! {
        SecretField { label, draft, can_manage, read_only }
    }
}
