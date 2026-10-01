//! Capture photo native Android (D21) — intent système `ACTION_IMAGE_CAPTURE`
//! sans déclarer la permission CAMERA (l'app caméra la détient déjà — aucune
//! API runtime-permissions nécessaire), sortie dans un fichier du cache dir
//! via `FileProvider` + `EXTRA_OUTPUT`, résultat détecté par **polling** du
//! fichier (jamais `onActivityResult` : la MainActivity est générée par dx).
//!
//! Le pattern est celui du pont login (polling + `spawn_forever` +
//! course `util::sleep`) et de la page pont : l'app passe en arrière-plan
//! (freezer API 36) pendant la capture — le polling ne tourne qu'au premier
//! plan, ce qui suffit puisque le retour caméra remet l'app au premier plan.
//! L'« exception fantôme » API 36 post-`startActivity` est classée (log +
//! poursuite), jamais traitée comme un échec — école patch webbrowser vendu.
//!
//! Upload immédiat en version 1 d'un nouvel asset `photo`, puis suppression
//! du fichier temporaire dans tous les cas.
//!
//! Hors Android : stubs no-op (le bouton n'est rendu que hors wasm —
//! garde `task check` natif/desktop vert).

/// Libellés résolus AVANT le détachement du watcher : `t!` exige le contexte
/// I18n fourni au composant — depuis `spawn_forever` il n'existe pas
/// (« Could not find context I18n » → panic → mutex runtime empoisonné →
/// abort en cascade, constat 2026-09-09). La page les résout dans le clic.
#[allow(dead_code)] // champs consommés par le watcher cfg android uniquement
pub struct CaptureMessages {
    pub launched: String,
    pub captured: String,
    pub failed: String,
    /// Permission caméra non accordée (pre-check Take 360, cf. bas de
    /// fichier).
    pub permission_denied: String,
}

/// Résultat du watcher, sondé par la page Média (atomiques purs : depuis
/// `spawn_forever`, tout accès GlobalSignal/toasts consomme un contexte
/// absent de ce scope → panic « Could not find context » → mutex runtime
/// empoisonné → abort en cascade, constat 2026-09-09).
pub static CAPTURE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static CAPTURE_RESULT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
// 0 = rien, 1 = upload OK, 2 = upload KO

