//! "My voice" card: the user enrols their own voice (with their consent)
//! and tests how well PNeX recognises it, without the voice profile pages.

use dioxus::prelude::*;
use dioxus_i18n::t;
use pnex_core::voice_id::{VoiceProfileStatus, VoiceTestResult, VoiceVerdict};

use crate::api::{ApiError, Session, VoiceSetup};
use crate::{audio, error_text, with_session};

/// Reading time of the enrolment: five 8 s windows, three are needed.
const ENROL_SECS: u64 = 40;
/// One full window of the voice test.
const TEST_SECS: u64 = 8;

#[derive(Clone, PartialEq)]
enum Step {
    Idle,
    Recording(u64),
    Training,
    Testing,
    Tested(VoiceTestResult),
    Failed(String),
}

/// Sentences in each locale (`voice-sentence-1`…); the user reads random
/// ones aloud, so nobody is left wondering what to say.
const SENTENCES: usize = 24;
/// About 40 s of reading.
const ENROL_LINES: usize = 6;
const TEST_LINES: usize = 2;

/// `k` distinct sentence numbers out of `n`, from `seed` (partial
/// Fisher-Yates with a xorshift generator: no dependency for a shuffle).
fn draw(n: usize, k: usize, mut seed: u64) -> Vec<usize> {
    let mut all: Vec<usize> = (1..=n).collect();
    for i in 0..k.min(n) {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = i + (seed % (n - i) as u64) as usize;
        all.swap(i, j);
    }
    all.truncate(k.min(n));
    all
}

/// `k` random sentences in the user's language.
fn sentences(k: usize) -> Vec<String> {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        | 1;
    draw(SENTENCES, k, seed)
        .into_iter()
        .map(|i| t!(&format!("voice-sentence-{i}")))
        .collect()
}

/// Records `secs` seconds; the error is already translated.
async fn record(secs: u64) -> Result<Vec<u8>, String> {
    match tokio::task::spawn_blocking(move || audio::record_wav(secs)).await {
        Ok(Ok(wav)) => Ok(wav),
        Ok(Err(audio::CaptureError::NoMicrophone)) => Err(t!("err-no-microphone")),
        Ok(Err(audio::CaptureError::Device(detail))) => Err(t!("err-microphone", detail: detail)),
        Err(e) => Err(t!("err-microphone", detail: e.to_string())),
    }
}

fn voice_error(e: &ApiError) -> String {
    match e {
        ApiError::Server(_, Some(code)) if code == "voice-id-not-enough-speech" => {
            t!("voice-not-enough-speech")
        }
        ApiError::Server(_, Some(code)) if code == "voice-id-failed" => t!("voice-failed"),
        ApiError::Server(_, Some(code)) if code == "ontology-not-found" => t!("voice-no-person"),
        e => error_text(e),
    }
}

/// Percentage shown for a cosine score.
fn percent(score: f32) -> i64 {
    (score.clamp(0.0, 1.0) * 100.0).round() as i64
}

