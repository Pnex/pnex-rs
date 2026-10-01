use crate::{Browser, BrowserOptions, Error, ErrorKind, Result, TargetType};
use std::process::{Command, Stdio};

/// Deal with opening of browsers on Android. Only [Browser::Default] is supported, and
/// in options, only [BrowserOptions::dry_run] is honoured.
pub(super) fn open_browser_internal(
    browser: Browser,
    target: &TargetType,
    options: &BrowserOptions
) -> Result<()> {
    // ensure we're opening only http/https urls, failing otherwise
    let url = target.get_http_url()?;

    match browser {
        Browser::Default => open_browser_default(url, options),
        _ => Err(Error::new(
            ErrorKind::NotFound,
            "only default browser SSO flows supported",
        )),
    }
}

// ---------------------------------------------------------------------------
// PATCH PNeX (2026-09-08) — cf. vendor/patches/README.md
//
// L'implémentation amont repose sur les macros haut niveau du crate `jni`
// (0.22) : leur chemin d'exception interne asserte (« Expected an exception
// after ExceptionCheck », jni env.rs:706) et SIGABRT l'application dès
// qu'une exception Java traverse la séquence — une panic Rust ne peut pas
// retraverser une frontière JNI (panic_cannot_unwind → abort, symboles
// Java_dev_dioxus_main_RustWebViewClient_handleRequest / WryActivity_create
// dans les tombstones 2026-09-08).
//
// Réécriture en JNI brut via la table de fonctions (`jni::sys`, appels
// non-variadiques `*A`, frame locale libérée en bloc) : ExceptionCheck/
// Clear systématiques, tout en Result — plus aucun panic possible depuis ce
// chemin, y compris quand aucun navigateur n'existe sur l'appareil
// (ActivityNotFoundException → Err propre).
// ---------------------------------------------------------------------------

type EnvPtr = *mut jni::sys::JNIEnv;

/// Open the default browser
fn open_browser_default(url: &str, options: &BrowserOptions) -> Result<()> {
    // always return true for a dry run
    if options.dry_run {
        return Ok(());
    }

    // first we try to see if we're in a termux env, because if we are, then
    // the android context may not have been initialized
    if try_for_termux(url, options).is_ok() {
        return Ok(());
    }

    // Variante non-paniquante (PATCH ndk-context) : None = pas de contexte
    // Android (tests, termux) → Err propre au lieu d'un panic.
    let Some(ctx) = ndk_context::try_android_context() else {
        return Err(Error::new(
            ErrorKind::NotFound,
            "no Android context available via ndk_context",
        ));
    };
    if ctx.vm().is_null() || ctx.context().is_null() {
        return Err(Error::new(
            ErrorKind::NotFound,
            "null JVM or Context pointer from ndk_context",
        ));
    }
    unsafe { jni_open_url(ctx, url) }
}

fn open_err(msg: impl std::fmt::Display) -> crate::Error {
    Error::other(format!("webbrowser(android): {msg}"))
}

/// Nettoie une exception Java pendante et retourne l'erreur correspondante.
///
/// `ExceptionDescribe` jette le détail (type + stack trace) dans logcat —
/// on ne s'attache qu'au fait qu'une exception a eu lieu ; lire son message
/// proprement demanderait une chaîne de calls JNI de plus.
unsafe fn clear_and_err(env: EnvPtr, what: &str) -> crate::Error {
    if ((**env).v1_2.ExceptionCheck)(env) {
        ((**env).v1_1.ExceptionDescribe)(env);
        ((**env).v1_1.ExceptionClear)(env);
        return open_err(format!("{what} — exception Java (détail dans logcat)"));
    }
    open_err(what.to_string())
}

/// Inspecte l'exception pendante : `(throwable présent ?, nom qualifié de
/// classe si lisible)`, puis nettoyage systématique (ne jamais laisser
/// d'exception pendante derrière soi), sans jamais paniquer.
///
/// Constaté sur API 36 juste après `startActivity` : `ExceptionCheck` répond
/// vrai alors que `ExceptionOccurred` ne rend AUCUN throwable — un état
/// « fantôme » impossible selon la spec JNI, traité comme non fatal (le
/// navigateur s'ouvre normalement).
unsafe fn inspect_pending_exception(env: EnvPtr) -> (bool, Option<String>) {
    let throwable = ((**env).v1_1.ExceptionOccurred)(env);
    if throwable.is_null() {
        // Check vrai + aucun throwable : fantôme ART, rien à décrire ni
        // nettoyer — ExceptionClear est un no-op sans exception réelle.
        return (false, None);
    }
    let mut class_name: Option<String> = None;
    let class = ((**env).v1_1.GetObjectClass)(env, throwable);
    if !class.is_null() {
        let get_name = ((**env).v1_1.GetMethodID)(
            env,
            class,
            c"getName".as_ptr(),
            c"()Ljava/lang/String;".as_ptr(),
        );
        if !get_name.is_null() {
            let name_obj = ((**env).v1_1.CallObjectMethodA)(
                env,
                class,
                get_name,
                std::ptr::null::<jni::sys::jvalue>(),
            );
            if !name_obj.is_null() {
                let chars =
                    ((**env).v1_1.GetStringUTFChars)(env, name_obj, std::ptr::null_mut());
                if !chars.is_null() {
                    class_name = Some(
                        std::ffi::CStr::from_ptr(chars).to_string_lossy().into_owned(),
                    );
                    ((**env).v1_1.ReleaseStringUTFChars)(env, name_obj, chars);
                }
            }
        }
    }
    ((**env).v1_1.ExceptionClear)(env);
    (true, class_name)
}

