//! `/cases/:id` — the case workspace (cases-workspace.md): the timeline on
//! top (events, raw signals, one lane per tracked member), then the object
//! palette, the links graph and the map with the undated pieces, and the
//! inspector. Every gesture is one server call returning the workspace.

mod frame;
mod graph;
mod imports;
mod inspector;
mod map;
mod markdown;
mod model;
mod money;
mod nest;
mod pages;
mod palette;
mod purge;
mod snapshots;
mod story;
mod timeline;

use std::future::Future;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::ontology::api::{
    CaseAttachInput, CaseDetachInput, CaseEventInput, CaseProposal, CaseSignals, CaseTracks,
    CaseTracksQuery, CaseWorkspace as Workspace, DetectQuery, ObjectInput, ObjectRef,
    ProposalReject, ProposalRetain, SignalsQuery,
};
use pnex_core::ontology::{CASE_STATUS, CASE_STATUSES};
use serde_json::Value;

use super::status_label;
use crate::api;
use crate::api::error::ApiError;
use crate::app::Route;
use crate::components::editor_shell::{EditorShell, EditorStatus, StatusTone};
use crate::components::modal::Modal;
use crate::components::ui::BTN_SECONDARY_SM;
use crate::state::toasts;

/// What the inspector shows.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Sel {
    None,
    Event(String),
    Object(String),
}

/// An object being dragged from the palette, the tray or the inspector.
#[derive(Clone, PartialEq, Debug)]
pub(super) struct Drag {
    pub object: ObjectRef,
    /// Client position of the pointer, for the ghost.
    pub at: (f64, f64),
    /// Moved far enough to be a drag rather than a click.
    pub moved: bool,
}

/// Where a dragged object would land.
#[derive(Clone, PartialEq, Debug)]
pub(super) enum Target {
    /// Attach to this event.
    Event(String),
    /// New event at this instant, with this tracked member already attached.
    Lane { at: i64, actor: Option<String> },
    /// Among the undated pieces.
    Tray,
}

/// Title of a proposal in the user's language (also the retained event's).
pub(super) fn proposal_title(p: &CaseProposal) -> String {
    let who = |i: usize| p.refs.get(i).map(|o| o.title.clone()).unwrap_or_default();
    match p.kind.as_str() {
        "co_presence" => t!("cw-proposal-co-presence", a : who(0), b : who(1)).to_string(),
        "stop" => t!("cw-proposal-stop", who : who(0)).to_string(),
        "signal_loss" => t!("cw-proposal-signal-loss", who : who(0)).to_string(),
        "zone" => {
            let zone = p
                .payload
                .get("zone")
                .and_then(|z| z.as_str())
                .unwrap_or_default();
            let title = |inside: bool| {
                p.refs
                    .iter()
                    .find(|o| (o.id == zone) == inside)
                    .map(|o| o.title.clone())
                    .unwrap_or_default()
            };
            t!("cw-proposal-zone", who : title(false), zone : title(true)).to_string()
        }
        _ => p.kind.clone(),
    }
}

/// The main view of the workspace.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum View {
    Graph,
    Map,
    /// Actors as threads through time (storyline).
    Story,
    /// Payments as a Sankey diagram (cases-money.md K57).
    Money,
}

/// Shared state of the workspace (all `Copy` handles).
#[derive(Clone, Copy)]
pub(super) struct Ws {
    pub case_id: Signal<String>,
    pub data: Signal<Option<Workspace>>,
    pub signals: Signal<Option<CaseSignals>>,
    pub tracks: Signal<Option<CaseTracks>>,
    /// Detections proposed to the case, all statuses.
    pub proposals: Signal<Vec<CaseProposal>>,
    /// Visible window of the timeline, Unix ms.
    pub window: Signal<(i64, i64)>,
    pub cursor: Signal<i64>,
    pub sel: Signal<Sel>,
    /// Only this member's chronology is shown.
    pub follow: Signal<Option<String>>,
    /// A chain of links between two objects (K51): the graph dims the rest.
    pub path: Signal<Vec<ObjectRef>>,
    pub drag: Signal<Option<Drag>>,
    pub target: Signal<Option<Target>>,
    pub playing: Signal<bool>,
    /// Playback speed: ×1, ×3, ×8 real time; 0 = the whole window in 60 s.
    pub speed: Signal<i64>,
    pub frame_open: Signal<bool>,
    /// Times shown in the browser's local time instead of UTC.
    pub local_time: Signal<bool>,
    /// Long empty stretches of the timeline folded into breaks.
    pub compact: Signal<bool>,
    /// The keyboard shortcuts dialog.
    pub help: Signal<bool>,
    /// The main view: the graph or the map.
    pub view: Signal<View>,
    /// The timeline dock unfolded (else a thin playback bar).
    pub dock_open: Signal<bool>,
    /// The inspector shows the case at the cursor (no selection needed).
    pub at_cursor: Signal<bool>,
    /// Raw signals drawn on the timeline (pending detections always are).
    pub show_signals: Signal<bool>,
    /// The "+" panel: members, search, undated pieces.
    pub add_open: Signal<bool>,
    /// The map's list of located items is open.
    pub map_list: Signal<bool>,
    pub can_write: bool,
}

