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
//! 8. (только если установлен источник UI) проход UI поверх компоновки и точка «UI готов»
//!    (`ui_pass`): ставится вызывающим отдельно после шага 7 — запись видоискателя от UI не зависит.
//!    Рисуется и поверх ЧЁРНОГО кадра (камеры нет, кадра ещё не было, скрытый кадр AWB, отказ ISP): тогда
//!    у стекла фон `Absent` чёрный. Исключение — отладочная мозаика и тестовая картинка: они без UI.
//!
//! Шаги 3 и 6 записываются ВСЕГДА вместе: даже если ISP отказал, кадр принят и тем же сабмитом
//! возвращён, а сабмит ждёт acquire-fence камеры — поэтому возврат лизинга с fence этого сабмита
//! корректен при любом исходе записи ISP.

use ash::vk;
use gpu_vulkan::compositor::{BackgroundColor, CompositeRequest, ImageCompositor, LogicalRect};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampCollect, TimestampStage};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, RawMosaicPreview};
use gpu_vulkan::source_window::{HorizontalMirror, SourceWindow};
use gpu_vulkan::storage_image::SampledImageView;
use gpu_vulkan::swapchain::DisplayTargetInfo;
use isp_model::params::IspParams;
use isp_vulkan::viewfinder::{
    IspStageSet, ViewfinderError, ViewfinderFrame, ViewfinderGeometry, ViewfinderIsp, ViewfinderIspConfig,
    ViewfinderSlotReport,
};

use super::placement::{PlaneSize, logical_target_size, resolve_destination};
use super::viewfinder_stats::{FRAME_POINT_COMPOSE_DONE, FRAME_POINT_ISP_DONE, FRAME_POINT_START, FRAME_POINTS};
use crate::config::{ScopesConfig, UiOverlayConfig};
use crate::viewfinder::EffectivePlacement;

mod frame_target;
mod held;
#[cfg(test)]
mod scope_frame_tests;
mod scope_pass;
#[cfg(test)]
mod screen_tests;
mod ui_pass;

pub use frame_target::FrameTarget;
pub use held::ShowFrameAs;
pub(in crate::vulkan_backend::camera) use held::mirror_of;
use scope_pass::ScopePass;
#[cfg(test)]
pub use ui_pass::ReadyUi;
use ui_pass::UiPass;
pub use ui_pass::UiTick;

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
    /// Номер сабмита этого кадра (для подложки изображения, из которой приборы берут барьер чтения).
    pub submission: gpu_vulkan::backdrop::SubmissionSerial,
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
    /// Что реально положить на экран (решение машины непрерывности: живое, удержанное или чёрное).
    pub show: ShowFrameAs,
    /// Зеркало изображения этого кадра при показе (фронтальная камера). Запоминается вместе с картинкой:
    /// перекомпоновка без нового кадра зеркалит её так же.
    pub mirror: HorizontalMirror,
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
    Viewfinder {
        /// Прямоугольник изображения на логической цели: посчитан ОДИН раз на кадр и общий для компоновщика и
        /// для фона стекла UI (они не могут разойтись).
        destination: LogicalRect,
    },
    /// Удержанный последний кадр (смена объектива, сессия 13). Прямоугольник — как у `Viewfinder`.
    Held {
        /// Прямоугольник удержанного кадра на логической цели.
        destination: LogicalRect,
    },
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
    /// Освобождение слота рендера UI (без рендера — `Ok`).
    pub ui: Result<(), ui_render_vulkan::UiRenderError>,
}

/// Объекты GPU показа камеры (см. модуль).
pub struct CameraPipelines {
    /// Отладочная мозаика 05 и чёрный кадр.
    pub preview: RawMosaicPreview,
    isp: ViewfinderIsp,
    compositor: ImageCompositor,
    frame_timestamps: GpuTimestamps,
    /// Рендер UI поверх видоискателя: создаётся лениво при первом кадре с установленным источником.
    ui: UiPass,
    /// Цветовые приборы (14h): объекты GPU создаются по запросу экрана МЕЖДУ кадрами и уходят вместе с ним.
    scopes: ScopePass,
    /// Удержанный кадр и зеркало живого изображения (сессия 13).
    view: held::ViewState,
    context: GpuContext,
}

