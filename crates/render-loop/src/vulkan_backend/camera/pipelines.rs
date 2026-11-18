//! Объекты GPU показа камеры и запись команд кадра видоискателя.
//!
//! [`CameraPipelines`] владеет всем, что записывается в кадры цели показа: отладочной мозаикой (05),
//! ISP видоискателя, компоновщиком и метками времени кадра. Поэтому, как и прежде пайплайн мозаики,
//! уничтожается только после того, как цель освободила GPU (`DisplayChain::release` в `session.rs`
//! бэкенда). Решений о лизингах здесь нет: что рисовать и что возвращать камере, решает
//! `CameraSession`, сюда приходят готовые источники и план владения.
//!
//! # Порядок команд нового кадра (один сабмит)
//! 1. сброс меток кадра, точка «начало»;
//! 2. возврат камере прежнего кадра отладочной мозаики, если он ещё на руках (стадия Fragment);
//! 3. приём кадра у камеры для compute (`QUEUE_FAMILY_FOREIGN → наша очередь`);
//! 4. слитый проход ISP → изображение видоискателя (читатель — компоновщик, Fragment);
//! 5. дополнительный читатель того же кадра, если есть ([`AcquiredSourceReader`]: стенд V4) — пока
//!    кадр ещё принят для compute;
//! 6. возврат кадра камере (`наша очередь → FOREIGN`) — RAW больше не читается никогда;
//! 7. точка «ISP готов»; компоновка изображения на экран; точка «компоновка готова».
//!
//! Шаги 3 и 6 записываются ВСЕГДА вместе: даже если ISP отказал, кадр принят и тем же сабмитом
//! возвращён, а сабмит ждёт acquire-fence камеры — поэтому возврат лизинга с fence этого сабмита
//! корректен при любом исходе записи ISP.

use ash::vk;
use gpu_vulkan::compositor::{BackgroundColor, CompositeRequest, ImageCompositor};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampCollect, TimestampStage};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, RawMosaicPreview};
use gpu_vulkan::swapchain::{DisplayFrame, DisplayTargetInfo};
use isp_model::params::IspParams;
use isp_vulkan::viewfinder::{
    IspStageSet, ViewfinderError, ViewfinderFrame, ViewfinderGeometry, ViewfinderIsp, ViewfinderIspConfig,
    ViewfinderSlotReport,
};

use super::placement::{PlaneSize, logical_target_size, viewfinder_destination};
use super::viewfinder_stats::{FRAME_POINT_COMPOSE_DONE, FRAME_POINT_ISP_DONE, FRAME_POINT_START, FRAME_POINTS};
use crate::config::ViewfinderPlacement;

const LOG_TARGET: &str = "fasticamera::render::camera";

/// Не удалось создать объекты GPU показа камеры.
#[derive(Debug, thiserror::Error)]
pub enum CameraPipelinesError {
    /// Пайплайн мозаики, компоновщик или метки времени.
    #[error("GPU: {0}")]
    Gpu(#[from] GpuError),
    /// ISP видоискателя.
    #[error("ISP видоискателя: {0}")]
    Isp(#[from] ViewfinderError),
}

/// Ещё один читатель НОВОГО кадра камеры в том же сабмите (стенд V4): записывается после ISP
/// видоискателя, пока кадр принят для compute, и до его возврата камере. Барьер возврата
/// (`ConsumerStage::Compute`) покрывает чтения обоих. Ошибки читатель учитывает сам: запись кадра
/// видоискателя от него не зависит.
pub trait AcquiredSourceReader {
    /// Записывает чтение `source` в `command_buffer` слота `frame_slot`.
    fn record_while_acquired(&mut self, command_buffer: vk::CommandBuffer, frame_slot: usize, source: &MosaicSource);
}

/// Выводить ли обработанный кадр на экран.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IspPresentation {
    /// Скомпоновать изображение видоискателя на экран.
    Show,
    /// Кадр обработан (нужна статистика AWB), но на экран не выводится: цвет ещё не известен (холодный
    /// старт AWB, решение 65). Вместо изображения — чёрный кадр.
    HideUntilColorKnown,
}

/// Новый кадр для ISP.
#[derive(Debug, Clone, Copy)]
pub struct NewIspFrame<'a> {
    /// Кадр камеры (ещё в собственности камеры: приём записывается здесь).
    pub source: &'a MosaicSource,
    /// Параметры ISP.
    pub params: &'a IspParams,
    /// Полезная область, кадрирование, ориентация сенсора.
    pub geometry: ViewfinderGeometry,
    /// Включённые стадии.
    pub stages: IspStageSet,
    /// Номер кадра (`FrameSequence`) для подложки стекла.
    pub frame_number: u64,
    /// Показывать ли результат на экране.
    pub presentation: IspPresentation,
}

/// Чем кончилась запись кадра видоискателя.
#[derive(Debug)]
pub struct IspFrameRecording {
    /// Прошёл ли кадр через ISP (`Err` — причина отказа; кадр всё равно возвращён камере).
    pub isp: Result<(), ViewfinderError>,
    /// Что попало на экран.
    pub screen: ScreenContent,
    /// Сколько раз не удалось записать метку времени.
    pub timing_failures: u64,
}