impl Ws {
    /// Runs `fut` in the workspace's scope, not the caller's: a panel that
    /// closes on the click (the inspector after "Remove from the case")
    /// would otherwise cancel the request with itself.
    fn spawn(self, fut: impl std::future::Future<Output = ()> + 'static) {
        dioxus::dioxus_core::Runtime::current().in_scope(self.data.origin_scope(), || spawn(fut));
    }

    fn id(&self) -> String {
        self.case_id.peek().clone()
    }

    /// The window is too wide to read tracks or run the detectors (K27).
    pub fn too_wide(window: (i64, i64)) -> bool {
        window.1 - window.0 > pnex_core::track::MAX_WINDOW_DAYS * 86_400_000
    }

    /// Runs a gesture returning the workspace, then shows it.
    pub fn apply<F>(self, call: F)
    where
        F: Future<Output = Result<Workspace, ApiError>> + 'static,
    {
        let mut data = self.data;
        self.spawn(async move {
            match call.await {
                Ok(w) => data.set(Some(w)),
                Err(e) => toasts::error(e),
            }
        });
    }

    /// Runs the detectors over the visible window, then lists the proposals.
    /// Too wide a window only lists them: zoom in to recompute.
    pub fn detect(self) {
        let (id, win, mut proposals) = (self.id(), *self.window.peek(), self.proposals);
        if Self::too_wide(win) {
            self.spawn(async move {
                proposals.set(api::cases::proposals(&id).await.unwrap_or_default());
            });
            return;
        }
        self.spawn(async move {
            let q = DetectQuery {
                from: Some(model::rfc3339(win.0)),
                to: Some(model::rfc3339(win.1)),
            };
            if let Err(e) = api::cases::detect(&id, &q).await {
                toasts::error(e);
            }
            proposals.set(api::cases::proposals(&id).await.unwrap_or_default());
        });
    }

    /// Retains a proposal as an event titled `title`.
    pub fn retain(self, pid: String, title: String) {
        let (id, mut proposals) = (self.id(), self.proposals);
        self.apply(async move {
            let w = api::cases::retain(&id, &pid, &ProposalRetain { title }).await;
            proposals.set(api::cases::proposals(&id).await.unwrap_or_default());
            w
        });
    }

    /// Rejects a proposal; it is never proposed again.
    pub fn reject(self, pid: String) {
        let (id, mut proposals) = (self.id(), self.proposals);
        self.spawn(async move {
            match api::cases::reject(&id, &pid, &ProposalReject::default()).await {
                Ok(p) => proposals.set(p),
                Err(e) => toasts::error(e),
            }
        });
    }

    pub fn reload(self) {
        let id = self.id();
        self.apply(async move { api::cases::workspace(&id).await });
    }

    pub fn attach(self, input: CaseAttachInput) {
        let id = self.id();
        self.apply(async move { api::cases::attach(&id, &input).await });
    }

    pub fn detach(self, input: CaseDetachInput) {
        let id = self.id();
        self.apply(async move { api::cases::detach(&id, &input).await });
    }

