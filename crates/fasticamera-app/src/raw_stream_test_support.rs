//! Подставной «живой поток кадров» для функциональных тестов сценариев `raw-stream*`. Только для тестов.
//!
//! [`StreamPump`] — поток, который пока камера fake стримит, делает всё то, что на телефоне делают
//! бэкенд камеры, лизинг и рендер: растит счётчики читателя и потока, ведёт счётчики показа и
//! историю показов в статусе рендера. Неисправности (копия через CPU, утечка лизинга, перепутанные
//! номера кадров…) включаются полями [`PumpFaults`], чтобы каждое правило вердикта можно было
//! проверить на настоящем пути «сценарий -> маршрутизатор -> строка результата».

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use render_loop::backend::PastPresentation;
use render_loop::camera_diagnostics::{CameraStreamDiagnostics, PresentedCameraFrame, PresentedSample};
use shell_contract::clock::MonotonicNanos;

use crate::camera_test_support::{FakeCameraRig, FrameFeeder};
use crate::clock_offset::{ClockOffsetSource, SystemClocks};
use crate::gpu_test_support::FakeRenderStatus;

/// Интервал между кадрами сенсора в тестах, нс (60 к/с).
pub const SENSOR_INTERVAL: Duration = Duration::from_nanos(16_666_667);
/// Задержка «сенсор -> экран», которую имитирует рендер, мс.
pub const SIMULATED_LATENCY_MS: i64 = 40;
/// Сколько записей истории показов держит подставной рендер (как реальный).
const HISTORY_CAPACITY: usize = 64;
const RECENT_CAPACITY: usize = 256;

/// Что делать с номерами кадров Camera2 у показанных кадров.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FrameNumbers {
    /// Номер растёт на 1 с каждым кадром.
    Correct,
    /// Соседние кадры меняются номерами: паспорт чужого кадра.
    SwappedInPairs,
    /// Номеров нет (пара-результат не пришла).
    Absent,
}

/// Включаемые неисправности. По умолчанию — здоровый поток.
#[derive(Clone, Copy)]
pub struct PumpFaults {
    pub frame_numbers: FrameNumbers,
    /// Каждый кадр копируется через CPU (нарушение инварианта «0 копий»).
    pub cpu_copy_per_frame: bool,
    /// Лизинг каждого кадра выброшен без возврата.
    pub leak_leases: bool,
    /// Читатель сообщает «все буферы на руках».
    pub pool_exhausted: bool,
    /// Метаданные не приходят: кадры уходят по таймауту без пары.
    pub unpaired: bool,
    /// Экспорт release-fence в рендере не удаётся.
    pub release_export_failures: bool,
    /// Рендер не показывает кадры вообще.
    pub render_shows_nothing: bool,
    /// Платформа отвергает пробу CPU-доступа (при взведённой пробе).
    pub probe_refused_by_platform: bool,
    /// Проба взведена, но счётчик копий «мёртвый» (копия не засчитывается).
    pub probe_counter_dead: bool,
    /// Пик кадров на руках, который сообщает поток.
    pub max_frames_outstanding: u64,
}

impl Default for PumpFaults {
    fn default() -> Self {
        Self {
            frame_numbers: FrameNumbers::Correct,
            cpu_copy_per_frame: false,
            leak_leases: false,
            pool_exhausted: false,
            unpaired: false,
            release_export_failures: false,
            render_shows_nothing: false,
            probe_refused_by_platform: false,
            probe_counter_dead: false,
            max_frames_outstanding: 3,
        }
    }
}

/// Поток-имитатор. Живёт, пока жив объект; `Drop` останавливает поток.
pub struct StreamPump {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    // Подаёт результаты в fake-движок и считает кадры читателя.
    _feeder: FrameFeeder,
}

