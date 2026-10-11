//! `/cases/:id` — the case workspace (cases.md K1, K2, K6): the case
//! header edited in place, members added as you type, dated links between
//! members, and the merged timeline, on one screen without dialogs.

use std::collections::BTreeMap;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{
    BoardClaim, CaseBoard, CaseExportQuery, CaseTimeline, LinkClose, LinkInput, LinkTypeView,
    LinkView, ObjectInput, ObjectTypeView, ObjectView, OntologyQuery, TimelineItem, TimelineQuery,
};
use pnex_core::ontology::{
    CASE_FROZEN_AT, CASE_LEGAL_BASIS, CASE_QUESTION, CASE_STATUS, CASE_STATUSES, CASE_WINDOW_FROM,
    CASE_WINDOW_TO, IN_CASE_ROLE, IN_CASE_ROLES, REL_IN_CASE,
};
use serde_json::Value;

use super::pages_panel::PagesPanel;
use super::purge_panel::PurgePanel;
use super::snapshot::SnapshotPanel;
use super::{prop_str, status_class, status_label};
use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::ontology::add_to_case::role_label;
use crate::components::ontology::{
    fuzzy_label, link_label, precision_label, short_time, type_label_of,
};
use crate::components::ui::{BTN_PRIMARY, BTN_SECONDARY_SM, FIELD_LABEL as LABEL, INPUT};
use crate::state::{org, toasts};

const CARD: &str = "bg-white rounded-lg border border-gray-200 p-4 space-y-3";
const CHIP_ON: &str = "px-2.5 py-1 text-xs rounded-full bg-ink text-white";
const CHIP_OFF: &str = "px-2.5 py-1 text-xs rounded-full bg-white border border-gray-300 text-gray-700 hover:bg-gray-50";
const DEBOUNCE_MS: u64 = 200;
/// Lane colours, assigned by member order.
const LANE_COLORS: [&str; 8] = [
    "bg-blue-500",
    "bg-emerald-500",
    "bg-amber-500",
    "bg-rose-500",
    "bg-violet-500",
    "bg-cyan-500",
    "bg-lime-500",
    "bg-orange-500",
];

/// `YYYY-MM-DDTHH:MM` of an RFC 3339 instant (UTC), for datetime-local.
fn to_input(rfc3339: &str) -> String {
    rfc3339.get(..16).unwrap_or_default().to_string()
}

/// datetime-local value (UTC) → RFC 3339; empty stays empty.
fn from_input(v: &str) -> Option<String> {
    (!v.is_empty()).then(|| format!("{v}:00Z"))
}

