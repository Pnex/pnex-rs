//! Call detection (media.md §3.13): a call app holding the
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

/// The first stream of a call app among `(app names, device index)`.
pub fn call_device(streams: &[(Vec<String>, u32)]) -> Option<u32> {
    streams
        .iter()
        .find(|(names, _)| call_app(names).is_some())
        .map(|(_, device)| *device)
}

/// The microphone and the output the call app really uses, as PulseAudio
/// source names (`None` each = the default device). A call app that is
/// not on the default devices (a USB headset picked in the app) would
/// otherwise leave the recording with an idle microphone and a silent
/// output: seen on 2026-11-16, a Brave call on a headset recorded at
/// −40 dB on the microphone channel and digital silence on the other.
#[cfg(target_os = "linux")]
pub fn call_devices() -> Result<(Option<String>, Option<String>), String> {
    use std::cell::RefCell;
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
    let names = |props: &libpulse_binding::proplist::Proplist| -> Vec<String> {
        props
            .get_str("application.process.binary")
            .into_iter()
            .chain(props.get_str("application.name"))
            .collect()
    };
    // Record streams → their source; playback streams → their sink.
    let inputs = Rc::new(RefCell::new(Vec::new()));
    let outputs = Rc::new(RefCell::new(Vec::new()));
    let pending = Rc::new(RefCell::new(2u8));
    let (ins, p1) = (inputs.clone(), pending.clone());
    let _a = context
        .introspect()
        .get_source_output_info_list(move |item| match item {
            ListResult::Item(i) => ins.borrow_mut().push((names(&i.proplist), i.source)),
            _ => *p1.borrow_mut() -= 1,
        });
    let (outs, p2) = (outputs.clone(), pending.clone());
    let _b = context
        .introspect()
        .get_sink_input_info_list(move |item| match item {
            ListResult::Item(i) => outs.borrow_mut().push((names(&i.proplist), i.sink)),
            _ => *p2.borrow_mut() -= 1,
        });
    while *pending.borrow() > 0 {
        step(&mut mainloop)?;
    }
    let source = call_device(&inputs.borrow());
    let sink = call_device(&outputs.borrow());
    // Index → name: the source itself, the sink's monitor source.
    let mic = Rc::new(RefCell::new(None));
    let monitor = Rc::new(RefCell::new(None));
    let pending = Rc::new(RefCell::new(0u8));
    let mut _ops = Vec::new();
    if let Some(index) = source {
        *pending.borrow_mut() += 1;
        let (m, p) = (mic.clone(), pending.clone());
        _ops.push(Box::new(
            context
                .introspect()
                .get_source_info_by_index(index, move |item| match item {
                    ListResult::Item(i) => *m.borrow_mut() = i.name.as_ref().map(|n| n.to_string()),
                    _ => *p.borrow_mut() -= 1,
                }),
        ) as Box<dyn std::any::Any>);
    }
    if let Some(index) = sink {
        *pending.borrow_mut() += 1;
        let (m, p) = (monitor.clone(), pending.clone());
        _ops.push(Box::new(context.introspect().get_sink_info_by_index(
            index,
            move |item| match item {
                ListResult::Item(i) => {
                    *m.borrow_mut() = i.monitor_source_name.as_ref().map(|n| n.to_string())
                }
                _ => *p.borrow_mut() -= 1,
            },
        )) as Box<dyn std::any::Any>);
    }
    while *pending.borrow() > 0 {
        step(&mut mainloop)?;
    }
    context.disconnect();
    let (mic, monitor) = (mic.borrow().clone(), monitor.borrow().clone());
    Ok((mic, monitor))
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
    use super::{call_app, call_device};

    /// Live: what the running call app uses (needs PulseAudio and a call).
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn live_call_devices() {
        println!("{:?}", super::call_devices());
    }

    #[test]
    fn the_call_app_devices_win_over_others() {
        let s = |n: &str, d: u32| (vec![n.to_string()], d);
        assert_eq!(
            call_device(&[s("pnex-recorder", 1), s("Brave", 7)]),
            Some(7)
        );
        assert_eq!(call_device(&[s("audacity", 3)]), None);
        assert_eq!(call_device(&[]), None);
    }

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
