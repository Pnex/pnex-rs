//! Maintien du réveil d'écran — `Window.FLAG_KEEP_SCREEN_ON` via JNI brut
//! (pas de permission, déterministe dans la webview ; l'API JS Wake Lock
//! est inégale selon les versions de WebView).
//!
//! `ndk_context` porte l'Activity (posée par dioxus-mobile) :
//! `getWindow().addFlags/clearFlags(0x80)`.

use crate::capture::attach_env;

/// Constante `WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON`.
const FLAG_KEEP_SCREEN_ON: i32 = 0x0000_0080;

/// Active/désactive le maintien du réveil d'écran.
pub fn keep_screen_on(on: bool) {
    let Some(env) = attach_env() else {
        return;
    };
    let Some(ctx) = ndk_context::try_android_context() else {
        return;
    };
    let activity = ctx.context() as jni::sys::jobject;

    unsafe {
        let act_class = ((**env).v1_1.FindClass)(env, c"android/app/Activity".as_ptr());
        if act_class.is_null() {
            return;
        }
        let mid_get_window = ((**env).v1_1.GetMethodID)(
            env,
            act_class,
            c"getWindow".as_ptr(),
            c"()Landroid/view/Window;".as_ptr(),
        );
        if mid_get_window.is_null() {
            return;
        }
        let window =
            ((**env).v1_1.CallObjectMethodA)(env, activity, mid_get_window, std::ptr::null())
                as jni::sys::jobject;
        if window.is_null() || ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
            return;
        }
        let win_class = ((**env).v1_1.FindClass)(env, c"android/view/Window".as_ptr());
        if win_class.is_null() {
            return;
        }
        let mid_flags = ((**env).v1_1.GetMethodID)(
            env,
            win_class,
            if on { c"addFlags" } else { c"clearFlags" }.as_ptr(),
            c"(I)V".as_ptr(),
        );
        if mid_flags.is_null() {
            return;
        }
        let args = [jni::sys::jvalue {
            i: FLAG_KEEP_SCREEN_ON,
        }];
        ((**env).v1_1.CallVoidMethodA)(env, window, mid_flags, args.as_ptr());
        if ((**env).v1_2.ExceptionCheck)(env) {
            ((**env).v1_1.ExceptionClear)(env);
        }
    }
}