impl StreamPump {
    pub fn start(rig: &FakeCameraRig, render: Arc<FakeRenderStatus>, faults: PumpFaults) -> Self {
        let feeder = FrameFeeder::start(rig, SENSOR_INTERVAL);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let control = Arc::clone(&rig.control);
        let counters = Arc::clone(&rig.stream_counters);
        let diagnostics = Arc::clone(&rig.diagnostics);
        let probe = rig.cpu_probe.clone();
        let join = thread::spawn(move || {
            let mut frame_index: u64 = 0;
            // Метки сенсора синтетические: точка отсчёта — «сейчас» по BOOTTIME.
            let offset = SystemClocks.sample_offset();
            let base_boottime = offset
                .as_ref()
                .map(|offset| offset.monotonic_to_boottime(MonotonicNanos::from_nanos(0)).as_nanos())
                .unwrap_or(0);
            while !thread_stop.load(Ordering::SeqCst) {
                if control.is_repeating() {
                    frame_index += 1;
                    let probe_armed = probe.is_armed();
                    counters.update(|stream| {
                        stream.frames_offered += 1;
                        stream.frames_published += 1;
                        stream.viewfinder_delivered += 1;
                        stream.frames_returned += 1;
                        stream.released_after_fence += 1;
                        stream.max_frames_outstanding = faults.max_frames_outstanding;
                        stream.frames_outstanding = 2;
                        if faults.unpaired {
                            stream.frames_missing_meta_timeout += 1;
                        } else {
                            stream.frames_paired += 1;
                        }
                        if faults.cpu_copy_per_frame {
                            stream.cpu_copies += 1;
                        }
                        if faults.leak_leases {
                            stream.leaked_leases += 1;
                        }
                        if faults.pool_exhausted {
                            stream.dropped_pool_exhausted += 1;
                        }
                        if probe_armed && !faults.probe_refused_by_platform && !faults.probe_counter_dead {
                            stream.cpu_copies += 1;
                        }
                    });
                    if probe_armed {
                        if faults.probe_refused_by_platform {
                            diagnostics.reader().record_cpu_probe_failure();
                        } else {
                            diagnostics.reader().record_cpu_probe_copy();
                        }
                    }
                    if faults.pool_exhausted {
                        diagnostics.reader().record_pool_exhausted();
                    }
                    if !faults.render_shows_nothing {
                        show_frame(&render, frame_index, base_boottime, offset.is_ok(), faults);
                    }
                }
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            stop,
            join: Some(join),
            _feeder: feeder,
        }
    }
}

/// Рендер показал кадр `frame_index`: счётчики, последний кадр, история показов.
fn show_frame(render: &FakeRenderStatus, frame_index: u64, base_boottime: i64, clocks_ok: bool, faults: PumpFaults) {
    let interval = i64::try_from(SENSOR_INTERVAL.as_nanos()).unwrap_or(i64::MAX);
    let index = i64::try_from(frame_index).unwrap_or(i64::MAX);
    let sensor_nanos = base_boottime.saturating_add(index.saturating_mul(interval));
    // Момент показа в часах MONOTONIC: от метки сенсора + задержка, через то же смещение, что читает сценарий.
    let present_monotonic = if clocks_ok {
        let offset = SystemClocks.sample_offset().ok();
        offset.map_or(0, |offset| {
            let zero = offset.monotonic_to_boottime(MonotonicNanos::from_nanos(0)).as_nanos();
            sensor_nanos - zero + SIMULATED_LATENCY_MS * 1_000_000
        })
    } else {
        0
    };
    let number = i64::try_from(frame_index).unwrap_or(i64::MAX) + 100;
    let frame_number = match faults.frame_numbers {
        FrameNumbers::Correct => Some(number),
        FrameNumbers::SwappedInPairs => Some(if frame_index.is_multiple_of(2) {
            number - 1
        } else {
            number + 1
        }),
        FrameNumbers::Absent => None,
    };
    let present_id = u32::try_from(frame_index).unwrap_or(u32::MAX);
    render.update(|status| {
        let camera = status
            .backend
            .camera
            .get_or_insert_with(CameraStreamDiagnostics::default);
        camera.camera_frames_presented += 1;
        camera.camera_frames_imported += 1;
        camera.import_cache_hits += 1;
        if faults.release_export_failures {
            camera.release_export_failures += 1;
        } else {
            camera.release_fences_exported += 1;
        }
        camera.last_presented_frame = Some(PresentedCameraFrame {
            sequence: frame_index,
            sensor_timestamp_nanos: sensor_nanos,
            frame_number,
            present_id,
        });
        camera.present_history.push(PresentedSample {
            present_id,
            sensor_timestamp_nanos: sensor_nanos,
        });
        if camera.present_history.len() > HISTORY_CAPACITY {
            camera.present_history.remove(0);
        }
        status.recent_presentations.push(PastPresentation {
            present_id,
            frame_index,
            actual_present: MonotonicNanos::from_nanos(present_monotonic),
        });
        if status.recent_presentations.len() > RECENT_CAPACITY {
            status.recent_presentations.remove(0);
        }
    });
}

impl Drop for StreamPump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
