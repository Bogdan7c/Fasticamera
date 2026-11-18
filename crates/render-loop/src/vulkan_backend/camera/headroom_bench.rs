//! Стенд «запас под видео» (V4, сессия 07) в кадре видоискателя: объекты GPU и их жизненный цикл.
//!
//! Владелец — [`super::CameraSession`] (сессия показа одной surface): только она знает серии
//! сабмитов и поэтому решает, когда объекты стенда можно уничтожить. Запись команд стенда
//! вызывает `CameraPipelines::record_new_frame` через [`AcquiredSourceReader`] — между ISP
//! видоискателя и возвратом кадра камере, пока кадр принят для compute.
//!
//! # Кадр
//! 1. `prepare_frame` (решение до записи команд): стенд включён и не остановлен отказом →
//!    поколение потока, параметры ISP с демозаиком `Quality` (Menon 2007) из ТЕХ ЖЕ входов, что у
//!    видоискателя, нагрузка размера полезной области (создаётся лениво; другой размер — старая
//!    выводится из обращения, создаётся новая);
//! 2. запись (`HeadroomRecorder`) — `VideoHeadroomWorkload::record` в `OffscreenFrameSink`;
//! 3. `finish_frame` по итогу сабмита: счётчики, `FrameSink::after_submit`, серия последнего
//!    сабмита нагрузки (нужна для освобождения).
//!
//! # Память и освобождение (без ожиданий GPU)
//! Нагрузка на 4K — около 10 буферов по 36 МиБ. Она создаётся лениво (первый новый кадр после
//! включения) и выводится из обращения выключением, сменой размера, отказом записи или
//! непринятым сабмитом. Выведенная живёт, пока не завершится последний принятый сабмит, который
//! её записал (`retire_completed` по завершённой серии, как `ViewfinderIsp` освобождает свои
//! объекты); если её не записал ни один принятый сабмит — уничтожается сразу.
//!
//! # Смена потока
//! Поколение потока ([`StreamGeneration`]) растёт, когда сменился паспорт потока (новый `Arc`) или
//! поток был неактивен. Шумодав сам начинает историю заново, увидев новое поколение
//! (`HistoryUse::Restarted`): кадры разных сессий камеры не смешиваются.
//! TODO(raw-frame-stream): лизинг пока не отдаёт эпоху потока; когда отдаст — брать поколение из неё.
//!
//! # Отказ
//! Отказ создания, параметров или записи считается и останавливает стенд до повторного включения
//! командой (выключить → включить): иначе стойкая причина пересоздавала бы ~360 МиБ на каждом кадре.
//! Видоискатель отказ стенда не затрагивает.

use std::sync::Arc;

use ash::vk;
use camera_contract::frame_passport::{RawFramePassport, StreamPassport};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::frame_sink::{FrameSink, OffscreenFrameSink, OffscreenFrameSinkDesc};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, ValidArea};
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_vulkan::quality::{
    HeadroomRecordOutcome, HistoryUse, QualityError, StreamGeneration, VideoFrameInput, VideoHeadroomConfig,
    VideoHeadroomWorkload,
};

use super::SubmissionSerial;
use super::headroom_stats::{HeadroomBenchStats, HeadroomLiveState};
use super::isp_params::{IspParamsCache, ParamsOutcome};
use super::pipelines::AcquiredSourceReader;
use crate::viewfinder::{HeadroomBench, ViewfinderExtent};
use isp_model::color::chromaticity::Chromaticity;

const LOG_TARGET: &str = "fasticamera::render::headroom";

/// Что стенд получает при создании сессии показа.
#[derive(Debug, Clone, Copy)]
pub struct HeadroomBenchParts {
    /// Настройки конвейера из конфига (`viewfinder.headroom_bench`); `frame_slots` заменяется.
    pub config: VideoHeadroomConfig,
    /// Слотов кадров у кольца цели: слоты стенда обязаны совпадать со слотами кольца (наборы
    /// дескрипторов и метки слота переписываются только после fence ЭТОГО слота).
    pub frame_slots: usize,
    /// Включён ли стенд сейчас (настройки бэкенда переживают пересоздание surface).
    pub mode: HeadroomBench,
}

/// Новый кадр, который стенд запишет (решено до записи команд).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadroomFrame {
    valid_area: ValidArea,
    stream: StreamGeneration,
    sequence: u64,
    presentation_time_ns: u64,
}

/// Чем кончился сабмит кадра (для итогов стенда).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchSubmit {
    /// Очередь приняла сабмит этой серии.
    Submitted(SubmissionSerial),
    /// Сабмит не принят: записанные команды GPU не исполнит.
    NotSubmitted,
}