/// Что попало на экран.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenContent {
    /// Изображение видоискателя.
    Viewfinder,
    /// Чёрный: компоновать нечего или компоновщик отказал (причина в тексте).
    Black { compose_failure: Option<String> },
}

/// Итоги слота, собранные после его fence.
#[derive(Debug)]
pub struct SlotCollection {
    /// Итоги ISP (время прохода, статистика AWB).
    pub isp: Result<ViewfinderSlotReport, ViewfinderError>,
    /// Метки кадра видоискателя.
    pub frame: Result<TimestampCollect, GpuError>,
    /// Освобождение набора дескрипторов компоновщика слота.
    pub compositor: Result<(), GpuError>,
}

/// Объекты GPU показа камеры (см. модуль).
pub struct CameraPipelines {
    /// Отладочная мозаика 05 и чёрный кадр.
    pub preview: RawMosaicPreview,
    isp: ViewfinderIsp,
    compositor: ImageCompositor,
    frame_timestamps: GpuTimestamps,
    context: GpuContext,
}

impl CameraPipelines {
    /// Создаёт объекты под формат цели. Слотов ISP — столько же, сколько кадров в полёте у кольца
    /// (`frame_slots` из конфига заменяется: слот ISP обязан совпадать со слотом кольца кадров).
    pub fn new(
        context: &GpuContext,
        target: &DisplayTargetInfo,
        isp_config: &ViewfinderIspConfig,
    ) -> Result<Self, CameraPipelinesError> {
        let (target_format, frames_in_flight) = (target.format, target.frames_in_flight);
        let isp_config = ViewfinderIspConfig {
            frame_slots: frames_in_flight,
            ..*isp_config
        };
        Ok(Self {
            preview: RawMosaicPreview::new(context, target_format, frames_in_flight)?,
            isp: ViewfinderIsp::new(context, &isp_config)?,
            compositor: ImageCompositor::new(context, target_format, frames_in_flight)?,
            frame_timestamps: GpuTimestamps::new(context, frames_in_flight, &FRAME_POINTS)?,
            context: context.clone(),
        })
    }

    /// Пересоздаёт то, что зависит от формата цели (мозаика и компоновщик). Вызывающий уже дождался
    /// сабмитов цели. ISP от формата цели не зависит и живёт дальше (его изображение тоже).
    pub fn rebuild_for_format(&mut self, target: &DisplayTargetInfo) -> Result<(), GpuError> {
        self.preview = RawMosaicPreview::new(&self.context, target.format, target.frames_in_flight)?;
        self.compositor = ImageCompositor::new(&self.context, target.format, target.frames_in_flight)?;
        Ok(())
    }

    /// ISP видоискателя (только чтение: изображение, readback в тестах).
    pub fn isp(&self) -> &ViewfinderIsp {
        &self.isp
    }

    /// Итоги слота: вызывать для КАЖДОГО слота, выданного кольцом (его fence пройден). Заодно ISP
    /// уничтожает выведенные из обращения объекты. Никогда не ждёт GPU.
    pub fn collect_slot(&mut self, slot: usize) -> SlotCollection {
        SlotCollection {
            isp: self.isp.collect(slot),
            frame: self.frame_timestamps.try_collect(slot),
            // Набор дескрипторов компоновщика этого слота снова можно переписывать (новое изображение
            // видоискателя после смены размера): fence слота пройден.
            compositor: self.compositor.release_slot_after_fence(slot),
        }
    }

    /// Записывает новый кадр (см. порядок в описании модуля). Никогда не оставляет командный буфер
    /// «на полпути»: отказ ISP или компоновщика заменяется чёрным, приём/возврат кадра записаны всегда.
    /// `extra_reader` (`None` — читателя нет) читает тот же кадр после ISP; с ним отрезок
    /// `frame_isp_span` диагностики включает и его работу.
    pub fn record_new_frame(
        &mut self,
        frame: &DisplayFrame,
        input: &NewIspFrame<'_>,
        extra_reader: Option<&mut dyn AcquiredSourceReader>,
        release_displayed: Option<MosaicSource>,
        placement: ViewfinderPlacement,
    ) -> IspFrameRecording {
        let command_buffer = frame.command_buffer();
        let slot = frame.ring_slot();
        let mut timing_failures = self.start_frame_timing(frame);
        if let Some(displayed) = release_displayed {
            // Кадр отладочной мозаики читался фрагментным шейдером.
            displayed.record_release_for(&self.context, command_buffer, ConsumerStage::Fragment);
        }
        input
            .source
            .record_acquire_for(&self.context, command_buffer, ConsumerStage::Compute);
        let isp = self
            .isp
            .record(
                command_buffer,
                &ViewfinderFrame {
                    source: input.source,
                    params: input.params,
                    geometry: input.geometry,
                    display_color_space: frame.display_color_space(),
                    stages: input.stages,
                    frame: gpu_vulkan::backdrop::BackdropFrameNumber(input.frame_number),
                    frame_slot: slot,
                    reader: ConsumerStage::Fragment,
                },
            )
            .map(|_recorded| ());
        if let Some(reader) = extra_reader {
            reader.record_while_acquired(command_buffer, slot, input.source);
        }
        // RAW больше не нужен: возврат в этом же сабмите, до экспорта его release-fence.
        input
            .source
            .record_release_for(&self.context, command_buffer, ConsumerStage::Compute);
        timing_failures += self.point(frame, FRAME_POINT_ISP_DONE, TimestampStage::ComputeShaderDone);
        let screen = match (&isp, input.presentation) {
            (Ok(()), IspPresentation::Show) => self.compose_or_black(frame, placement),
            // Скрытый кадр: изображение видоискателя записано (статистика посчитана), экран остаётся чёрным.
            (Ok(()), IspPresentation::HideUntilColorKnown) | (Err(_), _) => self.black(frame, None),
        };
        timing_failures += self.point(frame, FRAME_POINT_COMPOSE_DONE, TimestampStage::AllCommandsDone);
        IspFrameRecording {
            isp,
            screen,
            timing_failures,
        }
    }

