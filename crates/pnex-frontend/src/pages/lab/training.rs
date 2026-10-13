//! Training of a project (media-vision-studio.md D204): pick a frozen
//! version, a registered YOLOX model to start from and a number of epochs;
//! runs listed with their progress, refreshed while one is active.

use std::time::Duration;

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::lab::{
    LabRunStatus, LabTrainRequest, LabTrainingRun, LAB_TRAIN_DEFAULT_EPOCHS, LAB_TRAIN_MAX_EPOCHS,
};
use pnex_core::vision::{MlModel, VisionFamily, VisionTask};

use crate::api;
use crate::components::ui::{BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_LABEL, INPUT};
use crate::state::toasts;
use crate::Route;

/// Overall completion (0–100) of a run, for the progress bar.
fn percent(run: &LabTrainingRun) -> u32 {
    let p = &run.progress;
    let frac = |lo: f32, hi: f32| {
        let f = if p.total == 0 {
            0.0
        } else {
            p.done as f32 / p.total as f32
        };
        (lo + (hi - lo) * f.clamp(0.0, 1.0)) as u32
    };
    match (run.status, p.phase.as_str()) {
        (LabRunStatus::Succeeded, _) => 100,
        (_, "features") => frac(0.0, 30.0),
        (_, "epochs") => frac(30.0, 95.0),
        (_, "evaluating") => 95,
        _ => 0,
    }
}

/// Registry models a training can start from.
fn bases(models: &[MlModel]) -> Vec<(String, String)> {
    models
        .iter()
        .filter(|m| m.task == VisionTask::Detection && m.spec.family == VisionFamily::Yolox)
        .map(|m| (m.id.clone(), m.name.clone()))
        .collect()
}

