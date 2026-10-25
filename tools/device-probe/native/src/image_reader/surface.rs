//! `AImageReader` → `android.view.Surface` для Camera2 на стороне Kotlin.

use std::ffi::c_void;
use std::ptr;

use ndk_sys::{AImageReader_getWindow, ANativeWindow, ANativeWindow_toSurface};

use super::registry::find_reader;

/// Возвращает Java `Surface` ридера или `None` (неизвестный handle / ридер удалён / ошибка NDK).
///
/// # Safety
/// `env` обязан быть валидным `JNIEnv*` текущего потока (его даёт JNI-вызов). Типы JNI
/// (`jni-sys` 0.3 у ndk-sys и 0.4 у jni) различаются, поэтому границу держим на `c_void`.
pub unsafe fn reader_surface(handle: i64, env: *mut c_void) -> Option<*mut c_void> {
    let entry = find_reader(handle)?;
    let guard = entry.lock_native();
    let native = guard.as_ref()?;

    let mut window: *mut ANativeWindow = ptr::null_mut();
    // SAFETY: ридер жив (держим мьютекс), `window` — валидный out-параметр.
    // Окно принадлежит ридеру, его не нужно освобождать.
    let status = unsafe { AImageReader_getWindow(native.as_ptr(), &mut window) };
    if status.0 != 0 || window.is_null() {
        log::warn!("AImageReader_getWindow -> {}", status.0);
        return None;
    }
    // SAFETY: `env` валиден по контракту функции, `window` только что получен и жив.
    let surface = unsafe { ANativeWindow_toSurface(env.cast(), window) };
    (!surface.is_null()).then_some(surface.cast())
}
