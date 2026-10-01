//! Journal fichier pour le smoke test device — le logger logcat ne sort
//! pas (constat 2026-09-09) et la ligne debug DOM est racy (re-render qui
//! réinitialise le textContent toutes les 33 ms).
//!
//! Écrit en append dans `files/take360.log` — lisible via
//! `adb shell run-as com.example.PnexFrontend cat files/take360.log`
//! (APK debug-signé). Jamais de panic : tout échec dégrade en no-op.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

static LOG_PATH: OnceLock<Option<Mutex<std::path::PathBuf>>> = OnceLock::new();

/// (interne) Chemin du fichier — résolu une fois (JNI getFilesDir, école
/// capture.rs).
#[cfg(target_os = "android")]
fn path() -> &'static Option<Mutex<std::path::PathBuf>> {
    LOG_PATH.get_or_init(|| {
        let env = crate::capture::attach_env()?;
        let ctx = ndk_context::try_android_context()?;
        let ctx_obj = ctx.context() as jni::sys::jobject;

        unsafe {
            let ctx_class = ((**env).v1_1.FindClass)(env, c"android/content/Context".as_ptr());
            if ctx_class.is_null() {
                return None;
            }
            let mid = ((**env).v1_1.GetMethodID)(
                env,
                ctx_class,
                c"getFilesDir".as_ptr(),
                c"()Ljava/io/File;".as_ptr(),
            );
            if mid.is_null() {
                return None;
            }
            let file = ((**env).v1_1.CallObjectMethodA)(env, ctx_obj, mid, std::ptr::null())
                as jni::sys::jobject;
            if file.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                ((**env).v1_1.ExceptionClear)(env);
                return None;
            }
            // File.getAbsolutePath()
            let file_class = ((**env).v1_1.FindClass)(env, c"java/io/File".as_ptr());
            if file_class.is_null() {
                return None;
            }
            let mid_path = ((**env).v1_1.GetMethodID)(
                env,
                file_class,
                c"getAbsolutePath".as_ptr(),
                c"()Ljava/lang/String;".as_ptr(),
            );
            if mid_path.is_null() {
                return None;
            }
            let jstr = ((**env).v1_1.CallObjectMethodA)(env, file, mid_path, std::ptr::null())
                as jni::sys::jstring;
            if jstr.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                ((**env).v1_1.ExceptionClear)(env);
                return None;
            }
            let chars = ((**env).v1_1.GetStringUTFChars)(env, jstr, std::ptr::null_mut());
            if chars.is_null() {
                return None;
            }
            let dir = std::ffi::CStr::from_ptr(chars)
                .to_string_lossy()
                .into_owned();
            ((**env).v1_1.ReleaseStringUTFChars)(env, jstr, chars);
            Some(Mutex::new(
                std::path::PathBuf::from(dir).join("take360.log"),
            ))
        }
    })
}

/// Desktop (smoke test natif éventuel) : le même fichier dans le répertoire
/// temporaire — pas de JNI hors Android (check natif rouge sinon, constat
/// 2026-09-10 : `jni`/`ndk_context` sont des deps cfg(android)).
#[cfg(not(target_os = "android"))]
fn path() -> &'static Option<Mutex<std::path::PathBuf>> {
    LOG_PATH.get_or_init(|| Some(Mutex::new(std::env::temp_dir().join("pnex-take360.log"))))
}

/// Journalise une ligne (horodatée). Jamais de panic, jamais de blocage
/// critique — tout échec est no-op.
pub fn log(msg: &str) {
    let Some(mutex) = path() else { return };
    let Ok(path) = mutex.lock() else { return };
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&*path)
    else {
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() % 10_000_000)
        .unwrap_or(0);
    let _ = writeln!(f, "{ts} {msg}");
}
