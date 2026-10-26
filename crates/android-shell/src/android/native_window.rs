//! Safety-модуль: владение ссылкой на `ANativeWindow` (NDK `android/native_window.h`).
//!
//! `ANativeWindow` — объект со счётчиком ссылок. [`NativeWindowRef`] держит ровно одну
//! ссылку: получает её от `ANativeWindow_fromSurface` или `ANativeWindow_acquire` (клон)
//! и отдаёт в `Drop` через `ANativeWindow_release`. Поэтому окно живо, пока жива хоть одна
//! ссылка, даже если Android уже уничтожил `Surface` (рисовать на нём тогда нельзя, но
//! обращение к указателю не обращается к освобождённой памяти).

use std::ffi::c_void;
use std::ptr::NonNull;

use jni::Env;
use jni::objects::JObject;
use ndk_sys::{
    ANativeWindow, ANativeWindow_ChangeFrameRateStrategy, ANativeWindow_FrameRateCompatibility, ANativeWindow_acquire,
    ANativeWindow_fromSurface, ANativeWindow_release, ANativeWindow_setFrameRateWithChangeStrategy,
};
use shell_contract::commands::FrameRate;
use shell_contract::surface::PlatformWindow;

/// Одна ссылка на окно Android.
#[derive(Debug)]
pub struct NativeWindowRef(NonNull<ANativeWindow>);

// SAFETY: ANativeWindow — потокобезопасный объект платформы со счётчиком ссылок (atomic
// incStrong/decStrong в libnativewindow); acquire/release и setFrameRate разрешено вызывать
// с любого потока. Мы не даём доступа к изменяемому состоянию окна в обход NDK.
unsafe impl Send for NativeWindowRef {}
// SAFETY: см. Send — все операции идут через потокобезопасные функции NDK.
unsafe impl Sync for NativeWindowRef {}

impl NativeWindowRef {
    /// Окно Java-объекта `android.view.Surface`. `None` — у Surface нет окна (уже уничтожена).
    pub fn from_surface(env: &Env<'_>, surface: &JObject<'_>) -> Option<Self> {
        // SAFETY: env — действующий JNIEnv текущего потока (мы внутри нативного метода),
        // surface — живая локальная ссылка на android.view.Surface из сигнатуры метода.
        // Типы JNIEnv/jobject из jni-sys 0.4 и 0.3 (ndk-sys) — одинаковые C-указатели.
        let window = unsafe { ANativeWindow_fromSurface(env.get_raw().cast(), surface.as_raw().cast()) };
        // fromSurface возвращает уже захваченную ссылку — она теперь наша.
        NonNull::new(window).map(Self)
    }

    /// Пожелание частоты кадров. Изменение частоты дисплея — только бесшовное
    /// (`ONLY_IF_SEAMLESS`): короткий видоискатель не должен мигать чёрным экраном.
    /// Возвращает код платформы: 0 — принято, отрицательный errno — отказ.
    pub fn set_frame_rate(&self, rate: FrameRate) -> i32 {
        // SAFETY: self.0 — действующее окно, на которое мы держим ссылку; значения
        // совместимости и стратегии — константы NDK из тех же заголовков.
        unsafe {
            ANativeWindow_setFrameRateWithChangeStrategy(
                self.0.as_ptr(),
                rate.hertz(),
                ANativeWindow_FrameRateCompatibility::ANATIVEWINDOW_FRAME_RATE_COMPATIBILITY_DEFAULT.0 as i8,
                ANativeWindow_ChangeFrameRateStrategy::ANATIVEWINDOW_CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS.0 as i8,
            )
        }
    }
}

impl Clone for NativeWindowRef {
    fn clone(&self) -> Self {
        // SAFETY: self.0 — действующее окно; acquire добавляет ссылку, которую отпустит Drop клона.
        unsafe { ANativeWindow_acquire(self.0.as_ptr()) };
        Self(self.0)
    }
}

impl Drop for NativeWindowRef {
    fn drop(&mut self) {
        // SAFETY: ссылка получена в from_surface или clone и отпускается ровно один раз.
        unsafe { ANativeWindow_release(self.0.as_ptr()) };
    }
}

impl PlatformWindow for NativeWindowRef {
    fn raw_window(&self) -> NonNull<c_void> {
        self.0.cast()
    }
}
