//! Safety-модуль: поток vsync на `AChoreographer` (NDK `android/choreographer.h`, API 33+).
//!
//! `ndk-sys` 0.6 уже содержит нужные объявления (`AChoreographer_postVsyncCallback`,
//! `AChoreographerFrameCallbackData_*`), собственного `extern "C"` здесь нет; библиотека
//! `libandroid.so` линкуется через `ndk-sys`. Минимальный `minSdk` проекта — 36, поэтому
//! символы API 33 есть всегда; проверка `android_get_device_api_level` оставлена как
//! честная ошибка `UnsupportedApiLevel`, если когда-нибудь minSdk снизят.
//!
//! # Модель потоков
//! Choreographer привязан к потоку с looper. Каждая подписка ([`start_choreographer_thread`])
//! получает СВОЙ поток: готовит looper, берёт `AChoreographer_getInstance`, постит колбэк и
//! крутит `ALooper_pollOnce`, внутри которого платформа вызывает [`on_vsync`]. Колбэк каждый
//! раз ставит себя в очередь заново (Choreographer вызывает колбэк один раз на постановку).
//! Несколько подписок одновременно допустимы — они независимы; на практике потребитель один
//! (поток рендера), поэтому общий реестр «занято» не нужен.
//!
//! # Остановка
//! [`ChoreographerThread`] в `Drop` ставит флаг, будит looper (`ALooper_wake`) и ждёт поток
//! (`join`). Ссылка на looper у ручки собственная (`ALooper_acquire`), как у потока датчика.
//! Контекст колбэка живёт в куче до конца потока; колбэки вызываются только из `pollOnce`
//! этого же потока, поэтому гонки с освобождением контекста нет.
//!
//! # Ошибки колбэка
//! Паника через границу FFI — UB, поэтому ВСЁ тело колбэка (кадр, учёт сбоя, постановка) обёрнуто
//! в `catch_unwind`; паника самой обвязки останавливает поток vsync. Любая
//! ошибка (нет линии кадра, паника) логируется, считается через `on_failure` (счётчик
//! `native_callback_failures` оболочки), кадр пропускается, колбэк ставится снова.

use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{NonNull, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ndk_sys::{
    AChoreographer, AChoreographer_getInstance, AChoreographer_postVsyncCallback, AChoreographerFrameCallbackData,
    AChoreographerFrameCallbackData_getFrameTimeNanos, AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos,
    AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos,
    AChoreographerFrameCallbackData_getFrameTimelineVsyncId, AChoreographerFrameCallbackData_getFrameTimelinesLength,
    AChoreographerFrameCallbackData_getPreferredFrameTimelineIndex, ALOOPER_POLL_ERROR, ALooper_pollOnce,
    ALooper_prepare, android_get_device_api_level,
};
use shell_contract::vsync::{VsyncSlot, VsyncSourceError};

use super::sensor_thread::LooperHandle;
use crate::vsync_timeline::{FrameTimeline, FrameTimelineReader, build_tick};

/// Первый API с `AChoreographer_postVsyncCallback` и `AChoreographerFrameCallbackData_*`
/// (`__INTRODUCED_IN(33)` в choreographer.h).
const REQUIRED_API_LEVEL: u32 = 33;
/// Страховочный таймаут `pollOnce`: флаг остановки проверяется хотя бы так часто, даже если
/// `ALooper_wake` не дошёл.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// Имя потока (видно в systrace и в отчётах паник).
const THREAD_NAME: &str = "fasticamera-vsync";
/// Повторяющиеся ошибки пишем в лог первый раз и затем раз в столько срабатываний:
/// при 120 Гц постоянный сбой иначе завалил бы logcat.
const LOG_EVERY_NTH_FAILURE: u64 = 600;

/// Колбэк учёта ошибки: вызывается из потока vsync, обязан быть быстрым и не паниковать.
pub type FailureCounter = Box<dyn Fn() + Send + 'static>;

/// Работающий поток vsync. Уничтожение останавливает колбэки и поток.
pub struct ChoreographerThread {
    stop_requested: Arc<AtomicBool>,
    looper: Option<LooperHandle>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for ChoreographerThread {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
        if let Some(looper) = &self.looper {
            looper.wake();
        }
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!(target: "fasticamera::shell", "поток vsync завершился паникой");
        }
        // Ссылку на looper отпускаем только после join.
        drop(self.looper.take());
    }
}

