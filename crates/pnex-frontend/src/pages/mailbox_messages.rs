//! Page `/mailboxes/:id`: the mails PneX polled from one mailbox, for those
//! who read it (D454; the server answers 404 to anyone else).
//!
//! A mail is untrusted data: shown as plain text only (never HTML, no link
//! followed, no image loaded, D458), with what PneX verified of its sender.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::mail::{AuthVerdict, MailAddress, MailInvitation, MailMessage};

use crate::api;
use crate::app::Route;
use crate::components::badges::date_label;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::ui::{State, StateMark, BTN_GHOST, PANEL};
use crate::state::media::OPEN_MEDIA;
use crate::state::{access, toasts};

fn address_label(a: &MailAddress) -> String {
    if a.name.is_empty() {
        a.address.clone()
    } else {
        format!("{} <{}>", a.name, a.address)
    }
}

/// What PneX verified of the sender (D458), as an ISA-101 mark.
fn sender_check(m: &MailMessage) -> (State, String) {
    match m.auth.dkim {
        AuthVerdict::Pass => (State::Ok, t!("mailboxes-sender-verified").to_string()),
        AuthVerdict::Fail => (State::Alarm, t!("mailboxes-sender-forged").to_string()),
        AuthVerdict::None => (State::Idle, t!("mailboxes-sender-unverified").to_string()),
    }
}