pub fn capture_state() -> (u64, u8) {
    (
        CAPTURE_SEQ.load(std::sync::atomic::Ordering::Relaxed),
        CAPTURE_RESULT.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// Phase courante (atomique pur) : true = capture lancée, upload en vol —
/// la page affiche l'indicateur d'attente pendant ce laps.
pub static CAPTURE_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn capture_in_progress() -> bool {
    CAPTURE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// Lancé la capture puis détache la boucle d'upload. `true` si l'intent est
/// parti (l'app caméra s'est ouverte).
pub fn capture_and_upload(msgs: CaptureMessages) -> bool {
    capture_impl(msgs)
}

#[cfg(target_os = "android")]
#[allow(unused_imports)]
use {
    crate::api,
    crate::api::media::{MediaKind, UploadParams},
    crate::state::toasts,
    crate::util::sleep,
    std::time::Duration,
};

#[cfg(target_os = "android")]
fn capture_impl(msgs: CaptureMessages) -> bool {
    let CaptureMessages {
        launched,
        captured,
        failed,
        permission_denied,
    } = msgs;
    // Permission CAMERA déclarée (Take 360) : sans elle l'intent lève
    // SecurityException (Android ≥ 7) — pre-check + demande runtime ; le
    // dialog est asynchrone, le toast demande de relancer.
    if !camera_granted() {
        request_camera();
        toasts::error(permission_denied);
        return false;
    }
    let Some(dir) = app_cache_dir().map(|d| d.join("camera")) else {
        toasts::error(failed);
        return false;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        toasts::error(failed);
        return false;
    }
    let temp = dir.join(format!(
        "pnex-media-{}.jpg",
        chrono::Utc::now().timestamp_millis()
    ));
    let Some(uri) = file_provider_uri(&temp) else {
        toasts::error(failed);
        return false;
    };
    if !start_camera_intent(uri) {
        toasts::error(failed);
        return false;
    }
    CAPTURE_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    toasts::info(launched);
    spawn_watcher(temp, captured, failed);
    true
}

/// Watcher détaché du scope UI (`spawn_forever` — le sous-arbre de la page
/// peut être démonté pendant la capture, un `spawn` normal serait annulé) :
/// sonde le fichier de sortie jusqu'à stabilisation de sa taille, upload en
/// version 1, puis purge du temporaire dans tous les cas.
#[cfg(target_os = "android")]
fn spawn_watcher(temp: std::path::PathBuf, captured: String, failed: String) {
    dioxus::dioxus_core::spawn_forever(async move {
        // Timeout global ~10 min (école pont login : 500 ms × 1200).
        let mut last_size: Option<u64> = None;
        let mut stable: u32 = 0;
        for _ in 0..1200 {
            sleep(Duration::from_millis(500)).await;
            let Ok(meta) = std::fs::metadata(&temp) else {
                // La caméra n'a pas encore créé le fichier (ou capture
                // annulée) — on continue jusqu'au timeout.
                last_size = None;
                stable = 0;
                continue;
            };
            let size = meta.len();
            if size > 0 && last_size == Some(size) {
                stable += 1;
                if stable >= 1 {
                    break;
                }
            } else {
                stable = 0;
                last_size = Some(size);
            }
        }
        // Capture annulée ou timeout : fichier absent/vide → purge et fin.
        let bytes = std::fs::read(&temp).unwrap_or_default();
        let _ = std::fs::remove_file(&temp);
        if bytes.is_empty() {
            return;
        }
        let params = UploadParams {
            name: Some(format!(
                "Photo {}",
                chrono::Utc::now().format("%d/%m/%Y %H:%M")
            )),
            filename: Some(format!(
                "photo-{}.jpg",
                chrono::Utc::now().timestamp_millis()
            )),
            kind: Some(MediaKind::Photo),
            content_type: Some("image/jpeg".to_string()),
            note: Some("Android camera capture".to_string()),
        };
        let result = match api::media::upload(&params, bytes).await {
            Ok(_) => 1u8,
            Err(_) => 2u8,
        };
        CAPTURE_RESULT.store(result, std::sync::atomic::Ordering::Relaxed);
        CAPTURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        CAPTURE_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        let _ = (captured, failed); // libellés consommés par la page (sondage)
    });
}

/// Hors Android : pas de capture — le bouton n'est même pas rendu (page).
#[cfg(not(target_os = "android"))]
fn capture_impl(msgs: CaptureMessages) -> bool {
    let _ = msgs;
    false
}

// ─────────────────── JNI (école patch webbrowser vendu) ───────────────────

/// Attach au thread JVM (GetEnv d'abord, attach daemon en filet de
/// sécurité) — jamais de panic, échec → None. `::jni::sys` OBLIGATOIRE :
/// le prelude dioxus masque la dep directe (jni-sys 0.3, tables v1_x).
#[cfg(target_os = "android")]
pub(crate) fn attach_env() -> Option<*mut jni::sys::JNIEnv> {
    type EnvPtr = *mut jni::sys::JNIEnv;
    let ctx = ndk_context::try_android_context()?;
    let vm = ctx.vm() as *mut jni::sys::JavaVM;

    unsafe {
        let mut env: EnvPtr = std::ptr::null_mut();
        let status = ((**vm).v1_2.GetEnv)(
            vm,
            (&mut env as *mut EnvPtr).cast::<*mut std::ffi::c_void>(),
            jni::sys::JNI_VERSION_1_6,
        );
        if status == jni::sys::JNI_OK && !env.is_null() {
            return Some(env);
        }
        let args = jni::sys::JavaVMAttachArgs {
            version: jni::sys::JNI_VERSION_1_6,
            name: c"pnex-capture".as_ptr().cast_mut(),
            group: std::ptr::null_mut(),
        };
        let attached = ((**vm).v1_4.AttachCurrentThreadAsDaemon)(
            vm,
            (&mut env as *mut EnvPtr).cast::<*mut std::ffi::c_void>(),
            std::ptr::from_ref(&args)
                .cast::<std::ffi::c_void>()
                .cast_mut(),
        );
        if attached == jni::sys::JNI_OK && !env.is_null() {
            Some(env)
        } else {
            None
        }
    }
}

/// Context.getCacheDir() — jumeau de `storage::app_files_dir` (JNI bas
/// niveau, GetEnv/getCacheDir/getAbsolutePath, jamais de panic).
#[cfg(target_os = "android")]
fn app_cache_dir() -> Option<std::path::PathBuf> {
    let env = attach_env()?;
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
            c"getCacheDir".as_ptr(),
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
        let path = file_absolute_path(env, file)?;
        Some(std::path::PathBuf::from(path))
    }
}

/// File.getAbsolutePath() → String (utilisé par app_cache_dir).
#[cfg(target_os = "android")]
unsafe fn file_absolute_path(
    env: *mut jni::sys::JNIEnv,
    file: jni::sys::jobject,
) -> Option<String> {
    let file_class = ((**env).v1_1.FindClass)(env, c"java/io/File".as_ptr());
    if file_class.is_null() {
        return None;
    }
    let mid = ((**env).v1_1.GetMethodID)(
        env,
        file_class,
        c"getAbsolutePath".as_ptr(),
        c"()Ljava/lang/String;".as_ptr(),
    );
    if mid.is_null() {
        return None;
    }
    let jstr =
        ((**env).v1_1.CallObjectMethodA)(env, file, mid, std::ptr::null()) as jni::sys::jstring;
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
}

/// Context.getPackageName() → String.
#[cfg(target_os = "android")]
pub(crate) unsafe fn package_name(
    env: *mut jni::sys::JNIEnv,
    ctx_obj: jni::sys::jobject,
) -> Option<String> {
    let ctx_class = ((**env).v1_1.FindClass)(env, c"android/content/Context".as_ptr());
    if ctx_class.is_null() {
        return None;
    }
    let mid = ((**env).v1_1.GetMethodID)(
        env,
        ctx_class,
        c"getPackageName".as_ptr(),
        c"()Ljava/lang/String;".as_ptr(),
    );
    if mid.is_null() {
        return None;
    }
    let jstr =
        ((**env).v1_1.CallObjectMethodA)(env, ctx_obj, mid, std::ptr::null()) as jni::sys::jstring;
    if jstr.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
        ((**env).v1_1.ExceptionClear)(env);
        return None;
    }
    let chars = ((**env).v1_1.GetStringUTFChars)(env, jstr, std::ptr::null_mut());
    if chars.is_null() {
        return None;
    }
    let name = std::ffi::CStr::from_ptr(chars)
        .to_string_lossy()
        .into_owned();
    ((**env).v1_1.ReleaseStringUTFChars)(env, jstr, chars);
    Some(name)
}

/// androidx.core.content.FileProvider.getUriForFile(ctx, authorities, file)
/// — authorities JAMAIS hardcodées : `getPackageName() + ".fileprovider"`
/// (le manifest est patché par patch-android-manifest.py avec le même
/// schéma).
#[cfg(target_os = "android")]
fn file_provider_uri(path: &std::path::Path) -> Option<jni::sys::jobject> {
    let env = attach_env()?;
    let ctx = ndk_context::try_android_context()?;
    let ctx_obj = ctx.context() as jni::sys::jobject;

    unsafe {
        // getPackageName (déjà posé sur Context).
        let name = package_name(env, ctx_obj)?;
        let authorities = format!("{name}.fileprovider");

        // java.io.File du chemin temporaire.
        let file_class = ((**env).v1_1.FindClass)(env, c"java/io/File".as_ptr());
        if file_class.is_null() {
            return None;
        }
        let mid_init = ((**env).v1_1.GetMethodID)(
            env,
            file_class,
            c"<init>".as_ptr(),
            c"(Ljava/lang/String;)V".as_ptr(),
        );
        if mid_init.is_null() {
            return None;
        }
        // CString OBLIGATOIRE : une &str Rust n'est pas terminée par NUL —
        // NewStringUTF lirait la mémoire d'après (crash JNI constaté
        // 2026-09-09 : « input is not valid Modified UTF-8 »).
        let path_c = std::ffi::CString::new(path.to_str()?).ok()?;
        let jpath = ((**env).v1_1.NewStringUTF)(env, path_c.as_ptr());
        if jpath.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        let obj_args = [jni::sys::jvalue { l: jpath }];
        let jfile = ((**env).v1_1.NewObjectA)(env, file_class, mid_init, obj_args.as_ptr())
            as jni::sys::jobject;
        if jfile.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }

        // FileProvider.getUriForFile(...) — statique.
        let provider_class =
            ((**env).v1_1.FindClass)(env, c"androidx/core/content/FileProvider".as_ptr());
        if provider_class.is_null() {
            return None;
        }
        let mid_uri = ((**env).v1_1.GetStaticMethodID)(
            env,
            provider_class,
            c"getUriForFile".as_ptr(),
            c"(Landroid/content/Context;Ljava/lang/String;Ljava/io/File;)Landroid/net/Uri;"
                .as_ptr(),
        );
        if mid_uri.is_null() {
            return None;
        }
        let authorities_c = std::ffi::CString::new(authorities).ok()?;
        let jauthorities = ((**env).v1_1.NewStringUTF)(env, authorities_c.as_ptr());
        if jauthorities.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        let args = [
            jni::sys::jvalue { l: ctx_obj },
            jni::sys::jvalue { l: jauthorities },
            jni::sys::jvalue { l: jfile },
        ];
        let uri =
            ((**env).v1_1.CallStaticObjectMethodA)(env, provider_class, mid_uri, args.as_ptr())
                as jni::sys::jobject;
        if uri.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        // Référence GLOBALE : le jobject doit survivre aux attach_env des
        // appels suivants (start_camera_intent refait son propre attach).
        let global =
            ((**env).v1_1.NewGlobalRef)(env, uri as jni::sys::jobject) as jni::sys::jobject;
        if global.is_null() {
            return None;
        }
        Some(global)
    }
}

