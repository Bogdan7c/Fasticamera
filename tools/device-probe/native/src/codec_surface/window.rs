//! RAII-обёртка над `ANativeWindow`, полученным из Java `Surface` (входная Surface кодека).

use std::ffi::c_void;
use std::ptr::NonNull;

use ndk_sys::{
    ANativeWindow, ANativeWindow_fromSurface, ANativeWindow_getFormat, ANativeWindow_getHeight, ANativeWindow_getWidth,
    ANativeWindow_release,
};

use super::report::WindowReport;

/// Владеет одной ссылкой на `ANativeWindow`; освобождает её в `Drop`.
pub struct NativeWindow {
    raw: NonNull<ANativeWindow>,
}

impl NativeWindow {
    /// Берёт окно из Java `Surface`; `None`, если NDK вернул null.
    ///
    /// # Safety
    /// `env` — валидный `JNIEnv*` текущего потока, `surface` — живая локальная ссылка на
    /// `android.view.Surface`. Типы JNI у ndk-sys (jni-sys 0.3) и jni (0.4) разные,
    /// поэтому граница держится на `c_void`.
    pub unsafe fn from_surface(env: *mut c_void, surface: *mut c_void) -> Option<Self> {
        // SAFETY: контракт функции; ANativeWindow_fromSurface берёт свою ссылку на окно.
        let raw = unsafe { ANativeWindow_fromSurface(env.cast(), surface.cast()) };
        NonNull::new(raw).map(|raw| Self { raw })
    }

    /// Сырой указатель для `vkCreateAndroidSurfaceKHR` (живёт, пока жив `NativeWindow`).
    pub fn as_ptr(&self) -> *mut ANativeWindow {
        self.raw.as_ptr()
    }

    /// Ширина, высота и формат окна для отчёта.
    pub fn report(&self) -> WindowReport {
        // SAFETY: окно живо, пока существует self.
        unsafe {
            WindowReport {
                width: ANativeWindow_getWidth(self.raw.as_ptr()),
                height: ANativeWindow_getHeight(self.raw.as_ptr()),
                format: ANativeWindow_getFormat(self.raw.as_ptr()),
            }
        }
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        // SAFETY: освобождаем ровно ту ссылку, что взяли в from_surface.
        unsafe { ANativeWindow_release(self.raw.as_ptr()) };
    }
}