/// Нагрузка вместе с приёмником: создаются и освобождаются вместе.
///
/// Порядок полей = порядок уничтожения: нагрузка (её наборы дескрипторов ссылаются на картинки
/// приёмника) раньше приёмника.
struct BenchObjects {
    workload: VideoHeadroomWorkload,
    sink: OffscreenFrameSink,
    /// Последний принятый очередью сабмит, где записана эта нагрузка (`None` — ни одного).
    last_submitted: Option<SubmissionSerial>,
}

impl BenchObjects {
    fn extent(&self) -> vk::Extent2D {
        self.workload.extent()
    }
}

/// Итог записи текущего кадра, ждущий итога сабмита.
#[derive(Debug)]
enum PendingRecord {
    /// Кадр записан (`history` — что сделал шумодав).
    Recorded { history: Option<HistoryUse> },
    /// Приёмник занят: ничего не записано.
    SinkBusy,
    /// Запись отказала (причина в тексте).
    Failed(String),
}

/// Поколение потока камеры глазами стенда (см. модуль). Обобщено по типу паспорта только ради
/// теста без камерных фикстур; в работе — `StreamPassport`.
struct StreamWatch<Passport = StreamPassport> {
    passport: Option<Arc<Passport>>,
    generation: u64,
    inactive_seen: bool,
}

impl<Passport> Default for StreamWatch<Passport> {
    fn default() -> Self {
        Self {
            passport: None,
            generation: 0,
            inactive_seen: false,
        }
    }
}

impl<Passport> StreamWatch<Passport> {
    /// Поколение потока кадра; `true` вторым — поток сменился (не первый кадр).
    fn observe(&mut self, stream: &Arc<Passport>) -> (StreamGeneration, bool) {
        let same_passport = self.passport.as_ref().is_some_and(|known| Arc::ptr_eq(known, stream));
        let changed = self.passport.is_some() && (!same_passport || self.inactive_seen);
        if changed {
            self.generation += 1;
        }
        if !same_passport {
            self.passport = Some(Arc::clone(stream));
        }
        self.inactive_seen = false;
        (StreamGeneration(self.generation), changed)
    }
}

/// Стенд V4 одной сессии показа (см. модуль).
pub struct HeadroomBenchRunner {
    mode: HeadroomBench,
    /// Стенд остановлен отказом до повторного включения командой.
    suspended: bool,
    config: VideoHeadroomConfig,
    active: Option<BenchObjects>,
    retiring: Vec<BenchObjects>,
    params: IspParamsCache,
    stream: StreamWatch,
    pending: Option<PendingRecord>,
    context: GpuContext,
}

impl HeadroomBenchRunner {
    /// Пустой стенд: объекты не создаются до первого нового кадра при включённом стенде.
    pub fn new(context: &GpuContext, parts: HeadroomBenchParts) -> Self {
        Self {
            mode: parts.mode,
            suspended: false,
            config: VideoHeadroomConfig {
                frame_slots: parts.frame_slots,
                ..parts.config
            },
            active: None,
            retiring: Vec::new(),
            params: IspParamsCache::for_demosaic(DemosaicMethod::Quality),
            stream: StreamWatch::default(),
            pending: None,
            context: context.clone(),
        }
    }

    /// Включение/выключение командой. Выключение выводит нагрузку из обращения (освободится после
    /// fence) и снимает остановку после отказа; включение тоже снимает её (повторная попытка).
    pub fn set_mode(&mut self, mode: HeadroomBench, stats: &mut HeadroomBenchStats) {
        self.mode = mode;
        self.suspended = false;
        if mode == HeadroomBench::Disabled {
            self.retire_active(stats);
            self.params = IspParamsCache::for_demosaic(DemosaicMethod::Quality);
        }
    }

    /// Поток камеры неактивен: следующий кадр — уже другой поток (новое поколение).
    pub fn note_stream_inactive(&mut self) {
        self.stream.inactive_seen = true;
        // Параметры стенда выведены из кадров прежнего потока: новому они чужие (решение 42).
        self.params.forget_stream();
    }

