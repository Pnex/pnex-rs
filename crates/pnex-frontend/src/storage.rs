//! Front key/value storage, abstracted from the platform.
//!
//! - **web (wasm32)**: `localStorage` (persistence) and `sessionStorage`
//!   (PKCE verifier — survives the redirect to the IdP but not closing
//!   the tab) via web-sys. Storage-hostile browsers
//!   (Safari private mode) silently degrade to no-op.
//! - **native**: the persistent store is backed by a JSON file in the
//!   app's private folder (`pnex-storage.json`, Android: files dir via
//!   raw JNI — see `imp::app_files_dir`); the session store (PKCE
//!   verifier) stays memory only. Desktop: per-user data dir (see
//!   `imp::app_files_dir`); no resolvable dir → memory only, silently.

// Base laid before its consumers (session, PKCE, org selector): the
// keys and the session storage become used in the following commits.
#![allow(dead_code)]

/// Keys used by the app (`pnex.` prefix).
pub const KEY_ACCESS_TOKEN: &str = "pnex.access_token";
pub const KEY_REFRESH_TOKEN: &str = "pnex.refresh_token";
pub const KEY_ID_TOKEN: &str = "pnex.id_token";
/// Set by a logout that could not end the IdP session (no id_token):
/// the next login asks Rauthy for a fresh sign-in (SEC-41).
pub const KEY_FORCE_LOGIN: &str = "pnex.force_login";
pub const KEY_ORG: &str = "pnex.org";
pub const KEY_LOCALE: &str = "pnex.locale";
/// Desktop sidebar collapsed into an icon rail (UI preference, see `state::ui`).
pub const KEY_SIDEBAR_RAIL: &str = "pnex.sidebar_rail";
/// Last applied theme preference (light/dark/auto), replayed at boot so the
/// login screen does not flash light before the profile arrives.
pub const KEY_THEME: &str = "pnex.theme";
/// Basemap chosen on the map, per org: `pnex.basemap.<org id>` (geo-layers.md L25).
pub const KEY_BASEMAP_PREFIX: &str = "pnex.basemap.";
/// URL of the self-hosted server — desktop/mobile target only (the web is
/// same-origin, the key is never written).
pub const KEY_API_BASE: &str = "pnex.api_base";
/// Root CA (PEM) of the self-hosted server's TLS edge, pinned after a
/// trust-on-first-use confirmation — native targets only (cf. `api::tls`).
pub const KEY_SERVER_CA: &str = "pnex.server_ca";
/// PKCE verifier — *session* storage (consumed at callback, never persisted).
pub const KEY_PKCE_VERIFIER: &str = "pnex.pkce_verifier";

pub trait KeyValueStorage {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
    fn remove(&self, key: &str);
    /// Deletes several keys at once (logout).
    fn purge(&self, keys: &[&str]) {
        for key in keys {
            self.remove(key);
        }
    }
}

/// Persistent storage (tokens, org, locale).
pub fn local() -> impl KeyValueStorage {
    LocalStorage
}

/// Volatile per-tab storage (PKCE verifier).
pub fn session() -> impl KeyValueStorage {
    SessionStorage
}