#[component]
pub fn MailboxMessages(id: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    let page = use_signal(|| 0i64);
    let mut open = use_signal(|| None::<String>);
    let navigator = use_navigator();

    let mailbox_id = id.clone();
    let mailbox = use_resource(move || {
        let id = mailbox_id.clone();
        async move {
            api::mailboxes::list()
                .await
                .ok()
                .and_then(|all| all.into_iter().find(|m| m.id == id))
        }
    });
    let list_id = id.clone();
    let list = use_resource(move || {
        let _ = reload();
        let (id, offset) = (list_id.clone(), page() * PAGE_SIZE);
        async move { api::mailboxes::messages(&id, PAGE_SIZE, offset).await }
    });

    let (state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.count == 0,
            paged.count,
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };
    let selected = open().and_then(|key| rows.iter().find(|m| m.id == key).cloned());
    let (title, subtitle) = match mailbox.cloned().flatten() {
        Some(m) => (m.name, Some(m.address)),
        None => (t!("mailboxes-title").to_string(), None),
    };

    let columns = vec![
        Column::new(t!("mailboxes-col-from").to_string(), |m: &MailMessage| {
            rsx! {
                div { class: "text-sm text-gray-900 break-all", {address_label(m.correspondent())} }
                if m.forwarded.as_ref().is_some_and(|f| f.trusted) {
                    div { class: "text-xs text-gray-500", {t!("mailboxes-forwarded-by", by : m.from.address.clone())} }
                }
            }
        }),
        Column::new(t!("mailboxes-col-subject").to_string(), |m: &MailMessage| {
            rsx! {
                div { class: "text-sm text-gray-900", {m.subject.clone()} }
                if m.automatic {
                    div { class: "text-xs text-gray-500", {t!("mailboxes-automatic")} }
                }
            }
        }),
        Column::new(t!("mailboxes-col-sender-check").to_string(), |m: &MailMessage| {
            let (mark, label) = sender_check(m);
            rsx! {
                StateMark { state: mark, label }
            }
        })
        .secondary(),
        Column::new(t!("mailboxes-col-received").to_string(), |m: &MailMessage| {
            rsx! {
                span { class: "text-sm text-gray-600 whitespace-nowrap", {date_label(&m.received_at)} }
            }
        })
        .secondary(),
    ];

    rsx! {
        ListLayout {
            title,
            subtitle,
            on_back: move |_| {
                navigator.push(Route::Mailboxes {});
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write: false,
            ListStates {
                state,
                is_empty,
                empty_message: t!("mailboxes-messages-empty").to_string(),
                empty_detail: rsx! {
                    p { class: "text-sm text-gray-500 mt-2", {t!("mailboxes-messages-empty-hint")} }
                },
                div { class: "space-y-4",
                    DataTable {
                        columns,
                        rows,
                        row_key: RowKey::new(|m: &MailMessage| m.id.clone()),
                        on_row_click: move |key: String| {
                            open.set(if open().as_deref() == Some(key.as_str()) { None } else { Some(key) })
                        },
                    }
                    ListPager { count, page }
                    if let Some(message) = selected {
                        MessageText {
                            message,
                            on_saved: move |_| reload.with_mut(|r| *r += 1),
                        }
                    }
                }
            }
        }
    }
}

/// One mail read as text: headers, what PneX checked, attachments listed.
#[component]
fn MessageText(message: MailMessage, on_saved: EventHandler<()>) -> Element {
    let (mark, check) = sender_check(&message);
    let navigator = use_navigator();
    let mut saving = use_signal(|| None::<usize>);
    let can_save = access::can("media:write");
    let mut save = move |mailbox: String, id: String, i: usize| {
        saving.set(Some(i));
        spawn(async move {
            match api::mailboxes::save_attachment(&mailbox, &id, i).await {
                Ok(_) => on_saved.call(()),
                Err(err) => toasts::error(err),
            }
            saving.set(None);
        });
    };
    let to: Vec<String> = message.to.iter().map(address_label).collect();
    rsx! {
        article { class: "{PANEL} p-4 space-y-3",
            header { class: "space-y-1",
                h2 { class: "text-lg font-semibold text-gray-900", {message.subject.clone()} }
                p { class: "text-sm text-gray-700 break-all", {address_label(&message.from)} }
                if !to.is_empty() {
                    p { class: "text-xs text-gray-500 break-all",
                        {t!("mailboxes-to", to : to.join(", "))}
                    }
                }
                StateMark { state: mark, label: check }
                if let Some(f) = message.forwarded.clone() {
                    p { class: "text-xs text-gray-500",
                        if f.trusted {
                            {t!("mailboxes-forward-trusted", from : address_label(& f.from))}
                        } else {
                            {t!("mailboxes-forward-untrusted", from : address_label(& f.from))}
                        }
                    }
                }
            }
            // Plain text only: the mail is never rendered as HTML (D458).
            pre { class: "whitespace-pre-wrap break-words font-sans text-sm text-gray-800 max-h-[60vh] overflow-auto",
                {message.text.clone()}
            }
            if message.truncated {
                p { class: "text-xs text-gray-500", {t!("mailboxes-truncated")} }
            }
            if let Some(inv) = message.invitation.clone() {
                Invitation {
                    mailbox_id: message.mailbox_id.clone(),
                    message_id: message.id.clone(),
                    invitation: inv,
                }
            }
            if !message.attachments.is_empty() {
                ul { class: "text-xs text-gray-600 space-y-0.5",
                    for (i, a) in message.attachments.clone().into_iter().enumerate() {
                        li {
                            key: "{i}",
                            class: "flex flex-wrap items-center gap-2",
                            span {
                                {t!("mailboxes-attachment", name : a.name.clone(), size : (a.size / 1024) as i64)}
                            }
                            if let Some(media) = a.media_id.clone() {
                                button {
                                    class: BTN_GHOST,
                                    onclick: move |_| {
                                        OPEN_MEDIA.with_mut(|v| *v = Some(media.clone()));
                                        navigator.push(Route::Media {});
                                    },
                                    {t!("mailboxes-attachment-open")}
                                }
                            } else if can_save {
                                button {
                                    class: BTN_GHOST,
                                    disabled: saving().is_some(),
                                    onclick: {
                                        let (mailbox, id) = (message.mailbox_id.clone(), message.id.clone());
                                        move |_| save(mailbox.clone(), id.clone(), i)
                                    },
                                    if saving() == Some(i) {
                                        {t!("mailboxes-attachment-saving")}
                                    } else {
                                        {t!("mailboxes-attachment-save")}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The meeting a mail invites to (D466), added to the calendar on demand.
#[component]
fn Invitation(mailbox_id: String, message_id: String, invitation: MailInvitation) -> Element {
    let mut adding = use_signal(|| false);
    let navigator = use_navigator();
    let when = invitation
        .start
        .as_deref()
        .map(date_label)
        .unwrap_or_default();
    let people: Vec<String> = invitation
        .organizer
        .iter()
        .chain(invitation.attendees.iter())
        .map(address_label)
        .collect();
    let add = move |_| {
        let (mailbox, id) = (mailbox_id.clone(), message_id.clone());
        adding.set(true);
        spawn(async move {
            match api::mailboxes::add_invitation(&mailbox, &id).await {
                Ok(_) => {
                    toasts::success(t!("mailboxes-invitation-added"));
                    navigator.push(Route::Calendar {});
                }
                Err(err) => toasts::error(err),
            }
            adding.set(false);
        });
    };
    rsx! {
        section { class: "rounded-md border border-line p-3 space-y-1",
            p { class: "text-sm font-medium text-gray-900",
                if invitation.cancelled() {
                    {t!("mailboxes-invitation-cancelled", title : invitation.summary.clone())}
                } else {
                    {t!("mailboxes-invitation", title : invitation.summary.clone())}
                }
            }
            if !when.is_empty() {
                p { class: "text-xs text-gray-600", {when} }
            }
            if let Some(place) = invitation.location.clone() {
                p { class: "text-xs text-gray-600 break-words", {place} }
            }
            if !people.is_empty() {
                p { class: "text-xs text-gray-500 break-all", {people.join(", ")} }
            }
            if access::can("time-ranges:write") {
                button { class: BTN_GHOST, disabled: adding(), onclick: add,
                    {t!("mailboxes-invitation-add")}
                }
            }
        }
    }
}