/// Lance l'intent caméra : `new Intent()` + `setAction(ACTION_IMAGE_CAPTURE)`
/// + `putExtra("output", uri)` (MediaStore.EXTRA_OUTPUT) + `addFlags(0x40 |
/// 0x80)` (GRANT_READ|WRITE_URI_PERMISSION) + `startActivity`. L'exception
/// fantôme API 36 post-startActivity est classée (clear + poursuite : le
/// polling du fichier reste la source de vérité) — `false` seulement si la
/// classe Intent elle-même est introuvable.
#[cfg(target_os = "android")]
fn start_camera_intent(uri: jni::sys::jobject) -> bool {
    let Some(env) = attach_env() else {
        return false;
    };
    let Some(ctx) = ndk_context::try_android_context() else {
        return false;
    };
    let ctx_obj = ctx.context() as jni::sys::jobject;

    unsafe {
        let intent_class = ((**env).v1_1.FindClass)(env, c"android/content/Intent".as_ptr());
        if intent_class.is_null() {
            return false;
        }
        let mid_init =
            ((**env).v1_1.GetMethodID)(env, intent_class, c"<init>".as_ptr(), c"()V".as_ptr());
        if mid_init.is_null() {
            return false;
        }
        let intent = ((**env).v1_1.NewObjectA)(env, intent_class, mid_init, std::ptr::null())
            as jni::sys::jobject;
        if intent.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return false;
        }

        // setAction("android.media.action.IMAGE_CAPTURE") (constante
        // MediaStore.ACTION_IMAGE_CAPTURE — littéral identique).
        let Some(action_c) = std::ffi::CString::new("android.media.action.IMAGE_CAPTURE").ok()
        else {
            return false;
        };
        let jaction = ((**env).v1_1.NewStringUTF)(env, action_c.as_ptr());
        if jaction.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return false;
        }
        let mid_action = ((**env).v1_1.GetMethodID)(
            env,
            intent_class,
            c"setAction".as_ptr(),
            c"(Ljava/lang/String;)Landroid/content/Intent;".as_ptr(),
        );
        if mid_action.is_null() {
            return false;
        }
        let action_args = [jni::sys::jvalue { l: jaction }];
        let _ = ((**env).v1_1.CallObjectMethodA)(env, intent, mid_action, action_args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return false;
        }

        // putExtra("output", uri) — MediaStore.EXTRA_OUTPUT = "output".
        let jkey =
            ((**env).v1_1.NewStringUTF)(env, c"output".as_ptr().cast::<std::os::raw::c_char>());
        if jkey.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return false;
        }
        let mid_extra = ((**env).v1_1.GetMethodID)(
            env,
            intent_class,
            c"putExtra".as_ptr(),
            c"(Ljava/lang/String;Landroid/os/Parcelable;)Landroid/content/Intent;".as_ptr(),
        );
        if mid_extra.is_null() {
            return false;
        }
        let extra_args = [jni::sys::jvalue { l: jkey }, jni::sys::jvalue { l: uri }];
        let _ = ((**env).v1_1.CallObjectMethodA)(env, intent, mid_extra, extra_args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return false;
        }

        // addFlags(GRANT_READ | GRANT_WRITE) = 0x40 | 0x80.
        let mid_flags = ((**env).v1_1.GetMethodID)(
            env,
            intent_class,
            c"addFlags".as_ptr(),
            c"(I)Landroid/content/Intent;".as_ptr(),
        );
        if mid_flags.is_null() {
            return false;
        }
        let flag_args = [jni::sys::jvalue {
            i: (0x40 | 0x80) as jni::sys::jint,
        }];
        let _ = ((**env).v1_1.CallObjectMethodA)(env, intent, mid_flags, flag_args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return false;
        }

        // context.startActivity(intent) — exception fantôme API 36 : clear
        // + poursuite (école patch webbrowser : le polling décidera).
        let ctx_class = ((**env).v1_1.FindClass)(env, c"android/content/Context".as_ptr());
        if ctx_class.is_null() {
            return false;
        }
        let mid_start = ((**env).v1_1.GetMethodID)(
            env,
            ctx_class,
            c"startActivity".as_ptr(),
            c"(Landroid/content/Intent;)V".as_ptr(),
        );
        if mid_start.is_null() {
            return false;
        }
        let start_args = [jni::sys::jvalue { l: intent }];
        ((**env).v1_1.CallVoidMethodA)(env, ctx_obj, mid_start, start_args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionDescribe)(env);
            ((**env).v1_1.ExceptionClear)(env);
        }
        true
    }
}