pub struct LocalStorage;
pub struct SessionStorage;

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::{KeyValueStorage, LocalStorage, SessionStorage};

    fn web_storage(persistent: bool) -> Option<web_sys::Storage> {
        web_sys::window().and_then(|w| {
            if persistent {
                w.local_storage().ok().flatten()
            } else {
                w.session_storage().ok().flatten()
            }
        })
    }

    macro_rules! web_impl {
        ($ty:ty, $persistent:expr) => {
            impl KeyValueStorage for $ty {
                fn get(&self, key: &str) -> Option<String> {
                    web_storage($persistent).and_then(|s| s.get_item(key).ok().flatten())
                }
                fn set(&self, key: &str, value: &str) {
                    if let Some(s) = web_storage($persistent) {
                        let _ = s.set_item(key, value);
                    }
                }
                fn remove(&self, key: &str) {
                    if let Some(s) = web_storage($persistent) {
                        let _ = s.remove_item(key);
                    }
                }
            }
        };
    }

    web_impl!(LocalStorage, true);
    web_impl!(SessionStorage, false);
}

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::{KeyValueStorage, LocalStorage, SessionStorage};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    /// Path of the persistent store file — resolved only once. The file
    /// name is joined HERE (and not in `app_files_dir`): without it we
    /// read/write the directory itself (EISDIR, finding 2026-09-09).
    fn store_file() -> &'static Option<std::path::PathBuf> {
        static PATH: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
        // Unit tests stay memory-only: never read or clobber the real user store.
        if cfg!(test) {
            return &None;
        }
        PATH.get_or_init(|| app_files_dir().map(|dir| dir.join("pnex-storage.json")))
    }

    fn load_store() -> HashMap<String, String> {
        let Some(path) = store_file() else {
            return HashMap::new();
        };
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(map) => map,
                Err(err) => {
                    eprintln!("pnex-storage: unreadable file ({err}) — starting empty");
                    HashMap::new()
                }
            },
            // Absent = first launch (not an error).
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(err) => {
                eprintln!("pnex-storage: read failed ({err}) — starting empty");
                HashMap::new()
            }
        }
    }

    /// Atomic write (tmp + rename) — a kill during the write never
    /// leaves a half-written file.
    fn save_store(map: &HashMap<String, String>) {
        let Some(path) = store_file() else { return };
        let tmp = path.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            // A failed write makes the rename right after fail (file
            // missing) — the error surfaces through the rename.
            let _ = std::fs::write(&tmp, serde_json::to_vec(map).unwrap_or_default());
            std::fs::rename(&tmp, path)
        };
        if let Err(err) = write() {
            eprintln!("pnex-storage: write failed ({err})");
        }
    }

    fn memory(persistent: bool) -> &'static Mutex<HashMap<String, String>> {
        static LOCAL: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        static SESSION: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        if persistent {
            LOCAL.get_or_init(|| Mutex::new(load_store()))
        } else {
            SESSION.get_or_init(Default::default)
        }
    }

    macro_rules! mem_impl {
        ($ty:ty, $persistent:expr) => {
            impl KeyValueStorage for $ty {
                fn get(&self, key: &str) -> Option<String> {
                    memory($persistent).lock().ok()?.get(key).cloned()
                }
                fn set(&self, key: &str, value: &str) {
                    if let Ok(mut map) = memory($persistent).lock() {
                        map.insert(key.to_string(), value.to_string());
                        if $persistent {
                            save_store(&map);
                        }
                    }
                }
                fn remove(&self, key: &str) {
                    if let Ok(mut map) = memory($persistent).lock() {
                        map.remove(key);
                        if $persistent {
                            save_store(&map);
                        }
                    }
                }
            }
        };
    }

    mem_impl!(LocalStorage, true);
    mem_impl!(SessionStorage, false);

    /// Private app folder (`Context.getFilesDir()`), via raw JNI on the
    /// context set by Dioxus's Android glue — same school as the vendored
    /// webbrowser patch (low-level jni, never a panic: a failure degrades
    /// to memory only).
    #[cfg(target_os = "android")]
    fn app_files_dir() -> Option<std::path::PathBuf> {
        type EnvPtr = *mut jni::sys::JNIEnv;

        let ctx = ndk_context::try_android_context()?;
        let vm = ctx.vm() as *mut jni::sys::JavaVM;

        unsafe {
            // Glue threads already attached: GetEnv is enough; otherwise a
            // daemon attach as a safety net (async tasks of the executor).
            let mut env: EnvPtr = std::ptr::null_mut();
            let status = ((**vm).v1_2.GetEnv)(
                vm,
                (&mut env as *mut EnvPtr).cast::<*mut std::ffi::c_void>(),
                jni::sys::JNI_VERSION_1_6,
            );
            if status != jni::sys::JNI_OK || env.is_null() {
                let args = jni::sys::JavaVMAttachArgs {
                    version: jni::sys::JNI_VERSION_1_6,
                    name: c"pnex-storage".as_ptr().cast_mut(),
                    group: std::ptr::null_mut(),
                };
                let attached = ((**vm).v1_4.AttachCurrentThreadAsDaemon)(
                    vm,
                    (&mut env as *mut EnvPtr).cast::<*mut std::ffi::c_void>(),
                    std::ptr::from_ref(&args)
                        .cast::<std::ffi::c_void>()
                        .cast_mut(),
                );
                if attached != jni::sys::JNI_OK || env.is_null() {
                    eprintln!("pnex-storage: JVM attach failed — memory only");
                    return None;
                }
            }

            // Context.getFilesDir() → java.io.File
            let ctx_obj = ctx.context() as jni::sys::jobject;
            let Some(files_dir) = (|| {
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

                // File.getAbsolutePath() → String
                let file_class = ((**env).v1_1.FindClass)(env, c"java/io/File".as_ptr());
                if file_class.is_null() {
                    return None;
                }
                let mid_abs = ((**env).v1_1.GetMethodID)(
                    env,
                    file_class,
                    c"getAbsolutePath".as_ptr(),
                    c"()Ljava/lang/String;".as_ptr(),
                );
                if mid_abs.is_null() {
                    return None;
                }
                let jstr = ((**env).v1_1.CallObjectMethodA)(env, file, mid_abs, std::ptr::null())
                    as jni::sys::jstring;
                if jstr.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                    ((**env).v1_1.ExceptionClear)(env);
                    return None;
                }

                let chars = ((**env).v1_1.GetStringUTFChars)(env, jstr, std::ptr::null_mut());
                if chars.is_null() {
                    return None;
                }
                let path = std::ffi::CStr::from_ptr(chars)
                    .to_string_lossy()
                    .into_owned();
                ((**env).v1_1.ReleaseStringUTFChars)(env, jstr, chars);
                Some(path)
            })() else {
                eprintln!("pnex-storage: getFilesDir JNI failed — memory only");
                return None;
            };
            Some(std::path::PathBuf::from(files_dir))
        }
    }

    /// Desktop: per-user data dir (`%APPDATA%\PNEX`, `~/Library/Application
    /// Support/PNEX`, `$XDG_DATA_HOME/pnex` or `~/.local/share/pnex`),
    /// created on first use. Unresolvable → memory only.
    #[cfg(not(target_os = "android"))]
    fn app_files_dir() -> Option<std::path::PathBuf> {
        use std::path::PathBuf;
        let env_dir = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let dir = if cfg!(windows) {
            env_dir("APPDATA")?.join("PNEX")
        } else if cfg!(target_os = "macos") {
            env_dir("HOME")?.join("Library/Application Support/PNEX")
        } else {
            env_dir("XDG_DATA_HOME")
                .or_else(|| env_dir("HOME").map(|home| home.join(".local/share")))?
                .join("pnex")
        };
        match std::fs::create_dir_all(&dir) {
            Ok(()) => Some(dir),
            Err(err) => {
                eprintln!(
                    "pnex-storage: cannot create {} ({err}) — memory only",
                    dir.display()
                );
                None
            }
        }
    }
}
