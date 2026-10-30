//! The outbox of a mailbox (mail.md D519), on its page: the drafts flows
//! wrote, to read, correct and send — PneX sends nothing a person did not
//! see unless the mailbox is set to send on its own — and what was sent or
//! failed. Read with the mailbox's own rules (D454).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::mail::{OutboxMail, OutboxSendInput, OutboxState};

use crate::api;
use crate::components::badges::date_label;
use crate::components::ui::{
    State, StateMark, BTN_GHOST, BTN_PRIMARY_SM, FIELD_LABEL, INPUT, PANEL,
};
use crate::state::{access, toasts};

const OUTBOX_PAGE: i64 = 20;

fn state_mark(s: OutboxState) -> (State, String) {
    match s {
        OutboxState::Draft => (State::Warn, t!("mailboxes-outbox-draft").to_string()),
        OutboxState::Sending => (State::Idle, t!("mailboxes-outbox-sending").to_string()),
        OutboxState::Sent => (State::Ok, t!("mailboxes-outbox-sent").to_string()),
        OutboxState::Failed => (State::Alarm, t!("mailboxes-outbox-failed").to_string()),
        OutboxState::Discarded => (State::Idle, t!("mailboxes-outbox-discarded").to_string()),
    }
}

#[component]
pub fn Outbox(mailbox: String) -> Element {
    let reload = use_signal(|| 0u32);
    let open = use_signal(|| None::<String>);
    let id = mailbox.clone();
    let list = use_resource(move || {
        let _ = reload();
        let id = id.clone();
        async move { api::mailboxes::outbox(&id, OUTBOX_PAGE, 0).await }
    });
    let rows: Vec<OutboxMail> = match &*list.read() {
        Some(Ok(p)) => p.results.clone(),
        _ => Vec::new(),
    };
    if rows.is_empty() {
        return rsx! {};
    }
    let waiting = rows
        .iter()
        .filter(|m| m.state == OutboxState::Draft)
        .count();
    rsx! {
        section { class: "{PANEL} p-4 space-y-3",
            h2 { class: "text-base font-semibold text-gray-900",
                {t!("mailboxes-outbox-title")}
                if waiting > 0 {
                    span { class: "ml-2 text-sm font-normal text-amber-700",
                        {t!("mailboxes-outbox-waiting", count : waiting)}
                    }
                }
            }
            ul { class: "divide-y divide-gray-100",
                for m in rows {
                    OutboxRow {
                        key: "{m.id}",
                        mailbox: mailbox.clone(),
                        mail: m,
                        open,
                        reload,
                    }
                }
            }
        }
    }
}

/// One line of the outbox; a click opens or closes its mail.
#[component]
fn OutboxRow(
    mailbox: String,
    mail: OutboxMail,
    mut open: Signal<Option<String>>,
    mut reload: Signal<u32>,
) -> Element {
    let (mark, label) = state_mark(mail.state);
    let shown = open().as_deref() == Some(mail.id.as_str());
    let key = mail.id.clone();
    let toggle = move |_| open.set(if shown { None } else { Some(key.clone()) });
    let date = date_label(mail.sent_at.as_deref().unwrap_or(&mail.created_at));
    rsx! {
        li { class: "py-2",
            button {
                class: "w-full text-left flex flex-wrap items-center gap-3",
                onclick: toggle,
                StateMark { state: mark, label }
                span { class: "text-sm text-gray-900 break-all", {mail.to.join(", ")} }
                span { class: "text-sm text-gray-600 flex-1", {mail.subject.clone()} }
                span { class: "text-xs text-gray-500 whitespace-nowrap", {date} }
            }
            if shown {
                Draft {
                    mailbox: mailbox.clone(),
                    mail: mail.clone(),
                    on_done: move |_| {
                        open.set(None);
                        *reload.write() += 1;
                    },
                }
            }
        }
    }
}

/// One mail: read-only once sent; a draft or a failed mail is corrected,
/// then sent or discarded by the person.
#[component]
fn Draft(mailbox: String, mail: OutboxMail, on_done: EventHandler<()>) -> Element {
    let editable =
        matches!(mail.state, OutboxState::Draft | OutboxState::Failed) && access::can("mail:send");
    let mut subject = use_signal(|| mail.subject.clone());
    let mut text = use_signal(|| mail.text.clone());
    let mut busy = use_signal(|| false);
    let (box_id, mail_id) = (mailbox.clone(), mail.id.clone());
    let send = move |_| {
        let (b, m) = (box_id.clone(), mail_id.clone());
        let input = OutboxSendInput {
            subject: Some(subject()),
            text: Some(text()),
        };
        busy.set(true);
        spawn(async move {
            match api::mailboxes::outbox_send(&b, &m, &input).await {
                Ok(_) => {
                    toasts::success(t!("mailboxes-outbox-sent-toast"));
                    on_done.call(());
                }
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    let (box_id, mail_id) = (mailbox.clone(), mail.id.clone());
    let discard = move |_| {
        let (b, m) = (box_id.clone(), mail_id.clone());
        busy.set(true);
        spawn(async move {
            match api::mailboxes::outbox_discard(&b, &m).await {
                Ok(_) => on_done.call(()),
                Err(err) => toasts::error(err),
            }
            busy.set(false);
        });
    };
    rsx! {
        div { class: "mt-3 space-y-3",
            if let Some(e) = mail.error.clone() {
                p { class: "text-sm text-red-700",
                    {crate::api::error_i18n::localize_code(&e.code, Some(&e.args), &e.code)}
                }
            }
            div {
                label { r#for: "outbox-subject", class: FIELD_LABEL, {t!("mailboxes-col-subject")} }
                input {
                    id: "outbox-subject",
                    class: INPUT,
                    value: "{subject}",
                    disabled: !editable,
                    oninput: move |e| subject.set(e.value()),
                }
            }
            div {
                label { r#for: "outbox-text", class: FIELD_LABEL, {t!("mailboxes-outbox-text")} }
                textarea {
                    id: "outbox-text",
                    class: "{INPUT} min-h-40",
                    value: "{text}",
                    disabled: !editable,
                    oninput: move |e| text.set(e.value()),
                }
            }
            if editable {
                div { class: "flex gap-2",
                    button {
                        class: BTN_PRIMARY_SM,
                        disabled: busy(),
                        onclick: send,
                        {t!("mailboxes-outbox-send")}
                    }
                    button {
                        class: BTN_GHOST,
                        disabled: busy(),
                        onclick: discard,
                        {t!("mailboxes-outbox-discard")}
                    }
                }
                p { class: "text-xs text-gray-500", {t!("mailboxes-outbox-send-help")} }
            }
        }
    }
}
