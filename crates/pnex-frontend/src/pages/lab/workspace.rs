//! Page `/lab/:id` — annotation workspace of a Vision Lab project (D198):
//! the picture with its boxes (drag to draw, click to select), class
//! picker, the timeline of its source (`timeline.rs`), live statistics and
//! frozen dataset versions. Every edit is saved at once: work resumes
//! where it stopped ("Resume" jumps to the next picture to label).
//!
//! Edits are applied locally at once and saved in the background, one
//! request at a time per image (each answer carries the version the next
//! save sends: a burst of edits never conflicts with itself).
//! Keyboard: 1–9 class, Delete removes the selected box, ← / → image,
//! N toggles "no object".

use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::lab::{
    self, LabAnnotationsInput, LabBox, LabImage, LabProject, LabSplit, LabSuggestRequest,
    LabSuggestion,
};

use crate::api;
use crate::app::Route;
use crate::components::crud::layout::ListLayout;
use crate::components::icons;
use crate::components::modal::Modal;
use crate::components::stepper::{Station, Stepper};
use crate::components::ui::{
    BTN_PRIMARY, BTN_PRIMARY_SM, BTN_SECONDARY, BTN_SECONDARY_SM, FIELD_LABEL, INPUT, PANEL,
};
use crate::state::{org, toasts};
use crate::util::sleep;

use super::picker::PhotoPicker;
use super::recording::RecordingImport;
use super::timeline::{self as tl, LabTimeline};
use super::training::TrainingPanel;
use super::video::VideoImport;
use super::ProjectForm;
use crate::components::gerbe::Gerbe;

/// Box colours by class index (cycled), readable on photos.
const CLASS_COLORS: [&str; 8] = [
    "#e11d48", "#2563eb", "#16a34a", "#d97706", "#9333ea", "#0891b2", "#db2777", "#65a30d",
];

/// The training button: green, the goal of the whole page.
pub(super) const BTN_TRAIN: &str = "inline-flex items-center justify-center gap-1.5 min-h-10 px-4 py-2 rounded-md bg-ok text-white text-sm font-semibold shadow-sm hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ok active:scale-[0.97] motion-reduce:active:scale-100 transition-[opacity,scale] duration-150 ease-out-ui";

/// Smallest box kept (normalized side): a click is not a box.
const MIN_BOX: f32 = 0.005;

pub(crate) fn class_color(class: u32) -> &'static str {
    CLASS_COLORS[class as usize % CLASS_COLORS.len()]
}

/// Normalized box from two corners.
fn rect_of(a: (f32, f32), b: (f32, f32)) -> [f32; 4] {
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    [x0, y0, x1 - x0, y1 - y0]
}

/// CSS of a box overlay (percentages of the picture).
fn box_style(b: &[f32; 4], color: &str) -> String {
    format!(
        "left: {}%; top: {}%; width: {}%; height: {}%; border-color: {color};",
        b[0] * 100.0,
        b[1] * 100.0,
        b[2] * 100.0,
        b[3] * 100.0
    )
}

/// Screens of a project; training opens as a dialog on top of either.
#[derive(Clone, Copy, PartialEq)]
enum Step {
    Images,
    Label,
}

/// Save state of the image being edited.
#[derive(Clone, Copy, Default, PartialEq)]
struct SaveState {
    saving: bool,
    /// An edit arrived during a save: save again when it returns.
    dirty: bool,
}