fn now_minus_hours(h: i64) -> String {
    let now = crate::util::now_secs() as i64;
    chrono::DateTime::from_timestamp(now - h * 3600, 0)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

#[component]
pub fn CaseWorkspace(id: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    // Timeline range preset: `case` = the case window, else hours back.
    let range = use_signal(|| "case".to_string());
    let focus = use_signal(|| None::<String>);
    // Snapshot instant (K14), `None` = now.
    let cursor = use_signal(|| None::<String>);
    let navigator = use_navigator();
    let can_write = org::current_can_write();

    let oid = id.clone();
    let case = use_resource(move || {
        let id = oid.clone();
        async move {
            let _ = reload();
            api::ontology::object(&id).await
        }
    });
    let tid = id.clone();
    let timeline = use_resource(move || {
        let id = tid.clone();
        let q = match range().as_str() {
            "case" => TimelineQuery::default(),
            h => TimelineQuery {
                from: Some(now_minus_hours(h.parse().unwrap_or(24))),
                to: None,
            },
        };
        async move {
            let _ = reload();
            api::ontology::case_timeline(&id, &q).await
        }
    });
    let bid = id.clone();
    let board = use_resource(move || {
        let id = bid.clone();
        async move {
            let _ = reload();
            api::ontology::case_board(&id).await.ok()
        }
    });
    let types = use_resource(|| async { api::ontology::types().await.unwrap_or_default() });
    let link_types =
        use_resource(|| async { api::ontology::link_types().await.unwrap_or_default() });

    let c = match &*case.read() {
        Some(Ok(c)) => c.clone(),
        Some(Err(e)) => {
            return rsx! {
                p { class: "p-6 text-sm text-red-600", "{e}" }
            };
        }
        None => {
            return rsx! {
                p { class: "p-6 text-sm text-gray-500", {t!("common-loading")} }
            };
        }
    };
    let tl: Option<CaseTimeline> = timeline
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .cloned();
    let members = tl.as_ref().map(|t| t.members.clone()).unwrap_or_default();
    let types = types.read().clone().unwrap_or_default();
    let link_types = link_types.read().clone().unwrap_or_default();
    let archived = c.valid_to.is_some();
    let editable = can_write && !archived;
    let changed = move |_| reload.with_mut(|r| *r += 1);
    let persons = members
        .iter()
        .filter(|m| m.source.type_key == "person")
        .count();
    let board = board.read().clone().flatten().unwrap_or_default();
    let mut steps: Vec<String> = tl
        .as_ref()
        .map(|t| t.items.iter().map(|i| i.at.clone()).collect())
        .unwrap_or_default();
    steps.dedup();

    rsx! {
        ListLayout {
            title: t!("case-subtitle").to_string(),
            can_write,
            on_back: move |_| {
                navigator.push(Route::Cases {});
            },
            on_refresh: move |_| reload.with_mut(|r| *r += 1),
            actions: rsx! {
                Link {
                    class: "inline-flex items-center gap-1 px-3 py-1 text-sm text-gray-700 border border-gray-300 rounded-lg hover:bg-gray-50",
                    to: Route::OntologyObject {
                        id: c.id.clone(),
                    },
                    {t!("onto-open-object")}
                }
            },
            div { class: "space-y-4",
                CaseHeader {
                    case: c.clone(),
                    editable,
                    persons,
                    on_saved: changed,
                }
                if can_write && prop_str(&c, CASE_STATUS) == "archived" {
                    PurgePanel {
                        case_id: c.id.clone(),
                        types: types.clone(),
                        on_purged: changed,
                    }
                }
                div { class: "grid gap-4 lg:grid-cols-[minmax(300px,380px)_1fr]",
                    div { class: "space-y-4",
                        MembersPanel {
                            case_id: c.id.clone(),
                            members: members.clone(),
                            types: types.clone(),
                            editable,
                            focus,
                            on_changed: changed,
                        }
                        PagesPanel {
                            case: c.clone(),
                            members: members.clone(),
                            editable,
                            on_changed: changed,
                        }
                    }
                    div { class: "space-y-4 min-w-0",
                        if editable && members.len() >= 2 {
                            LinkComposer {
                                members: members.clone(),
                                link_types: link_types.clone(),
                                on_created: changed,
                            }
                        }
                        if !board.hypotheses.is_empty() || !board.unsourced.is_empty()
                            || !board.contradicted.is_empty()
                        {
                            BoardPanel { board: board.clone(), focus }
                        }
                        SnapshotPanel {
                            case_id: c.id.clone(),
                            steps,
                            cursor,
                            focus,
                            link_types: link_types.clone(),
                            reload,
                        }
                        TimelinePanel {
                            timeline: tl.clone(),
                            loading: timeline.read().is_none(),
                            link_types: link_types.clone(),
                            range,
                            focus,
                            cursor,
                        }
                    }
                }
            }
        }
    }
}

// ─────────────────────────── header ───────────────────────────

/// Title, status, question and window, each saved when it is left.
#[component]
fn CaseHeader(case: ObjectView, editable: bool, persons: usize, on_saved: Callback<()>) -> Element {
    let c = case.clone();
    let mut title = use_signal(|| c.title.clone());
    let mut question = use_signal(|| prop_str(&c, CASE_QUESTION));
    let mut basis = use_signal(|| prop_str(&c, CASE_LEGAL_BASIS));
    let mut busy = use_signal(|| false);
    // Saves `title` + properties with one property replaced, on the version read.
    let save = use_callback(move |(key, value): (Option<String>, Value)| {
        let mut props = case.properties.clone();
        if let Some(k) = key {
            if value.is_null() {
                props.remove(&k);
            } else {
                props.insert(k, value);
            }
        }
        let input = ObjectInput {
            type_key: case.type_key.clone(),
            title: title().trim().to_string(),
            properties: props,
            expected_version: Some(case.version),
        };
        let id = case.id.clone();
        busy.set(true);
        spawn(async move {
            let res = api::ontology::update_object(&id, &input).await;
            busy.set(false);
            match res {
                Ok(_) => on_saved.call(()),
                Err(e) => toasts::error(e),
            }
        });
    });
    let status = prop_str(&c, CASE_STATUS);
    let wf = to_input(&prop_str(&c, CASE_WINDOW_FROM));
    let wt = to_input(&prop_str(&c, CASE_WINDOW_TO));
    let window_input = move |key: &'static str| {
        move |e: Event<FormData>| {
            let v = from_input(&e.value())
                .map(Value::String)
                .unwrap_or(Value::Null);
            save.call((Some(key.to_string()), v));
        }
    };
    let orig_title = c.title.clone();
    let orig_question = prop_str(&c, CASE_QUESTION);
    let orig_basis = prop_str(&c, CASE_LEGAL_BASIS);
    let frozen = prop_str(&c, CASE_FROZEN_AT);
    let export_id = c.id.clone();
    let export = move |_| {
        let id = export_id.clone();
        spawn(async move {
            match api::ontology::case_export(&id, &CaseExportQuery::default()).await {
                Ok(x) => {
                    let bytes = serde_json::to_vec_pretty(&x).unwrap_or_default();
                    let day = x.as_of.get(..10).unwrap_or_default().to_string();
                    let name = format!("case-{}-{day}.json", id.get(..8).unwrap_or_default());
                    crate::util::trigger_download(&name, "application/json", &bytes);
                }
                Err(e) => toasts::error(e),
            }
        });
    };

    rsx! {
        div { class: CARD, "data-testid": "case-header",
            div { class: "flex flex-wrap items-center gap-3",
                input {
                    class: "flex-1 min-w-48 text-lg font-semibold text-gray-900 border-0 border-b border-transparent focus:border-blue-500 focus:ring-0 px-0",
                    "data-testid": "case-title",
                    disabled: !editable,
                    maxlength: 255,
                    value: "{title}",
                    oninput: move |e| title.set(e.value()),
                    onblur: move |_| {
                        if !title().trim().is_empty() && title().trim() != orig_title {
                            save.call((None, Value::Null));
                        }
                    },
                }
                div { class: "flex gap-1", role: "radiogroup",
                    for s in CASE_STATUSES {
                        button {
                            key: "{s}",
                            r#type: "button",
                            role: "radio",
                            "aria-checked": status == s,
                            "data-status": "{s}",
                            disabled: !editable || busy(),
                            class: if status == s { status_class(s) } else { "rounded px-2 py-0.5 text-xs text-gray-500 hover:bg-gray-100" },
                            onclick: move |_| save.call((Some(CASE_STATUS.to_string()), Value::String(s.to_string()))),
                            {status_label(s)}
                        }
                    }
                }
            }
            div {
                label { class: LABEL, {t!("case-question")} }
                textarea {
                    class: INPUT,
                    "data-testid": "case-question",
                    rows: 2,
                    maxlength: 1000,
                    disabled: !editable,
                    placeholder: t!("case-question-placeholder"),
                    value: "{question}",
                    oninput: move |e| question.set(e.value()),
                    onblur: move |_| {
                        let q = question().trim().to_string();
                        if q != orig_question {
                            let v = if q.is_empty() { Value::Null } else { Value::String(q) };
                            save.call((Some(CASE_QUESTION.to_string()), v));
                        }
                    },
                }
            }
            div { class: "grid gap-3 sm:grid-cols-2",
                div {
                    label { class: LABEL, {t!("case-window-from")} }
                    input {
                        r#type: "datetime-local",
                        class: INPUT,
                        "data-testid": "case-window-from",
                        disabled: !editable,
                        value: "{wf}",
                        onchange: window_input(CASE_WINDOW_FROM),
                    }
                }
                div {
                    label { class: LABEL, {t!("case-window-to")} }
                    input {
                        r#type: "datetime-local",
                        class: INPUT,
                        "data-testid": "case-window-to",
                        disabled: !editable,
                        value: "{wt}",
                        onchange: window_input(CASE_WINDOW_TO),
                    }
                }
            }
            div {
                label { class: LABEL, {t!("case-legal-basis")} }
                input {
                    class: INPUT,
                    "data-testid": "case-legal-basis",
                    maxlength: 255,
                    disabled: !editable,
                    placeholder: t!("case-legal-basis-placeholder"),
                    value: "{basis}",
                    oninput: move |e| basis.set(e.value()),
                    onblur: move |_| {
                        let b = basis().trim().to_string();
                        if b != orig_basis {
                            let v = if b.is_empty() { Value::Null } else { Value::String(b) };
                            save.call((Some(CASE_LEGAL_BASIS.to_string()), v));
                        }
                    },
                }
            }
            div { class: "flex flex-wrap items-center gap-2 border-t border-gray-100 pt-3",
                if frozen.is_empty() {
                    if editable {
                        button {
                            class: BTN_SECONDARY_SM,
                            r#type: "button",
                            "data-testid": "case-freeze",
                            disabled: busy(),
                            onclick: move |_| {
                                save.call((Some(CASE_FROZEN_AT.to_string()), Value::String(now_minus_hours(0))))
                            },
                            {t!("case-freeze")}
                        }
                    }
                } else {
                    span {
                        class: "text-sm text-gray-700",
                        "data-testid": "case-frozen",
                        {t!("case-frozen", at : short_time(& frozen))}
                    }
                    if editable {
                        button {
                            class: "text-xs text-gray-500 hover:underline",
                            r#type: "button",
                            "data-testid": "case-unfreeze",
                            disabled: busy(),
                            onclick: move |_| save.call((Some(CASE_FROZEN_AT.to_string()), Value::Null)),
                            {t!("case-unfreeze")}
                        }
                    }
                }
                button {
                    class: BTN_SECONDARY_SM,
                    r#type: "button",
                    "data-testid": "case-export",
                    onclick: export,
                    {t!("case-export")}
                }
                if persons > 0 {
                    span {
                        class: "text-xs text-amber-700",
                        "data-testid": "case-export-persons",
                        {t!("case-export-persons", count : persons)}
                    }
                }
                p { class: "basis-full text-xs text-gray-500", {t!("case-freeze-help")} }
            }
        }
    }
}

