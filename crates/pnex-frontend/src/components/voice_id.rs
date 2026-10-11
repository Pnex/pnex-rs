//! Voice identification UI (voice-id.md): the "Voice" tab of a Person
//! object (consent → clips → generate → test, revoke) and the test box
//! shared with the `/voice-id` page. A match is shown as a proposal, the
//! human decides.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::media_ingest::AsrModel;
use pnex_core::voice_id::{
    VoiceProfileInput, VoiceProfileView, VoiceTestResult, VoiceVerdict, CONSENT_PROOF_MAX,
    CONSENT_SCOPE_MAX, STATUS_READY,
};

use crate::api;
use crate::components::confirm::ConfirmDialog;
use crate::components::ui::{
    BTN_DANGER, BTN_PRIMARY_SM, BTN_SECONDARY_SM, FIELD_HELP, FIELD_LABEL, INPUT, PANEL,
};
use crate::state::{org, toasts};

const SECTION: &str = "rounded-lg border border-line bg-surface p-4 space-y-3";
const AUDIO_ACCEPT: &str = "audio/*,.wav,.mp3,.ogg,.opus,.flac,.m4a";

/// Valid speaker embedding models of the org.
pub async fn embedding_models() -> Vec<AsrModel> {
    api::media_streams::models()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.task == "speaker_embedding" && m.check.status == "valid")
        .collect()
}

pub fn status_label(status: &str) -> String {
    match status {
        "ready" => t!("voice-status-ready").to_string(),
        "failed" => t!("voice-status-failed").to_string(),
        _ => t!("voice-status-draft").to_string(),
    }
}

fn sample_label(status: &str, reason: Option<&str>) -> String {
    match (status, reason) {
        ("used", _) => t!("voice-sample-used").to_string(),
        ("rejected", Some("other_voice")) => t!("voice-sample-other-voice").to_string(),
        ("rejected", Some("unreadable")) => t!("voice-sample-unreadable").to_string(),
        ("rejected", _) => t!("voice-sample-no-speech").to_string(),
        _ => t!("voice-sample-pending").to_string(),
    }
}

fn secs(ms: Option<u32>) -> String {
    ms.map(|v| format!("{:.0} s", v as f64 / 1000.0))
        .unwrap_or_else(|| "—".into())
}

fn pct(v: Option<f32>) -> String {
    v.map(|s| format!("{:.0} %", s * 100.0))
        .unwrap_or_else(|| "—".into())
}

/// The "Voice" tab of a Person object.
#[component]
pub fn VoiceProfilePanel(object_id: String) -> Element {
    let mut reload = use_signal(|| 0u32);
    let oid = object_id.clone();
    let profile = use_resource(move || {
        let id = oid.clone();
        async move {
            let _ = reload();
            api::voice_id::by_object(&id).await
        }
    });
    let can_admin = org::current_can_administer();
    let on_change = move |_| reload.with_mut(|r| *r += 1);
    let state = profile.read().clone();
    match &state {
        None => rsx! {
            p { class: "text-sm text-slate", {t!("common-loading")} }
        },
        Some(Err(e)) => {
            let msg = api::error_i18n::localize(e);
            rsx! {
                p { class: "text-sm text-alarm", "{msg}" }
            }
        }
        Some(Ok(None)) => rsx! {
            div { class: SECTION,
                h3 { class: "text-base font-semibold text-ink", {t!("voice-title")} }
                p { class: "text-sm text-slate", {t!("voice-intro")} }
                if can_admin {
                    EnrolForm { object_id: object_id.clone(), on_created: on_change }
                } else {
                    p { class: "text-sm text-slate", {t!("voice-none")} }
                }
            }
        },
        Some(Ok(Some(p))) => rsx! {
            ProfileCard { profile: p.clone(), can_admin, on_change }
        },
    }
}