    /// Решает, пойдёт ли новый кадр через стенд (см. модуль, шаг 1). `None` — стенд выключен,
    /// остановлен, параметров нет или нагрузка не создалась (последние два — со счётчиком).
    pub fn prepare_frame(
        &mut self,
        passport: &RawFramePassport,
        white: Chromaticity,
        gamut: DisplayGamut,
        stats: &mut HeadroomBenchStats,
    ) -> Option<HeadroomFrame> {
        if self.mode == HeadroomBench::Disabled || self.suspended {
            return None;
        }
        let (stream, changed) = self.stream.observe(&passport.stream);
        if changed {
            stats.note_stream_change();
        }
        let decision = self.params.prepare(passport, white, gamut);
        if let ParamsOutcome::Unavailable(reason) = decision.outcome {
            tracing::debug!(target: LOG_TARGET, ?reason, "параметров качественного ISP нет: кадр мимо стенда");
            stats.note_no_params();
            return None;
        }
        let area = passport.stream.valid_area;
        let valid_area = ValidArea {
            left: area.left,
            top: area.top,
            width: area.size.width,
            height: area.size.height,
        };
        let extent = vk::Extent2D {
            width: valid_area.width,
            height: valid_area.height,
        };
        if let Err(error) = self.ensure_objects(extent, stats) {
            self.fail(
                format!("нагрузка {}x{} не создана: {error}", extent.width, extent.height),
                stats,
            );
            return None;
        }
        Some(HeadroomFrame {
            valid_area,
            stream,
            sequence: passport.sequence.0,
            // Метка сенсора (BOOTTIME) неотрицательна; отрицательная (сбой HAL) становится нулём —
            // это только метка кадра в приёмнике, на работу GPU она не влияет.
            presentation_time_ns: u64::try_from(passport.sensor_timestamp.as_nanos()).unwrap_or(0),
        })
    }

    /// Запись кадра `frame` в текущий командный буфер (через `CameraPipelines::record_new_frame`).
    pub fn recorder(&mut self, frame: HeadroomFrame) -> HeadroomRecorder<'_> {
        HeadroomRecorder { runner: self, frame }
    }

    /// Итог сабмита кадра (см. модуль, шаг 3).
    pub fn finish_frame(&mut self, frame_slot: usize, submit: BenchSubmit, stats: &mut HeadroomBenchStats) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        match (pending, submit) {
            (PendingRecord::Recorded { history }, BenchSubmit::Submitted(serial)) => {
                stats.note_frame_recorded(history);
                if let Some(active) = &mut self.active {
                    active.last_submitted = Some(serial);
                    if let Err(error) = active.sink.after_submit(frame_slot) {
                        tracing::warn!(target: LOG_TARGET, %error, "публикация кадра приёмника стенда не удалась");
                    }
                }
            }
            (PendingRecord::Recorded { .. }, BenchSubmit::NotSubmitted) => {
                // Приёмник и наборы дескрипторов помнят команды, которые GPU не исполнит: такую
                // нагрузку дальше использовать нельзя (правило `OffscreenFrameSink`). Это не отказ
                // стенда — следующий кадр создаст новую.
                tracing::debug!(target: LOG_TARGET, "сабмит с кадром стенда не принят: нагрузка будет создана заново");
                self.retire_active(stats);
            }
            (PendingRecord::SinkBusy, BenchSubmit::Submitted(_)) => stats.note_sink_busy(),
            (PendingRecord::SinkBusy, BenchSubmit::NotSubmitted) => {}
            (PendingRecord::Failed(reason), submit) => {
                // Запись могла оставить в буфере подготовку цели приёмника: если сабмит принят, эти
                // команды исполнятся — нагрузка освобождается только после него.
                if let (Some(active), BenchSubmit::Submitted(serial)) = (&mut self.active, submit) {
                    active.last_submitted = Some(serial);
                }
                self.fail(reason, stats);
            }
        }
    }

    /// Слот выдан кольцом (его fence пройден): время проходов слота и освобождение его наборов
    /// дескрипторов (`VideoHeadroomWorkload::collect` обязателен между записями одного слота).
    pub fn collect_slot(&mut self, frame_slot: usize, stats: &mut HeadroomBenchStats) {
        let Some(active) = &mut self.active else {
            return;
        };
        match active.workload.collect(frame_slot) {
            Ok(collected) => stats.note_timings(&collected),
            Err(error) => {
                stats.note_timing_failure();
                tracing::warn!(target: LOG_TARGET, %error, frame_slot, "итоги слота стенда не собраны");
            }
        }
    }

    /// GPU закончил сабмиты до `completed`: освобождаются выведенные нагрузки, которые больше
    /// никто не читает.
    pub fn retire_completed(&mut self, completed: SubmissionSerial, stats: &mut HeadroomBenchStats) {
        let before = self.retiring.len();
        self.retiring
            .retain(|objects| objects.last_submitted.is_some_and(|serial| serial > completed));
        stats.note_workloads_released(before - self.retiring.len());
    }

    /// GPU подтверждённо простаивает: освобождается всё (отсоединение surface).
    pub fn release_all_gpu_idle(&mut self, stats: &mut HeadroomBenchStats) {
        let released = self.retiring.len() + usize::from(self.active.is_some());
        self.retiring.clear();
        self.active = None;
        self.pending = None;
        stats.note_workloads_released(released);
    }

    /// Живое состояние для диагностики.
    pub fn live_state(&self) -> HeadroomLiveState {
        HeadroomLiveState {
            workload_extent: self.active.as_ref().map(|active| {
                let extent = active.extent();
                ViewfinderExtent {
                    width: extent.width,
                    height: extent.height,
                }
            }),
            workloads_awaiting_release: self.retiring.len(),
        }
    }

    /// Нагрузка размера `extent` есть (создаётся, если нет; другой размер — старая выводится).
    fn ensure_objects(&mut self, extent: vk::Extent2D, stats: &mut HeadroomBenchStats) -> Result<(), QualityError> {
        if self.active.as_ref().is_some_and(|active| active.extent() != extent) {
            self.retire_active(stats);
        }
        if self.active.is_some() {
            return Ok(());
        }
        let workload = VideoHeadroomWorkload::new(&self.context, extent, &self.config)?;
        let sink = OffscreenFrameSink::new(
            &self.context,
            &OffscreenFrameSinkDesc {
                extent,
                format_policy: self.config.output_format.storage_policy(),
                frame_slots: self.config.frame_slots,
                // Читателя на экране нет: кадр стенда «потребляет» compute (как будущий вход кодера
                // на той же очереди). Стадия влияет только на барьер передачи после записи.
                reader: ConsumerStage::Compute,
            },
        )?;
        stats.note_workload_created();
        self.active = Some(BenchObjects {
            workload,
            sink,
            last_submitted: None,
        });
        Ok(())
    }

    /// Выводит живую нагрузку из обращения: не записанная ни одним принятым сабмитом
    /// уничтожается сразу, иначе ждёт завершения своего последнего сабмита.
    fn retire_active(&mut self, stats: &mut HeadroomBenchStats) {
        let Some(active) = self.active.take() else {
            return;
        };
        if active.last_submitted.is_some() {
            self.retiring.push(active);
        } else {
            drop(active);
            stats.note_workloads_released(1);
        }
    }

    /// Отказ: счётчик, нагрузка выводится, стенд стоит до повторного включения.
    fn fail(&mut self, reason: String, stats: &mut HeadroomBenchStats) {
        tracing::warn!(target: LOG_TARGET, %reason, "стенд V4 отказал и остановлен до повторного включения");
        stats.note_failure(reason);
        self.retire_active(stats);
        self.suspended = true;
    }
}

