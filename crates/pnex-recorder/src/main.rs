//! PNeX recorder (call-recording.md R1): a tray utility that signs in to a
//! PNeX server, records calls while recording is enabled, sends them to the
//! org's media library for transcription, and checks with a 5 s
//! transcription test that the server transcribes.

// A tray app: no console window behind it on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod api;
mod audio;
mod calls;
mod capture;
mod detect;
mod queue;

use dioxus::desktop::trayicon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use dioxus::desktop::trayicon::{init_tray_icon, Icon as TrayIcon};
use dioxus::desktop::{
    tao, use_tray_menu_event_handler, window, Config, LogicalSize, WindowBuilder,
    WindowCloseBehaviour,
};
use dioxus::prelude::*;
use dioxus_i18n::prelude::*;
use dioxus_i18n::t;
use dioxus_i18n::unic_langid::{langid, LanguageIdentifier};
use pnex_core::media_ingest::{AsrModel, AsrProfile, AsrTestResult};
use pnex_core::OrgMembership;

use api::{ApiError, Session};

/// Length of the transcription test clip.
const TEST_SECS: u64 = 5;

const MARK: &[u8] = include_bytes!("../../pnex-frontend/assets/logo-mark.png");

const STYLE: &str = r#"
body { font-family: system-ui, sans-serif; margin: 0; background: #f9fafb; color: #111827; font-size: 14px; }
main { padding: 16px; display: flex; flex-direction: column; gap: 12px; }
h1 { font-size: 16px; margin: 0; }
h2 { font-size: 14px; margin: 0; }
label { display: block; font-size: 12px; color: #4b5563; margin-bottom: 4px; }
input[type=text], input[type=email], input[type=password], select { width: 100%; box-sizing: border-box; padding: 6px 8px; border: 1px solid #d1d5db; border-radius: 6px; font-size: 14px; }
button { padding: 8px 12px; border-radius: 6px; border: 1px solid #d1d5db; background: #fff; cursor: pointer; font-size: 14px; }
button.primary { background: #111827; color: #fff; border-color: #111827; }
button:disabled { opacity: .5; cursor: default; }
.card { background: #fff; border: 1px solid #e5e7eb; border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 8px; }
.muted { color: #6b7280; font-size: 12px; margin: 0; }
.error { color: #b91c1c; font-size: 13px; margin: 0; }
.row { display: flex; align-items: center; gap: 8px; justify-content: space-between; }
.check { display: flex; align-items: center; gap: 8px; color: #111827; font-size: 14px; margin: 0; }
.result { white-space: pre-wrap; background: #f3f4f6; border-radius: 6px; padding: 8px; margin: 0; }
.recording { color: #b91c1c; font-weight: 600; margin: 0; }
.recording::before { content: "\25CF  "; }
"#;

/// RGBA pixels of the PNeX mark.
fn mark_rgba() -> (Vec<u8>, u32, u32) {
    let mut reader = png::Decoder::new(MARK).read_info().expect("embedded png");
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("embedded png");
    buf.truncate(info.buffer_size());
    (buf, info.width, info.height)
}

/// The mark with a red dot: the tray icon while a call is recorded.
fn recording_rgba() -> (Vec<u8>, u32, u32) {
    let (mut buf, w, h) = mark_rgba();
    let (cx, cy, r) = (w as f32 * 0.72, h as f32 * 0.72, w as f32 * 0.26);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            if d <= r {
                let i = ((y * w + x) * 4) as usize;
                let px = if d > r - w as f32 * 0.04 {
                    [255, 255, 255, 255]
                } else {
                    [220, 38, 38, 255]
                };
                buf[i..i + 4].copy_from_slice(&px);
            }
        }
    }
    (buf, w, h)
}

fn main() {
    if !queue::single_instance() {
        eprintln!("pnex-recorder is already running (see the tray icon)");
        return;
    }
    let (rgba, w, h) = mark_rgba();
    let icon = tao::window::Icon::from_rgba(rgba, w, h).expect("window icon");
    dioxus::LaunchBuilder::desktop()
        .with_cfg(
            Config::new()
                .with_window(
                    WindowBuilder::new()
                        .with_title("PNeX")
                        .with_inner_size(LogicalSize::new(400.0, 560.0)),
                )
                .with_icon(icon)
                .with_menu(None)
                // Closing the window keeps the recorder in the tray.
                .with_close_behaviour(WindowCloseBehaviour::WindowHides),
        )
        .launch(App);
}

/// OS language, French or English.
// shortcut: reads LANG-style variables only (Linux, macOS terminals), so
// Windows and macOS GUI launches start in English; read the OS API when needed.
pub(crate) fn system_locale() -> LanguageIdentifier {
    let tag = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .unwrap_or_default();
    if tag.starts_with("fr") {
        langid!("fr-FR")
    } else {
        langid!("en-US")
    }
}

/// Translated text of an API failure.
pub(crate) fn error_text(e: &ApiError) -> String {
    match e {
        ApiError::Unreachable => t!("err-unreachable"),
        ApiError::NotPnex => t!("err-not-pnex"),
        ApiError::Incompatible(contract) => t!("err-incompatible", contract: *contract),
        ApiError::BadCredentials => t!("err-bad-credentials"),
        // Shown as the trust card, not as an error line.
        ApiError::UntrustedCa { .. } => t!("err-unreachable"),
        ApiError::SessionExpired => t!("err-session-expired"),
        ApiError::Server(status, code) => match code.as_deref() {
            Some("access-denied") => t!("err-access-denied"),
            Some("media-asr-model-invalid") => t!("err-media-asr-model-invalid"),
            Some("asr-test-failed") => t!("err-asr-test-failed"),
            Some("asr-model-not-found") => t!("err-asr-model-not-found"),
            Some("media-analysis-no-asr") => t!("err-media-analysis-no-asr"),
            Some("recording-unreadable") => t!("err-recording-unreadable"),
            code => t!("err-server", status: *status, code: code.unwrap_or("-")),
        },
    }
}

#[component]
fn App() -> Element {
    use_init_i18n(|| {
        I18nConfig::new(system_locale())
            .with_locale((langid!("en-US"), include_str!("../locales/en-US.ftl")))
            .with_locale((langid!("fr-FR"), include_str!("../locales/fr-FR.ftl")))
            .with_fallback(langid!("en-US"))
    });
    let mut session = use_signal(|| Session::load().unwrap_or_default());
    let calls = calls::use_calls(session);

    // Tray: open, recording switch, discard the recording, quit.
    let (enabled_item, discard_item, tray) = use_hook(|| {
        let open = MenuItem::with_id("open", t!("tray-open"), true, None);
        let enabled = CheckMenuItem::with_id(
            "enabled",
            t!("tray-enabled"),
            true,
            session.peek().enabled,
            None,
        );
        let discard = MenuItem::with_id("discard", t!("tray-discard"), false, None);
        let quit = MenuItem::with_id("quit", t!("tray-quit"), true, None);
        let menu = Menu::new();
        let _ = menu.append_items(&[
            &open,
            &enabled,
            &discard,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);
        let (rgba, w, h) = mark_rgba();
        let tray = init_tray_icon(menu, TrayIcon::from_rgba(rgba, w, h).ok());
        (enabled, discard, tray)
    });
    use_tray_menu_event_handler(move |event| match event.id.0.as_str() {
        "open" => {
            window().set_visible(true);
            window().set_focus();
        }
        "enabled" => {
            let mut s = session.write();
            s.enabled = !s.enabled;
            s.save();
        }
        "discard" => calls.discard(),
        "quit" => std::process::exit(0),
        _ => {}
    });
    // The tray check mark follows the window switch, and the reverse.
    use_effect(move || enabled_item.set_checked(session.read().enabled));
    // A red dot and the discard entry while a call is recorded.
    let tray_state = tray.clone();
    use_effect(move || {
        let recording = calls.recording();
        let (rgba, w, h) = if recording {
            recording_rgba()
        } else {
            mark_rgba()
        };
        let _ = tray_state.set_icon(TrayIcon::from_rgba(rgba, w, h).ok());
        let tip = if recording {
            t!("tray-recording")
        } else {
            t!("app-title")
        };
        let _ = tray_state.set_tooltip(Some(tip));
        discard_item.set_enabled(recording);
    });

    rsx! {
        document::Title { {t!("app-title")} }
        style { {STYLE} }
        main {
            if session.read().signed_in() {
                Signed { session }
            } else {
                Login { session }
            }
        }
    }
}

#[component]
fn Login(session: Signal<Session>) -> Element {
    let mut url = use_signal(|| session.peek().url.clone());
    let mut email = use_signal(|| session.peek().email.clone());
    let mut password = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<ApiError>);
    // `ca`: a root CA the user just trusted, else the one pinned for this server.
    let mut connect = move |ca: Option<String>| {
        busy.set(true);
        error.set(None);
        spawn(async move {
            let ca = ca.or_else(|| session.peek().pinned_ca(&url()));
            match Session::sign_in(&url(), &email(), &password(), ca).await {
                Ok(mut s) => {
                    // Keep the capture switch across sign-ins.
                    s.enabled = session.peek().enabled;
                    s.save();
                    password.set(String::new());
                    session.set(s);
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    let submit = move |e: FormEvent| {
        e.prevent_default();
        connect(None);
    };
    rsx! {
        h1 { {t!("login-title")} }
        if let Some(ApiError::UntrustedCa { pem, fingerprint }) = error() {
            div { class: "card", role: "alertdialog",
                h2 { {t!("trust-ca-title")} }
                p { class: "muted", {t!("trust-ca-message", base : url())} }
                label { {t!("trust-ca-fingerprint")} }
                p {
                    class: "result",
                    style: "font-family: monospace; font-size: 12px; word-break: break-all;",
                    "{fingerprint}"
                }
                div { class: "row",
                    button { r#type: "button", onclick: move |_| error.set(None),
                        {t!("trust-ca-cancel")}
                    }
                    button {
                        id: "trust-ca",
                        class: "primary",
                        r#type: "button",
                        onclick: move |_| connect(Some(pem.clone())),
                        {t!("trust-ca-accept")}
                    }
                }
            }
        }
        form { class: "card", onsubmit: submit,
            div {
                label { r#for: "server", {t!("login-server")} }
                input {
                    id: "server",
                    r#type: "text",
                    placeholder: "https://pnex.example.com",
                    value: "{url}",
                    oninput: move |e| url.set(e.value()),
                }
            }
            div {
                label { r#for: "email", {t!("login-email")} }
                input {
                    id: "email",
                    r#type: "email",
                    value: "{email}",
                    oninput: move |e| email.set(e.value()),
                }
            }
            div {
                label { r#for: "password", {t!("login-password")} }
                input {
                    id: "password",
                    r#type: "password",
                    value: "{password}",
                    oninput: move |e| password.set(e.value()),
                }
            }
            if let Some(e) = error().filter(|e| !matches!(e, ApiError::UntrustedCa { .. })) {
                p { class: "error", role: "alert", {error_text(&e)} }
            }
            button {
                class: "primary",
                r#type: "submit",
                disabled: busy() || url().trim().is_empty() || email().trim().is_empty()
                    || password().is_empty(),
                if busy() {
                    {t!("login-busy")}
                } else {
                    {t!("login-submit")}
                }
            }
        }
    }
}

/// Runs `call` on a copy of the session, then writes the (possibly
/// refreshed) session back; an expired session signs out.
macro_rules! with_session {
    ($session:expr, |$s:ident| $call:expr) => {{
        let mut $s = $session.peek().clone();
        let out = $call.await;
        if matches!(out, Err(ApiError::SessionExpired)) {
            $s.sign_out();
        }
        if $s != *$session.peek() {
            $session.set($s);
        }
        out
    }};
}

#[component]
fn Signed(session: Signal<Session>) -> Element {
    let orgs = use_resource(move || async move { with_session!(session, |s| s.orgs()) });
    let orgs: Vec<OrgMembership> = match &*orgs.read() {
        Some(Ok(orgs)) => orgs.clone(),
        _ => Vec::new(),
    };
    // The saved org, else the first one (the personal org).
    let org = session
        .read()
        .org
        .filter(|id| orgs.iter().any(|o| o.id == *id))
        .or_else(|| orgs.first().map(|o| o.id));
    // Recordings go to this org: keep it in the session.
    use_effect(use_reactive((&org,), move |(resolved,)| {
        if resolved.is_some() && session.peek().org != resolved {
            let mut s = session.write();
            s.org = resolved;
            s.save();
        }
    }));
    let (email, server, enabled) = {
        let s = session.read();
        (s.email.clone(), s.url.clone(), s.enabled)
    };
    rsx! {
        div { class: "row",
            p { class: "muted", {t!("main-signed-in", email : email, server : server)} }
            button {
                r#type: "button",
                onclick: move |_| session.write().sign_out(),
                {t!("main-sign-out")}
            }
        }
        if orgs.len() > 1 {
            div {
                label { r#for: "org", {t!("main-org")} }
                select {
                    id: "org",
                    onchange: move |e| {
                        let mut s = session.write();
                        s.org = e.value().parse().ok();
                        s.save();
                    },
                    for o in orgs.iter() {
                        option {
                            key: "{o.id}",
                            value: "{o.id}",
                            selected: Some(o.id) == org,
                            "{o.name}"
                        }
                    }
                }
            }
        }
        div { class: "card",
            label { class: "check",
                input {
                    r#type: "checkbox",
                    checked: enabled,
                    onchange: move |e| {
                        let mut s = session.write();
                        s.enabled = e.checked();
                        s.save();
                    },
                }
                {t!("main-enabled")}
            }
            p { class: "muted", {t!("main-enabled-help")} }
        }
        calls::CallsCard {}
        if let Some(org) = org {
            TranscriptionTest { key: "{org}", session, org }
        }
    }
}

/// Default profile and checked transcription models of the org.
#[derive(Clone, PartialEq)]
struct Setup {
    profile: Option<AsrProfile>,
    models: Vec<AsrModel>,
}

#[derive(Clone, PartialEq)]
enum Run {
    Idle,
    Recording,
    Sending,
    Done(AsrTestResult),
    Failed(String),
}

#[component]
fn TranscriptionTest(session: Signal<Session>, org: i64) -> Element {
    let setup = use_resource(move || async move {
        let profile = with_session!(session, |s| s.default_profile(org))?;
        let models = with_session!(session, |s| s.models(org))?;
        Ok::<_, ApiError>(Setup { profile, models })
    });
    let mut advanced = use_signal(|| false);
    let mut picked = use_signal(String::new);
    let mut run = use_signal(|| Run::Idle);

    let setup = match &*setup.read() {
        None => {
            return rsx! {
                p { class: "muted", {t!("test-loading")} }
            }
        }
        Some(Err(e)) => {
            return rsx! {
                p { class: "error", {error_text(e)} }
            }
        }
        Some(Ok(setup)) => setup.clone(),
    };
    let usable: Vec<AsrModel> = setup
        .models
        .iter()
        .filter(|m| api::usable(m))
        .cloned()
        .collect();
    // Model and language of the run: the picked model in advanced mode,
    // else the default profile's.
    let target = if advanced() {
        usable
            .iter()
            .find(|m| m.id == picked())
            .or(usable.first())
            .map(|m| (m.id.clone(), "auto".to_string()))
    } else {
        setup
            .profile
            .as_ref()
            .map(|p| (p.asr_model_id.clone(), p.language.clone()))
    };
    let notice = if setup.models.is_empty() {
        Some(t!("test-no-model"))
    } else if usable.is_empty() {
        Some(t!("test-no-valid-model"))
    } else if !advanced() && setup.profile.is_none() {
        Some(t!("test-no-default"))
    } else {
        None
    };
    let busy = matches!(run(), Run::Recording | Run::Sending);
    let start = {
        let target = target.clone();
        move |_| {
            let Some((model, language)) = target.clone() else {
                return;
            };
            run.set(Run::Recording);
            spawn(async move {
                let clip = tokio::task::spawn_blocking(|| audio::record_wav(TEST_SECS)).await;
                let wav = match clip {
                    Ok(Ok(wav)) => wav,
                    Ok(Err(audio::CaptureError::NoMicrophone)) => {
                        run.set(Run::Failed(t!("err-no-microphone")));
                        return;
                    }
                    Ok(Err(audio::CaptureError::Device(detail))) => {
                        run.set(Run::Failed(t!("err-microphone", detail: detail)));
                        return;
                    }
                    Err(e) => {
                        run.set(Run::Failed(t!("err-microphone", detail: e.to_string())));
                        return;
                    }
                };
                run.set(Run::Sending);
                let res = with_session!(session, |s| s.test(org, &model, &language, wav));
                run.set(match res {
                    Ok(result) => Run::Done(result),
                    Err(e) => Run::Failed(error_text(&e)),
                });
            });
        }
    };

    rsx! {
        div { class: "card",
            h2 { {t!("test-title")} }
            p { class: "muted", {t!("test-help", secs : TEST_SECS)} }
            label { class: "check",
                input {
                    r#type: "checkbox",
                    checked: advanced(),
                    onchange: move |e| advanced.set(e.checked()),
                }
                {t!("test-advanced")}
            }
            if advanced() && !usable.is_empty() {
                div {
                    label { r#for: "model", {t!("test-model")} }
                    select { id: "model", onchange: move |e| picked.set(e.value()),
                        for m in usable.iter() {
                            option {
                                key: "{m.id}",
                                value: "{m.id}",
                                selected: target.as_ref().is_some_and(|(id, _)| *id == m.id),
                                "{m.name}"
                            }
                        }
                    }
                }
            }
            if !advanced() {
                if let Some(p) = &setup.profile {
                    p { class: "muted",
                        {t!("test-profile", name : p.name.clone(), language : p.language.clone())}
                    }
                }
            }
            if let Some(notice) = notice.clone() {
                p { class: "error", role: "status", {notice} }
            }
            button {
                class: "primary",
                r#type: "button",
                disabled: busy || target.is_none() || notice.is_some(),
                onclick: start,
                {t!("test-run", secs : TEST_SECS)}
            }
            match run() {
                Run::Idle => rsx! {},
                Run::Recording => rsx! {
                    p { class: "muted", {t!("test-recording", secs : TEST_SECS)} }
                },
                Run::Sending => rsx! {
                    p { class: "muted", {t!("test-sending")} }
                },
                Run::Done(r) => rsx! {
                    p { class: "muted", {t!("test-result", ms : r.infer_ms)} }
                    p { class: "result",
                        if r.text.trim().is_empty() {
                            {t!("test-empty-text")}
                        } else {
                            "{r.text}"
                        }
                    }
                },
                Run::Failed(msg) => rsx! {
                    p { class: "error", role: "alert", "{msg}" }
                },
            }
        }
    }
}
