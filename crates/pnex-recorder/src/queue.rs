//! Recordings waiting for the server: one WAV per call in the data
//! directory, deleted only once the server has stored it.

use std::path::PathBuf;

fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("pnex").join("recordings"))
}

/// Takes the single-instance lock, held for the life of the process: two
/// recorders would capture the same call into the same file. The OS drops
/// the lock when the process ends, even on a crash. `false`: another
/// recorder runs.
pub fn single_instance() -> bool {
    static LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
    let Some(dir) = dir() else {
        // No data directory: no recording can be written, nothing to protect.
        return true;
    };
    let file = std::fs::create_dir_all(&dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("recorder.lock"))
    });
    match file {
        Ok(file) if file.try_lock().is_ok() => {
            let _ = LOCK.set(file);
            true
        }
        Ok(_) => false,
        // Unwritable data directory: recording will report it, run.
        Err(_) => true,
    }
}

/// App part of the file name of a recording started by hand.
pub const MANUAL: &str = "manual";

/// File of a new recording: `<local date>_<app>.wav`, the name the server
/// title is rebuilt from.
pub fn new_path(app: Option<&str>) -> Result<PathBuf, String> {
    let dir = dir().ok_or("no data directory (set XDG_DATA_HOME)")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let app: String = app
        .unwrap_or(MANUAL)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    Ok(dir.join(format!("{stamp}_{app}.wav")))
}

/// Closed recordings, oldest first. `open` is the one being written.
pub fn pending(open: Option<&std::path::Path>) -> Vec<PathBuf> {
    let Some(dir) = dir() else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "wav") && Some(p.as_path()) != open)
        .collect();
    files.sort();
    files
}

/// `(date "YYYY-MM-DD HH:MM", app)` of a recording file name; `None` for a
/// file this recorder did not name.
pub fn describe(path: &std::path::Path) -> Option<(String, String)> {
    let stem = path.file_stem()?.to_str()?;
    let (date, app) = stem.rsplit_once('_')?;
    let (day, time) = date.split_once('_')?;
    let time = time.get(..5)?.replace('-', ":");
    Some((format!("{day} {time}"), app.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_file_name_gives_back_its_date_and_app() {
        let p = std::path::Path::new("/x/2026-10-29_22-14-05_Teams.wav");
        assert_eq!(
            super::describe(p),
            Some(("2026-10-29 22:14".into(), "Teams".into()))
        );
        assert_eq!(super::describe(std::path::Path::new("/x/other.wav")), None);
    }
}
