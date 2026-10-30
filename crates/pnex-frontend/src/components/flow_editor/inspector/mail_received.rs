use super::helpers::*;
use super::*;

use crate::components::ui::FIELD_LABEL;
use pnex_core::mail::{MailReceivedConfig, MailSendConfig};

// ─────────────── mail-received (mail.md D456) ───────────────

/// Mail trigger inspector: the mailboxes the user reads, then simple
/// filters. Mailboxes they do not read are not offered (the deploy refuses
/// them anyway).
#[component]
pub(super) fn MailReceivedForm(
    mut cx: EditorCx,
    initial: MailReceivedConfig,
    can_write: bool,
) -> Element {
    let boxes = use_resource(|| async { api::mailboxes::list().await.unwrap_or_default() });
    let mut config = use_signal(move || initial.clone());
    let mut apply = move |next: MailReceivedConfig| {
        config.set(next.clone());
        patch_selected(&mut cx, move |node: &mut FlowNode| {
            if let FlowNodeKind::MailReceived { config } = &mut node.kind {
                *config = next;
            }
        });
    };
    let readable: Vec<(String, String)> = boxes
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.readable)
        .map(|b| (b.id, format!("{} · {}", b.name, b.address)))
        .collect();
    let empty = boxes.read().is_some() && readable.is_empty();

    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-mail-received-help")} }
            label { class: FIELD_LABEL, {t!("flows-mail-received-mailboxes")} }
            for (id, label) in readable {
                MailboxPick {
                    key: "{id}",
                    id: id.clone(),
                    label,
                    checked: config.read().mailboxes.contains(&id),
                    can_write,
                    on_toggle: move |(id, on): (String, bool)| {
                        let mut next = config();
                        next.mailboxes.retain(|m| m != &id);
                        if on {
                            next.mailboxes.push(id);
                        }
                        apply(next);
                    },
                }
            }
            if empty {
                p { class: "text-xs text-amber-700",
                    {t!("flows-mail-received-none")}
                    " "
                    Link {
                        to: crate::app::Route::Mailboxes {},
                        class: "underline",
                        {t!("flows-mail-received-open-mailboxes")}
                    }
                }
            }
            MailFilter {
                label: t!("flows-mail-received-from"),
                value: config.read().from_contains.clone(),
                can_write,
                on_change: move |v: String| {
                    let mut next = config();
                    next.from_contains = v;
                    apply(next);
                },
            }
            MailFilter {
                label: t!("flows-mail-received-subject"),
                value: config.read().subject_contains.clone(),
                can_write,
                on_change: move |v: String| {
                    let mut next = config();
                    next.subject_contains = v;
                    apply(next);
                },
            }
            label { class: "flex items-center gap-2 text-sm",
                input {
                    r#type: "checkbox",
                    checked: config.read().skip_automatic,
                    disabled: !can_write,
                    onchange: move |e: Event<FormData>| {
                        let mut next = config();
                        next.skip_automatic = e.checked();
                        apply(next);
                    },
                }
                {t!("flows-mail-received-skip-automatic")}
            }
        }
    }
}

#[component]
fn MailboxPick(
    id: String,
    label: String,
    checked: bool,
    can_write: bool,
    on_toggle: EventHandler<(String, bool)>,
) -> Element {
    rsx! {
        label { class: "flex items-center gap-2 text-sm",
            input {
                r#type: "checkbox",
                checked,
                disabled: !can_write,
                onchange: move |e: Event<FormData>| on_toggle.call((id.clone(), e.checked())),
            }
            "{label}"
        }
    }
}

#[component]
fn MailFilter(
    label: String,
    value: String,
    can_write: bool,
    on_change: EventHandler<String>,
) -> Element {
    rsx! {
        div {
            label { class: FIELD_LABEL, "{label}" }
            input {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                value: "{value}",
                disabled: !can_write,
                onchange: move |e: Event<FormData>| on_change.call(e.value()),
            }
        }
    }
}

// ─────────────── mail-send (mail.md D456, D518) ───────────────

/// Mail send inspector: one mailbox the user reads and that has an SMTP
/// server; what the mail becomes (draft or sent) is the mailbox's setting.
#[component]
pub(super) fn MailSendForm(mut cx: EditorCx, initial: MailSendConfig, can_write: bool) -> Element {
    let boxes = use_resource(|| async { api::mailboxes::list().await.unwrap_or_default() });
    let mut config = use_signal(move || initial.clone());
    let sending: Vec<(String, String)> = boxes
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.readable && !b.smtp_host.is_empty())
        .map(|b| {
            let mode = match b.send_mode {
                pnex_core::mail::SendMode::Draft => t!("mailboxes-send-mode-draft"),
                pnex_core::mail::SendMode::Send => t!("mailboxes-send-mode-send"),
            };
            (b.id, format!("{} · {} · {mode}", b.name, b.address))
        })
        .collect();
    let empty = boxes.read().is_some() && sending.is_empty();
    rsx! {
        div { class: "space-y-3",
            p { class: "text-xs text-gray-500", {t!("flows-mail-send-help")} }
            label { class: FIELD_LABEL, {t!("flows-mail-send-mailbox")} }
            select {
                class: "w-full px-2 py-1 border border-gray-300 rounded-lg text-sm",
                disabled: !can_write,
                onchange: move |e: Event<FormData>| {
                    let next = MailSendConfig {
                        mailbox: e.value(),
                    };
                    config.set(next.clone());
                    patch_selected(
                        &mut cx,
                        move |node: &mut FlowNode| {
                            if let FlowNodeKind::MailSend { config } = &mut node.kind {
                                *config = next;
                            }
                        },
                    );
                },
                option { value: "", selected: config.read().mailbox.is_empty(), "—" }
                for (id, label) in sending {
                    option {
                        key: "{id}",
                        value: "{id}",
                        selected: config.read().mailbox == id,
                        "{label}"
                    }
                }
            }
            if empty {
                p { class: "text-xs text-amber-700",
                    {t!("flows-mail-send-none")}
                    " "
                    Link {
                        to: crate::app::Route::Mailboxes {},
                        class: "underline",
                        {t!("flows-mail-received-open-mailboxes")}
                    }
                }
            }
        }
    }
}