/// Запускает поток vsync; тики публикуются в `slot`. Возвращается, когда первый колбэк
/// поставлен или запуск провалился. Когда поток заканчивается сам (ошибка looper), ящик
/// останавливается и потребитель видит `Stopped`.
pub fn start_choreographer_thread(
    slot: Arc<VsyncSlot>,
    on_failure: FailureCounter,
) -> Result<ChoreographerThread, VsyncSourceError> {
    check_api_level()?;
    let stop_requested = Arc::new(AtomicBool::new(false));
    let (startup_sender, startup_receiver) = sync_channel(1);
    let worker_stop = Arc::clone(&stop_requested);
    let worker = thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || {
            run_vsync_thread(slot, &worker_stop, on_failure, |startup| {
                // Запускающий всегда ждёт в recv; если его нет, отчёт о запуске просто не нужен.
                let _ = startup_sender.send(startup);
            });
        })
        .map_err(|error| VsyncSourceError::PlatformFailure {
            reason: format!("поток vsync не создан: {error}"),
        })?;
    let startup = startup_receiver.recv().map_err(|_| VsyncSourceError::PlatformFailure {
        reason: "поток vsync завершился до отчёта о запуске".to_owned(),
    });
    match startup.and_then(|result| result) {
        Ok(looper) => Ok(ChoreographerThread {
            stop_requested,
            looper: Some(looper),
            worker: Some(worker),
        }),
        Err(error) => {
            // Поток уже вышел (запуск не удался) — дожидаемся, чтобы не оставлять висящих.
            let _ = worker.join();
            Err(error)
        }
    }
}

fn check_api_level() -> Result<(), VsyncSourceError> {
    // SAFETY: функция без аргументов, только читает системное свойство.
    let reported = unsafe { android_get_device_api_level() };
    // Отрицательное значение — «не удалось узнать»: считаем уровень нулевым и честно отказываем.
    let actual = u32::try_from(reported).unwrap_or(0);
    if actual < REQUIRED_API_LEVEL {
        return Err(VsyncSourceError::UnsupportedApiLevel {
            required: REQUIRED_API_LEVEL,
            actual,
        });
    }
    Ok(())
}

type StartupResult = Result<LooperHandle, VsyncSourceError>;

/// Состояние колбэка. Создаётся и освобождается в потоке vsync, туда же и указывает `data`
/// колбэка; трогают его только колбэки внутри `pollOnce` этого потока.
struct CallbackContext {
    choreographer: NonNull<AChoreographer>,
    slot: Arc<VsyncSlot>,
    stop_requested: Arc<AtomicBool>,
    on_failure: FailureCounter,
    /// Номер следующего vsync. Растёт на каждый колбэк, даже неудачный: пропуск виден потребителю как `skipped`.
    next_sequence: u64,
    failures: u64,
}

impl CallbackContext {
    /// Обрабатывает один vsync: собирает тик и публикует его.
    fn handle_frame(&mut self, reader: &CallbackDataReader) {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        match build_tick(reader, sequence) {
            Ok(tick) => self.slot.publish(tick),
            Err(error) => {
                tracing::debug!(target: "fasticamera::shell", %error, sequence, "vsync пропущен: нет подходящей линии кадра");
                self.record_failure("линия кадра Choreographer");
            }
        }
    }

    fn record_failure(&mut self, what: &str) {
        self.failures += 1;
        (self.on_failure)();
        if self.failures == 1 || self.failures.is_multiple_of(LOG_EVERY_NTH_FAILURE) {
            tracing::error!(
                target: "fasticamera::shell",
                what,
                failures = self.failures,
                "сбой колбэка vsync (кадр пропущен, колбэк поставлен снова)"
            );
        }
    }