// ─────────────────────────── board ───────────────────────────

/// Hypotheses with the claims for and against, then the claims that need
/// a source or are contradicted (cases.md K8). No score: the human concludes.
#[component]
fn BoardPanel(board: CaseBoard, focus: Signal<Option<String>>) -> Element {
    rsx! {
        div { class: CARD, "data-testid": "case-board",
            h2 { class: "font-semibold text-gray-900", {t!("case-board")} }
            for h in board.hypotheses.iter() {
                div {
                    key: "{h.hypothesis.id}",
                    class: "rounded-md border border-gray-200 p-3 space-y-2",
                    div { class: "flex items-baseline gap-2",
                        ClaimTitle { r: h.hypothesis.clone(), focus }
                        if let Some(st) = &h.status {
                            span { class: "text-xs text-gray-500", "{st}" }
                        }
                    }
                    div { class: "grid gap-2 sm:grid-cols-2",
                        ClaimList {
                            label: t!("case-board-supports").to_string(),
                            claims: h.supports.clone(),
                            focus,
                        }
                        ClaimList {
                            label: t!("case-board-contradicts").to_string(),
                            claims: h.contradicts.clone(),
                            focus,
                        }
                    }
                }
            }
            if !board.unsourced.is_empty() {
                ClaimList {
                    label: t!("case-board-unsourced").to_string(),
                    claims: board.unsourced.clone(),
                    focus,
                }
            }
            if !board.contradicted.is_empty() {
                ClaimList {
                    label: t!("case-board-contradicted").to_string(),
                    claims: board.contradicted.clone(),
                    focus,
                }
            }
        }
    }
}