/// Attache le thread courant à la JVM si besoin et retourne le `JNIEnv`.
///
/// Les threads du runtime Dioxus (main + exécuteur async) sont déjà
/// attachés par le glue tao : `GetEnv` répond JNI_OK et l'attach n'est
/// qu'un filet de sécurité.
unsafe fn attach_env(vm: *mut jni::sys::JavaVM) -> Result<EnvPtr> {
    let mut env: EnvPtr = std::ptr::null_mut();
    let status = ((**vm).v1_2.GetEnv)(
        vm,
        (&mut env as *mut EnvPtr).cast::<*mut std::ffi::c_void>(),
        jni::sys::JNI_VERSION_1_6,
    );
    if status == jni::sys::JNI_OK {
        return Ok(env);
    }
    let args = jni::sys::JavaVMAttachArgs {
        version: jni::sys::JNI_VERSION_1_6,
        name: c"pnex-open".as_ptr().cast_mut(),
        group: std::ptr::null_mut(),
    };
    let status = ((**vm).v1_4.AttachCurrentThreadAsDaemon)(
        vm,
        (&mut env as *mut EnvPtr).cast::<*mut std::ffi::c_void>(),
        std::ptr::from_ref(&args).cast::<std::ffi::c_void>().cast_mut(),
    );
    if status != jni::sys::JNI_OK || env.is_null() {
        return Err(open_err(format!("attach: status={status}")));
    }
    Ok(env)
}

unsafe fn jclass_of(env: EnvPtr, name: &str) -> Result<jni::sys::jclass> {
    let cname = std::ffi::CString::new(name).map_err(|_| open_err("nom de classe invalide"))?;
    let class = ((**env).v1_1.FindClass)(env, cname.as_ptr());
    if class.is_null() {
        return Err(clear_and_err(env, &format!("FindClass({name})")));
    }
    Ok(class)
}

/// `NewStringUTF` attend du « modified UTF-8 » : nos URLs sont ASCII
/// (composants percent-encodés en amont) — garde-fou explicite sinon, sans
/// jamais paniquer.
unsafe fn jstring_of(env: EnvPtr, s: &str) -> Result<jni::sys::jstring> {
    if !s.is_ascii() {
        return Err(open_err("URL non-ASCII non supportée en ouverture native"));
    }
    let cs = std::ffi::CString::new(s).map_err(|_| open_err("URL contenant un NUL"))?;
    let jstr = ((**env).v1_1.NewStringUTF)(env, cs.as_ptr());
    if jstr.is_null() {
        return Err(clear_and_err(env, &format!("NewStringUTF({s})")));
    }
    Ok(jstr)
}

/// `static_ = true` → `GetStaticMethodID`, sinon `GetMethodID`.
unsafe fn method_id(
    env: EnvPtr,
    class: jni::sys::jclass,
    static_: bool,
    name: &str,
    sig: &str,
) -> Result<jni::sys::jmethodID> {
    let cname = std::ffi::CString::new(name).map_err(|_| open_err("nom de méthode invalide"))?;
    let csig = std::ffi::CString::new(sig).map_err(|_| open_err("signature invalide"))?;
    let mid = if static_ {
        ((**env).v1_1.GetStaticMethodID)(env, class, cname.as_ptr(), csig.as_ptr())
    } else {
        ((**env).v1_1.GetMethodID)(env, class, cname.as_ptr(), csig.as_ptr())
    };
    if mid.is_null() {
        return Err(clear_and_err(env, &format!("GetMethodID({name})")));
    }
    Ok(mid)
}

fn jval(obj: jni::sys::jobject) -> jni::sys::jvalue {
    jni::sys::jvalue { l: obj }
}

/// Attaché le thread, ouvre un frame local et déroule la séquence.
unsafe fn jni_open_url(ctx: ndk_context::AndroidContext, url: &str) -> Result<()> {
    let vm = ctx.vm() as *mut jni::sys::JavaVM;
    let env = attach_env(vm)?;

    // Frame local : toutes les refs créées dans ce bloc sont libérées d'un
    // coup au pop — pas de fuite de refs locales même en cas d'erreur.
    if ((**env).v1_2.PushLocalFrame)(env, 32) != 0 {
        return Err(clear_and_err(env, "PushLocalFrame"));
    }
    let outcome = open_in_frame(env, ctx, url);
    ((**env).v1_2.PopLocalFrame)(env, std::ptr::null_mut());
    outcome
}