    /// New event at `[from, to]`, selected once created.
    pub fn create_event(self, title: String, from: i64, to: i64, attach: Vec<CaseAttachInput>) {
        let id = self.id();
        let mut data = self.data;
        let mut sel = self.sel;
        let mut cursor = self.cursor;
        let before: Vec<String> = data
            .peek()
            .as_ref()
            .map(|w| w.events.iter().map(|e| e.event.id.clone()).collect())
            .unwrap_or_default();
        let input = CaseEventInput {
            title,
            occurred: model::occurred(from, to),
            origin: None,
            attach,
        };
        self.spawn(async move {
            match api::cases::create_event(&id, &input).await {
                Ok(w) => {
                    if let Some(e) = w.events.iter().find(|e| !before.contains(&e.event.id)) {
                        sel.set(Sel::Event(e.event.id.clone()));
                    }
                    cursor.set(from);
                    data.set(Some(w));
                }
                Err(e) => toasts::error(e),
            }
        });
    }

    /// Saves new properties of an object (event, case) on its version.
    pub fn save_object(
        self,
        id: String,
        title: String,
        properties: serde_json::Map<String, Value>,
        version: i32,
    ) {
        self.save_object_then(id, title, properties, version, false);
    }

    /// As [`Self::save_object`], then reruns the detectors once saved when
    /// `detect` (new thresholds).
    pub fn save_object_then(
        self,
        id: String,
        title: String,
        properties: serde_json::Map<String, Value>,
        version: i32,
        detect: bool,
    ) {
        let ws = self;
        self.spawn(async move {
            let input = ObjectInput {
                type_key: String::new(),
                title,
                properties,
                expected_version: Some(version),
            };
            match api::ontology::update_object(&id, &input).await {
                Ok(_) => {
                    ws.reload();
                    if detect {
                        ws.detect();
                    }
                }
                Err(e) => {
                    toasts::error(e);
                    ws.reload();
                }
            }
        });
    }

    /// Selects an event: the cursor jumps to its start.
    pub fn select_event(mut self, id: String) {
        let start = self.data.peek().as_ref().and_then(|w| {
            w.events
                .iter()
                .find(|e| e.event.id == id)
                .and_then(|e| model::span(&e.event))
        });
        if let Some((from, _)) = start {
            self.cursor.set(from);
        }
        self.sel.set(Sel::Event(id));
    }
}

/// True while the keyboard is in a text field (shortcuts stay off).
fn editing_text() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let tag = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.active_element())
            .map(|e| e.tag_name())
            .unwrap_or_default();
        matches!(tag.as_str(), "INPUT" | "TEXTAREA" | "SELECT")
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

fn now_ms() -> i64 {
    (crate::util::now_secs() * 1000.0) as i64
}

/// Keyed by the case: moving to a parent or a sub-case (GS16) mounts a
/// fresh workspace instead of keeping the previous case's state.
#[component]
pub fn CaseWorkspace(id: String) -> Element {
    // A key on a lone root node does not remount it (Dioxus 0.7): only a
    // keyed list does, so going to another case starts afresh.
    rsx! {
        for id in std::iter::once(id) {
            CaseWorkspaceOf { key: "{id}", id: id.clone() }
        }
    }
}

