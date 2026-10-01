//! HFOV de la caméra arrière — JNI brut Camera2 (école `capture.rs`).
//!
//! `getCameraCharacteristics` ne nécessite PAS d'ouvrir la caméra (simple
//! lecture de caractéristiques) : aucun conflit sur le flux getUserMedia
//! détenu par la webview. La caractéristique `get(Key)` exige les objets
//! Key statiques (`LENS_FACING`, `LENS_INFO_AVAILABLE_FOCAL_LENGTHS`,
//! `SENSOR_INFO_PHYSICAL_SIZE`) lus via GetStaticObjectField.
//!
//! Champ physique = long côté du capteur ≈ largeur image en paysage —
//! approximation v1 : getUserMedia peut recadrer. En cas d'échec, la
//! crate stitcher fournit le repli 65° (`FALLBACK_HFOV_DEG`).

use crate::capture::attach_env;

/// HFOV horizontale de la première caméra arrière (degrés), ou `None`.
pub fn back_camera_hfov_deg() -> Option<f32> {
    let env = attach_env()?;
    let ctx = ndk_context::try_android_context()?;
    let ctx_obj = ctx.context() as jni::sys::jobject;

    unsafe {
        // CameraManager = context.getSystemService("camera").
        let manager = get_system_service(env, ctx_obj, c"camera")?;
        let cc_class = ((**env).v1_1.FindClass)(
            env,
            c"android/hardware/camera2/CameraCharacteristics".as_ptr(),
        );
        if cc_class.is_null() {
            return None;
        }
        let mid_get_cc = ((**env).v1_1.GetMethodID)(
            env,
            manager_class(env)?,
            c"getCameraCharacteristics".as_ptr(),
            c"(Ljava/lang/String;)Landroid/hardware/camera2/CameraCharacteristics;".as_ptr(),
        );
        if mid_get_cc.is_null() {
            return None;
        }

        // getCameraIdList() → String[]
        let mid_ids = ((**env).v1_1.GetMethodID)(
            env,
            manager_class(env)?,
            c"getCameraIdList".as_ptr(),
            c"()[Ljava/lang/String;".as_ptr(),
        );
        if mid_ids.is_null() {
            return None;
        }
        let ids = ((**env).v1_1.CallObjectMethodA)(env, manager, mid_ids, std::ptr::null())
            as jni::sys::jobjectArray;
        if ids.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return None;
        }
        let n = ((**env).v1_1.GetArrayLength)(env, ids as jni::sys::jarray);

        for idx in 0..n {
            let id = ((**env).v1_1.GetObjectArrayElement)(env, ids, idx);
            if id.is_null() {
                continue;
            }
            let args = [jni::sys::jvalue { l: id }];
            let cc = ((**env).v1_1.CallObjectMethodA)(env, manager, mid_get_cc, args.as_ptr())
                as jni::sys::jobject;
            if cc.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                ((**env).v1_1.ExceptionClear)(env);
                ((**env).v1_1.DeleteLocalRef)(env, id as jni::sys::jobject);
                continue;
            }
            let hfov = characteristics_hfov(env, cc_class, cc);
            ((**env).v1_1.DeleteLocalRef)(env, id as jni::sys::jobject);
            if let Some(h) = hfov {
                return Some(h);
            }
        }
        None
    }
}

/// Context.getSystemService(String) → Object (service framework, pas de
/// chargement de classe custom).
unsafe fn get_system_service(
    env: *mut jni::sys::JNIEnv,
    ctx_obj: jni::sys::jobject,
    name: &std::ffi::CStr,
) -> Option<jni::sys::jobject> {
    let ctx_class = ((**env).v1_1.FindClass)(env, c"android/content/Context".as_ptr());
    if ctx_class.is_null() {
        return None;
    }
    let mid = ((**env).v1_1.GetMethodID)(
        env,
        ctx_class,
        c"getSystemService".as_ptr(),
        c"(Ljava/lang/String;)Ljava/lang/Object;".as_ptr(),
    );
    if mid.is_null() {
        return None;
    }
    let jname = ((**env).v1_1.NewStringUTF)(env, name.as_ptr());
    if jname.is_null() {
        return None;
    }
    let args = [jni::sys::jvalue { l: jname }];
    let obj =
        ((**env).v1_1.CallObjectMethodA)(env, ctx_obj, mid, args.as_ptr()) as jni::sys::jobject;
    if obj.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
        ((**env).v1_1.ExceptionClear)(env);
        return None;
    }
    Some(obj)
}

/// Classe CameraManager (FindClass, garde null).
unsafe fn manager_class(env: *mut jni::sys::JNIEnv) -> Option<jni::sys::jclass> {
    let cls = ((**env).v1_1.FindClass)(env, c"android/hardware/camera2/CameraManager".as_ptr());
    if cls.is_null() {
        return None;
    }
    Some(cls)
}