/// Consent form: a profile only exists with a recorded consent (K19).
#[component]
fn EnrolForm(object_id: String, on_created: Callback<()>) -> Element {
    let models = use_resource(embedding_models);
    let mut model_id = use_signal(String::new);
    let mut consent_at = use_signal(String::new);
    let mut scope = use_signal(String::new);
    let mut proof = use_signal(String::new);
    let mut confirmed = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let list = models.read().clone().unwrap_or_default();
    // Nothing picked yet = the first model (no signal write during render).
    let chosen = match model_id() {
        m if m.is_empty() => list.first().map(|m| m.id.clone()).unwrap_or_default(),
        m => m,
    };
    let valid = confirmed()
        && !chosen.is_empty()
        && !consent_at().is_empty()
        && !scope().trim().is_empty()
        && !proof().trim().is_empty();
    let picked = chosen.clone();
    let submit = move |_| {
        let input = VoiceProfileInput {
            object_id: object_id.clone(),
            model_id: picked.clone(),
            consent_at: consent_at(),
            consent_scope: scope(),
            consent_proof: proof(),
            consent_confirmed: confirmed(),
        };
        busy.set(true);
        spawn(async move {
            let res = api::voice_id::create(&input).await;
            busy.set(false);
            match res {
                Ok(_) => on_created.call(()),
                Err(e) => toasts::error(e),
            }
        });
    };
    if models.read().is_some() && list.is_empty() {
        return rsx! {
            p { class: "text-sm text-amber-700", {t!("err-voice-id-no-model")} }
        };
    }
    rsx! {
        div { class: "grid grid-cols-1 sm:grid-cols-2 gap-3",
            div {
                label { r#for: "voice-model", class: FIELD_LABEL, {t!("voice-model")} }
                select {
                    id: "voice-model",
                    class: INPUT,
                    onchange: move |e| model_id.set(e.value()),
                    for m in list.iter() {
                        option { value: "{m.id}", selected: chosen == m.id, "{m.name}" }
                    }
                }
            }
            div {
                label { r#for: "voice-consent-at", class: FIELD_LABEL, {t!("voice-consent-at")} }
                input {
                    id: "voice-consent-at",
                    class: INPUT,
                    r#type: "date",
                    value: "{consent_at}",
                    oninput: move |e| consent_at.set(e.value()),
                }
            }
            div { class: "sm:col-span-2",
                label { r#for: "voice-consent-scope", class: FIELD_LABEL, {t!("voice-consent-scope")} }
                input {
                    id: "voice-consent-scope",
                    class: INPUT,
                    r#type: "text",
                    maxlength: "{CONSENT_SCOPE_MAX}",
                    value: "{scope}",
                    oninput: move |e| scope.set(e.value()),
                }
                p { class: FIELD_HELP, {t!("voice-consent-scope-help")} }
            }
            div { class: "sm:col-span-2",
                label { r#for: "voice-consent-proof", class: FIELD_LABEL, {t!("voice-consent-proof")} }
                input {
                    id: "voice-consent-proof",
                    class: INPUT,
                    r#type: "text",
                    maxlength: "{CONSENT_PROOF_MAX}",
                    value: "{proof}",
                    oninput: move |e| proof.set(e.value()),
                }
                p { class: FIELD_HELP, {t!("voice-consent-proof-help")} }
            }
            label { class: "sm:col-span-2 flex items-start gap-2 text-sm text-ink",
                input {
                    r#type: "checkbox",
                    checked: confirmed(),
                    onchange: move |e| confirmed.set(e.checked()),
                }
                span { {t!("voice-consent-confirm")} }
            }
        }
        button {
            class: BTN_PRIMARY_SM,
            disabled: !valid || busy(),
            onclick: submit,
            {t!("voice-create")}
        }
    }
}

#[component]
fn ProfileCard(profile: VoiceProfileView, can_admin: bool, on_change: Callback<()>) -> Element {
    let mut busy = use_signal(|| false);
    let mut revoking = use_signal(|| false);
    let ready = profile.status == STATUS_READY;
    let models = use_resource(embedding_models);
    let model_list = models.read().clone().unwrap_or_default();
    let current_model = profile.model_id.clone().unwrap_or_default();
    let pid = profile.id.clone();
    // Another model sends the profile back to draft (vectors never mix).
    let change_model = move |e: Event<FormData>| {
        let id = pid.clone();
        let model = e.value();
        spawn(async move {
            if let Err(err) = api::voice_id::set_model(&id, &model).await {
                toasts::error(err);
            }
            on_change.call(());
        });
    };
    let pid = profile.id.clone();
    let generate = move |_| {
        let id = pid.clone();
        busy.set(true);
        spawn(async move {
            let res = api::voice_id::generate(&id).await;
            busy.set(false);
            match res {
                Ok(_) => toasts::success(t!("voice-generated").to_string()),
                Err(e) => toasts::error(e),
            }
            on_change.call(());
        });
    };
    let pid = profile.id.clone();
    let upload = move |evt: Event<FormData>| {
        let id = pid.clone();
        async move {
            let Some(file) = evt.files().first().cloned() else {
                return;
            };
            let Ok(bytes) = file.read_bytes().await else {
                return;
            };
            busy.set(true);
            let ct = file.content_type().unwrap_or_default();
            let res = api::voice_id::add_sample(&id, &file.name(), &ct, bytes.to_vec()).await;
            busy.set(false);
            match res {
                Ok(_) => on_change.call(()),
                Err(e) => toasts::error(e),
            }
        }
    };
    let threshold = pct(profile.threshold);
    let spread = match (profile.intra_mean, profile.intra_std) {
        (Some(m), Some(s)) => format!("{:.0} % ± {:.0}", m * 100.0, s * 100.0),
        _ => "—".into(),
    };
    let consent_day = profile.consent_at.get(..10).unwrap_or_default().to_string();
    let model = profile.model_name.clone().unwrap_or_else(|| "—".into());
    let windows = profile.windows_used;
    rsx! {
        div { class: "space-y-4",
            div { class: SECTION,
                div { class: "flex flex-wrap items-center justify-between gap-2",
                    h3 { class: "text-base font-semibold text-ink", {t!("voice-title")} }
                    span { class: "rounded bg-gray-100 px-2 py-0.5 text-sm text-ink",
                        {status_label(&profile.status)}
                    }
                }
                dl { class: "grid grid-cols-2 sm:grid-cols-4 gap-3 text-sm",
                    div {
                        dt { class: "text-slate", {t!("voice-model")} }
                        if can_admin && !model_list.is_empty() {
                            dd {
                                select {
                                    class: INPUT,
                                    aria_label: t!("voice-model").to_string(),
                                    onchange: change_model,
                                    for m in model_list.iter() {
                                        option {
                                            value: "{m.id}",
                                            selected: current_model == m.id,
                                            "{m.name}"
                                        }
                                    }
                                }
                            }
                        } else {
                            dd { class: "text-ink", "{model}" }
                        }
                    }
                    div {
                        dt { class: "text-slate", {t!("voice-threshold")} }
                        dd { class: "text-ink", "{threshold}" }
                    }
                    div {
                        dt { class: "text-slate", {t!("voice-windows")} }
                        dd { class: "text-ink", "{windows}" }
                    }
                    div {
                        dt { class: "text-slate", {t!("voice-spread")} }
                        dd { class: "text-ink", "{spread}" }
                    }
                }
                if let Some(err) = profile.last_error.clone() {
                    p { class: "text-sm text-alarm",
                        {t!("voice-last-failed")}
                        " ({err})"
                    }
                }
                p { class: "text-xs text-slate",
                    {
                        t!(
                            "voice-consent-summary", date : consent_day, scope : profile.consent_scope
                            .clone(), by : profile.consent_recorded_by.clone()
                        )
                    }
                }
            }
            div { class: SECTION,
                h3 { class: "text-base font-semibold text-ink", {t!("voice-samples")} }
                p { class: "text-sm text-slate", {t!("voice-samples-help")} }
                if profile.samples.is_empty() {
                    p { class: "text-sm text-slate", {t!("voice-samples-empty")} }
                }
                ul { class: "divide-y divide-line",
                    for s in profile.samples.clone() {
                        SampleRow {
                            key: "{s.id}",
                            profile_id: profile.id.clone(),
                            sample: s,
                            can_admin,
                            on_change,
                        }
                    }
                }
                if can_admin {
                    div { class: "flex flex-wrap items-end gap-3",
                        div { class: "grow",
                            label { r#for: "voice-clip", class: FIELD_LABEL, {t!("voice-add-sample")} }
                            input {
                                id: "voice-clip",
                                class: INPUT,
                                r#type: "file",
                                accept: AUDIO_ACCEPT,
                                disabled: busy(),
                                onchange: upload,
                            }
                        }
                        button {
                            class: BTN_PRIMARY_SM,
                            "data-testid": "voice-generate",
                            disabled: busy() || profile.samples.is_empty(),
                            onclick: generate,
                            {t!("voice-generate")}
                        }
                    }
                }
            }
            if ready {
                div { class: SECTION,
                    h3 { class: "text-base font-semibold text-ink", {t!("voice-test-title")} }
                    p { class: "text-sm text-slate", {t!("voice-test-verify-help")} }
                    VoiceTestBox {
                        object_id: Some(profile.object_id.clone()),
                        model_id: None,
                    }
                }
            }
            if can_admin {
                div { class: "flex justify-end",
                    button {
                        class: BTN_DANGER,
                        onclick: move |_| revoking.set(true),
                        {t!("voice-revoke")}
                    }
                }
            }
            if revoking() {
                ConfirmDialog {
                    title: t!("voice-revoke").to_string(),
                    message: t!("voice-revoke-confirm", person : profile.person.clone()).to_string(),
                    confirm_label: t!("voice-revoke").to_string(),
                    on_confirm: move |_| {
                        let id = profile.id.clone();
                        spawn(async move {
                            match api::voice_id::revoke(&id).await {
                                Ok(_) => toasts::success(t!("voice-revoked").to_string()),
                                Err(e) => toasts::error(e),
                            }
                            revoking.set(false);
                            on_change.call(());
                        });
                    },
                    on_cancel: move |_| revoking.set(false),
                }
            }
        }
    }
}

#[component]
fn SampleRow(
    profile_id: String,
    sample: pnex_core::voice_id::VoiceSampleView,
    can_admin: bool,
    on_change: Callback<()>,
) -> Element {
    let label = sample_label(&sample.status, sample.reason.as_deref());
    let duration = secs(sample.duration_ms);
    let speech = secs(sample.speech_ms);
    let score = pct(sample.score);
    let sid = sample.id.clone();
    let remove = move |_| {
        let (pid, sid) = (profile_id.clone(), sid.clone());
        spawn(async move {
            if let Err(e) = api::voice_id::remove_sample(&pid, &sid).await {
                toasts::error(e);
            }
            on_change.call(());
        });
    };
    rsx! {
        li { class: "flex flex-wrap items-center gap-3 py-2 text-sm",
            span { class: "grow text-ink", "{sample.name}" }
            span { class: "text-slate",
                {t!("voice-sample-stats", duration : duration, speech : speech)}
            }
            span { class: "text-slate", {t!("voice-sample-score", score : score)} }
            span { class: "rounded bg-gray-100 px-2 py-0.5 text-ink", "{label}" }
            if can_admin {
                button { class: BTN_SECONDARY_SM, onclick: remove, {t!("voice-sample-remove")} }
            }
        }
    }
}

/// Test of a clip: verification of one person (`object_id`) or
/// identification among the ready profiles of `model_id`.
#[component]
pub fn VoiceTestBox(object_id: Option<String>, model_id: Option<String>) -> Element {
    let mut file = use_signal(|| None::<(String, String, Vec<u8>)>);
    let mut busy = use_signal(|| false);
    let mut result = use_signal(|| None::<VoiceTestResult>);
    let can_write = org::current_can_write();
    let run = move |_| {
        let Some((name, ct, bytes)) = file.peek().clone() else {
            return;
        };
        let (oid, mid) = (object_id.clone(), model_id.clone());
        busy.set(true);
        result.set(None);
        spawn(async move {
            let res = api::voice_id::test(oid.as_deref(), mid.as_deref(), &name, &ct, bytes).await;
            busy.set(false);
            match res {
                Ok(r) => result.set(Some(r)),
                Err(e) => toasts::error(e),
            }
        });
    };
    rsx! {
        div { class: "flex flex-wrap items-end gap-3",
            div { class: "grow",
                label { r#for: "voice-test-file", class: FIELD_LABEL, {t!("voice-test-file")} }
                input {
                    id: "voice-test-file",
                    class: INPUT,
                    r#type: "file",
                    accept: AUDIO_ACCEPT,
                    onchange: move |evt| async move {
                        if let Some(f) = evt.files().first().cloned() {
                            if let Ok(bytes) = f.read_bytes().await {
                                file.set(
                                    Some((
                                        f.name(),
                                        f.content_type().unwrap_or_default(),
                                        bytes.to_vec(),
                                    )),
                                );
                            }
                        }
                    },
                }
            }
            button {
                class: BTN_PRIMARY_SM,
                "data-testid": "voice-test-run",
                disabled: !can_write || busy() || file.read().is_none(),
                onclick: run,
                {t!("voice-test-run")}
            }
        }
        if let Some(r) = result() {
            TestResult { result: r }
        }
    }
}

#[component]
fn TestResult(result: VoiceTestResult) -> Element {
    let (tone, verdict) = match (result.verdict, result.best.as_ref()) {
        (VoiceVerdict::Match, Some(b)) => (
            "text-green-700",
            t!("voice-verdict-match", person : b.person.clone()).to_string(),
        ),
        (VoiceVerdict::Ambiguous, _) => {
            ("text-amber-700", t!("voice-verdict-ambiguous").to_string())
        }
        _ => ("text-ink", t!("voice-verdict-none").to_string()),
    };
    let audio = secs(Some(result.audio_ms));
    let speech = secs(Some(result.speech_ms));
    rsx! {
        div {
            class: "{PANEL} p-3 space-y-2",
            "data-testid": "voice-test-result",
            p { class: "text-sm font-semibold {tone}", "{verdict}" }
            p { class: "text-xs text-slate",
                {
                    t!(
                        "voice-test-summary", audio : audio, speech : speech, windows : result
                        .windows
                    )
                }
            }
            if result.truncated {
                p { class: "text-xs text-amber-700", {t!("voice-test-truncated")} }
            }
            ul { class: "text-sm space-y-1",
                for c in result.candidates.iter() {
                    li {
                        key: "{c.profile_id}",
                        class: "flex justify-between gap-3",
                        span { class: "text-ink", "{c.person}" }
                        span { class: if c.accepted { "text-green-700" } else { "text-slate" },
                            {
                                t!(
                                    "voice-candidate-score", score : pct(Some(c.score)), threshold : pct(Some(c
                                    .threshold))
                                )
                            }
                        }
                    }
                }
            }
            p { class: "text-xs text-slate", {t!("voice-test-proposal")} }
        }
    }
}