#[component]
pub fn LabProjectPage(id: String) -> Element {
    let pid = id.clone();
    let can_write = org::current_can_write();
    let mut reload = use_signal(|| 0u32);
    let mut images = use_signal(Vec::<LabImage>::new);
    let mut loaded = use_signal(|| false);
    let mut selected = use_signal(|| 0usize);
    let mut class = use_signal(|| 0u32);
    let mut selected_box = use_signal(|| None::<usize>);
    let mut picker_open = use_signal(|| false);
    let mut recording_open = use_signal(|| false);
    let mut video_open = use_signal(|| false);
    let mut versions_tick = use_signal(|| 0u32);
    let mut playing = use_signal(|| false);
    let mut speed = use_signal(|| 2u32);
    // "Where do the pictures come from?" chooser of the add button.
    let mut adding = use_signal(|| false);
    // Step shown; `None` = the natural one (pictures first, then labeling).
    let mut step = use_signal(|| None::<Step>);
    let mut editing = use_signal(|| false);
    let mut training = use_signal(|| false);
    // Zoom of the picture (1 = whole picture), kept from picture to picture.
    let mut zoom = use_signal(|| 1.0f32);
    // Hand mode: a drag moves the zoomed picture instead of drawing.
    let mut hand = use_signal(|| false);
    let mut save_state = use_signal(SaveState::default);
    // Proposals of a registry model, for one image: (image id, boxes).
    let mut suggestions = use_signal(|| (String::new(), Vec::<LabSuggestion>::new()));
    let mut model = use_signal(String::new);
    let mut suggesting = use_signal(|| false);
    let models = use_resource(|| async {
        api::ml_models::list(Some(100), None)
            .await
            .map(|p| p.results)
            .unwrap_or_default()
    });

    let project = use_resource({
        let pid = pid.clone();
        move || {
            let _ = reload();
            let pid = pid.clone();
            async move { api::lab::project(&pid).await }
        }
    });
    use_effect({
        let pid = pid.clone();
        move || {
            let _ = reload();
            let pid = pid.clone();
            spawn(async move {
                match api::lab::images(&pid).await {
                    Ok(list) => {
                        // First load: open where labeling stopped.
                        if !*loaded.peek() {
                            let todo = tl::sources(&list)
                                .iter()
                                .flat_map(|s| s.frames.iter().map(|f| f.0))
                                .find(|i| !list[*i].annotated());
                            if let Some(i) = todo {
                                selected.set(i);
                            }
                        }
                        images.set(list);
                    }
                    Err(e) => toasts::error(e),
                }
                loaded.set(true);
            });
        }
    });

    // A picture of the review queue opens with the model's proposals shown.
    use_effect(move || {
        let list = images.read();
        let i = selected().min(list.len().saturating_sub(1));
        if let Some(img) = list.get(i) {
            if img.review && !img.proposals.is_empty() && suggestions.peek().0 != img.id {
                suggestions.set((img.id.clone(), img.proposals.clone()));
            }
        }
    });

    let project_now: Option<LabProject> = project
        .read()
        .as_ref()
        .and_then(|r| r.as_ref().ok().cloned());
    let classes: Vec<String> = project_now
        .as_ref()
        .map(|p| p.classes.clone())
        .unwrap_or_default();
    let list = images.read().clone();
    let count = list.len();
    let index = selected().min(count.saturating_sub(1));
    let current: Option<LabImage> = list.get(index).cloned();
    let stats = lab::stats(&classes, &list);

    // Saves the local annotations of image `i`, serialized (see SaveState).
    let mut persist = move |i: usize| {
        if save_state.peek().saving {
            save_state.write().dirty = true;
            return;
        }
        save_state.set(SaveState {
            saving: true,
            dirty: false,
        });
        spawn(async move {
            loop {
                let Some(img) = images.peek().get(i).cloned() else {
                    break;
                };
                let input = LabAnnotationsInput {
                    boxes: img.boxes.clone(),
                    negative: img.negative,
                    split: img.split,
                    expected_version: img.version,
                };
                match api::lab::save_annotations(&img.id, &input).await {
                    Ok(saved) => {
                        if let Some(slot) = images.write().get_mut(i) {
                            slot.version = saved.version;
                            slot.updated_at = saved.updated_at;
                        }
                    }
                    Err(e) => {
                        toasts::error(e);
                        // Resync with the server (another editor, rejected box).
                        reload.with_mut(|r| *r += 1);
                        break;
                    }
                }
                if !save_state.peek().dirty {
                    break;
                }
                save_state.write().dirty = false;
            }
            save_state.set(SaveState::default());
        });
    };

    // Local edit of the current image + background save.
    let mut edit = move |f: Box<dyn FnOnce(&mut LabImage)>| {
        let i = *selected.peek();
        let changed = {
            let mut list = images.write();
            match list.get_mut(i) {
                Some(img) => {
                    f(img);
                    true
                }
                None => false,
            }
        };
        if changed {
            persist(i);
        }
    };

    // The class of the next box; a selected box takes it too.
    let mut pick_class = move |c: u32| {
        class.set(c);
        if let Some(b) = *selected_box.peek() {
            edit(Box::new(move |img: &mut LabImage| {
                if let Some(bx) = img.boxes.get_mut(b) {
                    bx.class = c;
                }
            }));
        }
    };

    // Previous / next picture of the same source.
    let mut go = move |delta: i64| {
        let next = tl::step(&tl::sources(&images.peek()), *selected.peek(), delta);
        if let Some(n) = next {
            selected.set(n);
            selected_box.set(None);
        }
    };
    // Playback: the next picture of the source at the chosen pace.
    use_future(move || async move {
        loop {
            let pace = 1000 / u64::from((*speed.peek()).max(1));
            sleep(std::time::Duration::from_millis(pace)).await;
            if !*playing.peek() {
                continue;
            }
            let next = tl::step(&tl::sources(&images.peek()), *selected.peek(), 1);
            match next {
                Some(n) => {
                    selected.set(n);
                    selected_box.set(None);
                }
                None => playing.set(false),
            }
        }
    });

    let on_key = move |e: KeyboardEvent| {
        let key = e.key().to_string();
        if let Some(d) = key.chars().next().and_then(|c| c.to_digit(10)) {
            if key == "0" {
                zoom.set(1.0);
            } else if key.len() == 1 && (1..=9).contains(&d) {
                let c = d - 1;
                class.set(c);
                if let Some(b) = *selected_box.peek() {
                    edit(Box::new(move |img: &mut LabImage| {
                        if let Some(bx) = img.boxes.get_mut(b) {
                            bx.class = c;
                        }
                    }));
                }
            }
            return;
        }
        match key.as_str() {
            "ArrowRight" => go(1),
            "ArrowLeft" => go(-1),
            "+" | "=" => {
                let z = (*zoom.peek() * ZOOM_STEP).min(ZOOM_MAX);
                zoom.set(z);
            }
            "-" => {
                let z = (*zoom.peek() / ZOOM_STEP).max(ZOOM_MIN);
                zoom.set(z);
            }
            "h" | "H" => hand.toggle(),
            "Delete" | "Backspace" => {
                let sel = *selected_box.peek();
                if let Some(b) = sel {
                    selected_box.set(None);
                    edit(Box::new(move |img: &mut LabImage| {
                        if b < img.boxes.len() {
                            img.boxes.remove(b);
                        }
                    }));
                }
            }
            "n" | "N" => edit(Box::new(|img: &mut LabImage| {
                img.negative = !img.negative;
                if img.negative {
                    img.boxes.clear();
                }
            })),
            _ => {}
        }
    };

    let title = project_now
        .as_ref()
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let present: Vec<String> = list.iter().map(|i| i.asset_id.clone()).collect();
    let saving = save_state().saving;
    let shown: Vec<LabSuggestion> = {
        let (sid, list) = &*suggestions.read();
        if current.as_ref().is_some_and(|c| &c.id == sid) {
            list.clone()
        } else {
            Vec::new()
        }
    };
    let model_options: Vec<(String, String)> = models
        .read()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|m| (m.id, m.name))
        .collect();

    // Keeps proposals as boxes (their mapped class, else the current one).
    let mut accept = move |picked: Vec<usize>| {
        let (sid, list) = suggestions.peek().clone();
        let c = *class.peek();
        let boxes: Vec<LabBox> = picked
            .iter()
            .filter_map(|i| list.get(*i))
            .map(|s| LabBox {
                class: s.class.unwrap_or(c),
                bbox: s.bbox,
            })
            .collect();
        let rest: Vec<LabSuggestion> = list
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !picked.contains(i))
            .map(|(_, s)| s)
            .collect();
        suggestions.set((sid, rest));
        if !boxes.is_empty() {
            edit(Box::new(move |img: &mut LabImage| {
                img.negative = false;
                img.boxes.extend(boxes);
            }));
        }
    };
    let on_interpolate = move |_| {
        let Some(img) = images.peek().get(*selected.peek()).cloned() else {
            return;
        };
        let none = t!("lab-interpolate-none").to_string();
        suggesting.set(true);
        spawn(async move {
            match api::lab::interpolate(&img.id).await {
                Ok(list) => {
                    if list.is_empty() {
                        toasts::info(none);
                    }
                    suggestions.set((img.id, list));
                }
                Err(e) => toasts::error(e),
            }
            suggesting.set(false);
        });
    };
    let on_propagate = move |_| {
        let Some(img) = images.peek().get(*selected.peek()).cloned() else {
            return;
        };
        let req = LabSuggestRequest {
            model_id: model.peek().clone(),
            min_score: 0.0,
        };
        let none = t!("lab-propagate-none").to_string();
        suggesting.set(true);
        spawn(async move {
            match api::lab::propagate(&img.id, &req).await {
                Ok(list) => {
                    if list.is_empty() {
                        toasts::info(none);
                    }
                    suggestions.set((img.id, list));
                }
                Err(e) => toasts::error(e),
            }
            suggesting.set(false);
        });
    };
    let on_suggest = move |_| {
        let Some(img) = images.peek().get(*selected.peek()).cloned() else {
            return;
        };
        let req = LabSuggestRequest {
            model_id: model.peek().clone(),
            min_score: 0.0,
        };
        let none = t!("lab-suggest-none").to_string();
        suggesting.set(true);
        spawn(async move {
            match api::lab::suggest(&img.id, &req).await {
                Ok(list) => {
                    if list.is_empty() {
                        toasts::info(none);
                    }
                    suggestions.set((img.id, list));
                }
                Err(e) => toasts::error(e),
            }
            suggesting.set(false);
        });
    };

    let srcs = tl::sources(&list);
    let done: usize = srcs.iter().map(|s| s.labeled).sum();
    let state = if saving {
        t!("lab-saving")
    } else {
        t!("lab-all-saved")
    };
    let progress = format!(
        "{} · {state}",
        t!("lab-progress", done : done, total : count)
    );
    // First picture still to label, source by source: where work resumes.
    let todo = srcs
        .iter()
        .flat_map(|s| s.frames.iter().map(|f| f.0))
        .find(|i| list.get(*i).is_some_and(|img| !img.annotated()));
    let navigator = use_navigator();
    // One screen at a time: pictures, then labeling.
    let now = step().unwrap_or(if count == 0 {
        Step::Images
    } else {
        Step::Label
    });
    // The metro line: pictures → labeling → training (the dialog).
    let stations = vec![
        Station {
            label: t!("lab-step-images").to_string(),
            done: count > 0,
            testid: Some("lab-step"),
        },
        Station {
            label: t!("lab-step-label").to_string(),
            done: count > 0 && done == count,
            testid: Some("lab-step"),
        },
        Station {
            label: t!("lab-step-train").to_string(),
            done: false,
            testid: Some("lab-step"),
        },
    ];
    let station = match now {
        Step::Images => 0,
        Step::Label => 1,
    };
    let open_source = {
        let srcs = srcs.clone();
        let list = list.clone();
        move |k: usize| {
            // Opens a source where its labeling stopped.
            let Some(s) = srcs.get(k) else {
                return;
            };
            let at = s
                .frames
                .iter()
                .map(|f| f.0)
                .find(|i| list.get(*i).is_some_and(|img| !img.annotated()))
                .or(s.frames.first().map(|f| f.0));
            if let Some(i) = at {
                playing.set(false);
                selected.set(i);
                selected_box.set(None);
                step.set(Some(Step::Label));
            }
        }
    };

    rsx! {
        ListLayout {
            title,
            subtitle: Some(progress),
            on_back: move |_| {
                navigator.push(Route::Lab {});
            },
            can_write,
            // An empty project shows its own big button instead.
            add_label: (now == Step::Images && count > 0).then(|| t!("lab-add-images").to_string()),
            on_add: move |_| adding.set(true),
            title_aside: rsx! {
                Stepper {
                    label: t!("lab-steps").to_string(),
                    stations,
                    current: station,
                    on_pick: move |k: usize| {
                        match k {
                            0 => step.set(Some(Step::Images)),
                            1 => {
                                // Labeling opens where the work stopped.
                                if now != Step::Label {
                                    if let Some(i) = todo {
                                        selected.set(i);
                                        selected_box.set(None);
                                    }
                                }
                                step.set(Some(Step::Label));
                            }
                            _ => training.set(true),
                        }
                    },
                }
            },
            actions: rsx! {
                if can_write {
                    button {
                        class: BTN_SECONDARY,
                        r#type: "button",
                        onclick: move |_| editing.set(true),
                        {t!("lab-edit-short")}
                    }
                }
                button {
                    class: BTN_TRAIN,
                    r#type: "button",
                    "data-testid": "lab-train-open",
                    onclick: move |_| training.set(true),
                    icons::Dumbbell { class: "h-4 w-4" }
                    {t!("lab-train-button")}
                }
            },
            if classes.is_empty() && project_now.is_some() {
                p { class: "mb-4 rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800",
                    {t!("lab-no-class")}
                }
            }
            if now == Step::Images {
                SourceList {
                    sources: srcs.clone(),
                    loaded: loaded(),
                    can_write,
                    on_open: open_source.clone(),
                    on_add: move |_| adding.set(true),
                }
            }
            if now == Step::Label {
                div { class: "outline-none", tabindex: "0", onkeydown: on_key,
                    div { class: "min-w-0 space-y-3",
                        if current.is_some() {
                            p { class: "text-sm text-slate", {t!("lab-label-help")} }
                        }
                        if let Some(img) = current.clone() {
                            Annotator {
                                image: img.clone(),
                                classes: classes.clone(),
                                class: class(),
                                can_write: can_write && !classes.is_empty(),
                                can_add: can_write,
                                zoom: zoom(),
                                on_zoom: move |z: f32| zoom.set(z),
                                hand: hand(),
                                on_hand: move |h: bool| hand.set(h),
                                on_pick_class: move |c: u32| pick_class(c),
                                on_add_class: {
                                    let pid = pid.clone();
                                    let project = project_now.clone();
                                    move |name: String| {
                                        let Some(p) = project.clone() else {
                                            return;
                                        };
                                        let (pid, at) = (pid.clone(), p.classes.len() as u32);
                                        let mut classes = p.classes.clone();
                                        classes.push(name);
                                        let input = pnex_core::lab::LabProjectInput {
                                            name: p.name.clone(),
                                            description: p.description.clone(),
                                            classes,
                                        };
                                        spawn(async move {
                                            match api::lab::update(&pid, &input).await {
                                                Ok(_) => {
                                                    class.set(at);
                                                    reload.with_mut(|r| *r += 1);
                                                }
                                                Err(e) => toasts::error(e),
                                            }
                                        });
                                    }
                                },
                                selected_box: selected_box(),
                                suggestions: shown.clone(),
                                on_accept: move |i: usize| accept(vec![i]),
                                on_select_box: move |b: Option<usize>| selected_box.set(b),
                                on_add_box: move |bbox: [f32; 4]| {
                                    let c = *class.peek();
                                    edit(
                                        Box::new(move |img: &mut LabImage| {
                                            img.negative = false;
                                            img.boxes.push(LabBox { class: c, bbox });
                                        }),
                                    );
                                },
                            }
                            ImageTools {
                                image: img,
                                can_write,
                                selected_box: selected_box(),
                                has_prev: tl::step(&srcs, index, -1).is_some(),
                                has_next: tl::step(&srcs, index, 1).is_some(),
                                on_prev: move |_| go(-1),
                                on_next: move |_| go(1),
                                on_remove_box: move |_| {
                                    let sel = *selected_box.peek();
                                    if let Some(b) = sel {
                                        selected_box.set(None);
                                        edit(
                                            Box::new(move |img: &mut LabImage| {
                                                if b < img.boxes.len() {
                                                    img.boxes.remove(b);
                                                }
                                            }),
                                        );
                                    }
                                },
                                on_negative: move |on: bool| {
                                    edit(
                                        Box::new(move |img: &mut LabImage| {
                                            img.negative = on;
                                            if on {
                                                img.boxes.clear();
                                            }
                                        }),
                                    );
                                },
                                on_split: move |s: LabSplit| edit(Box::new(move |img: &mut LabImage| img.split = s)),
                                on_remove_image: move |_| {
                                    let Some(id) = images.peek().get(*selected.peek()).map(|i| i.id.clone()) else {
                                        return;
                                    };
                                    let done = t!("lab-image-removed").to_string();
                                    spawn(async move {
                                        match api::lab::remove_image(&id).await {
                                            Ok(_) => {
                                                toasts::success(done);
                                                selected_box.set(None);
                                                reload.with_mut(|r| *r += 1);
                                            }
                                            Err(e) => toasts::error(e),
                                        }
                                    });
                                },
                            }
                            LabTimeline {
                                list: list.clone(),
                                classes: classes.clone(),
                                sources: srcs.clone(),
                                selected: index,
                                playing: playing(),
                                speed: speed(),
                                on_select: move |i: usize| {
                                    selected.set(i);
                                    selected_box.set(None);
                                },
                                on_source: open_source.clone(),
                                on_play: move |on: bool| playing.set(on),
                                on_speed: move |v: u32| speed.set(v),
                            }
                            if can_write && !classes.is_empty() {
                                details { class: "rounded-lg border border-line bg-surface px-4 py-3",
                                    summary { class: "cursor-pointer text-sm font-medium text-ink",
                                        {t!("lab-auto-title")}
                                    }
                                    p { class: "mt-1 text-sm text-slate", {t!("lab-auto-help")} }
                                    div { class: "mt-3",
                                        SuggestBar {
                                            models: model_options.clone(),
                                            model: model(),
                                            busy: suggesting(),
                                            count: shown.iter().filter(|s| s.class.is_some()).count(),
                                            total: shown.len(),
                                            can_interpolate: current.as_ref().is_some_and(|c| c.frame_ms.is_some()),
                                            on_model: move |m: String| model.set(m),
                                            on_suggest,
                                            on_interpolate,
                                            on_propagate,
                                            // Only the proposals whose label is a class: the
                                            // others are kept one by one, on purpose.
                                            on_keep_all: move |_| {
                                                let mapped: Vec<usize> = suggestions
                                                    .peek()
                                                    .1
                                                    .iter()
                                                    .enumerate()
                                                    .filter(|(_, s)| s.class.is_some())
                                                    .map(|(i, _)| i)
                                                    .collect();
                                                accept(mapped);
                                            },
                                            on_dismiss: move |_| suggestions.set((String::new(), Vec::new())),
                                        }
                                    }
                                }
                            }
                        } else if loaded() {
                            EmptyState {
                                can_write,
                                on_add: move |_| adding.set(true),
                            }
                        }
                    }
                }
            }
            if training() {
                Modal {
                    title: t!("lab-train-title").to_string(),
                    max_width: "max-w-2xl".to_string(),
                    on_close: move |_| training.set(false),
                    div { class: "space-y-4",
                        StatsPanel { classes: classes.clone(), stats: stats.clone() }
                        TrainingPanel {
                            project: pid.clone(),
                            can_write,
                            models: models.read().clone().unwrap_or_default(),
                            versions_tick: versions_tick(),
                            labeled: done,
                        }
                        details { class: "text-sm",
                            summary { class: "cursor-pointer px-1 text-slate hover:text-ink",
                                {t!("lab-versions-more")}
                            }
                            div { class: "mt-3",
                                VersionsPanel {
                                    project: pid.clone(),
                                    can_write,
                                    on_frozen: move |_| versions_tick.with_mut(|t| *t += 1),
                                }
                            }
                        }
                    }
                }
            }
            if adding() {
                AddChooser {
                    on_close: move |_| adding.set(false),
                    on_pick: move |k: AddFrom| {
                        adding.set(false);
                        match k {
                            AddFrom::Photos => picker_open.set(true),
                            AddFrom::Videos => video_open.set(true),
                            AddFrom::Recording => recording_open.set(true),
                        }
                    },
                }
            }
            if picker_open() {
                PhotoPicker {
                    present,
                    on_close: move |_| picker_open.set(false),
                    on_add: {
                        let pid = pid.clone();
                        move |ids: Vec<String>| {
                            picker_open.set(false);
                            let pid = pid.clone();
                            let done = t!("lab-images-added").to_string();
                            spawn(async move {
                                match api::lab::add_images(&pid, &ids).await {
                                    Ok(_) => {
                                        toasts::success(done);
                                        reload.with_mut(|r| *r += 1);
                                    }
                                    Err(e) => toasts::error(e),
                                }
                            });
                        }
                    },
                }
            }
            if video_open() {
                VideoImport {
                    project: pid.clone(),
                    on_close: move |_| video_open.set(false),
                    on_done: move |_| {
                        video_open.set(false);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if recording_open() {
                RecordingImport {
                    project: pid.clone(),
                    on_close: move |_| recording_open.set(false),
                    on_done: move |_| {
                        recording_open.set(false);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
            if editing() {
                ProjectForm {
                    initial: project_now.clone(),
                    on_close: move |_| editing.set(false),
                    on_saved: move |_| {
                        editing.set(false);
                        reload.with_mut(|r| *r += 1);
                    },
                }
            }
        }
    }
}

/// The picture with its boxes: drag on the picture draws a box of the
/// current class, a click on a box selects it. Over the picture, kept
/// discreet so they never hide what is outlined: "+" (a new class) top
/// left, the zoom top right; a zoomed picture scrolls inside its frame.
#[component]
fn Annotator(
    image: LabImage,
    classes: Vec<String>,
    class: u32,
    can_write: bool,
    /// May create classes.
    can_add: bool,
    zoom: f32,
    on_zoom: EventHandler<f32>,
    /// Hand mode (only while zoomed): a drag scrolls the picture.
    hand: bool,
    on_hand: EventHandler<bool>,
    on_pick_class: EventHandler<u32>,
    on_add_class: EventHandler<String>,
    selected_box: Option<usize>,
    suggestions: Vec<LabSuggestion>,
    on_accept: EventHandler<usize>,
    on_select_box: EventHandler<Option<usize>>,
    on_add_box: EventHandler<[f32; 4]>,
) -> Element {
    // Not keyed by picture: the same nodes follow the picture, so moving
    // to the next one never rebuilds the block (which scrolled the page up).
    let asset = image.asset_id.clone();
    let url = use_resource(use_reactive((&asset,), |(asset,)| async move {
        crate::util::media_blob_url(&api::media::content_path(&asset), None).await
    }));
    let mut layer = use_signal(|| None::<Rc<MountedData>>);
    // Layer rect in client pixels (left, top, width, height), read on press.
    let mut rect = use_signal(|| None::<(f64, f64, f64, f64)>);
    let mut start = use_signal(|| None::<(f32, f32)>);
    let mut now = use_signal(|| None::<(f32, f32)>);

    let aspect = format!(
        "aspect-ratio: {} / {};",
        image.width.max(1),
        image.height.max(1)
    );
    // The whole labeling screen fits the window: no page scroll between
    // pictures.
    // The frame takes the picture's shape (no empty band beside a tall
    // picture): as wide as the column allows at the height cap.
    let frame = format!(
        "{aspect} width: min(100%, calc(max(16rem, 100vh - 22rem) * {} / {}));",
        image.width.max(1),
        image.height.max(1)
    );
    let content = format!("{aspect} height: {}%;", zoom * 100.0);
    let zoom_to = move |z: f32| on_zoom.call(z.clamp(ZOOM_MIN, ZOOM_MAX));
    let panning = hand && zoom > ZOOM_MIN;
    let mut scroller = use_signal(|| None::<Rc<MountedData>>);
    // Pointer and scroll offset when the hand grabbed the picture.
    let mut grab = use_signal(|| None::<(f64, f64, f64, f64)>);
    let on_grab = move |e: PointerEvent| async move {
        // Never reaches the layer below: no box is started.
        e.stop_propagation();
        let Some(el) = scroller.cloned() else {
            return;
        };
        let Ok(o) = el.get_scroll_offset().await else {
            return;
        };
        let c = e.client_coordinates();
        grab.set(Some((c.x, c.y, o.x, o.y)));
    };
    let on_drag = move |e: PointerEvent| {
        let (Some((x0, y0, sx, sy)), Some(el)) = (*grab.peek(), scroller.cloned()) else {
            return;
        };
        let c = e.client_coordinates();
        let to = dioxus::html::geometry::PixelsVector2D::new(sx - (c.x - x0), sy - (c.y - y0));
        spawn(async move {
            let _ = el.scroll(to, ScrollBehavior::Instant).await;
        });
    };
    let norm = move |x: f64, y: f64| -> Option<(f32, f32)> {
        let (l, t, w, h) = (*rect.peek())?;
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some((
            ((x - l) / w).clamp(0.0, 1.0) as f32,
            ((y - t) / h).clamp(0.0, 1.0) as f32,
        ))
    };
    let on_down = move |e: PointerEvent| async move {
        if !can_write {
            return;
        }
        let Some(el) = layer.cloned() else {
            return;
        };
        let Ok(r) = el.get_client_rect().await else {
            return;
        };
        rect.set(Some((r.origin.x, r.origin.y, r.size.width, r.size.height)));
        let c = e.client_coordinates();
        if let Some(p) = norm(c.x, c.y) {
            start.set(Some(p));
            now.set(Some(p));
            on_select_box.call(None);
        }
    };
    let on_move = move |e: PointerEvent| {
        if start.peek().is_none() {
            return;
        }
        let c = e.client_coordinates();
        if let Some(p) = norm(c.x, c.y) {
            now.set(Some(p));
        }
    };
    let mut finish = move || {
        let (Some(a), Some(b)) = (*start.peek(), *now.peek()) else {
            start.set(None);
            return;
        };
        start.set(None);
        now.set(None);
        let bbox = rect_of(a, b);
        if bbox[2] >= MIN_BOX && bbox[3] >= MIN_BOX {
            on_add_box.call(bbox);
        }
    };
    let drawing = match (start(), now()) {
        (Some(a), Some(b)) => Some(rect_of(a, b)),
        _ => None,
    };
    let draft_style = drawing.map(|b| box_style(&b, class_color(class)));
    let src = url.read().clone().flatten();
    let cursor = if can_write {
        "cursor-crosshair"
    } else {
        "cursor-default"
    };

    rsx! {
        div { class: "relative mx-auto", style: "{frame}",
            div {
                class: "absolute inset-0 overflow-auto rounded-lg border border-line bg-rail select-none touch-none",
                onmounted: move |e| scroller.set(Some(e.data())),
                div { class: "relative mx-auto", style: "{content}",
                    if let Some(src) = src {
                        img {
                            class: "absolute inset-0 w-full h-full pointer-events-none",
                            src: "{src}",
                            alt: "{image.name}",
                            draggable: "false",
                        }
                    } else {
                        Gerbe { size: 36 }
                    }
                    div {
                        class: "absolute inset-0 {cursor}",
                        onmounted: move |e| layer.set(Some(e.data())),
                        onpointerdown: on_down,
                        onpointermove: on_move,
                        onpointerup: move |_| finish(),
                        onpointerleave: move |_| finish(),
                        for (i, b) in image.boxes.iter().enumerate() {
                            BoxView {
                                key: "{i}",
                                index: i,
                                bbox: b.bbox,
                                color: class_color(b.class).to_string(),
                                label: classes.get(b.class as usize).cloned().unwrap_or_default(),
                                selected: selected_box == Some(i),
                                on_select: move |i: usize| on_select_box.call(Some(i)),
                            }
                        }
                        for (i, sug) in suggestions.iter().enumerate() {
                            SuggestionView {
                                key: "s{i}",
                                index: i,
                                suggestion: sug.clone(),
                                class_name: sug.class.and_then(|c| classes.get(c as usize).cloned()),
                                on_accept,
                            }
                        }
                        if let Some(style) = draft_style {
                            div {
                                class: "absolute border-2 border-dashed pointer-events-none",
                                style: "{style}",
                            }
                        }
                        if panning {
                            div {
                                class: "absolute inset-0 z-10 cursor-grab active:cursor-grabbing",
                                "data-testid": "lab-pan",
                                onpointerdown: on_grab,
                                onpointermove: on_drag,
                                onpointerup: move |_| grab.set(None),
                                onpointerleave: move |_| grab.set(None),
                            }
                        }
                    }
                }
            }
            if can_add {
                div { class: "absolute left-2 top-2 z-10",
                    ClassAdd {
                        classes: classes.clone(),
                        current: class,
                        on_pick: on_pick_class,
                        on_add: on_add_class,
                    }
                }
            }
            div { class: "absolute right-2 top-2 z-10 flex items-center gap-1 rounded-full bg-ink/40 p-1 text-white opacity-70 backdrop-blur-sm transition-opacity duration-150 hover:opacity-100",
                button {
                    class: ZOOM_BTN,
                    r#type: "button",
                    title: t!("lab-zoom-out"),
                    "aria-label": t!("lab-zoom-out"),
                    disabled: zoom <= ZOOM_MIN,
                    onclick: move |_| zoom_to(zoom / ZOOM_STEP),
                    icons::ZoomOut { class: "h-4 w-4" }
                }
                button {
                    class: "min-w-12 rounded-full px-1 text-xs font-medium tabular-nums hover:bg-white/20",
                    r#type: "button",
                    title: t!("lab-zoom-fit"),
                    "data-testid": "lab-zoom-fit",
                    onclick: move |_| zoom_to(1.0),
                    "{(zoom * 100.0).round()} %"
                }
                button {
                    class: ZOOM_BTN,
                    r#type: "button",
                    title: t!("lab-zoom-in"),
                    "aria-label": t!("lab-zoom-in"),
                    "data-testid": "lab-zoom-in",
                    disabled: zoom >= ZOOM_MAX,
                    onclick: move |_| zoom_to(zoom * ZOOM_STEP),
                    icons::ZoomIn { class: "h-4 w-4" }
                }
                if zoom > ZOOM_MIN {
                    button {
                        class: if hand { "flex h-7 w-7 items-center justify-center rounded-full bg-white/30" } else { ZOOM_BTN },
                        r#type: "button",
                        title: t!("lab-zoom-hand"),
                        "aria-label": t!("lab-zoom-hand"),
                        "aria-pressed": hand,
                        "data-testid": "lab-zoom-hand",
                        onclick: move |_| on_hand.call(!hand),
                        icons::Pan { class: "h-4 w-4" }
                    }
                    button {
                        class: ZOOM_BTN,
                        r#type: "button",
                        title: t!("lab-zoom-fit"),
                        "aria-label": t!("lab-zoom-fit"),
                        "data-testid": "lab-zoom-whole",
                        onclick: move |_| zoom_to(1.0),
                        icons::Maximize { class: "h-4 w-4" }
                    }
                }
            }
        }
    }
}

/// Zoom of the picture: bounds and step of one click.
const ZOOM_MIN: f32 = 1.0;
const ZOOM_MAX: f32 = 6.0;
const ZOOM_STEP: f32 = 1.5;
const ZOOM_BTN: &str =
    "flex h-7 w-7 items-center justify-center rounded-full hover:bg-white/20 disabled:opacity-40";

/// The round "+" of the picture: a bubble with the classes (a click picks
/// the class of the next box) and a field for a new one (Enter adds, Escape
/// closes). The bubble's keys never reach the labeling shortcuts, and
/// pressing it never starts a box.
#[component]
fn ClassAdd(
    classes: Vec<String>,
    current: u32,
    on_pick: EventHandler<u32>,
    on_add: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut typing = use_signal(String::new);
    let mut submit = move || {
        let name = typing().trim().to_string();
        typing.set(String::new());
        if !name.is_empty() {
            open.set(false);
            on_add.call(name);
        }
    };
    let shown = open();
    rsx! {
        div {
            class: "flex items-start gap-2",
            onpointerdown: move |e: PointerEvent| e.stop_propagation(),
            onkeydown: move |e: KeyboardEvent| e.stop_propagation(),
            button {
                class: if shown { "flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-signal text-white shadow-lg" } else { "flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-signal/60 text-white shadow-lg backdrop-blur-sm transition-colors duration-150 hover:bg-signal active:scale-[0.97]" },
                r#type: "button",
                title: t!("lab-class-add"),
                "aria-label": t!("lab-class-add"),
                "aria-expanded": shown,
                "data-testid": "lab-class-add",
                onclick: move |_| open.set(!shown),
                icons::Plus { class: if shown { "h-5 w-5 rotate-45 transition-transform duration-150 ease-out-ui" } else { "h-5 w-5 transition-transform duration-150 ease-out-ui" } }
            }
            if shown {
                div {
                    class: "w-56 space-y-1 rounded-lg border border-line bg-surface p-1.5 shadow-lg",
                    "data-testid": "lab-class-menu",
                    for (i, name) in classes.iter().enumerate() {
                        button {
                            key: "{i}",
                            class: if i as u32 == current { "flex w-full items-center gap-2 rounded-md bg-signal/10 px-2 py-1.5 text-left text-sm font-medium text-ink" } else { "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm text-ink hover:bg-rail" },
                            r#type: "button",
                            "aria-pressed": i as u32 == current,
                            onclick: move |_| {
                                open.set(false);
                                on_pick.call(i as u32);
                            },
                            span {
                                class: "h-3 w-3 shrink-0 rounded-full",
                                style: "background: {class_color(i as u32)}",
                            }
                            span { class: "flex-1 truncate", "{name}" }
                            if i < 9 {
                                span { class: "font-mono text-xs text-slate", "{i + 1}" }
                            }
                        }
                    }
                    input {
                        class: "{INPUT} w-full",
                        "aria-label": t!("lab-class-new"),
                        placeholder: t!("lab-class-new"),
                        autofocus: true,
                        value: "{typing}",
                        oninput: move |e| typing.set(e.value()),
                        onkeydown: move |e: KeyboardEvent| match e.key() {
                            Key::Enter => submit(),
                            Key::Escape => open.set(false),
                            _ => {}
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn BoxView(
    index: usize,
    bbox: [f32; 4],
    color: String,
    label: String,
    selected: bool,
    on_select: EventHandler<usize>,
) -> Element {
    let style = box_style(&bbox, &color);
    let class = if selected {
        "absolute border-2 ring-2 ring-white shadow-lg"
    } else {
        "absolute border-2"
    };
    rsx! {
        div {
            class,
            style: "{style}",
            onpointerdown: move |e: PointerEvent| {
                e.stop_propagation();
                on_select.call(index);
            },
            span {
                class: "absolute left-0 -top-5 px-1 rounded text-xs font-medium text-white whitespace-nowrap",
                style: "background: {color};",
                "{label}"
            }
        }
    }
}

/// A proposal of the model: dashed, click to keep it as a box.
#[component]
fn SuggestionView(
    index: usize,
    suggestion: LabSuggestion,
    class_name: Option<String>,
    on_accept: EventHandler<usize>,
) -> Element {
    let color = suggestion.class.map(class_color).unwrap_or("#6b7280");
    let style = box_style(&suggestion.bbox, color);
    let percent = (suggestion.score * 100.0).round();
    // An empty label = an interpolated box, not a model proposal.
    let label = match class_name {
        Some(c) if suggestion.label.is_empty() => format!("{c} · {}", t!("lab-interpolated")),
        Some(c) => format!("{c} ← {} {percent}%", suggestion.label),
        None => format!("{} {percent}%", suggestion.label),
    };
    let title = t!("lab-suggest-keep-one").to_string();
    rsx! {
        div {
            class: "absolute border-2 border-dashed cursor-pointer hover:bg-surface/20",
            style: "{style}",
            title: "{title}",
            onpointerdown: move |e: PointerEvent| {
                e.stop_propagation();
                on_accept.call(index);
            },
            span {
                class: "absolute left-0 -bottom-5 px-1 rounded text-xs text-white whitespace-nowrap opacity-90",
                style: "background: {color};",
                "{label}"
            }
        }
    }
}

/// Pre-annotation: a model of the registry proposes boxes (§4.6.1 step 4).
#[component]
fn SuggestBar(
    models: Vec<(String, String)>,
    model: String,
    busy: bool,
    count: usize,
    total: usize,
    /// The image has a position in its source (video, recording).
    can_interpolate: bool,
    on_model: EventHandler<String>,
    on_suggest: EventHandler<()>,
    on_interpolate: EventHandler<()>,
    on_propagate: EventHandler<()>,
    on_keep_all: EventHandler<()>,
    on_dismiss: EventHandler<()>,
) -> Element {
    // `count` = proposals whose label matches a class.
    let keep_all = t!("lab-suggest-keep-all", count : count).to_string();
    rsx! {
        div { class: "flex flex-wrap items-center gap-2 rounded-lg border border-line bg-surface p-2",
            span { class: "text-xs text-slate", {t!("lab-suggest-label")} }
            select {
                class: "px-2 py-1 border border-line-strong rounded-lg text-sm max-w-56",
                aria_label: t!("lab-suggest-model").to_string(),
                onchange: move |e| on_model.call(e.value()),
                option { value: "", selected: model.is_empty(), {t!("lab-suggest-model")} }
                for (id, name) in models {
                    option { key: "{id}", value: "{id}", selected: id == model, "{name}" }
                }
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: model.is_empty() || busy,
                onclick: move |_| on_suggest.call(()),
                {t!("lab-suggest")}
            }
            if can_interpolate {
                button {
                    class: BTN_SECONDARY_SM,
                    title: t!("lab-propagate-help").to_string(),
                    r#type: "button",
                    disabled: model.is_empty() || busy,
                    onclick: move |_| on_propagate.call(()),
                    {t!("lab-propagate")}
                }
                button {
                    class: BTN_SECONDARY_SM,
                    title: t!("lab-interpolate-help").to_string(),
                    r#type: "button",
                    disabled: busy,
                    onclick: move |_| on_interpolate.call(()),
                    {t!("lab-interpolate")}
                }
            }
            if count > 0 {
                button {
                    class: BTN_PRIMARY_SM,
                    title: t!("lab-suggest-keep-all-help").to_string(),
                    r#type: "button",
                    onclick: move |_| on_keep_all.call(()),
                    "{keep_all}"
                }
            }
            if total > 0 {
                button {
                    class: "text-xs text-slate hover:underline",
                    r#type: "button",
                    onclick: move |_| on_dismiss.call(()),
                    {t!("lab-suggest-dismiss")}
                }
            }
        }
    }
}
/// Under the picture: previous / next, "nothing here", removal of the
/// selected box; the rarer settings (training / validation side, removal
/// from the project) under "More".
#[component]
fn ImageTools(
    image: LabImage,
    can_write: bool,
    selected_box: Option<usize>,
    has_prev: bool,
    has_next: bool,
    on_prev: EventHandler<()>,
    on_next: EventHandler<()>,
    on_remove_box: EventHandler<()>,
    on_negative: EventHandler<bool>,
    on_split: EventHandler<LabSplit>,
    on_remove_image: EventHandler<()>,
) -> Element {
    let effective = match image.effective_split() {
        LabSplit::Val => t!("lab-split-val").to_string(),
        _ => t!("lab-split-train").to_string(),
    };
    let split_value = match image.split {
        LabSplit::Auto => "auto",
        LabSplit::Train => "train",
        LabSplit::Val => "val",
    };
    let negative = image.negative;
    rsx! {
        div { class: "{PANEL} flex flex-wrap items-center gap-2 px-3 py-2",
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: !has_prev,
                onclick: move |_| on_prev.call(()),
                icons::ChevronLeft { class: "h-4 w-4" }
                {t!("lab-prev-image")}
            }
            button {
                class: BTN_PRIMARY_SM,
                r#type: "button",
                disabled: !has_next,
                "data-testid": "lab-next",
                onclick: move |_| on_next.call(()),
                {t!("lab-next-image")}
                icons::ChevronRight { class: "h-4 w-4" }
            }
            if can_write {
                span { class: "mx-1 h-6 w-px bg-line", "aria-hidden": "true" }
                button {
                    class: if negative { "inline-flex items-center gap-1.5 rounded-md border border-ink bg-ink px-3 py-1.5 text-sm font-medium text-white" } else { BTN_SECONDARY_SM },
                    r#type: "button",
                    "aria-pressed": negative,
                    title: t!("lab-negative-help"),
                    onclick: move |_| on_negative.call(!negative),
                    {t!("lab-negative")}
                }
                if selected_box.is_some() {
                    button {
                        class: "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm text-alarm hover:bg-red-50",
                        r#type: "button",
                        onclick: move |_| on_remove_box.call(()),
                        {t!("lab-remove-box")}
                    }
                }
                details { class: "relative ml-auto",
                    summary { class: "{BTN_SECONDARY_SM} cursor-pointer list-none", {t!("lab-more")} }
                    div { class: "absolute right-0 z-10 mt-1 w-72 space-y-3 rounded-lg border border-line bg-surface p-3 shadow-lg",
                        label { class: "block",
                            span { class: FIELD_LABEL, {t!("lab-split")} }
                            select {
                                class: "{INPUT} w-full",
                                onchange: move |e| on_split.call(split_of(&e.value())),
                                option {
                                    value: "auto",
                                    selected: split_value == "auto",
                                    {t!("lab-split-auto", side : effective.clone())}
                                }
                                option {
                                    value: "train",
                                    selected: split_value == "train",
                                    {t!("lab-split-train")}
                                }
                                option {
                                    value: "val",
                                    selected: split_value == "val",
                                    {t!("lab-split-val")}
                                }
                            }
                            span { class: "mt-1 block text-xs text-slate", {t!("lab-split-help")} }
                        }
                        p { class: "text-xs text-slate", {t!("lab-keys-help")} }
                        button {
                            class: "text-sm text-red-600 hover:underline",
                            r#type: "button",
                            onclick: move |_| on_remove_image.call(()),
                            {t!("lab-remove-image")}
                        }
                    }
                }
            }
        }
    }
}

fn split_of(v: &str) -> LabSplit {
    match v {
        "train" => LabSplit::Train,
        "val" => LabSplit::Val,
        _ => LabSplit::Auto,
    }
}

/// "Ready to train?": one line per class with a tick once it has enough
/// pictures, then the first advice in sight and the others on demand
/// (§4.6.2, indicative floors: never blocking).
#[component]
fn StatsPanel(classes: Vec<String>, stats: lab::LabStats) -> Element {
    let rows: Vec<(u32, String, usize)> = classes
        .iter()
        .enumerate()
        .map(|(i, c)| {
            (
                i as u32,
                c.clone(),
                stats.images_per_class.get(i).copied().unwrap_or(0),
            )
        })
        .collect();
    let warnings: Vec<String> = stats.warnings.iter().map(warning_text).collect();
    let min = lab::LAB_MIN_PER_CLASS;
    rsx! {
        section { class: "rounded-lg border border-line bg-surface p-4 space-y-3",
            h2 { class: "text-base font-semibold text-ink", {t!("lab-ready-title")} }
            p { class: "text-sm text-slate", {t!("lab-ready-help", min : min)} }
            ul { class: "space-y-1.5",
                for (i, name, imgs) in rows {
                    li { key: "{i}", class: "flex items-center gap-2 text-sm",
                        span {
                            class: if imgs >= min { "w-5 text-center font-bold text-emerald-600" } else { "w-5 text-center font-bold text-amber-600" },
                            "aria-hidden": "true",
                            if imgs >= min {
                                "✓"
                            } else {
                                "•"
                            }
                        }
                        span {
                            class: "h-2.5 w-2.5 shrink-0 rounded-full",
                            style: "background: {class_color(i)};",
                        }
                        span { class: "flex-1 truncate text-ink", "{name}" }
                        span { class: "font-mono text-xs text-slate", "{imgs} / {min}" }
                    }
                }
            }
            if !warnings.is_empty() {
                details { class: "text-sm",
                    summary { class: "cursor-pointer text-slate hover:text-ink",
                        {t!("lab-more-advice", count : warnings.len())}
                    }
                    ul { class: "mt-2 list-disc space-y-1 pl-5 text-amber-800",
                        for (i, w) in warnings.iter().enumerate() {
                            li { key: "{i}", "{w}" }
                        }
                    }
                }
            }
        }
    }
}

/// Step "Pictures": the sources of the project (each video, recording or
/// batch of photos), how far each is labeled, and the way to add more.
#[component]
fn SourceList(
    sources: Vec<tl::Source>,
    loaded: bool,
    can_write: bool,
    on_open: EventHandler<usize>,
    on_add: EventHandler<()>,
) -> Element {
    if sources.is_empty() {
        return rsx! {
            if loaded {
                EmptyState { can_write, on_add }
            }
        };
    }
    rsx! {
        div { class: "rounded-lg border border-line bg-surface",
            table { class: "w-full text-sm",
                thead {
                    tr { class: "border-b border-line text-left text-xs text-slate",
                        th { class: "px-4 py-2 font-medium", {t!("lab-source")} }
                        th { class: "px-4 py-2 font-medium", {t!("lab-tl-images")} }
                        th { class: "px-4 py-2 font-medium", {t!("lab-source-done")} }
                        th { class: "px-4 py-2" }
                    }
                }
                tbody {
                    for (k, s) in sources.iter().enumerate() {
                        tr {
                            key: "{s.key}",
                            class: "border-b border-line last:border-0",
                            td { class: "px-4 py-3",
                                span { class: "block font-medium text-ink", "{s.name}" }
                                span { class: "block text-xs text-slate", {tl::kind_label(s.kind)} }
                            }
                            td { class: "px-4 py-3 font-mono text-slate", "{s.frames.len()}" }
                            td { class: "px-4 py-3",
                                div { class: "flex items-center gap-2",
                                    div { class: "h-2 w-32 overflow-hidden rounded-full bg-gray-100",
                                        div {
                                            class: "h-full bg-emerald-500",
                                            style: "width: {s.labeled * 100 / s.frames.len().max(1)}%",
                                        }
                                    }
                                    span { class: "font-mono text-xs text-slate",
                                        "{s.labeled} / {s.frames.len()}"
                                    }
                                }
                            }
                            td { class: "px-4 py-3 text-right",
                                button {
                                    class: BTN_SECONDARY_SM,
                                    r#type: "button",
                                    "data-testid": "lab-source-open",
                                    onclick: move |_| on_open.call(k),
                                    {t!("lab-source-label")}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// No picture yet: what to do, and the button to do it.
#[component]
fn EmptyState(can_write: bool, on_add: EventHandler<()>) -> Element {
    rsx! {
        div { class: "rounded-lg border border-dashed border-line-strong bg-surface p-10 text-center space-y-3",
            p { class: "text-base font-medium text-ink", {t!("lab-no-image")} }
            p { class: "text-sm text-slate", {t!("lab-no-image-help")} }
            if can_write {
                button {
                    class: BTN_PRIMARY,
                    r#type: "button",
                    onclick: move |_| on_add.call(()),
                    {t!("lab-add-images")}
                }
            }
        }
    }
}

fn warning_text(w: &lab::LabWarning) -> String {
    let class = w.class.clone().unwrap_or_default();
    match w.code.as_str() {
        "lab-warn-class-few" => {
            t!("lab-warn-class-few", class : class, min : lab::LAB_MIN_PER_CLASS).to_string()
        }
        "lab-warn-val-few" => t!("lab-warn-val-few", min : lab::LAB_MIN_VAL_IMAGES).to_string(),
        "lab-warn-no-negative" => t!("lab-warn-no-negative").to_string(),
        "lab-warn-no-class" => t!("lab-warn-no-class").to_string(),
        "lab-warn-class-sources" => {
            t!("lab-warn-class-sources", class : class, min : lab::LAB_MIN_SOURCES).to_string()
        }
        "lab-warn-class-same-size" => t!("lab-warn-class-same-size", class : class).to_string(),
        "lab-warn-class-same-place" => t!("lab-warn-class-same-place", class : class).to_string(),
        "lab-warn-light-uniform" => t!("lab-warn-light-uniform").to_string(),
        other => other.to_string(),
    }
}

/// Frozen dataset versions: freeze the current annotations, download one.
#[component]
fn VersionsPanel(project: String, can_write: bool, on_frozen: EventHandler<()>) -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut note = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let pid = project.clone();
    let versions = use_resource(move || {
        let _ = reload();
        let pid = pid.clone();
        async move { api::lab::versions(&pid).await }
    });
    let list = versions
        .read()
        .clone()
        .and_then(Result::ok)
        .unwrap_or_default();
    let on_freeze = move |_| {
        let pid = project.clone();
        let n = note.peek().clone();
        let done = t!("lab-frozen").to_string();
        busy.set(true);
        spawn(async move {
            match api::lab::freeze(&pid, &n).await {
                Ok(_) => {
                    toasts::success(done);
                    note.set(String::new());
                    reload.with_mut(|r| *r += 1);
                    on_frozen.call(());
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        section { class: "rounded-lg border border-line bg-surface p-4 space-y-2",
            h2 { class: "text-sm font-semibold text-ink", {t!("lab-versions-title")} }
            p { class: "text-xs text-slate", {t!("lab-versions-help")} }
            if can_write {
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-version-note")} }
                    input {
                        class: "{INPUT} w-full",
                        maxlength: "500",
                        value: "{note}",
                        oninput: move |e| note.set(e.value()),
                    }
                }
                button {
                    class: BTN_PRIMARY_SM,
                    r#type: "button",
                    disabled: busy(),
                    onclick: on_freeze,
                    {t!("lab-freeze")}
                }
            }
            ul { class: "divide-y divide-gray-100",
                for v in list {
                    VersionRow { key: "{v.id}", version: v }
                }
            }
        }
    }
}

#[component]
fn VersionRow(version: lab::LabDatasetVersion) -> Element {
    let mut busy = use_signal(|| false);
    let title = t!("lab-version-label", n : version.version).to_string();
    let detail = t!(
        "lab-version-detail", images : version.stats.annotated, val : version.stats.val
    )
    .to_string();
    let date = version.created_at.get(..10).unwrap_or_default().to_string();
    let id = version.id.clone();
    let on_export = move |_| {
        let id = id.clone();
        let file = format!("lab-dataset-v{}.zip", version.version);
        busy.set(true);
        spawn(async move {
            match api::lab::export(&id).await {
                Ok(bytes) => crate::util::save_blob(&file, &bytes),
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    rsx! {
        li { class: "py-2 flex items-center gap-2",
            div { class: "flex-1 min-w-0",
                div { class: "text-sm font-medium text-ink", "{title}" }
                div { class: "text-xs text-slate", "{date} · {detail}" }
                if !version.note.is_empty() {
                    div { class: "text-xs text-slate truncate", "{version.note}" }
                }
            }
            button {
                class: BTN_SECONDARY_SM,
                r#type: "button",
                disabled: busy(),
                onclick: on_export,
                {t!("lab-export")}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_to_box() {
        let r = rect_of((0.5, 0.6), (0.1, 0.2));
        assert_eq!((r[0], r[1]), (0.1, 0.2));
        assert!((r[2] - 0.4).abs() < 1e-6 && (r[3] - 0.4).abs() < 1e-6);
        assert_eq!(split_of("val"), LabSplit::Val);
        assert_eq!(class_color(9), class_color(1));
    }
}

/// Where added pictures come from.
#[derive(Clone, Copy, PartialEq)]
enum AddFrom {
    Photos,
    Videos,
    Recording,
}

/// First step of "Add pictures": where they come from.
#[component]
fn AddChooser(on_close: EventHandler<()>, on_pick: EventHandler<AddFrom>) -> Element {
    let choices = [
        (
            AddFrom::Photos,
            t!("lab-add-photos"),
            t!("lab-add-photos-help"),
        ),
        (
            AddFrom::Videos,
            t!("lab-add-videos"),
            t!("lab-add-videos-help"),
        ),
        (
            AddFrom::Recording,
            t!("lab-add-recording"),
            t!("lab-add-recording-help"),
        ),
    ];
    rsx! {
        Modal {
            title: t!("lab-add-title").to_string(),
            max_width: "max-w-md".to_string(),
            on_close: move |_| on_close.call(()),
            div { class: "grid gap-2",
                for (k, title, help) in choices {
                    button {
                        r#type: "button",
                        class: "rounded-lg border border-line px-3 py-2.5 text-left hover:border-signal hover:bg-signal-soft",
                        onclick: move |_| on_pick.call(k),
                        span { class: "block text-sm font-medium text-ink", "{title}" }
                        span { class: "block text-xs text-slate", "{help}" }
                    }
                }
            }
        }
    }
}
