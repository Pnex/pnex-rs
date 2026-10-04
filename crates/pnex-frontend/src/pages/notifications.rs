//! `/notifications` page (D49–D54) — three tabs:
//! - **Channels**: channel table (kind icon, enabled toggle, last status
//!   from the O2 journal), send test, "Journal" jumps to the Events tab
//!   filtered on the channel, dynamic form (`components/notify_channel_form.rs`);
//! - **Templates**: minijinja templates (`components/notify_template_form.rs`);
//! - **Events**: the delivery journal read from OpenObserve (D86,
//!   `components/notify_deliveries.rs`).
//!
//! École `pages/flows.rs` (squelette) + `resource_picker.rs` (onglets pill).
//! Écriture réservée owner/admin (le serveur force, l'UI masque).

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::{NotifyChannel, NotifyTemplate};

use crate::api;
use crate::components::badges::date_label;
use crate::components::crud::layout::{ListLayout, DANGER_BTN};
use crate::components::crud::pager::ListPager;
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};
use crate::components::icons;
use crate::state::{org, session, toasts};

fn current_role() -> Option<String> {
    let user = session::user()?;
    let org_id = org::current()?;
    user.orgs
        .iter()
        .find(|m| m.id == org_id)
        .map(|m| m.role.clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Channels,
    Templates,
    Events,
}

/// Icône d'un kind — registre LOCAL au front (les specs de champs viennent
/// du backend, les icônes restent locales, doctrine §3).
fn kind_icon(kind: &str) -> dioxus::prelude::Element {
    rsx! {
        match kind {
            "websocket" => rsx! {
                icons::Bell { class: "h-5 w-5" }
            },
            "webhook" => rsx! {
                icons::Zap { class: "h-5 w-5" }
            },
            "ntfy" => rsx! {
                icons::BellRing { class: "h-5 w-5" }
            },
            "telegram" => rsx! {
                icons::Send { class: "h-5 w-5" }
            },
            "slack" => rsx! {
                icons::Hash { class: "h-5 w-5" }
            },
            "discord" => rsx! {
                icons::MessageCircle { class: "h-5 w-5" }
            },
            "smtp" => rsx! {
                icons::Mail { class: "h-5 w-5" }
            },
            _ => rsx! {
                icons::Package { class: "h-5 w-5" }
            },
        }
    }
}

#[component]
pub fn Notifications() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut tab = use_signal(|| Tab::Channels);
    let channel_page = use_signal(|| 0i64);
    let template_page = use_signal(|| 0i64);
    // Formulaire canal : (kind_info, canal existant ou None).
    let mut edit_channel =
        use_signal(|| None::<(pnex_core::NotifyKindInfo, Option<NotifyChannel>)>);
    let mut edit_template = use_signal(|| None::<Option<NotifyTemplate>>);
    // Picker de kind à la création (doctrine §8 : tous les systèmes visibles).
    let mut pick_kind = use_signal(|| false);
    let mut delete_target = use_signal(|| None::<(&'static str, String, String)>);
    // Channel filter of the Events tab (the channel "Journal" button
    // switches to it pre-filtered).
    let journal_channel = use_signal(String::new);

    let can_write = current_role().is_some_and(|role| crate::state::org::role_can_write(&role));

    // Valeurs des signaux-modals lues AVANT le rsx (pas de `let` dans rsx).
    let channel_modal = edit_channel.read().clone();
    let channel_modal_key = channel_modal
        .as_ref()
        .and_then(|(_, e)| e.as_ref())
        .map(|c| c.id.to_string())
        .unwrap_or_default();
    let template_modal = edit_template.read().clone();
    let template_modal_key = template_modal
        .as_ref()
        .and_then(|t| t.as_ref())
        .map(|t| t.id.to_string())
        .unwrap_or_default();
    let delete_modal = delete_target.read().clone();

    let channels = use_resource(move || async move {
        let _ = reload();
        api::notify::list_channels().await
    });
    let templates = use_resource(move || async move {
        let _ = reload();
        api::notify::list_templates().await
    });
    let kinds = use_resource(move || async move {
        let _ = reload();
        api::notify::kinds().await
    });

    rsx! {
        ListLayout {
            title: t!("nav-notifications").to_string(),
            subtitle: Some(t!("notify-subtitle").to_string()),
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            can_write,
            // Header "add" action, like the whole CRUD socle — the label
            // follows the active tab (channels vs templates).
            add_label: match tab() {
                Tab::Channels => Some(t!("notify-new-channel").to_string()),
                Tab::Templates => Some(t!("notify-new-template").to_string()),
                Tab::Events => None,
            },
            on_add: move |_| match tab() {
                Tab::Channels => pick_kind.set(true),
                Tab::Templates => edit_template.set(Some(None)),
                Tab::Events => {}
            },
            if org::current().is_none() {
                p { class: "text-gray-500 text-center py-12", {t!("orgs-empty")} }
            } else {
                // Pill tabs (resource_picker school) — refresh lives in the
                // tab bar (socle filter-bar role), the "add" action is in
                // the header.
                div { class: "mb-6 flex flex-wrap gap-1 border-b border-gray-200 pb-2",
                    for tab_def in [
                        (Tab::Channels, "notify-tab-connect"),
                        (Tab::Templates, "notify-tab-templates"),
                        (Tab::Events, "notify-tab-events"),
                    ]
                    {
                        button {
                            class: if tab() == tab_def.0 { "px-4 py-2 rounded-lg text-sm font-medium bg-blue-600 text-white" } else { "px-4 py-2 rounded-lg text-sm font-medium text-gray-600 hover:bg-gray-100" },
                            onclick: move |_| tab.set(tab_def.0),
                            {t!(tab_def.1)}
                        }
                    }
                }

                match tab() {
                    Tab::Channels => rsx! {
                        ChannelsTab {
                            channels,
                            kinds,
                            channel_page,
                            can_write,
                            reload,
                            edit_channel,
                            delete_target,
                            journal_channel,
                            tab,
                        }
                    },
                    Tab::Templates => rsx! {
                        TemplatesTab {
                            templates,
                            template_page,
                            can_write,
                            reload,
                            edit_template,
                            delete_target,
                        }
                    },
                    Tab::Events => rsx! {
                        crate::components::notify_deliveries::NotifyDeliveriesTab {
                            channels: channels
                                .value()
                                .read()
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .map(|p| p.results.clone())
                                .unwrap_or_default(),
                            templates: templates
                                .value()
                                .read()
                                .as_ref()
                                .and_then(|r| r.as_ref().ok())
                                .map(|p| p.results.clone())
                                .unwrap_or_default(),
                            channel_filter: journal_channel,
                            reload,
                        }
                    },
                }
            }

            // ── Modals ──────────────────────────────────────────────
            if pick_kind() {
                KindPickerModal {
                    kinds,
                    on_pick: move |info: pnex_core::NotifyKindInfo| {
                        pick_kind.set(false);
                        edit_channel.set(Some((info, None)));
                    },
                    on_close: move |_| pick_kind.set(false),
                }
            }
            if let Some((kind_info, existing)) = channel_modal {
                crate::components::notify_channel_form::NotifyChannelForm {
                    key: "{channel_modal_key}",
                    kind_info,
                    existing,
                    on_close: move |_| edit_channel.set(None),
                    on_saved: move |_| {
                        edit_channel.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some(existing) = template_modal {
                crate::components::notify_template_form::NotifyTemplateForm {
                    key: "{template_modal_key}",
                    existing,
                    on_close: move |_| edit_template.set(None),
                    on_changed: move |_| reload.with_mut(|r| *r += 1),
                    on_saved: move |_| {
                        edit_template.set(None);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if let Some((what, id, name)) = delete_modal {
                crate::components::confirm::ConfirmDialog {
                    title: t!("notify-confirm-delete-title"),
                    message: t!(
                        "common-quoted-message", name : name.clone(), message :
                        t!("notify-confirm-delete-message")
                    ),
                    confirm_label: t!("notify-delete"),
                    on_confirm: move |_| {
                        let (what, id, _) = (what, id.clone(), name.clone());
                        delete_target.set(None);
                        spawn(async move {
                            let result = match what {
                                "channel" => api::notify::delete_channel(&id).await,
                                _ => api::notify::delete_template(&id).await,
                            };
                            match result {
                                Ok(()) => {
                                    toasts::success("toast-notify-deleted");
                                    reload.with_mut(|r| *r += 1);
                                }
                                Err(err) => toasts::error(err),
                            }
                        });
                    },
                    on_cancel: move |_| delete_target.set(None),
                }
            }
        }
    }
}

#[component]
fn ChannelsTab(
    channels: Resource<ChannelListResult>,
    kinds: Resource<KindListResult>,
    mut channel_page: Signal<i64>,
    can_write: bool,
    mut reload: Signal<u32>,
    mut edit_channel: Signal<Option<(pnex_core::NotifyKindInfo, Option<NotifyChannel>)>>,
    mut delete_target: Signal<Option<(&'static str, String, String)>>,
    mut journal_channel: Signal<String>,
    mut tab: Signal<Tab>,
) -> Element {
    // Lecture synchrone de la ressource (doctrine socle CRUD).
    let (list_state, is_empty, count, rows) = match &*channels.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.results.is_empty() && paged.count == 0,
            paged.count,
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    // Test d'envoi en vol (id du canal) — l'état vit au niveau de la table
    // (l'ancienne carte portait son signal local). La logique reste INLINE
    // dans le handler : une closure cellule doit rester Fn (pas de `.set`
    // hors handler).
    let mut testing_id = use_signal(|| None::<String>);

    let toggle_enabled = move |channel: NotifyChannel| {
        spawn(async move {
            let result = api::notify::update_channel(
                &channel.id.to_string(),
                &channel.kind,
                &channel.name,
                !channel.enabled,
                channel.config.clone(),
            )
            .await;
            match result {
                Ok(_) => reload.with_mut(|r| *r += 1),
                Err(err) => toasts::error(err),
            }
        });
    };

    let do_edit = move |channel: NotifyChannel| {
        // Reprendre le kind_info du registre backend (formulaires dynamiques).
        spawn(async move {
            match api::notify::kinds().await {
                Ok(list) => {
                    if let Some(info) = list.iter().find(|k| k.kind == channel.kind) {
                        let ch = api::notify::get_channel(&channel.id.to_string()).await.ok();
                        edit_channel.set(Some((info.clone(), ch)));
                    }
                }
                Err(err) => toasts::error(err),
            }
        });
    };

    // Colonnes de la table — canal, type (icône + label), statut (toggle
    // enabled + dernier statut de livraison), actions (test, journal,
    // édition, suppression).
    let columns = vec![
        Column::new(t!("notify-col-name").to_string(), |ch: &NotifyChannel| {
            rsx! { {ch.name.clone()} }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(t!("notify-col-kind").to_string(), |ch: &NotifyChannel| {
            rsx! {
                div { class: "flex items-center gap-2",
                    {kind_icon(&ch.kind)}
                    {t!(kind_label(&ch.kind))}
                }
            }
        })
        .with_td_class("text-gray-600"),
        Column::new(
            t!("notify-col-status").to_string(),
            move |ch: &NotifyChannel| {
                let ch_toggle = ch.clone();
                rsx! {
                    div { class: "flex flex-col items-start gap-1",
                        // Toggle enabled (le serveur force) — badge cliquable
                        // pour un writer.
                        if can_write {
                            button {
                                class: if ch.enabled {
                                    "px-2 py-1 rounded-full text-xs bg-green-100 text-green-800"
                                } else {
                                    "px-2 py-1 rounded-full text-xs bg-gray-100 text-gray-600"
                                },
                                onclick: move |_| toggle_enabled(ch_toggle.clone()),
                                {if ch.enabled { t!("notify-enabled").to_string() } else { t!("notify-disabled").to_string() }}
                            }
                        } else {
                            span {
                                class: if ch.enabled {
                                    "px-2 py-1 rounded-full text-xs bg-green-100 text-green-800"
                                } else {
                                    "px-2 py-1 rounded-full text-xs bg-gray-100 text-gray-600"
                                },
                                {if ch.enabled { t!("notify-enabled").to_string() } else { t!("notify-disabled").to_string() }}
                            }
                        }
                        if let Some(status) = &ch.last_status {
                            span { class: "inline-flex items-center px-2 py-0.5 rounded-full text-xs {crate::components::notify_deliveries::status_classes(status)}",
                                {last_status_label(status)} " · " {ch.last_delivery_at.clone().map(|d| date_label(&d)).unwrap_or_default()}
                            }
                        }
                    }
                }
            },
        ),
        Column::new(
            t!("common-actions").to_string(),
            move |ch: &NotifyChannel| {
                let id_test = ch.id.to_string();
                let ch_edit = ch.clone();
                let id_journal = ch.id.to_string();
                let id_delete = ch.id.to_string();
                let name_delete = ch.name.clone();
                let is_testing = testing_id() == Some(ch.id.to_string());
                rsx! {
                    div { class: "flex flex-wrap gap-2",
                        button {
                            class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50 disabled:opacity-50",
                            disabled: is_testing,
                            onclick: move |_| {
                                if testing_id() == Some(id_test.clone()) {
                                    return;
                                }
                                testing_id.set(Some(id_test.clone()));
                                let id = id_test.clone();
                                spawn(async move {
                                    match api::notify::test_channel(&id, None, Default::default()).await {
                                        Ok(outcome) if outcome.ok() => {
                                            toasts::success("toast-notify-test-sent")
                                        }
                                        Ok(outcome) => toasts::error(
                                            outcome
                                                .error
                                                .unwrap_or_else(|| t!("notify-test-failed").to_string()),
                                        ),
                                        Err(err) => toasts::error(err),
                                    }
                                    testing_id.set(None);
                                    reload.with_mut(|r| *r += 1);
                                });
                            },
                            {if is_testing { t!("notify-testing").to_string() } else { t!("notify-test").to_string() }}
                        }
                        button {
                            class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                            onclick: move |_| {
                                journal_channel.set(id_journal.clone());
                                tab.set(Tab::Events);
                            },
                            {t!("notify-journal")}
                        }
                        if can_write {
                            button {
                                class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                                onclick: move |_| do_edit(ch_edit.clone()),
                                {t!("notify-edit")}
                            }
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| delete_target.set(Some(("channel", id_delete.clone(), name_delete.clone()))),
                                icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("notify-delete")}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        ListStates {
            state: list_state,
            is_empty,
            empty_message: t!("notify-empty-channels").to_string(),
            div { class: "space-y-4",
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|ch: &NotifyChannel| ch.id.to_string()),
                }
                ListPager { count, page: channel_page }
            }
        }
    }
}

/// Résultats typés pour les props de tabs (alias lisibles).
type ChannelListResult = Result<pnex_core::Paginated<NotifyChannel>, crate::api::error::ApiError>;
type KindListResult = Result<Vec<pnex_core::NotifyKindInfo>, crate::api::error::ApiError>;

/// Channel card badge: last journaled attempt.
fn last_status_label(status: &str) -> String {
    match status {
        "sent" => t!("notify-last-sent").to_string(),
        "blocked" => t!("notify-last-blocked").to_string(),
        _ => t!("notify-last-failed").to_string(),
    }
}

/// Label local d'un kind (i18n `notify-kind-*`).
fn kind_label(kind: &str) -> &'static str {
    match kind {
        "websocket" => "notify-kind-websocket",
        "webhook" => "notify-kind-webhook",
        "ntfy" => "notify-kind-ntfy",
        "telegram" => "notify-kind-telegram",
        "slack" => "notify-kind-slack",
        "discord" => "notify-kind-discord",
        "smtp" => "notify-kind-smtp",
        _ => "notify-kind-unknown",
    }
}

/// Description 1 ligne d'un kind (picker, i18n `notify-kind-*-desc`) —
/// vide pour un kind inconnu.
fn kind_desc(kind: &str) -> &'static str {
    match kind {
        "websocket" => "notify-kind-websocket-desc",
        "webhook" => "notify-kind-webhook-desc",
        "ntfy" => "notify-kind-ntfy-desc",
        "telegram" => "notify-kind-telegram-desc",
        "slack" => "notify-kind-slack-desc",
        "discord" => "notify-kind-discord-desc",
        "smtp" => "notify-kind-smtp-desc",
        _ => "",
    }
}

/// Picker de kind à la création — grille des systèmes de notification
/// livrés (ordre du registre backend). L'utilisateur voit tous les
/// systèmes et leur rôle avant d'arriver sur le formulaire (doctrine §8).
#[component]
fn KindPickerModal(
    kinds: Resource<KindListResult>,
    on_pick: EventHandler<pnex_core::NotifyKindInfo>,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50 p-4",
            div {
                class: "bg-white rounded-lg shadow-xl w-full max-w-lg max-h-[80vh] flex flex-col",
                role: "dialog",
                aria_modal: "true",
                aria_label: t!("notify-pick-title"),
                div { class: "flex items-center justify-between border-b border-gray-200 px-4 py-3",
                    h2 { class: "text-lg font-semibold text-gray-900", {t!("notify-pick-title")} }
                    button {
                        class: "text-gray-400 hover:text-gray-600",
                        title: t!("common-close"),
                        aria_label: t!("common-close"),
                        onclick: move |_| on_close.call(()),
                        icons::X { class: "h-5 w-5" }
                    }
                }
                match &*kinds.value().read() {
                    Some(Ok(list)) => rsx! {
                        div { class: "grid gap-2 p-4 overflow-y-auto sm:grid-cols-2",
                            for info in list.clone() {
                                button {
                                    key: "{info.kind}",
                                    class: "flex items-start gap-3 rounded-lg border border-gray-200 p-3 text-left hover:border-blue-400 hover:bg-blue-50",
                                    onclick: move |_| on_pick.call(info.clone()),
                                    span { class: "flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-gray-100 text-gray-700",
                                        {kind_icon(&info.kind)}
                                    }
                                    span { class: "min-w-0",
                                        span { class: "block font-medium text-gray-900", {t!(kind_label(& info.kind))} }
                                        {
                                            match kind_desc(&info.kind) {
                                                "" => rsx! {},
                                                desc => rsx! {
                                                    span { class: "block text-xs text-gray-500", {t!(desc)} }
                                                },
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    Some(Err(err)) => rsx! {
                        div { class: "p-4 text-sm text-red-700", {err.message.clone()} }
                    },
                    None => rsx! {
                        div { class: "flex justify-center p-8",
                            span { class: "animate-spin rounded-full h-8 w-8 border-b-2 border-blue-600" }
                        }
                    },
                }
                div { class: "border-t border-gray-200 px-4 py-3 text-right",
                    button {
                        class: "px-4 py-2 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                        onclick: move |_| on_close.call(()),
                        {t!("notify-pick-cancel")}
                    }
                }
            }
        }
    }
}

#[component]
fn TemplatesTab(
    templates: Resource<TemplateListResult>,
    mut template_page: Signal<i64>,
    can_write: bool,
    mut reload: Signal<u32>,
    mut edit_template: Signal<Option<Option<NotifyTemplate>>>,
    mut delete_target: Signal<Option<(&'static str, String, String)>>,
) -> Element {
    // Lecture synchrone de la ressource (doctrine socle CRUD).
    let (list_state, is_empty, count, rows) = match &*templates.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.results.is_empty() && paged.count == 0,
            paged.count,
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    // Colonnes de la table — les closures d'action capturent les signaux
    // (Copy) de la page.
    let columns = vec![
        Column::new(t!("notify-col-name").to_string(), |tpl: &NotifyTemplate| {
            rsx! { {tpl.name.clone()} }
        })
        .with_td_class("font-medium text-gray-900"),
        Column::new(
            t!("notify-col-subject").to_string(),
            |tpl: &NotifyTemplate| {
                rsx! { {tpl.subject.clone().unwrap_or_else(|| t!("notify-no-subject").to_string())} }
            },
        )
        .with_td_class("text-gray-600").secondary(),
        Column::new(t!("notify-col-vars").to_string(), |tpl: &NotifyTemplate| {
            rsx! { "{tpl.vars.len()}" }
        })
        .with_td_class("text-gray-600").secondary(),
        Column::new(
            t!("common-actions").to_string(),
            move |tpl: &NotifyTemplate| {
                // Cloné avant les closures onclick (référence &T ne sort pas
                // du corps de cellule).
                let tpl_edit = tpl.clone();
                let id_delete = tpl.id.to_string();
                let name_delete = tpl.name.clone();
                rsx! {
                    if can_write {
                        div { class: "flex gap-2",
                            button {
                                class: "px-3 py-1 text-sm border border-gray-300 rounded-lg hover:bg-gray-50",
                                onclick: move |_| edit_template.set(Some(Some(tpl_edit.clone()))),
                                {t!("notify-edit")}
                            }
                            button {
                                class: DANGER_BTN,
                                onclick: move |_| delete_target.set(Some(("template", id_delete.clone(), name_delete.clone()))),
                                icons::Trash2 { class: "h-3.5 w-3.5 inline mr-0.5" }
                                {t!("notify-delete")}
                            }
                        }
                    }
                }
            },
        ).actions(),
    ];

    rsx! {
        ListStates {
            state: list_state,
            is_empty,
            empty_message: t!("notify-empty-templates").to_string(),
            div { class: "space-y-4",
                DataTable {
                    columns,
                    rows,
                    row_key: RowKey::new(|tpl: &NotifyTemplate| tpl.id.to_string()),
                }
                ListPager { count, page: template_page }
            }
        }
    }
}

type TemplateListResult = Result<pnex_core::Paginated<NotifyTemplate>, crate::api::error::ApiError>;