/// La séquence complète, en appel `*A` non-variadiques :
///
/// ```java
/// Uri uri = Uri.parse(url);
/// Intent intent = new Intent(Intent.ACTION_VIEW, uri);
/// intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK); // requis depuis un Context non-Activity
/// context.startActivity(intent);
/// ```
unsafe fn open_in_frame(env: EnvPtr, ctx: ndk_context::AndroidContext, url: &str) -> Result<()> {
    use jni::sys::jobject;
    use jni::sys::jvalue;

    // Uri uri = Uri.parse(url)
    let uri_class = jclass_of(env, "android/net/Uri")?;
    let url_j = jstring_of(env, url)?;
    let parse = method_id(env, uri_class, true, "parse", "(Ljava/lang/String;)Landroid/net/Uri;")?;
    let uri = ((**env).v1_1.CallStaticObjectMethodA)(env, uri_class, parse, [jval(url_j)].as_ptr());
    if uri.is_null() {
        return Err(clear_and_err(env, "Uri.parse"));
    }

    // Intent intent = new Intent(ACTION_VIEW, uri)
    let intent_class = jclass_of(env, "android/content/Intent")?;
    let action = jstring_of(env, "android.intent.action.VIEW")?;
    let ctor = method_id(env, intent_class, false, "<init>", "(Ljava/lang/String;Landroid/net/Uri;)V")?;
    let intent = ((**env).v1_1.NewObjectA)(env, intent_class, ctor, [jval(action), jval(uri)].as_ptr());
    if intent.is_null() {
        return Err(clear_and_err(env, "new Intent(ACTION_VIEW, uri)"));
    }

    // intent.addFlags(FLAG_ACTIVITY_NEW_TASK) — requis depuis un Context
    // non-Activity (ndk-context expose généralement l'Application).
    let add_flags = method_id(env, intent_class, false, "addFlags", "(I)Landroid/content/Intent;")?;
    let new_task = jvalue { i: 0x1000_0000 }; // FLAG_ACTIVITY_NEW_TASK
    let _ = ((**env).v1_1.CallObjectMethodA)(env, intent, add_flags, [new_task].as_ptr());
    if ((**env).v1_2.ExceptionCheck)(env) {
        return Err(clear_and_err(env, "Intent.addFlags"));
    }

    // context.startActivity(intent) — ndk-context garantit un Context
    let ctx_obj = ctx.context() as jobject;
    let ctx_class = ((**env).v1_1.GetObjectClass)(env, ctx_obj);
    if ctx_class.is_null() {
        return Err(clear_and_err(env, "GetObjectClass(context)"));
    }
    let start = method_id(env, ctx_class, false, "startActivity", "(Landroid/content/Intent;)V")?;
    ((**env).v1_1.CallVoidMethodA)(env, ctx_obj, start, [jval(intent)].as_ptr());
    if ((**env).v1_2.ExceptionCheck)(env) {
        // Classifier avant de trancher (constaté sur API 36 : ExceptionCheck
        // répond vrai après startActivity alors que le navigateur s'ouvre
        // normalement — cf. vendor/patches/README.md). Seule une exception
        // RÉELLE classée ActivityNotFoundException est une vraie erreur :
        // aucun navigateur sur l'appareil.
        let (present, class) = inspect_pending_exception(env);
        if class.as_deref() == Some("android.content.ActivityNotFoundException") {
            return Err(open_err(
                "Context.startActivity — aucun navigateur sur l'appareil (ActivityNotFoundException)",
            ));
        }
        if present && class.is_none() {
            // Throwable réel mais classe illisible : prudence, on remonte.
            return Err(open_err("Context.startActivity — exception Java illisible"));
        }
        // Soit fantôme (check vrai sans throwable — artefact ART), soit
        // exception lisible et non fatale : la navigation est lancée.
        match class {
            Some(class) => eprintln!(
                "pnex-webbrowser: startActivity a signalé {class} — navigation lancée, exception non fatale ignorée"
            ),
            None => eprintln!(
                "pnex-webbrowser: ExceptionCheck vrai sans throwable après startActivity (fantôme API 36) — navigation lancée"
            ),
        }
        return Ok(());
    }
    Ok(())
}

/// Attemps to open a browser assuming a termux environment
///
/// See [issue #53](https://github.com/amodm/webbrowser-rs/issues/53)
fn try_for_termux(url: &str, options: &BrowserOptions) -> Result<()> {
    use std::env;
    if env::var("TERMUX_VERSION").is_ok() {
        // return true for a dry-run given that termux-open command is guaranteed to be present
        if options.dry_run {
            return Ok(());
        }
        let mut cmd = Command::new("termux-open");
        cmd.arg(url);
        if options.suppress_output {
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        }
        cmd.status().and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(Error::other("command present but exited unsuccessfully"))
            }
        })
    } else {
        Err(Error::other("Not a termux environment"))
    }
}