    /// Один полный цикл колбэка: обработать кадр и поставить колбэк снова. Паника при обработке
    /// кадра считается сбоем кадра (кадр пропущен, цикл продолжается); паника ВНЕ обработки кадра
    /// (в `record_failure`, `post_next`) всплывает к вызывающему — тот остановит поток.
    fn run_vsync_cycle(&mut self, callback_data: *const AChoreographerFrameCallbackData, user_data: *mut c_void) {
        let handled = catch_unwind(AssertUnwindSafe(|| match NonNull::new(callback_data.cast_mut()) {
            Some(data) => self.handle_frame(&CallbackDataReader { data }),
            None => self.record_failure("null вместо данных колбэка"),
        }));
        if handled.is_err() {
            self.record_failure("паника в колбэке vsync");
        }
        self.post_next(user_data);
    }

    /// Ставит колбэк на следующий vsync (Choreographer вызывает его один раз на постановку).
    fn post_next(&self, user_data: *mut c_void) {
        if self.stop_requested.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: choreographer получен AChoreographer_getInstance этого потока и живёт столько же;
        // вызываем из его потока. user_data — наш контекст, живущий до конца потока.
        unsafe { AChoreographer_postVsyncCallback(self.choreographer.as_ptr(), Some(on_vsync), user_data) };
    }
}

/// Читает данные колбэка. Указатель действителен ТОЛЬКО во время вызова колбэка
/// (choreographer.h: данные принадлежат системе и не должны сохраняться).
struct CallbackDataReader {
    data: NonNull<AChoreographerFrameCallbackData>,
}

impl FrameTimelineReader for CallbackDataReader {
    fn frame_time_nanos(&self) -> i64 {
        // SAFETY: data действителен во время колбэка, ридер не переживает колбэк.
        unsafe { AChoreographerFrameCallbackData_getFrameTimeNanos(self.data.as_ptr()) }
    }

    fn timeline_count(&self) -> usize {
        // SAFETY: см. frame_time_nanos.
        unsafe { AChoreographerFrameCallbackData_getFrameTimelinesLength(self.data.as_ptr()) }
    }

    fn preferred_timeline_index(&self) -> usize {
        // SAFETY: см. frame_time_nanos.
        unsafe { AChoreographerFrameCallbackData_getPreferredFrameTimelineIndex(self.data.as_ptr()) }
    }

    fn timeline(&self, index: usize) -> FrameTimeline {
        // SAFETY: см. frame_time_nanos; build_tick вызывает только с index < timeline_count()
        // (в заголовке: «index must be less than getFrameTimelinesLength»).
        unsafe {
            FrameTimeline {
                vsync_id: AChoreographerFrameCallbackData_getFrameTimelineVsyncId(self.data.as_ptr(), index),
                expected_present_nanos: AChoreographerFrameCallbackData_getFrameTimelineExpectedPresentationTimeNanos(
                    self.data.as_ptr(),
                    index,
                ),
                deadline_nanos: AChoreographerFrameCallbackData_getFrameTimelineDeadlineNanos(
                    self.data.as_ptr(),
                    index,
                ),
            }
        }
    }
}

/// `AChoreographer_vsyncCallback`: вызывается платформой из `ALooper_pollOnce` потока vsync.
///
/// Никогда не паникует наружу (`catch_unwind` охватывает всё тело) и пытается поставить себя снова;
/// если упала сама обвязка колбэка, колбэк не ставится и поток vsync останавливается.
unsafe extern "C" fn on_vsync(callback_data: *const AChoreographerFrameCallbackData, user_data: *mut c_void) {
    let Some(context_pointer) = NonNull::new(user_data.cast::<CallbackContext>()) else {
        // Платформа вернула нам не наш указатель: продолжать нельзя и сообщить некуда, кроме лога.
        tracing::error!(target: "fasticamera::shell", "колбэк vsync получил null вместо контекста");
        return;
    };
    // SAFETY: user_data — Box<CallbackContext>, созданный в open_choreographer и живущий до конца
    // потока; колбэки идут последовательно из pollOnce этого потока, второй &mut не существует.
    let context = unsafe { &mut *context_pointer.as_ptr() };
    // Внешний catch_unwind охватывает ВСЁ тело колбэка, включая обвязку (пользовательский
    // `on_failure`, лог, постановку следующего колбэка): паника в любой из них иначе прошла бы
    // через `extern "C"` — это UB.
    let cycle = catch_unwind(AssertUnwindSafe(|| context.run_vsync_cycle(callback_data, user_data)));
    if cycle.is_err() {
        // Обвязка упала: состояние неизвестно, колбэк заново НЕ ставим. Поток видит флаг остановки,
        // выходит из цикла и закрывает слот (потребитель получает `Stopped`, а не вечную тишину).
        context.stop_requested.store(true, Ordering::Release);
        // Лог тоже может паниковать (кривой подписчик tracing): результат осознанно не используем —
        // сообщить больше некуда, а наружу паника выйти не должна.
        let _best_effort_log = catch_unwind(|| {
            tracing::error!(
                target: "fasticamera::shell",
                "паника в обвязке колбэка vsync: колбэк не ставится снова, поток vsync останавливается"
            );
        });
    }
}

