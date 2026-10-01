//! Téléchargement média natif Android — enregistrement dans
//! `Download/Pnex` via MediaStore.Downloads (JNI bas niveau, école
//! capture.rs : jamais de panic, échec → `false`, CString obligatoire
//! pour NewStringUTF).
//!
//! API 29+ : insertion MediaStore.Downloads (aucune permission requise —
//! contribution scoped storage), écriture en un `write(byte[])`, purge de
//! la ligne insérée si l'écriture échoue. API < 29 : repli
//! `getExternalFilesDir("Download")` (app-specific, sans permission).
//!
//! Hors Android : no-op (le web passe par l'ancre blob de `util.rs`).

/// Enregistre `bytes` sous `filename` (type `mime`) dans `Download/Pnex`.
/// `true` si le fichier est posé. Bloquant bref — à appeler depuis un
/// contexte async (spawn), pas depuis le thread UI dioxus.
#[cfg(target_os = "android")]
pub fn save_to_downloads(filename: &str, mime: &str, bytes: &[u8]) -> bool {
    let Some(env) = crate::capture::attach_env() else {
        return false;
    };
    let Some(ctx) = ndk_context::try_android_context() else {
        return false;
    };
    let ctx_obj = ctx.context() as ::jni::sys::jobject;

    unsafe {
        if sdk_int(env) >= 29 {
            media_store_save(env, ctx_obj, filename, mime, bytes)
        } else {
            external_files_save(env, ctx_obj, filename, bytes)
        }
    }
}

/// Hors Android : no-op (échec → toast d'erreur côté UI).
#[cfg(not(target_os = "android"))]
pub fn save_to_downloads(_filename: &str, _mime: &str, _bytes: &[u8]) -> bool {
    false
}

// ─────────────────────── JNI (école capture.rs) ───────────────────────

/// Build.VERSION.SDK_INT (MediaStore.Downloads existe depuis l'API 29).
#[cfg(target_os = "android")]
unsafe fn sdk_int(env: *mut ::jni::sys::JNIEnv) -> i32 {
    unsafe {
        let cls = ((**env).v1_1.FindClass)(env, c"android/os/Build$Version".as_ptr());
        if cls.is_null() {
            return 0;
        }
        let fid = ((**env).v1_1.GetStaticFieldID)(env, cls, c"SDK_INT".as_ptr(), c"I".as_ptr());
        if fid.is_null() {
            return 0;
        }
        ((**env).v1_1.GetStaticIntField)(env, cls, fid)
    }
}