// ───────────────── Permission CAMERA (Take 360) ─────────────────
//
// La déclaration de la permission au manifest (patch-android-manifest.py,
// Take 360) change le contrat d'ACTION_IMAGE_CAPTURE : déclarée mais non
// accordée, l'intent lève SecurityException (Android ≥ 7). Le flux photo
// gagne donc un pre-check : pas accordée → demande runtime + toast, et
// l'utilisateur relance. La demande runtime elle-même est câblée dans wry
// (RustWebChromeClient.onPermissionRequest → launcher Capacitor) côté
// getUserMedia ; ici on passe par requestPermissions(Activity) en direct.

/// Permission caméra accordée ? (Context.checkSelfPermission, API 23+).
#[cfg(target_os = "android")]
pub(crate) fn camera_granted() -> bool {
    let Some(env) = attach_env() else {
        return false;
    };
    let Some(ctx) = ndk_context::try_android_context() else {
        return false;
    };
    let ctx_obj = ctx.context() as jni::sys::jobject;

    unsafe {
        let ctx_class = ((**env).v1_1.FindClass)(env, c"android/content/Context".as_ptr());
        if ctx_class.is_null() {
            return false;
        }
        let mid = ((**env).v1_1.GetMethodID)(
            env,
            ctx_class,
            c"checkSelfPermission".as_ptr(),
            c"(Ljava/lang/String;)I".as_ptr(),
        );
        if mid.is_null() {
            return false;
        }
        let Some(jperm) = new_java_string(env, c"android.permission.CAMERA") else {
            return false;
        };
        let args = [jni::sys::jvalue { l: jperm }];
        let granted = ((**env).v1_1.CallIntMethodA)(env, ctx_obj, mid, args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return false;
        }
        granted == 0 // PackageManager.PERMISSION_GRANTED = 0
    }
}