fn run_vsync_thread(
    slot: Arc<VsyncSlot>,
    stop_requested: &Arc<AtomicBool>,
    on_failure: FailureCounter,
    report_startup: impl FnOnce(StartupResult),
) {
    let opened = open_choreographer(&slot, stop_requested, on_failure);
    let (context_pointer, looper_handle) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            report_startup(Err(error));
            slot.stop();
            return;
        }
    };
    report_startup(Ok(looper_handle));
    let timeout_ms = c_int::try_from(POLL_TIMEOUT.as_millis()).unwrap_or(c_int::MAX);
    while !stop_requested.load(Ordering::Acquire) {
        // SAFETY: looper этого потока подготовлен в open_choreographer; выходные указатели не нужны.
        // Колбэки Choreographer вызываются изнутри этого вызова, на этом же потоке.
        let outcome = unsafe { ALooper_pollOnce(timeout_ms, null_mut(), null_mut(), null_mut()) };
        if outcome == ALOOPER_POLL_ERROR {
            // Looper сломан: без него тиков не будет. Выходим громко — потребитель получит Stopped.
            // SAFETY: контекст создан в open_choreographer и освобождается ниже; сейчас колбэк не идёт.
            unsafe {
                (*context_pointer.as_ptr()).record_failure("ошибка ALooper_pollOnce, поток vsync останавливается")
            };
            break;
        }
    }
    // SAFETY: указатель создан Box::into_raw в open_choreographer; колбэки идут только из pollOnce
    // этого потока, цикл закончен — больше обращений к контексту не будет.
    drop(unsafe { Box::from_raw(context_pointer.as_ptr()) });
    slot.stop();
}

/// Готовит looper потока, берёт Choreographer и ставит первый колбэк.
fn open_choreographer(
    slot: &Arc<VsyncSlot>,
    stop_requested: &Arc<AtomicBool>,
    on_failure: FailureCounter,
) -> Result<(NonNull<CallbackContext>, LooperHandle), VsyncSourceError> {
    // SAFETY: готовим looper текущего (нового) потока; флаги 0 — обычный looper с колбэками,
    // через которые Choreographer доставляет vsync.
    let looper = unsafe { ALooper_prepare(0) };
    let looper = NonNull::new(looper).ok_or(VsyncSourceError::NoLooper)?;
    let looper_handle = LooperHandle::acquire(looper);
    // SAFETY: вызывается на потоке с looper (choreographer.h: «must be called on a thread with a looper»).
    let choreographer = unsafe { AChoreographer_getInstance() };
    let choreographer = NonNull::new(choreographer).ok_or(VsyncSourceError::NoLooper)?;
    let context = Box::new(CallbackContext {
        choreographer,
        slot: Arc::clone(slot),
        stop_requested: Arc::clone(stop_requested),
        on_failure,
        next_sequence: VsyncSlot::FIRST_SEQUENCE,
        failures: 0,
    });
    // Владение контекстом уходит в сырой указатель до конца потока (освобождается в run_vsync_thread).
    let context_pointer = NonNull::from(Box::leak(context));
    // SAFETY: контекст только что создан и жив; пост — с потока Choreographer.
    unsafe { (*context_pointer.as_ptr()).post_next(context_pointer.as_ptr().cast::<c_void>()) };
    Ok((context_pointer, looper_handle))
}
