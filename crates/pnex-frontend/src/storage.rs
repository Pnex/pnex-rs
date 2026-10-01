//! Stockage clé/valeur du front, abstrait de la plateforme.
//!
//! - **web (wasm32)** : `localStorage` (persistance) et `sessionStorage`
//!   (verifier PKCE — survit à la redirection vers l'IdP mais pas à la
//!   fermeture de l'onglet) via web-sys. Les navigateurs hostiles au storage
//!   (mode privé Safari) dégradent silencieusement no-op.
//! - **natif** : le store persistant est dosé par un fichier JSON dans le
//!   dossier privé de l'app (`pnex-storage.json`, Android : files dir via
//!   JNI brut — cf. `imp::app_files_dir`) ; le store session (verifier
//!   PKCE) reste mémoire seule. Desktop: per-user data dir (see
//!   `imp::app_files_dir`); no resolvable dir → memory only, silently.

// Socle posé avant ses consommateurs (session, PKCE, sélecteur d'org) : les
// clés et le storage session deviennent utilisés aux commits suivants.
#![allow(dead_code)]

/// Clés utilisées par l'app (préfixe `pnex.`).
pub const KEY_ACCESS_TOKEN: &str = "pnex.access_token";
pub const KEY_REFRESH_TOKEN: &str = "pnex.refresh_token";
pub const KEY_ID_TOKEN: &str = "pnex.id_token";
pub const KEY_ORG: &str = "pnex.org";
pub const KEY_LOCALE: &str = "pnex.locale";
/// Sidebar desktop repliée en rail d'icônes (préférence UI, cf. `state::ui`).
pub const KEY_SIDEBAR_RAIL: &str = "pnex.sidebar_rail";
/// URL du serveur auto-hébergé — cible desktop/mobile uniquement (le web est
/// same-origin, la clé n'est jamais écrite).
pub const KEY_API_BASE: &str = "pnex.api_base";
/// Root CA (PEM) of the self-hosted server's TLS edge, pinned after a
/// trust-on-first-use confirmation — native targets only (cf. `api::tls`).
pub const KEY_SERVER_CA: &str = "pnex.server_ca";
/// Verifier PKCE — stockage *session* (consommé au callback, jamais persisté).
pub const KEY_PKCE_VERIFIER: &str = "pnex.pkce_verifier";

pub trait KeyValueStorage {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
    fn remove(&self, key: &str);
    /// Supprime plusieurs clés d'un coup (logout).
    fn purge(&self, keys: &[&str]) {
        for key in keys {
            self.remove(key);
        }
    }
}

/// Stockage persistant (tokens, org, locale).
pub fn local() -> impl KeyValueStorage {
    LocalStorage
}

/// Stockage volatil onglet (verifier PKCE).
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

    /// Chemin du fichier store persistant — résolu une seule fois. Le nom
    /// de fichier est joint ICI (et pas dans `app_files_dir`) : sans lui on
    /// lit/écrit le répertoire lui-même (EISDIR, constat 2026-09-09).
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
                    eprintln!("pnex-storage: fichier illisible ({err}) — démarrage à vide");
                    HashMap::new()
                }
            },
            // Absent = premier lancement (pas une erreur).
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(err) => {
                eprintln!("pnex-storage: lecture impossible ({err}) — démarrage à vide");
                HashMap::new()
            }
        }
    }

    /// Écriture atomique (tmp + rename) — un kill pendant l'écriture ne
    /// laisse jamais un fichier à moitié écrit.
    fn save_store(map: &HashMap<String, String>) {
        let Some(path) = store_file() else { return };
        let tmp = path.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            // Un write en échec fait échouer le rename juste après (fichier
            // absent) — l'erreur remonte par le rename.
            let _ = std::fs::write(&tmp, serde_json::to_vec(map).unwrap_or_default());
            std::fs::rename(&tmp, path)
        };
        if let Err(err) = write() {
            eprintln!("pnex-storage: écriture impossible ({err})");
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

    /// Dossier privé de l'app (`Context.getFilesDir()`), par JNI brut sur le
    /// contexte posé par le glue Android de Dioxus — même école que le patch
    /// webbrowser vendu (jni bas niveau, jamais de panic : un échec dégrade
    /// en mémoire seule).
    #[cfg(target_os = "android")]
    fn app_files_dir() -> Option<std::path::PathBuf> {
        type EnvPtr = *mut jni::sys::JNIEnv;

        let ctx = ndk_context::try_android_context()?;
        let vm = ctx.vm() as *mut jni::sys::JavaVM;

        unsafe {
            // Threads du glue déjà attachés : GetEnv suffit ; sinon attach
            // daemon en filet de sécurité (tâches async de l'exécuteur).
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
                    eprintln!("pnex-storage: attach JVM impossible — mémoire seule");
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
                eprintln!("pnex-storage: getFilesDir JNI impossible — mémoire seule");
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
