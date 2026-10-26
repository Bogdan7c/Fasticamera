//! Safety-модуль: тепловой сервис NDK (`android/thermal.h`, API 30–31).
//!
//! В `ndk-sys` 0.6 тепловых функций нет, поэтому объявления — здесь, по заголовку NDK 30
//! (`toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/include/android/thermal.h`).
//! Символы живут в `libandroid.so`, её уже линкует `ndk-sys`.
//!
//! Колбэк статуса Android вызывает на потоке binder: там только отправка сигнала в свой
//! канал, никакой работы и никакой паники наружу (`catch_unwind`).

use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use shell_contract::thermal::{HeadroomForecast, ThermalHeadroom, ThermalSource, ThermalSourceError, ThermalStatus};

use crate::thermal_codes::thermal_status_from_platform_code;

/// Непрозрачный `AThermalManager`.
#[repr(C)]
struct AThermalManager {
    _private: [u8; 0],
}

/// `AThermal_StatusCallback`: `void (*)(void* data, AThermalStatus status)`.
type AThermalStatusCallback = Option<unsafe extern "C" fn(data: *mut c_void, status: c_int)>;

unsafe extern "C" {
    fn AThermal_acquireManager() -> *mut AThermalManager;
    fn AThermal_releaseManager(manager: *mut AThermalManager);
    fn AThermal_getCurrentThermalStatus(manager: *mut AThermalManager) -> c_int;
    fn AThermal_registerThermalStatusListener(
        manager: *mut AThermalManager,
        callback: AThermalStatusCallback,
        data: *mut c_void,
    ) -> c_int;
    fn AThermal_unregisterThermalStatusListener(
        manager: *mut AThermalManager,
        callback: AThermalStatusCallback,
        data: *mut c_void,
    ) -> c_int;
    fn AThermal_getThermalHeadroom(manager: *mut AThermalManager, forecast_seconds: c_int) -> f32;
}

/// Наш экземпляр теплового менеджера.
#[derive(Debug)]
pub struct ThermalManager(NonNull<AThermalManager>);

// SAFETY: функции AThermal_* — обёртки над вызовами binder к системному сервису; NDK не
// требует вызывать их с потока, создавшего менеджер. Менеджер освобождается один раз в Drop.
unsafe impl Send for ThermalManager {}
// SAFETY: см. Send — состояние менеджера меняет только сам сервис.
unsafe impl Sync for ThermalManager {}

impl ThermalManager {
    pub fn acquire() -> Result<Self, ThermalSourceError> {
        // SAFETY: функция без аргументов; null — честный отказ, проверяется ниже.
        let manager = unsafe { AThermal_acquireManager() };
        NonNull::new(manager)
            .map(Self)
            .ok_or_else(|| ThermalSourceError::Unavailable("AThermal_acquireManager вернул null".to_owned()))
    }

    /// Подписывает канал на смену статуса: при каждой смене в `wake` уходит сигнал.
    /// Подписка действует, пока жива [`StatusListener`].
    pub fn listen_status_changes(self: &Arc<Self>, wake: SyncSender<()>) -> Result<StatusListener, c_int> {
        let context = Box::into_raw(Box::new(ListenerContext { wake }));
        // SAFETY: менеджер жив; context — указатель на нашу коробку, которую колбэк только читает,
        // а освобождаем её после успешной отписки (гарантия NDK: после неё колбэков нет).
        let status =
            unsafe { AThermal_registerThermalStatusListener(self.0.as_ptr(), Some(status_changed), context.cast()) };
        if status == 0 {
            Ok(StatusListener {
                manager: Arc::clone(self),
                context,
            })
        } else {
            // SAFETY: регистрация не удалась, колбэк никогда не увидит context — забираем коробку.
            drop(unsafe { Box::from_raw(context) });
            Err(status)
        }
    }
}

impl Drop for ThermalManager {
    fn drop(&mut self) {
        // SAFETY: менеджер получен acquire и освобождается ровно один раз.
        unsafe { AThermal_releaseManager(self.0.as_ptr()) };
    }
}

impl ThermalSource for ThermalManager {
    fn current_status(&self) -> Result<ThermalStatus, ThermalSourceError> {
        // SAFETY: менеджер жив.
        let code = unsafe { AThermal_getCurrentThermalStatus(self.0.as_ptr()) };
        thermal_status_from_platform_code(code).map_err(|error| ThermalSourceError::Unavailable(error.to_string()))
    }

    fn headroom(&self, forecast: HeadroomForecast) -> ThermalHeadroom {
        let seconds = c_int::try_from(forecast.horizon().as_secs()).unwrap_or(c_int::MAX);
        // SAFETY: менеджер жив; NaN при слишком частом опросе превращается в Unavailable.
        let headroom = unsafe { AThermal_getThermalHeadroom(self.0.as_ptr(), seconds) };
        ThermalHeadroom::from_platform_value(headroom)
    }
}

/// Данные колбэка: канал пробуждения потока теплового монитора.
struct ListenerContext {
    wake: SyncSender<()>,
}

/// Колбэк статуса (поток binder). Только неблокирующий сигнал; паника не уходит в C.
unsafe extern "C" fn status_changed(data: *mut c_void, _status: c_int) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: data — указатель на ListenerContext, живой до успешной отписки.
        let context = unsafe { &*data.cast::<ListenerContext>() };
        // Канал на одно место: если сигнал уже ждёт, второй не нужен — монитор прочтёт свежий статус.
        let _ = context.wake.try_send(());
    }));
}

/// Действующая подписка на смену статуса.
pub struct StatusListener {
    manager: Arc<ThermalManager>,
    context: *mut ListenerContext,
}

// SAFETY: context — коробка с SyncSender (Send); доступ к ней — только колбэк (чтение) и Drop.
unsafe impl Send for StatusListener {}

impl Drop for StatusListener {
    fn drop(&mut self) {
        // SAFETY: те же колбэк и данные, что при регистрации; менеджер жив (держим Arc).
        let status = unsafe {
            AThermal_unregisterThermalStatusListener(self.manager.0.as_ptr(), Some(status_changed), self.context.cast())
        };
        if status == 0 {
            // SAFETY: после успешной отписки колбэков с этим context больше не будет.
            drop(unsafe { Box::from_raw(self.context) });
        } else {
            // Отписка не удалась: колбэк ещё может прийти, освобождать коробку нельзя — оставляем
            // её (утечка нескольких байт лучше обращения к освобождённой памяти) и сообщаем.
            tracing::error!(target: "fasticamera::shell", status, "отписка от теплового статуса не удалась");
        }
    }
}
