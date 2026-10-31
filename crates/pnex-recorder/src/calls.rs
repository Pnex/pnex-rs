//! Call recording (call-recording.md R1.1): while recording is enabled, a
//! call app holding the microphone starts a capture, which stops
//! [`RELEASE`] after the app lets the microphone go. A recording can also
//! be started and stopped by hand. Closed recordings are sent to the org's
//! media library, then transcribed with the org's default profile; a file
//! is deleted only once the server has stored it.

use std::time::{Duration, Instant};

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api::Session;
use crate::capture::{self, Recording};
use crate::{detect, encode, queue};

/// Poll period of the call detection.
const POLL: Duration = Duration::from_secs(2);
/// Silence of the call app before an automatic recording stops.
const RELEASE: Duration = Duration::from_secs(45);
/// Waiting recordings are retried this often.
const RETRY: Duration = Duration::from_secs(120);
/// Poll period of a running transcription.
const FOLLOW: Duration = Duration::from_secs(3);

#[derive(Clone, PartialEq)]
pub enum Status {
    Idle,
    Sending(String),
    /// Stored; the server transcribes it (progress 0–100).
    Transcribing(String, i16),
    /// Transcribed: name and the first part's asset id (its review page).
    Transcribed(String, String),
    /// Stored, but the transcription did not start or failed (the reason).
    Untranscribed(String, String),
    Failed(String),
}

#[derive(Clone, Copy)]
pub struct Calls {
    pub rec: Signal<Option<Recording>>,
    pub status: Signal<Status>,
    pub pending: Signal<usize>,
    /// Re-render tick for the recording clock.
    pub tick: Signal<u64>,
    /// Why the last call could not be filed in the user's case (it was
    /// sent and transcribed all the same).
    pub filing: Signal<Option<String>>,
    released: Signal<Option<Instant>>,
    sending: Signal<bool>,
    session: Signal<Session>,
}

impl Calls {
    pub fn recording(&self) -> bool {
        self.rec.read().is_some()
    }

    /// Starts a capture; `app` is the call app, `None` by hand.
    pub fn start(mut self, app: Option<String>) {
        if self.rec.peek().is_some() {
            return;
        }
        let started = queue::new_path(app.as_deref()).and_then(|p| capture::start(&p, app));
        match started {
            Ok(rec) => {
                self.released.set(None);
                self.rec.set(Some(rec));
                // Consent: the window comes up with its stop buttons.
                dioxus::desktop::window().set_visible(true);
            }
            Err(e) => self
                .status
                .set(Status::Failed(t!("err-capture", detail: e))),
        }
    }

    /// Stops the capture and sends the waiting recordings.
    pub fn stop_and_send(mut self) {
        let Some(rec) = self.rec.write().take() else {
            return;
        };
        spawn(async move {
            let closed = tokio::task::spawn_blocking(move || rec.finish()).await;
            if let Ok(Err(e)) = closed {
                self.status
                    .set(Status::Failed(t!("err-capture", detail: e)));
            }
            self.send_pending().await;
        });
    }

    /// Stops the capture and deletes the file.
    pub fn discard(mut self) {
        if let Some(rec) = self.rec.write().take() {
            spawn(async move {
                let _ = tokio::task::spawn_blocking(move || rec.discard()).await;
            });
        }
    }