#[component]
pub fn VoiceCard(session: Signal<Session>, org: i64) -> Element {
    let mut loaded =
        use_resource(move || async move { with_session!(session, |s| s.voice_setup(org)) });
    let mut consent = use_signal(|| false);
    let mut step = use_signal(|| Step::Idle);
    let mut lines = use_signal(Vec::<String>::new);

    let setup: VoiceSetup = match &*loaded.read() {
        None => return rsx! {},
        Some(Err(e)) => {
            return rsx! {
                div { class: "card",
                    h2 { {t!("voice-title")} }
                    p { class: "error", {error_text(e)} }
                }
            }
        }
        Some(Ok(s)) => s.clone(),
    };
    let ready = setup
        .profile
        .as_ref()
        .filter(|p| p.status == VoiceProfileStatus::Ready)
        .cloned();
    let notice = if setup.model.is_none() {
        Some(t!("voice-no-model"))
    } else if !setup.can_enrol {
        Some(t!("voice-no-right"))
    } else {
        None
    };
    let busy = matches!(step(), Step::Recording(_) | Step::Training | Step::Testing);

    let enrol = {
        let setup = setup.clone();
        move |_| {
            let Some(model) = setup.model.clone() else {
                return;
            };
            let profile = setup.profile.clone();
            lines.set(sentences(ENROL_LINES));
            step.set(Step::Recording(ENROL_SECS));
            spawn(async move {
                let wav = match record(ENROL_SECS).await {
                    Ok(wav) => wav,
                    Err(msg) => return step.set(Step::Failed(msg)),
                };
                step.set(Step::Training);
                let res = with_session!(session, |s| s.enrol_voice(org, &model, profile, wav));
                match res {
                    Ok(_) => {
                        step.set(Step::Idle);
                        loaded.restart();
                    }
                    Err(e) => {
                        step.set(Step::Failed(voice_error(&e)));
                        // A failed generation leaves a draft profile: reread it.
                        loaded.restart();
                    }
                }
            });
        }
    };
    let test = {
        let object = ready.as_ref().map(|p| p.object_id.clone());
        move |_| {
            let Some(object) = object.clone() else {
                return;
            };
            lines.set(sentences(TEST_LINES));
            step.set(Step::Recording(TEST_SECS));
            spawn(async move {
                let wav = match record(TEST_SECS).await {
                    Ok(wav) => wav,
                    Err(msg) => return step.set(Step::Failed(msg)),
                };
                step.set(Step::Testing);
                let res = with_session!(session, |s| s.test_voice(org, &object, wav));
                step.set(match res {
                    Ok(r) => Step::Tested(r),
                    Err(e) => Step::Failed(voice_error(&e)),
                });
            });
        }
    };

    rsx! {
        div { class: "card",
            h2 { {t!("voice-title")} }
            if let Some(p) = &ready {
                p { class: "muted",
                    {
                        t!(
                            "voice-ready", windows : p.windows_used, threshold : percent(p.threshold
                            .unwrap_or(0.0))
                        )
                    }
                }
                button {
                    class: "primary",
                    r#type: "button",
                    disabled: busy,
                    onclick: test,
                    {t!("voice-test", secs : TEST_SECS)}
                }
            } else {
                p { class: "muted", {t!("voice-help", secs : ENROL_SECS)} }
            }
            if let Some(notice) = notice.clone() {
                p { class: "error", role: "status", {notice} }
            } else {
                if ready.is_none() {
                    label { class: "check",
                        input {
                            r#type: "checkbox",
                            checked: consent(),
                            onchange: move |e| consent.set(e.checked()),
                        }
                        {t!("voice-consent")}
                    }
                }
                button {
                    r#type: "button",
                    class: if ready.is_none() { "primary" } else { "" },
                    disabled: busy || (ready.is_none() && !consent()),
                    onclick: enrol,
                    if ready.is_some() {
                        {t!("voice-retrain", secs : ENROL_SECS)}
                    } else {
                        {t!("voice-enrol", secs : ENROL_SECS)}
                    }
                }
                if ready.is_some() {
                    p { class: "muted", {t!("voice-withdraw")} }
                }
            }
            if matches!(step(), Step::Recording(_)) {
                p { class: "result", {lines.read().join("\n")} }
            }
            match step() {
                Step::Idle => rsx! {},
                Step::Recording(secs) if secs == ENROL_SECS => rsx! {
                    p { class: "muted", {t!("voice-reading", secs : secs)} }
                },
                Step::Recording(secs) => rsx! {
                    p { class: "muted", {t!("voice-speak", secs : secs)} }
                },
                Step::Training => rsx! {
                    p { class: "muted", {t!("voice-training")} }
                },
                Step::Testing => rsx! {
                    p { class: "muted", {t!("voice-testing")} }
                },
                Step::Tested(r) => rsx! {
                    VoiceScore { result: r }
                },
                Step::Failed(msg) => rsx! {
                    p { class: "error", role: "alert", "{msg}" }
                },
            }
        }
    }
}

#[component]
fn VoiceScore(result: VoiceTestResult) -> Element {
    let Some(best) = result
        .best
        .clone()
        .or_else(|| result.candidates.first().cloned())
    else {
        return rsx! {
            p { class: "error", role: "status", {t!("voice-no-speech")} }
        };
    };
    let score = percent(best.score);
    let threshold = percent(best.threshold);
    let verdict = match (result.verdict, best.accepted) {
        (VoiceVerdict::Match, _) | (_, true) => t!("voice-match"),
        _ => t!("voice-no-match"),
    };
    rsx! {
        p { class: "score", "{score} %" }
        meter {
            min: "0",
            max: "100",
            low: "{threshold}",
            optimum: "100",
            value: "{score}",
        }
        p { class: "muted", {t!("voice-score", score : score, threshold : threshold)} }
        p { class: "result", "{verdict}" }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn draws_distinct_sentences_in_range() {
        for seed in [1, 42, u64::MAX] {
            let mut d = super::draw(24, 6, seed);
            assert_eq!(d.len(), 6);
            assert!(d.iter().all(|i| (1..=24).contains(i)));
            d.sort();
            d.dedup();
            assert_eq!(d.len(), 6);
        }
        assert_ne!(super::draw(24, 6, 1), super::draw(24, 6, 2));
        assert_eq!(super::draw(3, 6, 7).len(), 3);
    }

    #[test]
    fn every_sentence_exists_in_both_locales() {
        for ftl in [
            include_str!("../locales/en-US.ftl"),
            include_str!("../locales/fr-FR.ftl"),
        ] {
            for i in 1..=super::SENTENCES {
                assert!(
                    ftl.contains(&format!("\nvoice-sentence-{i} = ")),
                    "voice-sentence-{i}"
                );
            }
        }
    }

    #[test]
    fn percent_rounds_and_clamps() {
        assert_eq!(super::percent(0.874), 87);
        assert_eq!(super::percent(1.2), 100);
        assert_eq!(super::percent(-0.1), 0);
    }
}