#[component]
fn CaseWorkspaceOf(id: String) -> Element {
    let ws = Ws {
        case_id: use_signal(|| id.clone()),
        data: use_signal(|| None),
        signals: use_signal(|| None),
        tracks: use_signal(|| None),
        proposals: use_signal(Vec::new),
        window: use_signal(|| (0, 0)),
        cursor: use_signal(|| 0),
        sel: use_signal(|| Sel::None),
        follow: use_signal(|| None),
        path: use_signal(Vec::new),
        drag: use_signal(|| None),
        target: use_signal(|| None),
        playing: use_signal(|| false),
        speed: use_signal(|| 3),
        frame_open: use_signal(|| false),
        local_time: use_signal(|| false),
        compact: use_signal(|| true),
        help: use_signal(|| false),
        view: use_signal(|| View::Graph),
        dock_open: use_signal(|| true),
        at_cursor: use_signal(|| false),
        show_signals: use_signal(|| false),
        add_open: use_signal(|| false),
        map_list: use_signal(|| true),
        can_write: crate::state::access::can("cases:write"),
    };
    use_context_provider(|| ws);
    let mut load_error = use_signal(|| None::<String>);
    let mut help = ws.help;

    // First load: the window frames the events, the cursor sits at the end.
    let first_id = id.clone();
    use_hook(move || {
        let (mut data, mut window, mut cursor) = (ws.data, ws.window, ws.cursor);
        spawn(async move {
            match api::cases::workspace(&first_id).await {
                Ok(w) => {
                    let win = model::initial_window(&w.events, now_ms());
                    window.set(win);
                    cursor.set(win.1.min(now_ms()));
                    data.set(Some(w));
                    // Detections at opening (a viewer only reads them).
                    if ws.can_write {
                        ws.detect();
                    } else {
                        let mut proposals = ws.proposals;
                        let id = ws.id();
                        proposals.set(api::cases::proposals(&id).await.unwrap_or_default());
                    }
                }
                Err(e) => load_error.set(Some(e.to_string())),
            }
        });
    });

    // Signals and tracks follow the window and the members.
    use_effect(move || {
        let win = (ws.window)();
        let members = (ws.data)()
            .map(|w| w.events.iter().map(|e| e.attachments.len()).sum::<usize>() + w.undated.len());
        if win.1 <= win.0 || members.is_none() {
            return;
        }
        let id = ws.id();
        let (mut signals, mut tracks) = (ws.signals, ws.tracks);
        spawn(async move {
            let q = SignalsQuery {
                from: Some(model::rfc3339(win.0)),
                to: Some(model::rfc3339(win.1)),
            };
            signals.set(api::cases::signals(&id, &q).await.ok());
            // Tracks serve at most 31 days: a wider window shows none.
            if Ws::too_wide(win) {
                tracks.set(None);
                return;
            }
            let tq = CaseTracksQuery {
                from: model::rfc3339(win.0),
                to: model::rfc3339(win.1),
                resolution_ms: Some(((win.1 - win.0) / 1200).max(1_000)),
                signal_gap_s: None,
            };
            tracks.set(api::cases::tracks(&id, &tq).await.ok());
        });
    });

    // Playback: real time × speed, or the window in 60 s; stops at the end.
    use_future(move || async move {
        let mut last = crate::util::now_secs();
        loop {
            crate::util::sleep(std::time::Duration::from_millis(100)).await;
            let now = crate::util::now_secs();
            let dt_ms = ((now - last) * 1000.0) as i64;
            last = now;
            if !*ws.playing.peek() {
                continue;
            }
            let win = *ws.window.peek();
            let speed = *ws.speed.peek();
            let step = if speed == 0 {
                (win.1 - win.0) * dt_ms / 60_000
            } else {
                dt_ms * speed
            };
            let next = *ws.cursor.peek() + step.max(1);
            let (mut cursor, mut playing) = (ws.cursor, ws.playing);
            if next >= win.1 {
                cursor.set(win.1);
                playing.set(false);
            } else {
                cursor.set(next);
            }
        }
    });

    if let Some(e) = load_error() {
        return rsx! {
            p { class: "p-6 text-sm text-alarm", "{e}" }
        };
    }
    let Some(w) = (ws.data)() else {
        return rsx! {
            p { class: "p-6 text-sm text-slate", {t!("common-loading")} }
        };
    };

    let on_key = move |e: KeyboardEvent| {
        if editing_text() {
            return;
        }
        let (mut playing, mut sel, mut follow, mut frame_open, mut cursor, mut window) = (
            ws.playing,
            ws.sel,
            ws.follow,
            ws.frame_open,
            ws.cursor,
            ws.window,
        );
        match e.key() {
            Key::Character(c) if c == " " => {
                e.prevent_default();
                let p = !playing();
                playing.set(p);
            }
            Key::Character(c) if c == "?" => help.set(!help()),
            Key::Character(c) if (c == "n" || c == "N") && ws.can_write => {
                let t = cursor();
                ws.create_event(t!("cw-new-event").to_string(), t, t, Vec::new());
            }
            // The pending detection under the cursor (a click on its chip
            // puts the cursor there).
            Key::Character(c) if (c == "r" || c == "R") && ws.can_write => {
                let t = cursor();
                let hit = ws
                    .proposals
                    .peek()
                    .iter()
                    .find(|p| p.status == "proposed" && model::ms(&p.t_start) == Some(t))
                    .cloned();
                if let Some(p) = hit {
                    ws.retain(p.id.clone(), proposal_title(&p));
                }
            }
            Key::Escape => {
                if help() {
                    help.set(false);
                } else if frame_open() {
                    frame_open.set(false);
                } else if (ws.add_open)() {
                    let mut add = ws.add_open;
                    add.set(false);
                } else if sel() != Sel::None {
                    sel.set(Sel::None);
                } else if (ws.at_cursor)() {
                    let mut at = ws.at_cursor;
                    at.set(false);
                } else {
                    follow.set(None);
                }
            }
            Key::ArrowRight | Key::ArrowLeft => {
                e.prevent_default();
                let right = e.key() == Key::ArrowRight;
                if e.modifiers().shift() {
                    let win = window();
                    let step = model::tick_step(win, 8);
                    let next = cursor() + if right { step } else { -step };
                    cursor.set(next.clamp(win.0, win.1));
                    return;
                }
                let t = cursor();
                let mut starts: Vec<(i64, String)> = ws
                    .data
                    .peek()
                    .as_ref()
                    .map(|w| {
                        w.events
                            .iter()
                            .filter_map(|e| Some((model::span(&e.event)?.0, e.event.id.clone())))
                            .collect()
                    })
                    .unwrap_or_default();
                starts.sort();
                let next = if right {
                    starts.into_iter().find(|(s, _)| *s > t)
                } else {
                    starts.into_iter().rev().find(|(s, _)| *s < t)
                };
                if let Some((_, id)) = next {
                    ws.select_event(id);
                }
            }
            Key::Character(c) if c == "+" || c == "=" => {
                window.set(model::zoom(window(), cursor(), 0.66))
            }
            Key::Character(c) if c == "-" => window.set(model::zoom(window(), cursor(), 1.5)),
            Key::Character(c) if c == "0" => {
                let win = model::initial_window(
                    &ws.data
                        .peek()
                        .as_ref()
                        .map(|w| w.events.clone())
                        .unwrap_or_default(),
                    now_ms(),
                );
                window.set(win);
            }
            _ => {}
        }
    };

    let case = w.case.clone();
    let rename = {
        let case = case.clone();
        move |title: String| {
            ws.save_object(
                case.id.clone(),
                title,
                case.properties.clone(),
                case.version,
            )
        }
    };
    let navigator = use_navigator();
    let summary = EditorStatus::new(
        StatusTone::Slate,
        t!("cw-summary", events : w.events.len()).to_string(),
    );
    let inspector_open = (ws.sel)() != Sel::None || (ws.at_cursor)();
    rsx! {
        EditorShell {
            on_back: {
                let parent = w.parents.first().map(|p| p.id.clone());
                move |_| {
                    let to = match parent.clone() {
                        Some(id) => Route::CaseWorkspace { id },
                        None => Route::Cases {},
                    };
                    navigator.push(to);
                }
            },
            title: case.title.clone(),
            status: summary,
            on_rename: if ws.can_write { Some(Callback::new(rename)) } else { None },
            actions: rsx! {
                Actions { workspace: w.clone() }
            },
            palette: rsx! {
                palette::AddMenu {}
            },
            palette_inset: (ws.view)() == View::Map && (ws.map_list)(),
            canvas: rsx! {
                div {
                    class: "flex h-full flex-col outline-none",
                    tabindex: 0,
                    "data-testid": "case-workspace",
                    onkeydown: on_key,
                    onpointermove: move |e: PointerEvent| {
                        let mut drag = ws.drag;
                        if let Some(mut d) = drag() {
                            let c = e.client_coordinates();
                            d.moved |= (c.x - d.at.0).abs() + (c.y - d.at.1).abs() > 4.0;
                            d.at = (c.x, c.y);
                            drag.set(Some(d));
                        }
                    },
                    onpointerup: move |_| drop_dragged(ws),
                    onpointerleave: move |_| {
                        let (mut drag, mut target) = (ws.drag, ws.target);
                        drag.set(None);
                        target.set(None);
                    },
                    nest::Nest { workspace: w.clone() }
                    // The main view, the inspector drawn over it on demand.
                    div { class: "relative min-h-0 flex-1 overflow-hidden",
                        match (ws.view)() {
                            View::Graph => rsx! {
                                graph::Graph { workspace: w.clone() }
                            },
                            View::Map => rsx! {
                                map::CaseMap {}
                            },
                            View::Story => rsx! {
                                story::Story { workspace: w.clone() }
                            },
                            View::Money => rsx! {
                                money::Money { workspace: w.clone() }
                            },
                        }
                        if w.events.is_empty() && w.undated.is_empty() {
                            EmptyCase {}
                        }
                        if inspector_open {
                            inspector::Inspector {}
                        }
                    }
                    timeline::Timeline {}
                }
            },
        }
        if (ws.frame_open)() {
            frame::Frame { workspace: w.clone() }
        }
        if help() {
            Modal {
                title: t!("cw-keys"),
                max_width: "sm:max-w-md",
                on_close: move |_| help.set(false),
                KeyHelp {}
            }
        }
        if let Some(d) = (ws.drag)().filter(|d| d.moved) {
            div {
                class: "pointer-events-none fixed z-50 rounded-md border border-signal bg-surface px-2 py-1 text-xs text-ink shadow-lg",
                style: "left: {d.at.0 + 12.0}px; top: {d.at.1 + 12.0}px;",
                "{d.object.title}"
            }
        }
    }
}