#[component]
pub(super) fn TrainingPanel(
    project: String,
    can_write: bool,
    models: Vec<MlModel>,
    /// Bumped when a version is frozen.
    versions_tick: u32,
) -> Element {
    let mut reload = use_signal(|| 0u32);
    let pid = project.clone();
    let versions = use_resource(use_reactive!(|versions_tick| {
        let pid = pid.clone();
        let _ = versions_tick;
        async move { api::lab::versions(&pid).await.unwrap_or_default() }
    }));
    let pid = project.clone();
    let runs = use_resource(move || {
        let _ = reload();
        let pid = pid.clone();
        async move { api::lab::runs(&pid).await.unwrap_or_default() }
    });
    // Polls while a run is queued or running.
    use_future(move || async move {
        loop {
            crate::util::sleep(Duration::from_secs(3)).await;
            let active = runs
                .peek()
                .as_ref()
                .is_some_and(|l| l.iter().any(|r| r.status.active()));
            if active {
                reload.with_mut(|r| *r += 1);
            }
        }
    });
    let mut version = use_signal(String::new);
    let mut base = use_signal(String::new);
    let mut epochs = use_signal(|| LAB_TRAIN_DEFAULT_EPOCHS.to_string());
    let mut busy = use_signal(|| false);

    let version_list = versions.read().clone().unwrap_or_default();
    let base_list = bases(&models);
    let run_list = runs.read().clone().unwrap_or_default();
    // Latest version and first base model unless picked.
    let picked_version = if version().is_empty() {
        version_list
            .first()
            .map(|v| v.id.clone())
            .unwrap_or_default()
    } else {
        version()
    };
    let picked_base = if base().is_empty() {
        base_list.first().map(|b| b.0.clone()).unwrap_or_default()
    } else {
        base()
    };
    let epochs_v = epochs().trim().parse::<u32>().ok();
    let request = epochs_v.map(|e| LabTrainRequest {
        base_model_id: picked_base.clone(),
        epochs: e,
    });
    let valid = request.as_ref().is_some_and(|r| r.check().is_none())
        && !picked_version.is_empty()
        && !picked_base.is_empty();
    let train_version = picked_version.clone();
    let on_train = move |_| {
        let Some(req) = request.clone() else {
            return;
        };
        let vid = train_version.clone();
        let done = t!("lab-train-queued").to_string();
        busy.set(true);
        spawn(async move {
            match api::lab::train(&vid, &req).await {
                Ok(_) => {
                    toasts::success(done);
                    reload.with_mut(|r| *r += 1);
                }
                Err(e) => toasts::error(e),
            }
            busy.set(false);
        });
    };
    let max_epochs = LAB_TRAIN_MAX_EPOCHS.to_string();

    rsx! {
        section { class: "rounded-lg border border-gray-200 bg-white p-3 space-y-2",
            h2 { class: "text-sm font-semibold text-gray-900", {t!("lab-train-title")} }
            p { class: "text-xs text-gray-500", {t!("lab-train-help")} }
            if can_write {
                if base_list.is_empty() {
                    p { class: "text-xs text-amber-700", {t!("lab-train-no-base")} }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-train-version")} }
                    select {
                        class: "{INPUT} w-full",
                        onchange: move |e| version.set(e.value()),
                        for v in version_list.iter() {
                            option {
                                key: "{v.id}",
                                value: "{v.id}",
                                selected: v.id == picked_version,
                                {t!("lab-version-label", n : v.version)}
                            }
                        }
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-train-base")} }
                    select {
                        class: "{INPUT} w-full",
                        onchange: move |e| base.set(e.value()),
                        for (id, name) in base_list.iter() {
                            option {
                                key: "{id}",
                                value: "{id}",
                                selected: *id == picked_base,
                                "{name}"
                            }
                        }
                    }
                }
                label { class: "block",
                    span { class: FIELD_LABEL, {t!("lab-train-epochs")} }
                    input {
                        class: "{INPUT} w-full",
                        r#type: "number",
                        min: "1",
                        max: "{max_epochs}",
                        value: "{epochs}",
                        oninput: move |e| epochs.set(e.value()),
                    }
                }
                button {
                    class: BTN_PRIMARY_SM,
                    r#type: "button",
                    disabled: !valid || busy(),
                    onclick: on_train,
                    {t!("lab-train-start")}
                }
            }
            ul { class: "divide-y divide-gray-100",
                for run in run_list {
                    RunRow {
                        key: "{run.id}",
                        run,
                        can_write,
                        on_change: move |_| reload.with_mut(|r| *r += 1),
                    }
                }
            }
        }
    }
}

#[component]
fn RunRow(run: LabTrainingRun, can_write: bool, on_change: EventHandler<()>) -> Element {
    let title = format!(
        "{} · {}",
        t!("lab-version-label", n : run.dataset_version),
        run.base_model_name
    );
    let status = match run.status {
        LabRunStatus::Queued => t!("lab-run-queued"),
        LabRunStatus::Running => t!("lab-run-running"),
        LabRunStatus::Succeeded => t!("lab-run-succeeded"),
        LabRunStatus::Failed => t!("lab-run-failed"),
        LabRunStatus::Cancelled => t!("lab-run-cancelled"),
    };
    let p = &run.progress;
    let step = match (run.status, p.phase.as_str()) {
        (LabRunStatus::Running, "features") => {
            Some(t!("lab-run-features", done : p.done, total : p.total).to_string())
        }
        (LabRunStatus::Running, "epochs") => {
            let loss = p.loss.map(|l| format!("{l:.3}")).unwrap_or_default();
            Some(t!("lab-run-epoch", done : p.done, total : p.total, loss : loss).to_string())
        }
        (LabRunStatus::Running, "evaluating") => Some(t!("lab-run-evaluating").to_string()),
        _ => None,
    };
    let map = run
        .map50
        .map(|m| t!("lab-run-map", value : format!("{:.0} %", m * 100.0)).to_string());
    let error = run.error.as_ref().map(|code| {
        let args = serde_json::json!({ "detail": run.error_detail.clone().unwrap_or_default() });
        crate::api::error_i18n::localize_code(code, Some(&args), code)
    });
    let width = percent(&run);
    let active = run.status.active();
    let id = run.id.clone();
    let on_cancel = move |_| {
        let id = id.clone();
        spawn(async move {
            match api::lab::cancel_run(&id).await {
                Ok(_) => on_change.call(()),
                Err(e) => toasts::error(e),
            }
        });
    };
    rsx! {
        li { class: "py-2 space-y-1",
            div { class: "flex items-center gap-2",
                div { class: "flex-1 min-w-0 text-sm font-medium text-gray-900 truncate",
                    "{title}"
                }
                span { class: "text-xs text-gray-600", "{status}" }
            }
            if active {
                div { class: "h-1.5 rounded bg-gray-100 overflow-hidden",
                    div {
                        class: "h-full bg-signal-500 transition-all",
                        style: "width: {width}%",
                    }
                }
            }
            if let Some(step) = step {
                div { class: "text-xs text-gray-500", "{step}" }
            }
            if let Some(map) = map {
                div { class: "text-xs text-gray-700", "{map}" }
            }
            if let Some(error) = error {
                div { class: "text-xs text-red-700", "{error}" }
                if let Some(detail) = run.error_detail.clone() {
                    div { class: "text-xs text-gray-500 break-words", "{detail}" }
                }
            }
            div { class: "flex gap-2",
                if run.model_id.is_some() {
                    Link {
                        class: "text-xs text-signal-700 hover:underline",
                        to: Route::Models {},
                        {t!("lab-run-open-models")}
                    }
                }
                if active && can_write {
                    button {
                        class: BTN_SECONDARY_SM,
                        r#type: "button",
                        onclick: on_cancel,
                        {t!("common-cancel")}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pnex_core::lab::LabRunProgress;

    fn run(status: LabRunStatus, phase: &str, done: u32, total: u32) -> LabTrainingRun {
        LabTrainingRun {
            id: "r".into(),
            dataset_version_id: "v".into(),
            dataset_version: 1,
            base_model_id: None,
            base_model_name: String::new(),
            status,
            epochs: 10,
            progress: LabRunProgress {
                phase: phase.into(),
                done,
                total,
                loss: None,
            },
            map50: None,
            error: None,
            error_detail: None,
            model_id: None,
            created_at: String::new(),
            finished_at: None,
        }
    }

    #[test]
    fn progress_bar_follows_the_phases() {
        assert_eq!(percent(&run(LabRunStatus::Queued, "", 0, 0)), 0);
        assert_eq!(percent(&run(LabRunStatus::Running, "features", 5, 10)), 15);
        assert_eq!(percent(&run(LabRunStatus::Running, "epochs", 10, 10)), 95);
        assert_eq!(percent(&run(LabRunStatus::Succeeded, "done", 0, 0)), 100);
    }
}