/// Запись кадра стенда над кадром камеры, уже принятым для compute.
pub struct HeadroomRecorder<'a> {
    runner: &'a mut HeadroomBenchRunner,
    frame: HeadroomFrame,
}

impl AcquiredSourceReader for HeadroomRecorder<'_> {
    fn record_while_acquired(&mut self, command_buffer: vk::CommandBuffer, frame_slot: usize, source: &MosaicSource) {
        let runner = &mut *self.runner;
        let outcome = match (&mut runner.active, runner.params.current()) {
            (Some(active), Some(params)) => active
                .workload
                .record(
                    command_buffer,
                    frame_slot,
                    &VideoFrameInput {
                        source,
                        params,
                        valid_area: self.frame.valid_area,
                        stream: self.frame.stream,
                        sequence: self.frame.sequence,
                        presentation_time_ns: self.frame.presentation_time_ns,
                    },
                    &mut active.sink,
                )
                .map_err(|error| error.to_string()),
            // Недостижимо: `prepare_frame` пропускает кадр только с нагрузкой и параметрами.
            _ => Err("нет нагрузки или параметров стенда".to_owned()),
        };
        runner.pending = Some(match outcome {
            Ok(HeadroomRecordOutcome::Recorded { history }) => PendingRecord::Recorded { history },
            Ok(HeadroomRecordOutcome::SkippedSinkBusy) => PendingRecord::SinkBusy,
            Err(reason) => PendingRecord::Failed(reason),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Паспорт-заглушка: важна только идентичность `Arc`.
    fn passport() -> Arc<u32> {
        Arc::new(7)
    }

    #[test]
    fn stream_generation_changes_with_a_new_passport_or_after_inactivity() {
        let mut watch = StreamWatch::<u32>::default();
        let first = passport();
        assert_eq!(watch.observe(&first), (StreamGeneration(0), false), "первый поток");
        assert_eq!(watch.observe(&first), (StreamGeneration(0), false), "тот же поток");
        let second = passport();
        assert_eq!(watch.observe(&second), (StreamGeneration(1), true), "новый паспорт");
        watch.inactive_seen = true;
        assert_eq!(
            watch.observe(&second),
            (StreamGeneration(2), true),
            "тот же Arc после остановки потока — новая сессия"
        );
        assert_eq!(watch.observe(&second), (StreamGeneration(2), false));
    }
}
