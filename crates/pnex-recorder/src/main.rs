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
mod encode;
mod queue;
mod voice;

use dioxus::desktop::trayicon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use dioxus::desktop::trayicon::{init_tray_icon, Icon as TrayIcon};
use dioxus::desktop::{
    tao, use_muda_event_handler, window, Config, LogicalSize, WindowBuilder, WindowCloseBehaviour,
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
/// The wordmark of the web app's login card.
const WORDMARK: &[u8] = include_bytes!("../../pnex-frontend/assets/logo.png");
/// The web app's animated beam sheaf (login page), shared rather than copied.
const GERBE_JS: &str = include_str!("../../pnex-frontend/assets/tron-gerbe.js");

const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #f3f5f7; --surface: #ffffff; --ink: #1c2630; --muted: #5b6873;
  --line: #d4dbe1; --rec: #c8243a; --armed: #0f7472; --off: #9aa5ae;
  --error: #b3261e; --soft: #e9edf0;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #151b21; --surface: #1d252d; --ink: #e6ebef; --muted: #9aa7b2;
    --line: #2e3943; --rec: #f0566a; --armed: #3fb5b0; --off: #66727c;
    --error: #ff8a80; --soft: #26303a;
  }
}
:root[data-theme="dark"] {
    color-scheme: dark;
    --bg: #151b21; --surface: #1d252d; --ink: #e6ebef; --muted: #9aa7b2;
    --line: #2e3943; --rec: #f0566a; --armed: #3fb5b0; --off: #66727c;
    --error: #ff8a80; --soft: #26303a;
}
:root[data-theme="light"] { color-scheme: light; }
.theme { display: inline-flex; align-items: center; justify-content: center; width: 30px; height: 30px; padding: 0; border-radius: 50%; flex: none; }
.theme svg { width: 16px; height: 16px; }
.login-top { position: fixed; top: 10px; right: 12px; z-index: 2; }
* { box-sizing: border-box; }
body { font-family: system-ui, sans-serif; margin: 0; background: var(--bg); color: var(--ink); font-size: 14px; line-height: 1.4; }
main { padding: 14px 16px 12px; display: flex; flex-direction: column; gap: 12px; min-height: 100vh; }
h1 { font-size: 17px; margin: 0; font-weight: 600; }
h2 { font-size: 14px; margin: 0; font-weight: 600; }
p { margin: 0; }
label { display: block; font-size: 12px; color: var(--muted); margin-bottom: 4px; }
input[type=text], select { width: 100%; padding: 7px 9px; border: 1px solid var(--line); border-radius: 6px; font-size: 14px; background: var(--surface); color: var(--ink); }
button { padding: 8px 12px; border-radius: 6px; border: 1px solid var(--line); background: var(--surface); color: var(--ink); cursor: pointer; font-size: 14px; }
button.primary { background: var(--ink); color: var(--bg); border-color: var(--ink); }
button.link { border: 0; background: none; padding: 0; color: var(--muted); text-decoration: underline; font-size: 12px; white-space: nowrap; }
button:disabled { opacity: .5; cursor: default; }
:focus-visible { outline: 2px solid var(--armed); outline-offset: 2px; }
.card { background: var(--surface); border: 1px solid var(--line); border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 8px; }
.muted { color: var(--muted); font-size: 12px; }
.error { color: var(--error); font-size: 13px; }
.row { display: flex; align-items: center; gap: 8px; justify-content: space-between; }
.check { display: flex; align-items: center; gap: 8px; color: var(--ink); font-size: 14px; margin: 0; }
.score { font-size: 28px; font-weight: 600; }
meter, progress { width: 100%; height: 8px; accent-color: var(--armed); }
.result { white-space: pre-wrap; background: var(--soft); border-radius: 6px; padding: 8px; }
.account { display: flex; align-items: center; justify-content: space-between; gap: 8px; color: var(--muted); font-size: 12px; }
.account span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* The status block: the one loud element of the window. */
.status { background: var(--surface); border: 1px solid var(--line); border-left: 6px solid var(--off); border-radius: 8px; padding: 14px; display: flex; flex-direction: column; gap: 10px; }
.status.armed { border-left-color: var(--armed); }
.status.live { border-left-color: var(--rec); }
.lamp { display: flex; align-items: center; gap: 10px; font-size: 16px; font-weight: 600; color: var(--ink); margin: 0; }
.lamp::before { content: ""; width: 12px; height: 12px; border-radius: 50%; background: var(--off); flex: none; }
.armed .lamp::before { background: var(--armed); }
.live .lamp::before { background: var(--rec); box-shadow: 0 0 0 4px color-mix(in srgb, var(--rec) 25%, transparent); }
.clock { font-size: 34px; font-weight: 600; font-variant-numeric: tabular-nums; letter-spacing: .02em; color: var(--rec); }
.switch { appearance: none; width: 38px; height: 22px; border-radius: 11px; background: var(--off); position: relative; cursor: pointer; flex: none; margin: 0; }
.switch::after { content: ""; position: absolute; top: 3px; left: 3px; width: 16px; height: 16px; border-radius: 50%; background: #fff; transition: transform .15s; }
.switch:checked { background: var(--armed); }
.switch:checked::after { transform: translateX(16px); }
.activity { display: flex; flex-direction: column; gap: 6px; padding: 0 2px; }
details.checks { border-top: 1px solid var(--line); padding-top: 10px; }
details.checks > summary { cursor: pointer; color: var(--muted); font-size: 13px; }
details.checks[open] > summary { margin-bottom: 10px; }
/* Login: the web app's sign-in page, beam sheaf behind a light card. */
.login-bg { position: fixed; inset: 0; background: linear-gradient(135deg, #040d10 0%, #0b2830 100%); }
.login { position: relative; min-height: calc(100vh - 34px); display: flex; flex-direction: column; justify-content: center; }
.panel { color-scheme: light; --bg: #ffffff; --surface: #ffffff; --ink: #1c2630; --muted: #5b6873; --line: #d4dbe1; --soft: #e9edf0; --error: #b3261e;
  background: rgba(255, 255, 255, .95); color: var(--ink); border-radius: 16px; padding: 24px 20px; display: flex; flex-direction: column; gap: 16px;
  box-shadow: 0 24px 60px rgba(0, 0, 0, .45); }
.panel .card { border: 0; padding: 0; background: none; }
.wordmark { display: block; margin: 0 auto; height: 52px; width: auto; }
.center { text-align: center; }
.tools { display: flex; gap: 6px; flex: none; }
/* Pages of the web app, pinned to the bottom of the window. */
.dock { margin-top: auto; position: sticky; bottom: 0; background: var(--bg); border-top: 1px solid var(--line); padding: 10px 0 2px; display: flex; flex-direction: column; gap: 8px; }
/* Live meters of a recording: microphone and computer audio. */
.meters { display: flex; flex-direction: column; gap: 6px; }
.meter { display: grid; grid-template-columns: 110px 1fr; align-items: center; gap: 8px; font-size: 12px; color: var(--muted); }
.segments { display: grid; grid-template-columns: repeat(24, 1fr); gap: 2px; height: 12px; }
.segments i { background: var(--soft); border-radius: 1px; }
.segments i.on { background: var(--armed); }
.segments i.hot { background: var(--rec); }
/* Buttons with a pictogram: the bottom bar and the recording actions. */
.dock .row > button, .status .actions > button { flex: 1; display: inline-flex; align-items: center; justify-content: center; gap: 8px; }
.dock svg, .status button svg { width: 18px; height: 18px; flex: none; }
.dock svg { color: var(--muted); }
.status .actions { display: flex; gap: 8px; }
.status button.primary svg { color: var(--bg); }
#rec-start svg { color: var(--rec); }
#rec-start { display: inline-flex; align-items: center; justify-content: center; gap: 8px; }
.about { display: flex; justify-content: space-between; align-items: center; color: var(--muted); font-size: 12px; }
button.quiet { border: 0; background: none; padding: 2px 4px; color: var(--muted); font-size: 12px; }
button.quiet:hover { color: var(--ink); }
.panel .about { border-top: 1px solid var(--line); padding-top: 12px; }

details.checks > div { display: flex; flex-direction: column; gap: 12px; }
@media (prefers-reduced-motion: reduce) { .switch::after { transition: none; } }
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
        ApiError::LoginTimedOut => t!("err-login-timed-out"),
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
    // muda keeps the first menu event handler only (a OnceCell), and dioxus
    // registers the menubar one first: tray clicks arrive as menubar events.
    use_muda_event_handler(move |event| match event.id.0.as_str() {
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

    // The picked theme on <html>; none = the system's.
    use_effect(move || {
        let theme = session.read().theme.clone().unwrap_or_default();
        document::eval(&format!(
            "document.documentElement.setAttribute('data-theme', {theme:?})"
        ));
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
    let mark = use_hook(|| {
        use base64::Engine;
        let png = base64::engine::general_purpose::STANDARD.encode(WORDMARK);
        format!("data:image/png;base64,{png}")
    });
    // Frees the canvas and its GL context once signed in.
    use_drop(|| {
        document::eval("window.pnexTronGerbe && window.pnexTronGerbe.unmount()");
    });
    let mut url = use_signal(|| session.peek().url.clone());
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<ApiError>);
    // `ca`: a root CA the user just trusted, else the one pinned for this server.
    let mut connect = move |ca: Option<String>| {
        busy.set(true);
        error.set(None);
        spawn(async move {
            let ca = ca.or_else(|| session.peek().pinned_ca(&url()));
            match Session::sign_in(&url(), ca).await {
                Ok(mut s) => {
                    // Keep the capture switch across sign-ins.
                    s.enabled = session.peek().enabled;
                    s.save();
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
        // The web app's login: the beam sheaf over the whole window, the
        // sign-in on a light card in front of it.
        div {
            id: "gerbe",
            class: "login-bg",
                // Loads the script once per process, then mounts the canvas.
                onmounted: move |_| {
                    document::eval(
                        &format!(
                            "if (!window.pnexTronGerbe) {{ {GERBE_JS} }}\nwindow.pnexTronGerbe.mount('gerbe');",
                        ),
                    );
                },
        }
        div { class: "login-top",
            ThemeToggle { session }
        }
        div { class: "login",
        div { class: "panel",
        img { class: "wordmark", src: "{mark}", alt: "PNeX" }
        p { class: "muted center", {t!("login-title")} }
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
            if let Some(e) = error().filter(|e| !matches!(e, ApiError::UntrustedCa { .. })) {
                p { class: "error", role: "alert", {error_text(&e)} }
            }
            button {
                class: "primary",
                r#type: "submit",
                disabled: busy() || url().trim().is_empty(),
                if busy() {
                    {t!("login-busy")}
                } else {
                    {t!("login-submit")}
                }
            }
        }
        About {}
        }
        }
    }
}

/// Opens a page of the PNeX web app in the system browser.
pub(crate) fn open_web(server: &str, path: &str) {
    if let Err(e) = webbrowser::open(&format!("{server}{path}")) {
        eprintln!("browser opening failed: {e}");
    }
}

/// Version of the recorder and the PNeX site.
#[component]
fn About() -> Element {
    rsx! {
        div { class: "about",
            span { {t!("about-version", version : env!("CARGO_PKG_VERSION"))} }
            button {
                class: "quiet",
                r#type: "button",
                onclick: move |_| open_web("https://pnex.io", ""),
                "pnex.io"
            }
        }
    }
}

/// Light / dark switch. The first click from the system theme flips what
/// the system shows.
#[component]
fn ThemeToggle(session: Signal<Session>) -> Element {
    let flip = move |_| {
        spawn(async move {
            let dark = match session.peek().theme.as_deref() {
                Some(t) => t == "dark",
                None => document::eval(
                    "return window.matchMedia('(prefers-color-scheme: dark)').matches",
                )
                .join::<bool>()
                .await
                .unwrap_or(false),
            };
            let mut s = session.write();
            s.theme = Some(if dark { "light" } else { "dark" }.into());
            s.save();
        });
    };
    rsx! {
        button {
            class: "theme",
            r#type: "button",
            title: t!("theme-toggle"),
            aria_label: t!("theme-toggle"),
            onclick: flip,
            Icon { d: MOON }
        }
    }
}

/// The media library filtered on the recorder's label.
const RECORDINGS: &str = "/media?label=source:recorder";

const MOON: &str = "M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9z";
const WAVEFORM: &str = "M3 12h2M7 8v8M11 4v16M15 8v8M19 11v2";
const PERSON: &str = "M12 4a4 4 0 1 1 0 8a4 4 0 1 1 0-8M4 21c1.5-4 4.5-6 8-6s6.5 2 8 6";
const SIGN_OUT: &str = "M10 4H5v16h5M14 8l4 4-4 4M18 12H9";
/// Record: a dot in a ring.
pub(crate) const RECORD: &str =
    "M12 3a9 9 0 1 1 0 18a9 9 0 1 1 0-18M12 9a3 3 0 1 1 0 6a3 3 0 1 1 0-6";
/// Send: a paper plane.
pub(crate) const SEND: &str = "M21 3L10 14M21 3l-7 18-4-7-7-4z";
/// Discard: a bin.
pub(crate) const DISCARD: &str = "M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6";

/// A 24 px line icon drawn from one SVG path.
#[component]
pub(crate) fn Icon(d: &'static str) -> Element {
    rsx! {
        svg {
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d }
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
pub(crate) use with_session;

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
    let (email, server) = {
        let s = session.read();
        (s.email.clone(), s.url.clone())
    };
    rsx! {
        div { class: "account",
            span { {t!("main-signed-in", email : email, server : server)} }
            div { class: "tools",
                ThemeToggle { session }
                button {
                    class: "theme",
                    r#type: "button",
                    title: t!("main-sign-out"),
                    aria_label: t!("main-sign-out"),
                    onclick: move |_| session.write().sign_out(),
                    Icon { d: SIGN_OUT }
                }
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
        calls::CallsCard {}
        if let Some(org) = org {
            details { class: "checks",
                summary { {t!("checks-title")} }
                div {
                    TranscriptionTest { key: "{org}", session, org }
                    voice::VoiceCard { key: "{org}", session, org }
                }
            }
        }
        nav { class: "dock", aria_label: t!("web-open"),
            div { class: "row",
                button {
                    r#type: "button",
                    title: t!("web-open"),
                    onclick: move |_| open_web(&session.peek().url, RECORDINGS),
                    Icon { d: WAVEFORM }
                    {t!("web-recordings")}
                }
                button {
                    r#type: "button",
                    title: t!("web-open"),
                    onclick: move |_| open_web(&session.peek().url, "/profile"),
                    Icon { d: PERSON }
                    {t!("web-profile")}
                }
            }
            About {}
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