/// Chemin API 29+ : MediaStore.Downloads.
#[cfg(target_os = "android")]
unsafe fn media_store_save(
    env: *mut ::jni::sys::JNIEnv,
    ctx_obj: ::jni::sys::jobject,
    filename: &str,
    mime: &str,
    bytes: &[u8],
) -> bool {
    unsafe {
        // ContentValues : les 3 colonnes (littéraux des MediaColumns —
        // DISPLAY_NAME / MIME_TYPE / RELATIVE_PATH, constants API 29+).
        let Some(cv) = new_object(env, c"android/content/ContentValues", c"()V", &[]) else {
            return false;
        };
        let Some(mid_put) = method_id(
            env,
            c"android/content/ContentValues",
            c"put",
            c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/ContentValues;",
        ) else {
            return false;
        };
        for (column, value) in [
            ("_display_name", filename),
            ("mime_type", mime),
            ("relative_path", "Download/Pnex"),
        ] {
            let Some(jcol) = java_string(env, column) else {
                return false;
            };
            let Some(jval) = java_string(env, value) else {
                return false;
            };
            let args = [
                ::jni::sys::jvalue { l: jcol },
                ::jni::sys::jvalue { l: jval },
            ];
            let _ = ((**env).v1_1.CallObjectMethodA)(env, cv, mid_put, args.as_ptr());
            if ((**env).v1_2.ExceptionCheck)(env) {
                ((**env).v1_1.ExceptionClear)(env);
                return false;
            }
        }

        // MediaStore.Downloads.EXTERNAL_CONTENT_URI.
        let Some(downloads_uri) = static_object_field(
            env,
            c"android/provider/MediaStore$Downloads",
            c"EXTERNAL_CONTENT_URI",
            c"Landroid/net/Uri;",
        ) else {
            return false;
        };

        // ContentResolver du contexte.
        let Some(cr) = call_object(
            env,
            ctx_obj,
            c"android/content/Context",
            c"getContentResolver",
            c"()Landroid/content/ContentResolver;",
            &[],
        ) else {
            return false;
        };

        // insert(uri, values) → Uri de la nouvelle entrée.
        let Some(entry) = call_object(
            env,
            cr,
            c"android/content/ContentResolver",
            c"insert",
            c"(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
            &[
                ::jni::sys::jvalue { l: downloads_uri },
                ::jni::sys::jvalue { l: cv },
            ],
        ) else {
            return false;
        };

        // openOutputStream(entry) → OutputStream (FileNotFoundException /
        // IOException levées possibles → None via call_object).
        let Some(out) = call_object(
            env,
            cr,
            c"android/content/ContentResolver",
            c"openOutputStream",
            c"(Landroid/net/Uri;)Ljava/io/OutputStream;",
            &[::jni::sys::jvalue { l: entry }],
        ) else {
            delete_entry(env, cr, entry);
            return false;
        };

        // write(byte[]) en un appel (taille bornée par la limite d'upload
        // serveur — media-error-413).
        let os_class = ((**env).v1_1.FindClass)(env, c"java/io/OutputStream".as_ptr());
        if os_class.is_null() {
            delete_entry(env, cr, entry);
            return false;
        }
        let mid_write =
            ((**env).v1_1.GetMethodID)(env, os_class, c"write".as_ptr(), c"([B)V".as_ptr());
        if mid_write.is_null() {
            delete_entry(env, cr, entry);
            return false;
        }
        let arr = ((**env).v1_1.NewByteArray)(env, bytes.len() as ::jni::sys::jsize);
        if arr.is_null() {
            delete_entry(env, cr, entry);
            return false;
        }
        ((**env).v1_1.SetByteArrayRegion)(
            env,
            arr as ::jni::sys::jbyteArray,
            0,
            bytes.len() as ::jni::sys::jsize,
            bytes.as_ptr() as *const ::jni::sys::jbyte,
        );
        let write_args = [::jni::sys::jvalue { l: arr }];
        ((**env).v1_1.CallVoidMethodA)(env, out, mid_write, write_args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionDescribe)(env);
            ((**env).v1_1.ExceptionClear)(env);
            delete_entry(env, cr, entry);
            return false;
        }

        // close() — une IOException ici n'invalide pas l'écriture si le
        // write a réussi : exception nettoyée, on garde `true`.
        let mid_close =
            ((**env).v1_1.GetMethodID)(env, os_class, c"close".as_ptr(), c"()V".as_ptr());
        if !mid_close.is_null() {
            ((**env).v1_1.CallVoidMethodA)(env, out, mid_close, std::ptr::null());
            if ((**env).v1_2.ExceptionCheck)(env) {
                ((**env).v1_1.ExceptionClear)(env);
            }
        }
        true
    }
}

/// Repli API < 29 : getExternalFilesDir("Download") (app-specific, sans
/// permission) + écriture std::fs — vise les API anciens ; l'utilisateur
/// ne voit que le toast générique.
#[cfg(target_os = "android")]
unsafe fn external_files_save(
    env: *mut ::jni::sys::JNIEnv,
    ctx_obj: ::jni::sys::jobject,
    filename: &str,
    bytes: &[u8],
) -> bool {
    unsafe {
        let jdir = java_string(env, "Download");
        let dir_args = match &jdir {
            Some(j) => [::jni::sys::jvalue { l: *j }],
            None => [::jni::sys::jvalue {
                l: std::ptr::null_mut(),
            }],
        };
        let Some(dir_file) = call_object(
            env,
            ctx_obj,
            c"android/content/Context",
            c"getExternalFilesDir",
            c"(Ljava/lang/String;)Ljava/io/File;",
            &dir_args,
        ) else {
            return false;
        };
        let Some(dir_path) = file_path(env, dir_file) else {
            return false;
        };
        let dir = std::path::PathBuf::from(dir_path);
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        std::fs::write(dir.join(filename), bytes).is_ok()
    }
}

/// Purge la ligne MediaStore insérée pour rien (ContentResolver.delete —
/// selector/args null).
#[cfg(target_os = "android")]
unsafe fn delete_entry(
    env: *mut ::jni::sys::JNIEnv,
    cr: ::jni::sys::jobject,
    entry: ::jni::sys::jobject,
) {
    unsafe {
        let Some(mid_delete) = method_id(
            env,
            c"android/content/ContentResolver",
            c"delete",
            c"(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I",
        ) else {
            return;
        };
        let args = [
            ::jni::sys::jvalue { l: entry },
            ::jni::sys::jvalue {
                l: std::ptr::null_mut(),
            },
            ::jni::sys::jvalue {
                l: std::ptr::null_mut(),
            },
        ];
        let _ = ((**env).v1_1.CallIntMethodA)(env, cr, mid_delete, args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
        }
    }
}

