//! Safety-модуль: поток одного датчика на своём `ALooper` (NDK `android/sensor.h`, `looper.h`).
//!
//! Поток готовит `ALooper`, создаёт очередь событий датчика, включает его с нужным периодом
//! и забирает пачки показаний, пока его не остановят. JNI здесь нет — датчики идут мимо Java
//! (задел V3 README: «без JNI на горячем пути»).
//!
//! Остановка: [`SensorThread`] в `Drop` ставит флаг, будит looper (`ALooper_wake`) и ждёт
//! поток. Чтобы `ALooper_wake` никогда не попал в уже освобождённый looper, ручка держит
//! на него собственную ссылку (`ALooper_acquire`) и отпускает её только после `join`.

use std::ffi::{CString, c_int};
use std::ptr::{NonNull, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ndk_sys::{
    ALOOPER_PREPARE_ALLOW_NON_CALLBACKS, ALooper, ALooper_acquire, ALooper_pollOnce, ALooper_prepare, ALooper_release,
    ALooper_wake, ASensorEvent, ASensorEventQueue, ASensorEventQueue_disableSensor, ASensorEventQueue_getEvents,
    ASensorEventQueue_registerSensor, ASensorManager, ASensorManager_createEventQueue,
    ASensorManager_destroyEventQueue, ASensorManager_getDefaultSensor, ASensorManager_getInstanceForPackage,
};
use shell_contract::motion::SensorSourceError;

/// Идентификатор нашей очереди в `ALooper_pollOnce` (любое неотрицательное число).
const SENSOR_LOOPER_IDENT: c_int = 1;
/// Сколько показаний забирать за один вызов `getEvents`.
const EVENTS_PER_BATCH: usize = 32;
/// Страховочный таймаут ожидания looper: флаг остановки проверяется хотя бы так часто,
/// даже если `ALooper_wake` по какой-то причине не дошёл.
const POLL_TIMEOUT: Duration = Duration::from_millis(250);
/// `maxBatchReportLatencyUs = 0`: потоковая доставка без пакетирования в FIFO датчика.
const STREAMING_BATCH_LATENCY_US: i64 = 0;

/// Что запускать.
#[derive(Debug, Clone)]
pub struct SensorThreadSpec {
    /// Имя датчика для логов и ошибок.
    pub sensor_name: &'static str,
    /// `ASENSOR_TYPE_*`.
    pub sensor_type: c_int,
    pub sampling_period: Duration,
    /// Имя пакета приложения для `ASensorManager_getInstanceForPackage`.
    pub package_name: CString,
    /// Имя потока (видно в systrace и в отчётах паник).
    pub thread_name: String,
}

/// Указатель на looper, который можно передать в другой поток для `ALooper_wake`.
struct LooperHandle(NonNull<ALooper>);

// SAFETY: ALooper — объект со счётчиком ссылок; ALooper_wake и ALooper_release документированы
// как вызываемые с любого потока. Ручка держит собственную ссылку (acquire) до своего Drop.
unsafe impl Send for LooperHandle {}

impl Drop for LooperHandle {
    fn drop(&mut self) {
        // SAFETY: ссылка взята ALooper_acquire в потоке датчика и отпускается один раз.
        unsafe { ALooper_release(self.0.as_ptr()) };
    }
}

/// Работающий поток датчика. Уничтожение останавливает датчик и поток.
pub struct SensorThread {
    stop_requested: Arc<AtomicBool>,
    looper: Option<LooperHandle>,
    worker: Option<JoinHandle<()>>,
    sensor_name: &'static str,
}

impl Drop for SensorThread {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Release);
        if let Some(looper) = &self.looper {
            // SAFETY: ручка держит ссылку на looper, объект жив.
            unsafe { ALooper_wake(looper.0.as_ptr()) };
        }
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            tracing::error!(target: "fasticamera::shell", sensor = self.sensor_name, "поток датчика завершился паникой");
        }
        // Ссылку на looper отпускаем только после join (порядок полей не важен: явный take).
        drop(self.looper.take());
    }
}

/// Запускает поток датчика; `on_event` вызывается в нём для каждого показания.
/// Возвращается только после того, как датчик включён или запуск провалился.
pub fn start_sensor_thread(
    spec: SensorThreadSpec,
    on_event: impl FnMut(&ASensorEvent) + Send + 'static,
) -> Result<SensorThread, SensorSourceError> {
    let stop_requested = Arc::new(AtomicBool::new(false));
    let (startup_sender, startup_receiver) = sync_channel(1);
    let worker_stop = Arc::clone(&stop_requested);
    let sensor_name = spec.sensor_name;
    let worker = thread::Builder::new()
        .name(spec.thread_name.clone())
        .spawn(move || {
            run_sensor_thread(&spec, &worker_stop, on_event, |startup| {
                // Если запускающий уже не ждёт (невозможно: он ждёт recv), отчёт просто не нужен.
                let _ = startup_sender.send(startup);
            });
        })
        .map_err(|error| SensorSourceError::PlatformFailure {
            sensor: sensor_name,
            reason: format!("поток не создан: {error}"),
        })?;
    let startup = startup_receiver.recv().map_err(|_| SensorSourceError::PlatformFailure {
        sensor: sensor_name,
        reason: "поток датчика завершился до отчёта о запуске".to_owned(),
    });
    match startup.and_then(|result| result) {
        Ok(looper) => Ok(SensorThread {
            stop_requested,
            looper: Some(looper),
            worker: Some(worker),
            sensor_name,
        }),
        Err(error) => {
            // Поток уже вышел (запуск не удался) — дожидаемся, чтобы не оставлять висящих.
            let _ = worker.join();
            Err(error)
        }
    }
}