/// Ends a drag: attaches the object where it was dropped.
fn drop_dragged(ws: Ws) {
    let (mut drag, mut target) = (ws.drag, ws.target);
    let (Some(d), t) = (drag(), target()) else {
        return;
    };
    drag.set(None);
    target.set(None);
    if !d.moved || !ws.can_write {
        return;
    }
    let object_id = d.object.id.clone();
    match t {
        Some(Target::Event(event_id)) => ws.attach(CaseAttachInput {
            event_id: Some(event_id),
            object_id,
            ..Default::default()
        }),
        Some(Target::Lane { at, actor }) => {
            let mut attach = vec![CaseAttachInput {
                object_id: object_id.clone(),
                ..Default::default()
            }];
            if let Some(a) = actor.filter(|a| *a != object_id) {
                attach.insert(
                    0,
                    CaseAttachInput {
                        object_id: a,
                        role: Some("actor".into()),
                        ..Default::default()
                    },
                );
            }
            ws.create_event(d.object.title.clone(), at, at, attach);
        }
        Some(Target::Tray) => ws.attach(CaseAttachInput {
            object_id,
            ..Default::default()
        }),
        None => {}
    }
}

/// Top bar actions: the status, the view (graph or map), the frame, the
/// exports, the shortcuts.
#[component]
fn Actions(workspace: Workspace) -> Element {
    let ws = use_context::<Ws>();
    let case = workspace.case.clone();
    let status = case
        .properties
        .get(CASE_STATUS)
        .and_then(Value::as_str)
        .unwrap_or("open")
        .to_string();
    let save_status = {
        let case = case.clone();
        move |new_status: String| {
            let mut props = case.properties.clone();
            props.insert(CASE_STATUS.into(), Value::String(new_status));
            ws.save_object(case.id.clone(), case.title.clone(), props, case.version);
        }
    };
    let export_id = case.id.clone();
    let export_name = case.title.clone();
    let (md_id, md_name) = (case.id.clone(), case.title.clone());
    let mut view = ws.view;
    let tab = |on: bool| {
        if on {
            "inline-flex items-center gap-1.5 px-3 py-1.5 text-sm bg-signal-soft text-signal font-medium"
        } else {
            "inline-flex items-center gap-1.5 px-3 py-1.5 text-sm text-slate hover:bg-gray-50"
        }
    };
    rsx! {
        div {
            class: "flex flex-wrap items-center gap-2",
            "data-testid": "case-header",
            div {
                class: "inline-flex overflow-hidden rounded-md border border-line-strong",
                role: "group",
                "aria-label": t!("cw-status"),
                for s in CASE_STATUSES {
                    button {
                        key: "{s}",
                        r#type: "button",
                        disabled: !ws.can_write,
                        "aria-pressed": status == s,
                        "data-status": "{s}",
                        class: if status == s { "px-3 py-1.5 text-sm bg-signal-soft text-signal font-medium" } else { "px-3 py-1.5 text-sm text-slate hover:bg-gray-50" },
                        onclick: {
                            let save = save_status.clone();
                            move |_| save(s.to_string())
                        },
                        {status_label(s)}
                    }
                }
            }
            div {
                class: "inline-flex overflow-hidden rounded-md border border-line-strong",
                role: "group",
                "aria-label": t!("cw-view"),
                button {
                    r#type: "button",
                    class: tab(view() == View::Graph),
                    "aria-pressed": view() == View::Graph,
                    "data-testid": "case-view-graph",
                    onclick: move |_| view.set(View::Graph),
                    {t!("cw-links")}
                }
                button {
                    r#type: "button",
                    class: tab(view() == View::Map),
                    "aria-pressed": view() == View::Map,
                    "data-testid": "case-view-map",
                    onclick: move |_| view.set(View::Map),
                    {t!("cw-map")}
                }
                button {
                    r#type: "button",
                    class: tab(view() == View::Story),
                    "aria-pressed": view() == View::Story,
                    "data-testid": "case-view-story",
                    onclick: move |_| {
                        view.set(View::Story);
                        // The story is a timeline itself: fold the dock.
                        let mut dock = ws.dock_open;
                        dock.set(false);
                    },
                    {t!("cw-story")}
                }
                button {
                    r#type: "button",
                    class: tab(view() == View::Money),
                    "aria-pressed": view() == View::Money,
                    "data-testid": "case-view-money",
                    onclick: move |_| view.set(View::Money),
                    {t!("money-view")}
                }
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                "data-testid": "case-frame",
                onclick: move |_| {
                    let mut f = ws.frame_open;
                    f.set(true);
                },
                {t!("cw-frame")}
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                "data-testid": "case-export",
                onclick: move |_| {
                    let (id, name) = (export_id.clone(), export_name.clone());
                    spawn(async move {
                        match api::cases::export(&id).await {
                            Ok(doc) => {
                                let bytes = serde_json::to_vec_pretty(&doc).unwrap_or_default();
                                let file = snapshots::file_name(&name, ".case.json");
                                crate::util::save_blob(&file, &bytes);
                                if doc.persons > 0 {
                                    toasts::info(
                                        t!("cw-export-persons", count : doc.persons).to_string(),
                                    );
                                }
                            }
                            Err(e) => toasts::error(e),
                        }
                    });
                },
                {t!("cw-export")}
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                "data-testid": "case-export-md",
                onclick: move |_| {
                    let (id, name) = (md_id.clone(), md_name.clone());
                    let local = *ws.local_time.peek();
                    spawn(async move {
                        match api::cases::export(&id).await {
                            Ok(doc) => {
                                let text = snapshots::markdown(&doc, None, local);
                                let file = snapshots::file_name(&name, ".case.md");
                                crate::util::save_blob(&file, text.as_bytes());
                            }
                            Err(e) => toasts::error(e),
                        }
                    });
                },
                {t!("cw-export-md")}
            }
            button {
                class: "{BTN_SECONDARY_SM} w-9",
                r#type: "button",
                "aria-label": t!("cw-keys"),
                title: t!("cw-keys"),
                "data-testid": "case-keys-open",
                onclick: move |_| {
                    let mut h = ws.help;
                    h.set(true);
                },
                "?"
            }
        }
    }
}