// ── Helpers JNI génériques (null-check + ExceptionCheck + clear — même
// sémantique que capture.rs, mutualisée ici) ──

/// FindClass + GetMethodID encapsulés.
#[cfg(target_os = "android")]
unsafe fn method_id(
    env: *mut ::jni::sys::JNIEnv,
    class: &std::ffi::CStr,
    method: &std::ffi::CStr,
    sig: &std::ffi::CStr,
) -> Option<::jni::sys::jmethodID> {
    unsafe {
        let cls = ((**env).v1_1.FindClass)(env, class.as_ptr());
        if cls.is_null() {
            return None;
        }
        let mid = ((**env).v1_1.GetMethodID)(env, cls, method.as_ptr(), sig.as_ptr());
        if mid.is_null() {
            return None;
        }
        Some(mid)
    }
}

/// NewObjectA encapsulé.
#[cfg(target_os = "android")]
unsafe fn new_object(
    env: *mut ::jni::sys::JNIEnv,
    class: &std::ffi::CStr,
    ctor_sig: &std::ffi::CStr,
    args: &[::jni::sys::jvalue],
) -> Option<::jni::sys::jobject> {
    unsafe {
        let cls = ((**env).v1_1.FindClass)(env, class.as_ptr());
        if cls.is_null() {
            return None;
        }
        let mid = ((**env).v1_1.GetMethodID)(env, cls, c"<init>".as_ptr(), ctor_sig.as_ptr());
        if mid.is_null() {
            return None;
        }
        let obj = ((**env).v1_1.NewObjectA)(env, cls, mid, args.as_ptr()) as ::jni::sys::jobject;
        if obj.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        Some(obj)
    }
}

/// CallObjectMethodA résolu par (classe, nom, signature) — None si absent
/// ou levée Java.
#[cfg(target_os = "android")]
unsafe fn call_object(
    env: *mut ::jni::sys::JNIEnv,
    obj: ::jni::sys::jobject,
    class: &std::ffi::CStr,
    method: &std::ffi::CStr,
    sig: &std::ffi::CStr,
    args: &[::jni::sys::jvalue],
) -> Option<::jni::sys::jobject> {
    let Some(mid) = method_id(env, class, method, sig) else {
        return None;
    };
    unsafe {
        let result =
            ((**env).v1_1.CallObjectMethodA)(env, obj, mid, args.as_ptr()) as ::jni::sys::jobject;
        if result.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        Some(result)
    }
}

/// GetStaticObjectField encapsulé (constante statique d'une classe).
#[cfg(target_os = "android")]
unsafe fn static_object_field(
    env: *mut ::jni::sys::JNIEnv,
    class: &std::ffi::CStr,
    field: &std::ffi::CStr,
    sig: &std::ffi::CStr,
) -> Option<::jni::sys::jobject> {
    unsafe {
        let cls = ((**env).v1_1.FindClass)(env, class.as_ptr());
        if cls.is_null() {
            return None;
        }
        let fid = ((**env).v1_1.GetStaticFieldID)(env, cls, field.as_ptr(), sig.as_ptr());
        if fid.is_null() {
            return None;
        }
        let value = ((**env).v1_1.GetStaticObjectField)(env, cls, fid);
        if value.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        Some(value)
    }
}

/// File.getAbsolutePath() → String (jumeau capture.rs).
#[cfg(target_os = "android")]
unsafe fn file_path(env: *mut ::jni::sys::JNIEnv, file: ::jni::sys::jobject) -> Option<String> {
    let Some(jstr) = call_object(
        env,
        file,
        c"java/io/File",
        c"getAbsolutePath",
        c"()Ljava/lang/String;",
        &[],
    ) else {
        return None;
    };
    unsafe {
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
}

/// NewStringUTF sûr (jumeau du helper capture.rs — CString obligatoire :
/// une &str Rust n'est pas terminée par NUL, constat 2026-09-09).
#[cfg(target_os = "android")]
unsafe fn java_string(env: *mut ::jni::sys::JNIEnv, s: &str) -> Option<::jni::sys::jobject> {
    let Some(cs) = std::ffi::CString::new(s).ok() else {
        return None;
    };
    unsafe {
        let jstr = ((**env).v1_1.NewStringUTF)(env, cs.as_ptr());
        if jstr.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        Some(jstr)
    }
}