/// Открытая очередь событий датчика; закрывается в `Drop` (выключить датчик, уничтожить очередь).
struct OpenSensorQueue {
    manager: NonNull<ASensorManager>,
    queue: NonNull<ASensorEventQueue>,
    sensor: NonNull<ndk_sys::ASensor>,
}

impl Drop for OpenSensorQueue {
    fn drop(&mut self) {
        // SAFETY: очередь и датчик получены от этого manager в этом же потоке и ещё не уничтожены.
        unsafe {
            ASensorEventQueue_disableSensor(self.queue.as_ptr(), self.sensor.as_ptr());
            ASensorManager_destroyEventQueue(self.manager.as_ptr(), self.queue.as_ptr());
        }
    }
}

type StartupResult = Result<LooperHandle, SensorSourceError>;

fn run_sensor_thread(
    spec: &SensorThreadSpec,
    stop_requested: &AtomicBool,
    mut on_event: impl FnMut(&ASensorEvent),
    report_startup: impl FnOnce(StartupResult),
) {
    let opened = open_sensor_queue(spec);
    let (open_queue, looper) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            report_startup(Err(error));
            return;
        }
    };
    report_startup(Ok(looper));
    let mut batch: [ASensorEvent; EVENTS_PER_BATCH] = std::array::from_fn(|_| zeroed_event());
    let timeout_ms = c_int::try_from(POLL_TIMEOUT.as_millis()).unwrap_or(c_int::MAX);
    while !stop_requested.load(Ordering::Acquire) {
        // SAFETY: looper этого потока подготовлен в open_sensor_queue; выходные указатели не нужны.
        let ident = unsafe { ALooper_pollOnce(timeout_ms, null_mut(), null_mut(), null_mut()) };
        if ident != SENSOR_LOOPER_IDENT {
            // Таймаут, пробуждение для остановки или ошибка looper: проверяем флаг и ждём снова.
            continue;
        }
        loop {
            // SAFETY: буфер из EVENTS_PER_BATCH элементов; getEvents пишет не больше count.
            let received =
                unsafe { ASensorEventQueue_getEvents(open_queue.queue.as_ptr(), batch.as_mut_ptr(), batch.len()) };
            let Ok(count) = usize::try_from(received) else {
                break; // Отрицательное значение — ошибка очереди; ждём следующего пробуждения.
            };
            if count == 0 {
                break;
            }
            batch[..count].iter().for_each(&mut on_event);
        }
    }
    drop(open_queue);
}

fn open_sensor_queue(spec: &SensorThreadSpec) -> Result<(OpenSensorQueue, LooperHandle), SensorSourceError> {
    let failure = |reason: String| SensorSourceError::PlatformFailure {
        sensor: spec.sensor_name,
        reason,
    };
    // SAFETY: готовим looper текущего (нового) потока; ALLOW_NON_CALLBACKS — мы опрашиваем
    // очередь по идентификатору, без колбэков.
    let looper = unsafe { ALooper_prepare(ALOOPER_PREPARE_ALLOW_NON_CALLBACKS as c_int) };
    let looper = NonNull::new(looper).ok_or_else(|| failure("ALooper_prepare вернул null".to_owned()))?;
    // SAFETY: looper действителен; лишняя ссылка — для ручки остановки (см. документацию модуля).
    unsafe { ALooper_acquire(looper.as_ptr()) };
    let looper_handle = LooperHandle(looper);
    // SAFETY: package_name — корректная C-строка, живёт весь вызов.
    let manager = unsafe { ASensorManager_getInstanceForPackage(spec.package_name.as_ptr()) };
    let manager = NonNull::new(manager).ok_or_else(|| failure("ASensorManager недоступен".to_owned()))?;
    // SAFETY: manager действителен; тип датчика — константа ASENSOR_TYPE_*.
    let sensor = unsafe { ASensorManager_getDefaultSensor(manager.as_ptr(), spec.sensor_type) };
    let sensor = NonNull::new(sensor.cast_mut()).ok_or(SensorSourceError::SensorAbsent {
        sensor: spec.sensor_name,
    })?;
    // SAFETY: manager и looper действительны; колбэка нет, данные не передаём.
    let queue = unsafe {
        ASensorManager_createEventQueue(manager.as_ptr(), looper.as_ptr(), SENSOR_LOOPER_IDENT, None, null_mut())
    };
    let queue = NonNull::new(queue).ok_or_else(|| failure("ASensorManager_createEventQueue вернул null".to_owned()))?;
    let open_queue = OpenSensorQueue { manager, queue, sensor };
    let period_us = i32::try_from(spec.sampling_period.as_micros()).unwrap_or(i32::MAX);
    // SAFETY: очередь и датчик действительны и принадлежат этому manager.
    let status = unsafe {
        ASensorEventQueue_registerSensor(queue.as_ptr(), sensor.as_ptr(), period_us, STREAMING_BATCH_LATENCY_US)
    };
    if status != 0 {
        // open_queue закроет очередь в Drop.
        return Err(failure(format!(
            "ASensorEventQueue_registerSensor({period_us} мкс) вернул {status}"
        )));
    }
    Ok((open_queue, looper_handle))
}

fn zeroed_event() -> ASensorEvent {
    // SAFETY: ASensorEvent — POD-структура C (числа и объединения чисел); нули — допустимое значение.
    unsafe { std::mem::zeroed() }
}

/// Первые три компоненты показания (`data[0..3]`: вектор датчика по осям x, y, z).
pub fn event_vector(event: &ASensorEvent) -> [f32; 3] {
    // SAFETY: для гироскопа и акселерометра объединение хранит вектор из float в data[0..3]
    // (NDK sensor.h: union { float data[16]; ASensorVector vector; ... }).
    let data = unsafe { event.__bindgen_anon_1.__bindgen_anon_1.data };
    [data[0], data[1], data[2]]
}