/// An empty case: what to do first, over the view.
#[component]
fn EmptyCase() -> Element {
    rsx! {
        div { class: "pointer-events-none absolute inset-0 z-10 grid place-items-center p-6",
            div {
                class: "max-w-md rounded-lg border border-line bg-surface/95 p-4 text-sm shadow-sm",
                "data-testid": "case-empty",
                p { class: "font-medium text-ink", {t!("cw-empty-title")} }
                ol { class: "mt-2 list-decimal space-y-1 pl-5 text-slate",
                    li { {t!("cw-empty-step-1")} }
                    li { {t!("cw-empty-step-2")} }
                    li { {t!("cw-empty-step-3")} }
                }
            }
        }
    }
}

/// The keyboard shortcuts of the workspace (K36), opened with "?".
#[component]
fn KeyHelp() -> Element {
    let keys = [
        (t!("cw-kbd-space").to_string(), t!("cw-key-play")),
        ("← →".into(), t!("cw-key-events")),
        ("⇧ ← →".into(), t!("cw-key-step")),
        ("+ −".into(), t!("cw-key-zoom")),
        ("0".into(), t!("cw-fit")),
        ("N".into(), t!("cw-key-new")),
        ("R".into(), t!("cw-key-retain")),
        (t!("cw-kbd-esc").to_string(), t!("cw-key-escape")),
        ("?".into(), t!("cw-keys")),
    ];
    rsx! {
        div { class: "p-4 sm:p-6", "data-testid": "case-keys",
            dl { class: "grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm",
                for (k, label) in keys {
                    dt { key: "{k}",
                        kbd { class: "rounded border border-line-strong bg-gray-50 px-1.5 font-mono text-xs text-ink",
                            "{k}"
                        }
                    }
                    dd { class: "text-slate", "{label}" }
                }
            }
            p { class: "mt-3 text-xs text-slate", {t!("cw-keys-help")} }
        }
    }
}
