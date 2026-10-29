//! Call detection (call-recording.md R1.1): a call app holding the
//! microphone. Linux: the PulseAudio record streams (source outputs) and the
//! app behind each one. Windows: the microphone entries of the privacy
//! registry that are in use.

/// Call apps, matched in the lowercase binary or app name. Browsers are in:
/// Meet, Teams web and the like run there.
const CALL_APPS: [(&str, &str); 14] = [
    ("teams", "Teams"),
    ("zoom", "Zoom"),
    ("slack", "Slack"),
    ("webex", "Webex"),
    ("discord", "Discord"),
    ("skype", "Skype"),
    ("signal", "Signal"),
    ("whatsapp", "WhatsApp"),
    ("jitsi", "Jitsi"),
    ("chrom", "Chrome"),
    ("firefox", "Firefox"),
    ("msedge", "Edge"),
    ("brave", "Brave"),
    ("opera", "Opera"),
];

/// Display name of the first call app among the apps recording, the
/// recorder itself excluded.
pub fn call_app(recording: &[String]) -> Option<&'static str> {
    recording
        .iter()
        .map(|name| name.to_lowercase())
        .filter(|name| !name.contains("pnex"))
        .find_map(|name| {
            CALL_APPS
                .iter()
                .find(|(needle, _)| name.contains(needle))
                .map(|(_, label)| *label)
        })
}

/// Binary and app names of every app recording audio right now.
#[cfg(target_os = "linux")]
pub fn recording_apps() -> Result<Vec<String>, String> {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use libpulse_binding::callbacks::ListResult;
    use libpulse_binding::context::{Context, FlagSet, State};
    use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};

    let mut mainloop = Mainloop::new().ok_or("no PulseAudio main loop")?;
    let mut context = Context::new(&mainloop, "pnex-recorder").ok_or("no PulseAudio context")?;
    context
        .connect(None, FlagSet::NOFLAGS, None)
        .map_err(|e| format!("{e}"))?;
    let step = |mainloop: &mut Mainloop| match mainloop.iterate(true) {
        IterateResult::Success(_) => Ok(()),
        _ => Err("PulseAudio main loop stopped".to_string()),
    };
    loop {
        step(&mut mainloop)?;
        match context.get_state() {
            State::Ready => break,
            State::Failed | State::Terminated => return Err("PulseAudio unreachable".into()),
            _ => {}
        }
    }
    let names = Rc::new(RefCell::new(Vec::new()));
    let done = Rc::new(Cell::new(false));
    let (out, end) = (names.clone(), done.clone());
    let _op = context
        .introspect()
        .get_source_output_info_list(move |item| match item {
            ListResult::Item(info) => {
                // Both: an Electron or browser app may only show itself in one.
                let props = &info.proplist;
                let mut out = out.borrow_mut();
                out.extend(props.get_str("application.process.binary"));
                out.extend(props.get_str("application.name"));
            }
            ListResult::End | ListResult::Error => end.set(true),
        });
    while !done.get() {
        step(&mut mainloop)?;
    }
    context.disconnect();
    let names = names.borrow().clone();
    Ok(names)
}

/// A microphone consent entry is in use: started and not stopped yet.
#[cfg(any(windows, test))]
pub fn in_use(last_start: u64, last_stop: u64) -> bool {
    last_start != 0 && last_stop == 0
}

/// Apps holding the microphone right now, from the Windows privacy
/// registry (`ConsentStore\microphone`): packaged apps by family name,
/// desktop apps under `NonPackaged` by their path (`#` for `\`).
#[cfg(windows)]
pub fn recording_apps() -> Result<Vec<String>, String> {
    use windows_registry::{Key, CURRENT_USER};

    const STORE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
    let base = CURRENT_USER.open(STORE).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut visit = |key: &Key, name: String| {
        let start = key.get_u64("LastUsedTimeStart").unwrap_or(0);
        let stop = key.get_u64("LastUsedTimeStop").unwrap_or(1);
        if in_use(start, stop) {
            out.push(name);
        }
    };
    for name in base.keys().map_err(|e| e.to_string())? {
        let Ok(key) = base.open(&name) else { continue };
        if name == "NonPackaged" {
            for app in key.keys().map_err(|e| e.to_string())? {
                if let Ok(k) = key.open(&app) {
                    visit(&k, app);
                }
            }
        } else {
            visit(&key, name);
        }
    }
    Ok(out)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn recording_apps() -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::call_app;

    #[test]
    fn a_call_app_holding_the_mic_is_found_the_recorder_never() {
        let apps = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            call_app(&apps(&["pnex-recorder", "teams-for-linux"])),
            Some("Teams")
        );
        assert_eq!(call_app(&apps(&["ZOOM"])), Some("Zoom"));
        assert_eq!(call_app(&apps(&["chromium-browser"])), Some("Chrome"));
        assert_eq!(call_app(&apps(&["pnex-recorder"])), None);
        assert_eq!(call_app(&apps(&["obs", "audacity"])), None);
        assert_eq!(call_app(&[]), None);
        // Windows names: a desktop app path, a packaged app family.
        let win = apps(&[
            "C:#Program Files#Zoom#bin#Zoom.exe",
            "MSTeams_8wekyb3d8bbwe",
        ]);
        assert_eq!(call_app(&win), Some("Zoom"));
        assert_eq!(call_app(&apps(&["C:#Users#a#pnex-recorder.exe"])), None);
        assert!(super::in_use(133_000, 0));
        assert!(!super::in_use(133_000, 133_100));
        assert!(!super::in_use(0, 0));
    }
}
