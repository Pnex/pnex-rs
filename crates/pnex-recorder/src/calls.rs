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
use crate::{detect, queue};

/// Poll period of the call detection.
const POLL: Duration = Duration::from_secs(2);
/// Silence of the call app before an automatic recording stops.
const RELEASE: Duration = Duration::from_secs(45);
/// Waiting recordings are retried this often.
const RETRY: Duration = Duration::from_secs(120);

#[derive(Clone, PartialEq)]
pub enum Status {
    Idle,
    Sending(String),
    Sent(String),
    /// Stored, but the transcription did not start (the reason).
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
                plain(&t!("rec-name", app: app, date: date))
            };
            self.status.set(Status::Sending(name.clone()));
            let mut session = self.session.peek().clone();
            let stored = session.upload_recording(org, &file, &name).await;
            let asset = match stored {
                Ok(asset) => asset,
                Err(e) => {
                    self.session.set(session);
                    self.status.set(Status::Failed(crate::error_text(&e)));
                    break;
                }
            };
            let _ = std::fs::remove_file(&file);
            self.pending.with_mut(|n| *n = n.saturating_sub(1));
            let started = session.transcribe(org, &asset, &locale).await;
            self.session.set(session);
            self.status.set(match started {
                Ok(()) => Status::Sent(name),
                Err(e) => Status::Untranscribed(name, crate::error_text(&e)),
            });
        }
        self.sending.set(false);
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
    let enabled = calls.session.read().enabled;
    let rec = calls
        .rec
        .read()
        .as_ref()
        .map(|r| (r.app.clone(), clock(r.started.elapsed())));
    let pending = (calls.pending)();
    let status = (calls.status)();
    rsx! {
        div { class: "card",
            h2 { {t!("rec-title")} }
            if !capture::supported() {
                p { class: "muted", {t!("rec-unsupported")} }
            } else if let Some((app, time)) = rec {
                p { class: "recording", role: "status",
                    {t!("rec-recording", app : app.unwrap_or_else(|| t!("rec-manual")), time : time)}
                }
                p { class: "muted", {t!("rec-consent")} }
                div { class: "row",
                    button {
                        id: "rec-discard",
                        r#type: "button",
                        onclick: move |_| calls.discard(),
                        {t!("rec-discard")}
                    }
                    button {
                        id: "rec-stop",
                        class: "primary",
                        r#type: "button",
                        onclick: move |_| calls.stop_and_send(),
                        {t!("rec-stop")}
                    }
                }
            } else {
                p { class: "muted",
                    if enabled {
                        {t!("rec-waiting")}
                    } else {
                        {t!("rec-off")}
                    }
                }
                button {
                    id: "rec-start",
                    r#type: "button",
                    onclick: move |_| calls.start(None),
                    {t!("rec-start")}
                }
            }
            match status {
                Status::Idle => rsx! {},
                Status::Sending(name) => rsx! {
                    p { class: "muted", {t!("rec-sending", name : name)} }
                },
                Status::Sent(name) => rsx! {
                    p { class: "muted", role: "status", {t!("rec-sent", name : name)} }
                },
                Status::Untranscribed(name, why) => rsx! {
                    p { class: "error", role: "alert", {t!("rec-untranscribed", name : name, error : why)} }
                },
                Status::Failed(why) => rsx! {
                    p { class: "error", role: "alert", "{why}" }
                },
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