    /// Sends every closed recording, oldest first; stops at the first one
    /// the server does not take (it stays on disk for the next try).
    pub async fn send_pending(mut self) {
        let Some(org) = self.session.peek().org else {
            return;
        };
        if *self.sending.peek() || !self.session.peek().signed_in() {
            return;
        }
        self.sending.set(true);
        let open = self.rec.peek().as_ref().map(|r| r.path.clone());
        let files = queue::pending(open.as_deref());
        self.pending.set(files.len());
        let locale = crate::system_locale().to_string();
        for file in files {
            let (date, app) = queue::describe(&file).unwrap_or_default();
            let name = if app == queue::MANUAL {
                plain(&t!("rec-name-manual", date: date))
            } else {
                plain(&t!("rec-name", app: app.clone(), date: date))
            };
            self.status.set(Status::Sending(name.clone()));
            let span = call_span(&file);
            let wav = file.clone();
            let parts =
                tokio::task::spawn_blocking(move || encode::ogg_parts(&wav, encode::PART_SECS))
                    .await
                    .ok()
                    .and_then(Result::ok);
            let Some(parts) = parts else {
                self.status.set(Status::Failed(crate::error_text(
                    &crate::api::ApiError::Server(0, Some("recording-unreadable".into())),
                )));
                break;
            };
            let stem = file.file_stem().and_then(|f| f.to_str()).unwrap_or("call");
            let count = parts.len();
            let mut session = self.session.peek().clone();
            let mut assets = Vec::new();
            let mut failed = None;
            // shortcut: a part that fails sends the whole call again on the
            // next try (earlier parts are duplicated); track sent parts if
            // calls over 2 h become common.
            for (i, part) in parts.into_iter().enumerate() {
                let (title, filename) = if count == 1 {
                    (name.clone(), format!("{stem}.ogg"))
                } else {
                    (
                        format!("{name} ({}/{count})", i + 1),
                        format!("{stem}_{}.ogg", i + 1),
                    )
                };
                match session.upload_recording(org, part, &filename, &title).await {
                    Ok(asset) => {
                        // A missing label only hides the call from the filtered link.
                        if let Err(e) = session.label_recording(org, &asset, &app).await {
                            eprintln!("recording label failed: {e:?}");
                        }
                        assets.push(asset);
                    }
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            if let Some(e) = failed {
                self.session.set(session);
                self.status.set(Status::Failed(crate::error_text(&e)));
                break;
            }
            let _ = std::fs::remove_file(&file);
            self.pending.with_mut(|n| *n = n.saturating_sub(1));
            let case_title = plain(&t!("rec-case-title", email: session.email.clone()));
            let filed = match span {
                Some((from, to)) => {
                    session
                        .file_call(org, &case_title, &name, from, to, &assets)
                        .await
                }
                None => Err(crate::api::ApiError::Server(
                    0,
                    Some("recording-unreadable".into()),
                )),
            };
            self.filing.set(filed.err().map(|e| crate::error_text(&e)));
            let mut started = Ok(());
            for asset in &assets {
                started = started.and(session.transcribe(org, asset, &locale).await);
            }
            self.session.set(session);
            match started {
                Ok(()) => {
                    self.status.set(Status::Transcribing(name.clone(), 0));
                    spawn(self.follow(org, name, assets));
                }
                Err(e) => self
                    .status
                    .set(Status::Untranscribed(name, crate::error_text(&e))),
            }
        }
        self.sending.set(false);
    }

    /// Follows the transcription of a sent call until every part is done or
    /// one failed; the status shows its progress meanwhile.
    async fn follow(mut self, org: i64, name: String, assets: Vec<String>) {
        use pnex_core::media_analysis::AnalysisState;
        loop {
            tokio::time::sleep(FOLLOW).await;
            let mut session = self.session.peek().clone();
            let mut progress = 0i32;
            let mut done = 0;
            let mut failure = None;
            for asset in &assets {
                match session.transcription(org, asset).await {
                    Ok(Some(a)) => match a.state {
                        AnalysisState::Done => {
                            done += 1;
                            progress += 100;
                        }
                        AnalysisState::Failed => {
                            let code = a.error_code.or(Some("asr-test-failed".into()));
                            failure = Some(crate::api::ApiError::Server(0, code));
                        }
                        _ => progress += i32::from(a.progress),
                    },
                    Ok(None) => {}
                    // A network blip: try again on the next round.
                    Err(crate::api::ApiError::Unreachable) => {}
                    Err(e) => failure = Some(e),
                }
            }
            self.session.set(session);
            // Another call took the status line over: stop following this one.
            let current = match &*self.status.peek() {
                Status::Transcribing(n, _) => n == &name,
                _ => false,
            };
            if !current {
                return;
            }
            if let Some(e) = failure {
                self.status
                    .set(Status::Untranscribed(name, crate::error_text(&e)));
                return;
            }
            if done == assets.len() {
                self.status
                    .set(Status::Transcribed(name, assets[0].clone()));
                return;
            }
            let pct = (progress / assets.len().max(1) as i32) as i16;
            self.status.set(Status::Transcribing(name.clone(), pct));
        }
    }

    /// One detection round: starts, keeps or stops the automatic capture.
    fn on_poll(self, call: Option<&'static str>) {
        let mut released = self.released;
        let enabled = self.session.peek().enabled;
        let auto = self.rec.peek().as_ref().map(|r| r.app.is_some());
        match auto {
            None if enabled => {
                if let Some(app) = call {
                    self.start(Some(app.to_string()));
                }
            }
            None => {}
            // Switched off during an automatic recording: it ends now.
            Some(true) if !enabled => self.stop_and_send(),
            Some(true) => match (call, released.cloned()) {
                (Some(_), _) => released.set(None),
                (None, None) => released.set(Some(Instant::now())),
                (None, Some(at)) if at.elapsed() >= RELEASE => self.stop_and_send(),
                (None, Some(_)) => {}
            },
            // A recording started by hand stops by hand.
            Some(false) => {}
        }
    }
}

/// Creates the controller and its detection loop (call once, in `App`).
pub fn use_calls(session: Signal<Session>) -> Calls {
    let calls = Calls {
        rec: use_signal(|| None),
        status: use_signal(|| Status::Idle),
        pending: use_signal(|| queue::pending(None).len()),
        tick: use_signal(|| 0),
        filing: use_signal(|| None),
        released: use_signal(|| None),
        sending: use_signal(|| false),
        session,
    };
    use_future(move || async move {
        let mut tick = calls.tick;
        let mut last_retry: Option<Instant> = None;
        loop {
            if last_retry.is_none_or(|at| at.elapsed() >= RETRY) {
                last_retry = Some(Instant::now());
                calls.send_pending().await;
            }
            if capture::supported() {
                let apps = tokio::task::spawn_blocking(detect::recording_apps)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default();
                calls.on_poll(detect::call_app(&apps));
            }
            tick.with_mut(|t| *t += 1);
            tokio::time::sleep(POLL).await;
        }
    });
    use_context_provider(|| calls)
}

/// Start and end of a closed recording: it ends when the file was last
/// written, and lasts as long as its samples.
fn call_span(
    wav: &std::path::Path,
) -> Option<(chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> {
    let reader = hound::WavReader::open(wav).ok()?;
    let spec = reader.spec();
    let secs = reader.duration() as i64 / i64::from(spec.sample_rate.max(1));
    let end: chrono::DateTime<chrono::Utc> = std::fs::metadata(wav).ok()?.modified().ok()?.into();
    Some((end - chrono::Duration::seconds(secs), end))
}

/// Text without the Unicode isolation marks Fluent puts around arguments:
/// the name is stored on the server, the marks would end up in its title.
fn plain(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\u{2068}' | '\u{2069}'))
        .collect()
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

#[component]
pub fn CallsCard() -> Element {
    let calls = use_context::<Calls>();
    let _ = (calls.tick)();
    let mut session = calls.session;
    let enabled = session.read().enabled;
    let rec = calls
        .rec
        .read()
        .as_ref()
        .map(|r| (r.app.clone(), clock(r.started.elapsed())));
    if !capture::supported() {
        return rsx! {
            div { class: "status",
                p { class: "lamp", {t!("rec-unsupported")} }
            }
        };
    }
    rsx! {
        if let Some((app, time)) = rec {
            div { class: "status live", role: "status",
                p { class: "lamp",
                    {t!("rec-recording", app : app.unwrap_or_else(|| t!("rec-manual")))}
                }
                p { class: "clock", "{time}" }
                p { class: "muted", {t!("rec-consent")} }
                div { class: "actions",
                    button {
                        id: "rec-discard",
                        r#type: "button",
                        onclick: move |_| calls.discard(),
                        crate::Icon { d: crate::DISCARD }
                        {t!("rec-discard")}
                    }
                    button {
                        id: "rec-stop",
                        class: "primary",
                        r#type: "button",
                        onclick: move |_| calls.stop_and_send(),
                        crate::Icon { d: crate::SEND }
                        {t!("rec-stop")}
                    }
                }
            }
        } else {
            div { class: if enabled { "status armed" } else { "status" },
                div { class: "row",
                    label { class: "lamp", r#for: "rec-auto",
                        if enabled {
                            {t!("rec-armed")}
                        } else {
                            {t!("rec-off")}
                        }
                    }
                    input {
                        id: "rec-auto",
                        class: "switch",
                        r#type: "checkbox",
                        role: "switch",
                        aria_label: t!("rec-auto"),
                        checked: enabled,
                        onchange: move |e| {
                            let mut s = session.write();
                            s.enabled = e.checked();
                            s.save();
                        },
                    }
                }
                p { class: "muted",
                    if enabled {
                        {t!("rec-waiting")}
                    } else {
                        {t!("rec-off-help")}
                    }
                }
                button {
                    id: "rec-start",
                    r#type: "button",
                    onclick: move |_| calls.start(None),
                    crate::Icon { d: crate::RECORD }
                    {t!("rec-start")}
                }
            }
        }
        Activity {}
    }
}

/// The last call sent and the recordings still waiting for the server.
#[component]
fn Activity() -> Element {
    let calls = use_context::<Calls>();
    let pending = (calls.pending)();
    let status = (calls.status)();
    if status == Status::Idle && pending == 0 && calls.filing.read().is_none() {
        return rsx! {};
    }
    rsx! {
        div { class: "activity",
            match status {
                Status::Idle => rsx! {},
                Status::Sending(name) => rsx! {
                    p { class: "muted", {t!("rec-sending", name : name)} }
                },
                Status::Transcribing(name, progress) => rsx! {
                    p { class: "muted", role: "status", {t!("rec-transcribing", name : name)} }
                    progress { max: "100", value: "{progress}" }
                },
                Status::Transcribed(name, asset) => rsx! {
                    div { class: "row",
                        p { role: "status", {t!("rec-transcribed", name : name)} }
                        button {
                            r#type: "button",
                            title: t!("web-open"),
                            onclick: move |_| crate::open_web(&calls.session.peek().url, &format!("/media/{asset}/review")),
                            {t!("rec-open")}
                        }
                    }
                },
                Status::Untranscribed(name, why) => rsx! {
                    p { class: "error", role: "alert", {t!("rec-untranscribed", name : name, error : why)} }
                },
                Status::Failed(why) => rsx! {
                    p { class: "error", role: "alert", "{why}" }
                },
            }
            if let Some(why) = (calls.filing)() {
                p { class: "muted", role: "status", {t!("rec-not-filed", error : why)} }
            }
            if pending > 0 {
                div { class: "row",
                    p { class: "muted", {t!("rec-pending", count : pending)} }
                    button {
                        id: "rec-send",
                        r#type: "button",
                        onclick: move |_| {
                            spawn(calls.send_pending());
                        },
                        {t!("rec-send")}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn plain_drops_the_fluent_isolation_marks() {
        assert_eq!(
            super::plain("Call \u{2068}Zoom\u{2069} \u{2068}2026-10-29 23:25\u{2069}"),
            "Call Zoom 2026-10-29 23:25"
        );
    }
}