#[component]
fn ClaimList(label: String, claims: Vec<BoardClaim>, focus: Signal<Option<String>>) -> Element {
    rsx! {
        div {
            p { class: LABEL, "{label} ({claims.len()})" }
            if claims.is_empty() {
                p { class: "text-xs text-gray-500", {t!("case-board-none")} }
            }
            ul { class: "space-y-1",
                for c in claims.iter() {
                    li {
                        key: "{c.claim.id}",
                        class: "flex items-baseline gap-2 text-sm",
                        ClaimTitle { r: c.claim.clone(), focus }
                        if let Some(st) = &c.status {
                            span { class: "text-xs text-gray-500", "{st}" }
                        }
                        span { class: "text-xs text-gray-500",
                            {t!("case-board-sources", count : c.sources)}
                        }
                        if let Some(j) = &c.justification {
                            span {
                                class: "text-xs text-amber-700 underline decoration-dotted cursor-help",
                                title: "{j}",
                                "data-testid": "case-board-justified",
                                {t!("case-board-justified")}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A board entry: clicking it focuses the timeline on that object.
#[component]
fn ClaimTitle(r: pnex_core::ontology::api::ObjectRef, focus: Signal<Option<String>>) -> Element {
    let mut focus = focus;
    let id = r.id.clone();
    rsx! {
        button {
            r#type: "button",
            class: "text-left text-sm text-gray-900 hover:underline",
            onclick: move |_| focus.set(Some(id.clone())),
            "{r.title}"
        }
    }
}

// ─────────────────────────── members ───────────────────────────

#[component]
fn MembersPanel(
    case_id: String,
    members: Vec<LinkView>,
    types: Vec<ObjectTypeView>,
    editable: bool,
    focus: Signal<Option<String>>,
    on_changed: Callback<()>,
) -> Element {
    let mut focus = focus;
    // Members grouped by role, in the role order of the link type.
    let mut groups: BTreeMap<usize, Vec<LinkView>> = BTreeMap::new();
    for m in &members {
        let role = m
            .attributes
            .get(IN_CASE_ROLE)
            .and_then(Value::as_str)
            .unwrap_or_default();
        let rank = IN_CASE_ROLES
            .iter()
            .position(|r| *r == role)
            .unwrap_or(IN_CASE_ROLES.len());
        groups.entry(rank).or_default().push(m.clone());
    }
    let member_ids: Vec<String> = members.iter().map(|m| m.source.id.clone()).collect();

    rsx! {
        div { class: CARD, "data-testid": "case-members",
            div { class: "flex items-center justify-between",
                h2 { class: "font-semibold text-gray-900", {t!("case-members")} }
                span { class: "text-xs text-gray-500", "{members.len()}" }
            }
            if editable {
                QuickAdd {
                    case_id: case_id.clone(),
                    exclude: member_ids,
                    types: types.clone(),
                    on_added: on_changed,
                }
            }
            if members.is_empty() {
                p { class: "text-sm text-gray-500", {t!("case-members-empty")} }
            }
            for (rank, list) in groups {
                div { key: "{rank}",
                    h3 { class: "text-xs font-medium text-gray-500 mb-1",
                        {IN_CASE_ROLES.get(rank).map(|r| role_label(r)).unwrap_or_default()}
                    }
                    ul { class: "space-y-1",
                        for m in list {
                            MemberRow {
                                key: "{m.id}",
                                color: LANE_COLORS[members.iter().position(|x| x.id == m.id).unwrap_or(0)
                                    % LANE_COLORS.len()],
                                kind: type_label_of(&types, &m.source.type_key),
                                member: m.clone(),
                                editable,
                                focused: focus().as_deref() == Some(m.source.id.as_str()),
                                on_focus: move |id: String| {
                                    let next = if focus().as_deref() == Some(id.as_str()) { None } else { Some(id) };
                                    focus.set(next);
                                },
                                on_changed,
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MemberRow(
    member: LinkView,
    kind: String,
    color: &'static str,
    editable: bool,
    focused: bool,
    on_focus: Callback<String>,
    on_changed: Callback<()>,
) -> Element {
    let o = member.source.clone();
    let link_id = member.id;
    let oid = o.id.clone();
    rsx! {
        li {
            class: if focused { "flex items-center gap-2 rounded-lg px-2 py-1 bg-blue-50 ring-1 ring-blue-300" } else { "flex items-center gap-2 rounded-lg px-2 py-1 hover:bg-gray-50" },
            "data-member": "{o.id}",
            button {
                r#type: "button",
                class: "flex-1 min-w-0 text-left",
                title: t!("case-focus-help"),
                onclick: move |_| on_focus.call(oid.clone()),
                span { class: "flex items-center gap-2",
                    span { class: "h-2.5 w-2.5 shrink-0 rounded-full {color}" }
                    span { class: "truncate text-sm font-medium text-gray-900", "{o.title}" }
                }
                span { class: "block text-xs text-gray-500", "{kind}" }
            }
            Link {
                class: "text-xs text-blue-600 hover:underline",
                to: Route::OntologyObject {
                    id: o.id.clone(),
                },
                {t!("case-open")}
            }
            if editable {
                button {
                    r#type: "button",
                    class: "text-gray-500 hover:text-red-600 px-1",
                    title: t!("case-remove"),
                    "aria-label": t!("case-remove"),
                    "data-testid": "case-remove-member",
                    onclick: move |_| {
                        spawn(async move {
                            match api::ontology::close_link(link_id, &LinkClose::default()).await {
                                Ok(_) => on_changed.call(()),
                                Err(e) => toasts::error(e),
                            }
                        });
                    },
                    "×"
                }
            }
        }
    }
}

/// Role chips + typeahead over every live object of the org; Enter or a
/// click adds the highlighted hit with the chosen role, focus stays.
#[component]
fn QuickAdd(
    case_id: String,
    exclude: Vec<String>,
    types: Vec<ObjectTypeView>,
    on_added: Callback<()>,
) -> Element {
    let mut role = use_signal(|| IN_CASE_ROLES[0].to_string());
    let mut q = use_signal(String::new);
    let mut debounced = use_signal(String::new);
    let mut active = use_signal(|| 0usize);
    let cid = case_id.clone();
    let hits = use_resource(move || {
        let term = debounced();
        async move {
            if term.is_empty() {
                return Vec::new();
            }
            let query = OntologyQuery {
                text: Some(term),
                limit: Some(12),
                ..Default::default()
            };
            api::ontology::query(&query)
                .await
                .map(|r| r.rows.into_iter().map(|r| r.object).collect())
                .unwrap_or_default()
        }
    });
    let hits: Vec<ObjectView> = hits
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|o| o.id != cid && !exclude.contains(&o.id))
        .take(8)
        .collect();
    let add = use_callback(move |object_id: String| {
        let mut attrs = serde_json::Map::new();
        attrs.insert(IN_CASE_ROLE.into(), Value::String(role()));
        let input = LinkInput {
            link_type: REL_IN_CASE.to_string(),
            source_id: object_id,
            target_id: case_id.clone(),
            attributes: attrs,
            valid_from: None,
        };
        spawn(async move {
            match api::ontology::create_link(&input).await {
                Ok(_) => {
                    q.set(String::new());
                    debounced.set(String::new());
                    active.set(0);
                    on_added.call(());
                }
                Err(e) => toasts::error(e),
            }
        });
    });
    let keys: Vec<String> = hits.iter().map(|h| h.id.clone()).collect();
    // (index, id for the key, id for the click, title, type label)
    let rows: Vec<(usize, String, String, String, String)> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| {
            (
                i,
                h.id.clone(),
                h.id.clone(),
                h.title.clone(),
                type_label_of(&types, &h.type_key),
            )
        })
        .collect();

    rsx! {
        div { class: "space-y-2",
            div {
                class: "flex flex-wrap gap-1",
                role: "radiogroup",
                "aria-label": t!("case-role"),
                for r in IN_CASE_ROLES {
                    button {
                        key: "{r}",
                        r#type: "button",
                        role: "radio",
                        "aria-checked": role() == r,
                        "data-role": "{r}",
                        class: if role() == r { CHIP_ON } else { CHIP_OFF },
                        onclick: move |_| role.set(r.to_string()),
                        {role_label(r)}
                    }
                }
            }
            div { class: "relative",
                input {
                    class: INPUT,
                    "data-testid": "case-quick-add",
                    placeholder: t!("case-quick-add-placeholder"),
                    autocomplete: "off",
                    value: "{q}",
                    oninput: move |e| {
                        let value = e.value();
                        q.set(value.clone());
                        active.set(0);
                        spawn(async move {
                            crate::util::sleep(std::time::Duration::from_millis(DEBOUNCE_MS)).await;
                            if q.cloned() == value {
                                debounced.set(value.trim().to_string());
                            }
                        });
                    },
                    onkeydown: move |e| match e.key() {
                        Key::ArrowDown => {
                            let next = (active() + 1).min(keys.len().saturating_sub(1));
                            active.set(next);
                        }
                        Key::ArrowUp => {
                            let next = active().saturating_sub(1);
                            active.set(next);
                        }
                        Key::Enter => {
                            e.prevent_default();
                            if let Some(id) = keys.get(active()) {
                                add.call(id.clone());
                            }
                        }
                        Key::Escape => {
                            q.set(String::new());
                            debounced.set(String::new());
                        }
                        _ => {}
                    },
                }
                if !q().trim().is_empty() && !rows.is_empty() {
                    ul {
                        class: "absolute z-20 mt-1 w-full rounded-lg border border-gray-200 bg-white shadow-lg max-h-72 overflow-auto",
                        role: "listbox",
                        for (i, key, id, title, kind) in rows {
                            li {
                                key: "{key}",
                                role: "option",
                                "aria-selected": i == active(),
                                class: if i == active() { "px-3 py-2 cursor-pointer bg-blue-50" } else { "px-3 py-2 cursor-pointer hover:bg-gray-50" },
                                onmousedown: move |e| e.prevent_default(),
                                onclick: move |_| add.call(id.clone()),
                                span { class: "text-sm text-gray-900", "{title}" }
                                span { class: "ml-2 text-xs text-gray-500", "{kind}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ─────────────────────────── dated links ───────────────────────────

/// One line: member — link type — member — since when. The link types
/// offered are those the two ends admit (D179), `in_case` excluded.
#[component]
fn LinkComposer(
    members: Vec<LinkView>,
    link_types: Vec<LinkTypeView>,
    on_created: Callback<()>,
) -> Element {
    let ends: Vec<(String, String, String)> = members
        .iter()
        .map(|m| {
            (
                m.source.id.clone(),
                m.source.title.clone(),
                m.source.type_key.clone(),
            )
        })
        .collect();
    let mut src = use_signal(String::new);
    let mut tgt = use_signal(String::new);
    let mut rel = use_signal(String::new);
    let mut from = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let type_of = |id: &str| ends.iter().find(|e| e.0 == id).map(|e| e.2.clone());
    let (st, tt) = (type_of(&src()), type_of(&tgt()));
    let usable: Vec<LinkTypeView> = link_types
        .iter()
        .filter(|l| l.def.key != REL_IN_CASE)
        .filter(|l| match (&st, &tt) {
            (Some(s), Some(t)) => l.def.from_types.contains(s) && l.def.to_types.contains(t),
            (Some(s), None) => l.def.from_types.contains(s),
            _ => true,
        })
        .cloned()
        .collect();
    let rel_ok = usable.iter().any(|l| l.def.key == rel());
    let none_usable = usable.is_empty();
    let valid = !src().is_empty() && !tgt().is_empty() && src() != tgt() && rel_ok;
    let submit = move |_| {
        let input = LinkInput {
            link_type: rel(),
            source_id: src(),
            target_id: tgt(),
            attributes: serde_json::Map::new(),
            valid_from: from_input(&from()),
        };
        let done = t!("case-link-created").to_string();
        busy.set(true);
        spawn(async move {
            let res = api::ontology::create_link(&input).await;
            busy.set(false);
            match res {
                Ok(_) => {
                    rel.set(String::new());
                    tgt.set(String::new());
                    from.set(String::new());
                    toasts::success(done);
                    on_created.call(());
                }
                Err(e) => toasts::error(e),
            }
        });
    };
    let ends2 = ends.clone();

    rsx! {
        div { class: CARD, "data-testid": "case-link-composer",
            h2 { class: "font-semibold text-gray-900", {t!("case-link-title")} }
            div { class: "grid gap-2 md:grid-cols-[1fr_1fr_1fr_auto_auto] items-end",
                div {
                    label { class: LABEL, {t!("case-link-from")} }
                    select {
                        class: INPUT,
                        "data-testid": "case-link-src",
                        onchange: move |e| src.set(e.value()),
                        option { value: "", "—" }
                        for (id, title, _) in ends {
                            option {
                                key: "{id}",
                                value: "{id}",
                                selected: id == src(),
                                "{title}"
                            }
                        }
                    }
                }
                div {
                    label { class: LABEL, {t!("onto-link-type")} }
                    select {
                        class: INPUT,
                        "data-testid": "case-link-type",
                        onchange: move |e| rel.set(e.value()),
                        option { value: "", "—" }
                        for l in usable {
                            option {
                                key: "{l.def.key}",
                                value: "{l.def.key}",
                                selected: l.def.key == rel(),
                                {link_label(Some(&l.def), &l.def.key, false)}
                            }
                        }
                    }
                }
                div {
                    label { class: LABEL, {t!("case-link-to")} }
                    select {
                        class: INPUT,
                        "data-testid": "case-link-tgt",
                        onchange: move |e| tgt.set(e.value()),
                        option { value: "", "—" }
                        for (id, title, _) in ends2 {
                            option {
                                key: "{id}",
                                value: "{id}",
                                selected: id == tgt(),
                                "{title}"
                            }
                        }
                    }
                }
                div {
                    label { class: LABEL, {t!("onto-valid-from")} }
                    input {
                        r#type: "datetime-local",
                        class: INPUT,
                        "data-testid": "case-link-since",
                        value: "{from}",
                        oninput: move |e| from.set(e.value()),
                    }
                }
                button {
                    r#type: "button",
                    class: BTN_PRIMARY,
                    "data-testid": "case-link-submit",
                    disabled: !valid || busy(),
                    onclick: submit,
                    {t!("onto-open-link")}
                }
            }
            if !src().is_empty() && !tgt().is_empty() && none_usable {
                p { class: "text-xs text-amber-700", {t!("case-link-none")} }
            }
            p { class: "text-xs text-gray-500", {t!("case-link-help")} }
        }
    }
}

// ─────────────────────────── timeline ───────────────────────────

#[component]
fn TimelinePanel(
    timeline: Option<CaseTimeline>,
    loading: bool,
    link_types: Vec<LinkTypeView>,
    range: Signal<String>,
    focus: Signal<Option<String>>,
    cursor: Signal<Option<String>>,
) -> Element {
    let mut cursor = cursor;
    let mut range = range;
    let mut focus = focus;
    let presets = [
        ("case", t!("case-range-case")),
        ("24", t!("case-range-24h")),
        ("168", t!("case-range-7d")),
        ("720", t!("case-range-30d")),
        ("8760", t!("case-range-1y")),
    ];
    let Some(tl) = timeline else {
        return rsx! {
            div { class: CARD,
                p { class: "text-sm text-gray-500",
                    if loading {
                        {t!("common-loading")}
                    } else {
                        {t!("case-timeline-error")}
                    }
                }
            }
        };
    };
    let lanes: Vec<String> = tl.members.iter().map(|m| m.source.id.clone()).collect();
    let titles: BTreeMap<String, String> = tl
        .members
        .iter()
        .map(|m| (m.source.id.clone(), m.source.title.clone()))
        .collect();
    // A focus on a member removed since then no longer filters.
    let current = focus().filter(|f| lanes.contains(f));
    let items: Vec<TimelineItem> = tl
        .items
        .iter()
        // Everything that touches the focused member, not only its lane.
        .filter(|i| {
            current.as_ref().is_none_or(|f| {
                &i.lane == f
                    || i.link
                        .as_ref()
                        .is_some_and(|l| &l.source.id == f || &l.target.id == f)
            })
        })
        .cloned()
        .collect();
    // Group by day (UTC), oldest first.
    let mut days: BTreeMap<String, Vec<TimelineItem>> = BTreeMap::new();
    for i in items {
        days.entry(i.at.get(..10).unwrap_or_default().to_string())
            .or_default()
            .push(i);
    }
    let span = format!("{} → {}", short_time(&tl.from), short_time(&tl.to));
    let focused_title = current.and_then(|f| titles.get(&f).cloned());
    let instant = |t: &str| chrono::DateTime::parse_from_rfc3339(t).ok();
    let cut = cursor().as_deref().and_then(instant);

    rsx! {
        div { class: CARD, "data-testid": "case-timeline",
            div { class: "flex flex-wrap items-center gap-2",
                h2 { class: "font-semibold text-gray-900 mr-auto", {t!("case-timeline")} }
                for (key, label) in presets {
                    button {
                        key: "{key}",
                        r#type: "button",
                        "data-range": "{key}",
                        class: if range() == key { CHIP_ON } else { CHIP_OFF },
                        onclick: move |_| range.set(key.to_string()),
                        "{label}"
                    }
                }
            }
            div { class: "flex flex-wrap items-center gap-2 text-xs text-gray-500",
                span { "{span} UTC" }
                if let Some(title) = focused_title {
                    button {
                        r#type: "button",
                        class: "rounded-full bg-blue-50 px-2 py-0.5 text-blue-700 hover:bg-blue-100",
                        onclick: move |_| focus.set(None),
                        {t!("case-focus-only", title : title)}
                        " ×"
                    }
                }
                if !tl.changes_available {
                    span { class: "text-amber-700", {t!("case-changes-unavailable")} }
                }
                if tl.truncated {
                    span { class: "text-amber-700", {t!("case-truncated")} }
                }
            }
            if days.is_empty() {
                p { class: "text-sm text-gray-500 py-6 text-center", {t!("case-timeline-empty")} }
            }
            for (day, list) in days {
                div { key: "{day}",
                    h3 { class: "sticky top-0 bg-white py-1 text-xs font-semibold text-gray-500",
                        "{day}"
                    }
                    ol { class: "relative ml-2 border-l border-gray-200",
                        for (n, item) in list.into_iter().enumerate() {
                            TimelineRow {
                                key: "{day}-{n}",
                                item: item.clone(),
                                color: LANE_COLORS[lanes.iter().position(|l| *l == item.lane).unwrap_or(0)
                                    % LANE_COLORS.len()],
                                link_types: link_types.clone(),
                                after: cut.is_some_and(|c| instant(&item.at).is_some_and(|a| a > c)),
                                on_cursor: move |at: String| cursor.set(Some(at)),
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn TimelineRow(
    item: TimelineItem,
    color: &'static str,
    link_types: Vec<LinkTypeView>,
    /// Later than the snapshot cursor: dimmed.
    after: bool,
    on_cursor: EventHandler<String>,
) -> Element {
    // An approximate date has no meaningful time of day.
    let time = if item.kind == "occurred" {
        "≈".to_string()
    } else {
        item.at.get(11..16).unwrap_or_default().to_string()
    };
    let body = match (&item.link, &item.change) {
        (Some(l), _) => {
            let def = link_types
                .iter()
                .find(|t| t.def.key == l.link_type)
                .map(|t| t.def.clone());
            let label = link_label(def.as_ref(), &l.link_type, false);
            let key = if item.kind == "link_close" {
                "case-tl-link-close"
            } else {
                "case-tl-link-open"
            };
            rsx! {
                span { class: "text-gray-500", {dioxus_i18n::prelude::i18n().translate(key)} }
                Link {
                    class: "font-medium text-gray-900 hover:underline",
                    to: Route::OntologyObject {
                        id: l.source.id.clone(),
                    },
                    "{l.source.title}"
                }
                span { class: "text-gray-500", "{label}" }
                Link {
                    class: "font-medium text-gray-900 hover:underline",
                    to: Route::OntologyObject {
                        id: l.target.id.clone(),
                    },
                    "{l.target.title}"
                }
            }
        }
        (None, Some(c)) => {
            let action = dioxus_i18n::prelude::i18n()
                .try_translate(&format!("onto-action-{}", c.action.replace('.', "-")))
                .unwrap_or_else(|_| c.action.clone());
            let title = c
                .after
                .get("title")
                .or_else(|| c.before.get("title"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            rsx! {
                span { class: "text-gray-500", "{action}" }
                Link {
                    class: "font-medium text-gray-900 hover:underline",
                    to: Route::OntologyObject {
                        id: item.lane.clone(),
                    },
                    "{title}"
                }
            }
        }
        _ => match (&item.range, &item.notification) {
            (Some(r), _) => {
                let end = r
                    .actual_end
                    .as_deref()
                    .or(r.planned_end.as_deref())
                    .map(short_time)
                    .unwrap_or_default();
                rsx! {
                    span { class: "text-gray-500", {t!("case-tl-range")} }
                    span { class: "font-medium text-gray-900", "{r.label}" }
                    if !end.is_empty() {
                        span { class: "text-xs text-gray-500", "→ {end}" }
                    }
                }
            }
            (None, Some(n)) => rsx! {
                span { class: "text-gray-500", {t!("case-tl-notification")} }
                span { class: "font-medium text-gray-900",
                    {n.subject.clone().unwrap_or_else(|| n.channel_kind.clone())}
                }
                span { class: "text-xs text-gray-500", "{n.status}" }
            },
            _ => match &item.fuzzy {
                Some(f) => {
                    let when = fuzzy_label(f);
                    let precision = precision_label(
                        f.get("precision")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    );
                    rsx! {
                        Link {
                            class: "font-medium text-gray-900 hover:underline",
                            to: Route::OntologyObject {
                                id: item.lane.clone(),
                            },
                            {item.title.clone().unwrap_or_default()}
                        }
                        span { class: "font-mono text-xs text-gray-700", "{when}" }
                        span { class: "text-xs text-gray-500", "({precision})" }
                    }
                }
                None => rsx! {},
            },
        },
    };
    let pad = if item.kind == "change" {
        "py-0.5"
    } else {
        "py-1.5"
    };
    let dim = if after {
        "opacity-40"
    } else if item.kind == "change" {
        "opacity-60"
    } else {
        ""
    };
    let at = item.at.clone();
    rsx! {
        li { class: "ml-4 {pad} {dim}", "data-kind": "{item.kind}",
            span { class: "absolute -left-1.5 mt-1.5 h-3 w-3 rounded-full ring-2 ring-white {color}" }
            div { class: "flex flex-wrap items-baseline gap-x-1.5 text-sm",
                button {
                    r#type: "button",
                    class: "font-mono text-xs text-gray-500 w-10 text-left hover:text-blue-700 hover:underline",
                    title: t!("case-tl-set-cursor"),
                    onclick: move |_| on_cursor.call(at.clone()),
                    "{time}"
                }
                {body}
            }
        }
    }
}
