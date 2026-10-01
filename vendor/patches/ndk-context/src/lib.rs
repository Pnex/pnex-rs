//! Provides a stable api to rust crates for interfacing with the Android platform. It is
//! initialized by the runtime, usually [__ndk-glue__](https://crates.io/crates/ndk-glue),
//! but could also be initialized by Java or Kotlin code when embedding in an existing Android
//! project.
//!
//! ```no_run
//! let ctx = ndk_context::android_context();
//! let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }?;
//! let env = vm.attach_current_thread();
//! let class_ctx = env.find_class("android/content/Context")?;
//! let audio_service = env.get_static_field(class_ctx, "AUDIO_SERVICE", "Ljava/lang/String;")?;
//! let audio_manager = env
//!     .call_method(
//!         ctx.context() as jni::sys::jobject,
//!         "getSystemService",
//!         "(Ljava/lang/String;)Ljava/lang/Object;",
//!         &[audio_service],
//!     )?
//!     .l()?;
//! ```
use std::ffi::c_void;

static mut ANDROID_CONTEXT: Option<AndroidContext> = None;

/// [`AndroidContext`] provides the pointers required to interface with the jni on Android
/// platforms.
#[derive(Clone, Copy, Debug)]
pub struct AndroidContext {
    java_vm: *mut c_void,
    context_jobject: *mut c_void,
}

impl AndroidContext {
    /// A handle to the `JavaVM` object.
    ///
    /// Usage with [__jni__](https://crates.io/crates/jni) crate:
    /// ```no_run
    /// let ctx = ndk_context::android_context();
    /// let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }?;
    /// let env = vm.attach_current_thread();
    /// ```
    pub fn vm(self) -> *mut c_void {
        self.java_vm
    }

    /// A handle to an [android.content.Context](https://developer.android.com/reference/android/content/Context).
    /// In most cases this will be a ptr to an `Activity`, but this isn't guaranteed.
    ///
    /// Usage with [__jni__](https://crates.io/crates/jni) crate:
    /// ```no_run
    /// let ctx = ndk_context::android_context();
    /// let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }?;
    /// let env = vm.attach_current_thread();
    /// let class_ctx = env.find_class("android/content/Context")?;
    /// let audio_service = env.get_static_field(class_ctx, "AUDIO_SERVICE", "Ljava/lang/String;")?;
    /// let audio_manager = env
    ///     .call_method(
    ///         ctx.context() as jni::sys::jobject,
    ///         "getSystemService",
    ///         "(Ljava/lang/String;)Ljava/lang/Object;",
    ///         &[audio_service],
    ///     )?
    ///     .l()?;
    /// ```
    pub fn context(self) -> *mut c_void {
        self.context_jobject
    }
}

/// Main entry point to this crate. Returns an [`AndroidContext`].
///
/// # Panics
///
/// Panics if the context was not initialized (or was released) — use
/// [`try_android_context`] for the non-panicking variant.
pub fn android_context() -> AndroidContext {
    try_android_context().expect("android context was not initialized")
}

/// Non-panicking variant of [`android_context`] — `None` if the context has
/// not been initialized (termux, tests, call after destroy).
///
/// PATCH PNeX (2026-09-08) : l'API amont n'expose qu'un `android_context()`
/// paniquant — inutilisable pour du code défensif sans `catch_unwind`
/// (utilisé par le webbrowser vendu — cf. `vendor/patches/README.md`).
pub fn try_android_context() -> Option<AndroidContext> {
    // Lecture d'un `static mut` : même justification que le code amont —
    // écrit uniquement par le glue Android (thread UI) avant tout usage.
    unsafe { std::ptr::addr_of!(ANDROID_CONTEXT).read() }
}

/// Initializes the [`AndroidContext`]. [`AndroidContext`] is initialized by [__ndk-glue__](https://crates.io/crates/ndk-glue)
/// before `main` is called.
///
/// # Safety
///
/// The pointers must be valid.
///
/// PATCH PNeX (2026-09-08) : le contrat « exactly once » du code amont est
/// violé en pratique — un second `onCreate` dans le même process (relance
/// explicite de l'app, deep-link, réglage « ne pas garder les activités »)
/// atteint de nouveau ce chemin, et l'assert amont `previous.is_none()`
/// tuait l'appli (SIGABRT « PnexFrontend keeps stopping », constat
/// 2026-09-08). La ré-initialisation écrase simplement : le contexte le plus
/// récent gagne, même sémantique que le glue tao (`replace` de son côté).
pub unsafe fn initialize_android_context(java_vm: *mut c_void, context_jobject: *mut c_void) {
    ANDROID_CONTEXT = Some(AndroidContext {
        java_vm,
        context_jobject,
    });
}

/// Removes the [`AndroidContext`]. It is released by [__ndk-glue__](https://crates.io/crates/ndk-glue)
/// when the activity is finished and destroyed.
///
/// # Safety
///
/// PATCH PNeX (2026-09-08) : plus d'assert `previous.is_some()` — un release
/// sans init (destroy après un create avorté) ne doit pas tuer le process.
/// Idempotent : release sans init = no-op.
pub unsafe fn release_android_context() {
    ANDROID_CONTEXT = None;
}