/// Demande runtime fire-and-forget (le résultat est observable via
/// `camera_granted()` une fois le dialog répondu).
#[cfg(target_os = "android")]
pub(crate) fn request_camera() {
    let Some(env) = attach_env() else {
        return;
    };
    let Some(ctx) = ndk_context::try_android_context() else {
        return;
    };
    let ctx_obj = ctx.context() as jni::sys::jobject;

    unsafe {
        let Some(jperm) = new_java_string(env, c"android.permission.CAMERA") else {
            return;
        };
        // String[]{ "android.permission.CAMERA" }
        let arr = ((**env).v1_1.NewObjectArray)(
            env,
            1,
            ((**env).v1_1.FindClass)(env, c"java/lang/String".as_ptr()),
            jperm,
        );
        if arr.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return;
        }
        let act_class = ((**env).v1_1.FindClass)(env, c"android/app/Activity".as_ptr());
        if act_class.is_null() {
            return;
        }
        let mid = ((**env).v1_1.GetMethodID)(
            env,
            act_class,
            c"requestPermissions".as_ptr(),
            c"([Ljava/lang/String;I)V".as_ptr(),
        );
        if mid.is_null() {
            return;
        }
        let args = [
            jni::sys::jvalue { l: arr },
            jni::sys::jvalue { i: 4201 }, // requestCode arbitraire
        ];
        ((**env).v1_1.CallVoidMethodA)(env, ctx_obj, mid, args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionDescribe)(env);
            ((**env).v1_1.ExceptionClear)(env);
        }
    }
}

/// NewStringUTF (helper commun — prend un littéral `c"..."`).
#[cfg(target_os = "android")]
fn new_java_string(env: *mut jni::sys::JNIEnv, s: &std::ffi::CStr) -> Option<jni::sys::jobject> {
    let jstr = unsafe { ((**env).v1_1.NewStringUTF)(env, s.as_ptr()) };
    if jstr.is_null() || unsafe { ((**env).v1_2.ExceptionCheck)(env) } {
        unsafe { ((**env).v1_1.ExceptionClear)(env) };
        return None;
    }
    Some(jstr)
}