/// HFOV d'une caractéristique : LENS_FACING = BACK (1) requis, puis
/// `2·atan(long côté capteur / (2·focale))`.
unsafe fn characteristics_hfov(
    env: *mut jni::sys::JNIEnv,
    cc_class: jni::sys::jclass,
    cc: jni::sys::jobject,
) -> Option<f32> {
    // Récupère l'objet Key statique de CameraCharacteristics.
    let get_key =
        |env: *mut jni::sys::JNIEnv, name: &std::ffi::CStr| -> Option<jni::sys::jobject> {
            let fid = ((**env).v1_1.GetStaticFieldID)(
                env,
                cc_class,
                name.as_ptr(),
                c"Landroid/hardware/camera2/CameraCharacteristics$Key;".as_ptr(),
            );
            if fid.is_null() {
                return None;
            }
            let key = ((**env).v1_1.GetStaticObjectField)(env, cc_class, fid);
            if key.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                ((**env).v1_1.ExceptionClear)(env);
                return None;
            }
            Some(key)
        };

    // characteristics.get(Key) → Object.
    let mid_get = ((**env).v1_1.GetMethodID)(
        env,
        cc_class,
        c"get".as_ptr(),
        c"(Landroid/hardware/camera2/CameraCharacteristics$Key;)Ljava/lang/Object;".as_ptr(),
    );
    if mid_get.is_null() {
        return None;
    }
    let call_get =
        |env: *mut jni::sys::JNIEnv, key: jni::sys::jobject| -> Option<jni::sys::jobject> {
            let args = [jni::sys::jvalue { l: key }];
            let obj = ((**env).v1_1.CallObjectMethodA)(env, cc, mid_get, args.as_ptr())
                as jni::sys::jobject;
            if obj.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
                ((**env).v1_1.ExceptionClear)(env);
                return None;
            }
            Some(obj)
        };

    // 1) Filtre LENS_FACING == 1 (BACK).
    let facing_key = get_key(env, c"LENS_FACING")?;
    let facing = call_get(env, facing_key)?;
    let int_class = ((**env).v1_1.FindClass)(env, c"java/lang/Integer".as_ptr());
    if int_class.is_null() {
        return None;
    }
    let mid_int = ((**env).v1_1.GetMethodID)(env, int_class, c"intValue".as_ptr(), c"()I".as_ptr());
    if mid_int.is_null() {
        return None;
    }
    let facing_val = ((**env).v1_1.CallIntMethodA)(env, facing, mid_int, std::ptr::null());
    if ((**env).v1_2.ExceptionCheck)(env) {
        ((**env).v1_1.ExceptionClear)(env);
        return None;
    }
    if facing_val != 1 {
        return None;
    }

    // 2) LENS_INFO_AVAILABLE_FOCAL_LENGTHS → float[].
    let focal_key = get_key(env, c"LENS_INFO_AVAILABLE_FOCAL_LENGTHS")?;
    let focals = call_get(env, focal_key)?;
    let flen = ((**env).v1_1.GetArrayLength)(env, focals as jni::sys::jarray);
    if flen <= 0 {
        return None;
    }
    let elems = ((**env).v1_1.GetFloatArrayElements)(
        env,
        focals as jni::sys::jfloatArray,
        std::ptr::null_mut(),
    );
    if elems.is_null() {
        return None;
    }
    let f = *elems;
    ((**env).v1_1.ReleaseFloatArrayElements)(
        env,
        focals as jni::sys::jfloatArray,
        elems,
        jni::sys::JNI_OK,
    );
    if f <= 0.0 {
        return None;
    }

    // 3) SENSOR_INFO_PHYSICAL_SIZE → SizeF (android/util/SizeF).
    let size_key = get_key(env, c"SENSOR_INFO_PHYSICAL_SIZE")?;
    let size = call_get(env, size_key)?;
    let sizef_class = ((**env).v1_1.FindClass)(env, c"android/util/SizeF".as_ptr());
    if sizef_class.is_null() {
        return None;
    }
    let mid_w = ((**env).v1_1.GetMethodID)(env, sizef_class, c"getWidth".as_ptr(), c"()F".as_ptr());
    let mid_h =
        ((**env).v1_1.GetMethodID)(env, sizef_class, c"getHeight".as_ptr(), c"()F".as_ptr());
    if mid_w.is_null() || mid_h.is_null() {
        return None;
    }
    let w = ((**env).v1_1.CallFloatMethodA)(env, size, mid_w, std::ptr::null());
    let h = ((**env).v1_1.CallFloatMethodA)(env, size, mid_h, std::ptr::null());
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    // Champ = long côté (capteur paysage) — 2·atan(w/2f) = hfov.
    let long = w.max(h);
    Some((2.0 * (long / (2.0 * f)).atan()).to_degrees())
}