impl CameraPipelines {
    /// Создаёт объекты под формат цели. Слотов ISP — столько же, сколько кадров в полёте у кольца
    /// (`frame_slots` из конфига заменяется: слот ISP обязан совпадать со слотом кольца кадров).
    pub fn new(
        context: &GpuContext,
        target: &DisplayTargetInfo,
        isp_config: &ViewfinderIspConfig,
        ui_config: &UiOverlayConfig,
        scopes_config: &ScopesConfig,
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
            ui: UiPass::new(context, target, ui_config),
            scopes: ScopePass::new(context, frames_in_flight, scopes_config),
            view: held::ViewState::default(),
            context: context.clone(),
        })
    }

    /// Пересоздаёт то, что зависит от формата цели (мозаика, компоновщик и рендер UI). Вызывающий уже
    /// дождался сабмитов цели. ISP от формата цели не зависит и живёт дальше (его изображение тоже).
    /// Рендер UI не создаётся заново сразу: он поднимется на первом кадре, если источник установлен.
    pub fn rebuild_for_format(&mut self, target: &DisplayTargetInfo) -> Result<(), GpuError> {
        self.preview = RawMosaicPreview::new(&self.context, target.format, target.frames_in_flight)?;
        self.compositor = ImageCompositor::new(&self.context, target.format, target.frames_in_flight)?;
        self.ui.reset_for_format(target);
        self.scopes.reset_for_format();
        Ok(())
    }

    /// ISP видоискателя (только чтение: изображение, readback в тестах).
    pub fn isp(&self) -> &ViewfinderIsp {
        &self.isp
    }

    /// Итоги слота: вызывать для КАЖДОГО слота, выданного кольцом (его fence пройден). Заодно ISP
    /// уничтожает выведенные из обращения объекты. Никогда не ждёт GPU.
    pub fn collect_slot(&mut self, slot: usize) -> SlotCollection {
        // Слот собран: приборы освобождают его у своих наборов и уничтожают выведенные, которые больше никто не читает.
        self.scopes.collect_slot(slot);
        SlotCollection {
            isp: self.isp.collect(slot),
            frame: self.frame_timestamps.try_collect(slot),
            // Набор дескрипторов компоновщика этого слота снова можно переписывать (новое изображение
            // видоискателя после смены размера): fence слота пройден.
            compositor: self.compositor.release_slot_after_fence(slot),
            ui: self.ui.release_slot_after_fence(slot),
        }
    }

    /// Записанный кадр слота `slot` НЕ отправлен (сбой сабмита после успешной записи): части, которые помнят
    /// записанную, но не выполненную работу (загрузки картинок UI, 14g), её забывают. Сам слот освобождается
    /// обычным `collect_slot`, когда кольцо выдаст его снова.
    pub fn note_frame_not_submitted(&mut self, slot: usize) -> Result<(), ui_render_vulkan::UiRenderError> {
        self.scopes.frame_not_submitted(slot);
        self.ui.frame_not_submitted(slot)
    }

    /// Записывает новый кадр (см. порядок в описании модуля). Никогда не оставляет командный буфер
    /// «на полпути»: отказ ISP или компоновщика заменяется чёрным, приём/возврат кадра записаны всегда.
    /// `extra_reader` (`None` — читателя нет) читает тот же кадр после ISP; с ним отрезок
    /// `frame_isp_span` диагностики включает и его работу.
    pub fn record_new_frame(
        &mut self,
        frame: &FrameTarget,
        input: &NewIspFrame<'_>,
        extra_reader: Option<&mut dyn AcquiredSourceReader>,
        release_displayed: Option<MosaicSource>,
        placement: EffectivePlacement,
    ) -> IspFrameRecording {
        let command_buffer = frame.command_buffer;
        let slot = frame.ring_slot;
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
                    display_color_space: frame.display_color_space,
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
        if isp.is_ok() {
            // Картинка ISP теперь содержит кадр с этим зеркалом (перекомпоновка зеркалит её так же).
            self.view.live_mirror = input.mirror;
            // Приборы читают изображение, которое сейчас ляжет на экран (решение 190: тот же командный буфер).
            // Их время входит в отрезок «компоновка» диагностики; своё время GPU у приборов есть в `scopes.gpu_time`.
            self.record_scopes_for_new_frame(frame, input);
        }
        let screen = match (&isp, input.show) {
            (Ok(()), ShowFrameAs::Live) => self.compose_or_black(frame, placement),
            // Скрытый кадр или переход: изображение ISP записано (статистика посчитана), а на экран идёт
            // удержанный последний кадр (или чёрный, если держать нечего — холодный старт).
            (_, ShowFrameAs::Held { magnification }) => self.compose_held_or_black(frame, placement, magnification),
            // Отказ ISP при удержании не должен давать чёрный: удержанный кадр остаётся на экране.
            (Err(_), ShowFrameAs::Live) if self.view.held.is_some() => {
                self.compose_held_or_black(frame, placement, 1.0)
            }
            (Ok(()), ShowFrameAs::Black) | (Err(_), ShowFrameAs::Live | ShowFrameAs::Black) => self.black(frame, None),
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
    /// То же, что [`Self::record_recompose`], но с явным решением, что положить на экран (живое, удержанное
    /// или чёрное): так машина непрерывности ведёт кадры без нового кадра камеры.
    pub fn record_recompose_as(
        &mut self,
        frame: &FrameTarget,
        release_displayed: Option<MosaicSource>,
        placement: EffectivePlacement,
        show: ShowFrameAs,
    ) -> IspFrameRecording {
        let mut timing_failures = self.start_frame_timing(frame);
        if let Some(displayed) = release_displayed {
            displayed.record_release_for(&self.context, frame.command_buffer, ConsumerStage::Fragment);
        }
        let screen = match show {
            ShowFrameAs::Live => self.compose_or_black(frame, placement),
            ShowFrameAs::Held { magnification } => self.compose_held_or_black(frame, placement, magnification),
            ShowFrameAs::Black => self.black(frame, None),
        };
        timing_failures += self.point(frame, FRAME_POINT_COMPOSE_DONE, TimestampStage::AllCommandsDone);
        IspFrameRecording {
            isp: Ok(()),
            screen,
            timing_failures,
        }
    }

    /// Кадр без изображения видоискателя (камера закрыта, потока нет, кадра ещё не было): чёрная цель.
    /// Нужен, чтобы UI рисовался поверх чёрного (экран камеры не пропадает без кадра, сессия 12): метки
    /// времени пишутся здесь, потому что проходу UI нужна их точка «UI готов».
    ///
    /// Чёрная цель проверяется до записи, поэтому при `Err` командный буфер чист (как у мозаики). Метка
    /// «начало» ставится уже после очистки: цифра `frame_total` чуть меньше настоящей, зато без очистки
    /// ничего не записано.
    pub fn record_black_frame(
        &mut self,
        frame: &FrameTarget,
        release_displayed: Option<MosaicSource>,
    ) -> Result<IspFrameRecording, GpuError> {
        self.preview
            .record_black(frame.command_buffer, frame.pattern_target, release_displayed)?;
        let mut timing_failures = self.start_frame_timing(frame);
        timing_failures += self.point(frame, FRAME_POINT_COMPOSE_DONE, TimestampStage::AllCommandsDone);
        Ok(IspFrameRecording {
            isp: Ok(()),
            screen: ScreenContent::Black { compose_failure: None },
            timing_failures,
        })
    }

    /// Сброс меток слота и точка начала; возвращает число отказов.
    fn start_frame_timing(&mut self, frame: &FrameTarget) -> u64 {
        match self
            .frame_timestamps
            .record_reset(frame.command_buffer, frame.ring_slot)
        {
            Ok(()) => self.point(frame, FRAME_POINT_START, TimestampStage::AllCommandsDone),
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "сброс меток кадра видоискателя не записан");
                1
            }
        }
    }

    /// Точка метки; 1 — если не записана (замер кадра пропадёт, кадр — нет).
    fn point(&mut self, frame: &FrameTarget, point: usize, stage: TimestampStage) -> u64 {
        match self
            .frame_timestamps
            .record_point(frame.command_buffer, frame.ring_slot, point, stage)
        {
            Ok(()) => 0,
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, point, "метка кадра видоискателя не записана");
                1
            }
        }
    }

    /// Компоновка изображения видоискателя на экран; отказ — чёрный с причиной.
    fn compose_or_black(&mut self, frame: &FrameTarget, placement: EffectivePlacement) -> ScreenContent {
        match self.compose(frame, placement) {
            Ok(destination) => ScreenContent::Viewfinder { destination },
            Err(error) => {
                tracing::warn!(target: LOG_TARGET, %error, "компоновка видоискателя не записана: чёрный кадр");
                self.black(frame, Some(error.to_string()))
            }
        }
    }

    /// Раскладка считается здесь ОДИН раз на кадр; возвращённый прямоугольник идёт и в компоновщик, и (через
    /// [`ScreenContent::Viewfinder`]) в фон стекла UI.
    fn compose(&mut self, frame: &FrameTarget, placement: EffectivePlacement) -> Result<LogicalRect, GpuError> {
        let (source, window) = match (self.isp.image(), self.live_window()) {
            (Some(image), Some(window)) => (image.sampled_view(), window),
            _ => {
                return Err(GpuError::InvalidState {
                    what: "изображения видоискателя ещё нет".to_owned(),
                });
            }
        };
        self.compose_windowed(frame, source, window, placement)
    }

    /// Компоновка целой готовой картинки `source` по действующему месту (тесты на offscreen-цели).
    #[cfg(test)]
    fn compose_view(
        &mut self,
        frame: &FrameTarget,
        source: SampledImageView,
        placement: EffectivePlacement,
    ) -> Result<LogicalRect, GpuError> {
        self.compose_windowed(frame, source, SourceWindow::full(source.extent), placement)
    }

    /// Компоновка окна `window` картинки `source`. Прямоугольник на экране считается по РАЗМЕРУ СОДЕРЖИМОГО
    /// (не картинки: у картинки постоянной ёмкости она больше) и не зависит от увеличения и зеркала.
    fn compose_windowed(
        &mut self,
        frame: &FrameTarget,
        source: SampledImageView,
        window: SourceWindow,
        placement: EffectivePlacement,
    ) -> Result<LogicalRect, GpuError> {
        let content = window.content();
        let destination = viewfinder_layout(
            frame,
            PlaneSize {
                width: content.width,
                height: content.height,
            },
            placement,
        )?;
        self.compositor.record_windowed(
            frame.command_buffer,
            frame.pattern_target,
            &CompositeRequest {
                source,
                transform: frame.transform,
                destination,
                background: BackgroundColor::BLACK,
                frame_slot: frame.ring_slot,
            },
            &window,
        )?;
        Ok(destination)
    }

    /// Чёрный кадр (очистка цели). Отказ возможен только при нулевом размере цели — тогда цель
    /// не очищается вовсе (логируется); командный буфер остаётся корректным.
    fn black(&self, frame: &FrameTarget, compose_failure: Option<String>) -> ScreenContent {
        if let Err(error) = self
            .preview
            .record_black(frame.command_buffer, frame.pattern_target, None)
        {
            tracing::error!(target: LOG_TARGET, %error, "чёрный кадр не записан");
        }
        ScreenContent::Black { compose_failure }
    }
}

/// Логический размер цели кадра (физический размер с учётом поворота дисплея).
fn logical_size(frame: &FrameTarget) -> Result<PlaneSize, GpuError> {
    let target = frame.pattern_target;
    Ok(logical_target_size(
        PlaneSize {
            width: target.extent.width,
            height: target.extent.height,
        },
        frame.transform.quarter_turns()?,
    ))
}

/// Прямоугольник изображения видоискателя на логической цели кадра (по действующему месту).
fn viewfinder_layout(
    frame: &FrameTarget,
    image_size: PlaneSize,
    placement: EffectivePlacement,
) -> Result<LogicalRect, GpuError> {
    let logical = logical_size(frame)?;
    resolve_destination(image_size, logical, placement).ok_or_else(|| GpuError::InvalidArgument {
        what: format!("пустой прямоугольник видоискателя: {image_size:?} на {logical:?}"),
    })
}