    /// Нового кадра нет: экран собирается из прежнего изображения видоискателя (без ISP и без
    /// импорта). Прежний кадр мозаики, если он ещё на руках, возвращается этим сабмитом.
    pub fn record_recompose(
        &mut self,
        frame: &DisplayFrame,
        release_displayed: Option<MosaicSource>,
        placement: ViewfinderPlacement,
    ) -> IspFrameRecording {
        let mut timing_failures = self.start_frame_timing(frame);
        if let Some(displayed) = release_displayed {
            displayed.record_release_for(&self.context, frame.command_buffer(), ConsumerStage::Fragment);
        }
        let screen = self.compose_or_black(frame, placement);
        timing_failures += self.point(frame, FRAME_POINT_COMPOSE_DONE, TimestampStage::AllCommandsDone);
        IspFrameRecording {
            isp: Ok(()),
            screen,
            timing_failures,
        }
    }

    /// Сброс меток слота и точка начала; возвращает число отказов.
    fn start_frame_timing(&mut self, frame: &DisplayFrame) -> u64 {
        match self
            .frame_timestamps
            .record_reset(frame.command_buffer(), frame.ring_slot())
        {
            Ok(()) => self.point(frame, FRAME_POINT_START, TimestampStage::AllCommandsDone),
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "сброс меток кадра видоискателя не записан");
                1
            }
        }
    }

    /// Точка метки; 1 — если не записана (замер кадра пропадёт, кадр — нет).
    fn point(&mut self, frame: &DisplayFrame, point: usize, stage: TimestampStage) -> u64 {
        match self
            .frame_timestamps
            .record_point(frame.command_buffer(), frame.ring_slot(), point, stage)
        {
            Ok(()) => 0,
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, point, "метка кадра видоискателя не записана");
                1
            }
        }
    }

    /// Компоновка изображения видоискателя на экран; отказ — чёрный с причиной.
    fn compose_or_black(&mut self, frame: &DisplayFrame, placement: ViewfinderPlacement) -> ScreenContent {
        match self.compose(frame, placement) {
            Ok(()) => ScreenContent::Viewfinder,
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "компоновка видоискателя не записана: чёрный кадр");
                self.black(frame, Some(error.to_string()))
            }
        }
    }

    fn compose(&mut self, frame: &DisplayFrame, placement: ViewfinderPlacement) -> Result<(), GpuError> {
        let image = self.isp.image().ok_or_else(|| GpuError::InvalidState {
            what: "изображения видоискателя ещё нет".to_owned(),
        })?;
        let source = image.sampled_view();
        let target = frame.pattern_target();
        let transform = frame.transform();
        let logical = logical_target_size(
            PlaneSize {
                width: target.extent.width,
                height: target.extent.height,
            },
            transform.quarter_turns()?,
        );
        let image_size = PlaneSize {
            width: source.extent.width,
            height: source.extent.height,
        };
        let destination =
            viewfinder_destination(image_size, logical, placement).ok_or_else(|| GpuError::InvalidArgument {
                what: format!("пустой прямоугольник видоискателя: {image_size:?} на {logical:?}"),
            })?;
        self.compositor.record(
            frame.command_buffer(),
            target,
            &CompositeRequest {
                source,
                transform,
                destination,
                background: BackgroundColor::BLACK,
                frame_slot: frame.ring_slot(),
            },
        )
    }

    /// Чёрный кадр (очистка цели). Отказ возможен только при нулевом размере цели — тогда цель
    /// не очищается вовсе (логируется); командный буфер остаётся корректным.
    fn black(&self, frame: &DisplayFrame, compose_failure: Option<String>) -> ScreenContent {
        if let Err(error) = self
            .preview
            .record_black(frame.command_buffer(), frame.pattern_target(), None)
        {
            tracing::error!(target: LOG_TARGET, %error, "чёрный кадр не записан");
        }
        ScreenContent::Black { compose_failure }
    }
}
