//! [`VideoHeadroomWorkload`] — нагрузка стенда «запас под видео» (V4): полный кадр RAW → уровни →
//! Menon 2007 → (шумодав) → цвет/охват/тон/OETF → нейтральный приёмник кадра (V5), с меткой времени
//! GPU после каждого прохода. Это стенд для плана `video`, не продуктовый видоискатель.
//!
//! # Кадр
//! 1. Вызывающий забирает кадр камеры для compute (`MosaicSource::record_acquire_for(.., Compute)`).
//! 2. [`VideoHeadroomWorkload::record`] — все проверки ДО записи команд; затем приёмник выдаёт цель
//!    (или `Busy` — кадр пропущен, ничего не записано), проходы с глобальным барьером перед каждым и
//!    меткой после каждого, передача цели приёмнику.
//! 3. Вызывающий отправляет командный буфер (с `FrameSink::submit_dependencies`), зовёт
//!    `FrameSink::after_submit`, а после fence слота — [`VideoHeadroomWorkload::collect`] (не ждёт GPU).
//!
//! # Слоты и общие плоскости
//! Плоскости и история — одни на все слоты: кадры на единственной очереди упорядочены барьером в начале
//! каждого кадра. Слоты нужны для uniform-параметров, наборов дескрипторов и меток: их можно
//! переписывать, только когда fence последнего сабмита слота сигнален (контракт вызывающего).

use ash::vk;
use camera_contract::geometry::PixelSize;
use gpu_vulkan::compute::BindingKind;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::record_compute_pass_dependency;
use gpu_vulkan::frame_sink::{FrameSink, FrameSinkDescription, FrameSinkGrant, FrameSinkStamp, FrameSinkTarget};
use gpu_vulkan::gpu_timing::{GpuTimestamps, TimestampStage};
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};

use super::config::VideoHeadroomConfig;
use super::demosaic::{MenonPass, QualityDemosaic};
use super::error::QualityError;
use super::frame_params::{
    CfaCode, FRAME_PARAMETERS_BYTES, OutputWindowOrigin, check_supported, frame_parameter_bytes,
};
use super::input::{VideoFrameInput, source_addressing};
use super::pass::PassPushConstants;
use super::planes::QualityPlanes;
use super::stages::{LevelsStage, OutputStage, OutputUniforms};
use super::temporal::{HistoryUse, TemporalDenoise};
use super::timing::{FRAME_START_POINT, HeadroomPass, HeadroomTimingCollect, point_labels, timings_from_collect};
use crate::frame_style::FrameStyleBuffers;
use crate::lens_shading_map::LensShadingMapBuffer;

/// Итог записи кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadroomRecordOutcome {
    /// Кадр записан; `history` — что сделал шумодав (`None` — шумодав выключен).
    Recorded {
        /// Использование истории шумодава.
        history: Option<HistoryUse>,
    },
    /// Приёмник занят (обратное давление): в командный буфер ничего не записано.
    SkippedSinkBusy,
}

/// Нагрузка стенда V4 (см. модуль).
///
/// Порядок полей = порядок уничтожения: проходы, ссылающиеся на плоскости, раньше самих плоскостей
/// (шумодав читает плоскости демозаика).
pub struct VideoHeadroomWorkload {
    timestamps: GpuTimestamps,
    output: OutputStage,
    levels: LevelsStage,
    temporal: Option<TemporalDenoise>,
    demosaic: QualityDemosaic,
    frame_parameters: Vec<HostBuffer>,
    /// Стиль кадра по слотам (основа тона, цвет вида, таблица вида): блоки пишутся каждый кадр, таблица — при смене.
    style_buffers: Vec<FrameStyleBuffers>,
    /// Карта виньетирования по слотам кадра (пишется, только когда карта изменилась).
    lens_shading_maps: Vec<LensShadingMapBuffer>,
    config: VideoHeadroomConfig,
    context: GpuContext,
}

impl std::fmt::Debug for VideoHeadroomWorkload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "VideoHeadroomWorkload({:?}, {:?})",
            self.demosaic, self.config
        )
    }
}

impl VideoHeadroomWorkload {
    /// Создаёт нагрузку для полезной области `extent` (на телефоне — 4096×2304, основная камера 16:9).
    pub fn new(context: &GpuContext, extent: vk::Extent2D, config: &VideoHeadroomConfig) -> Result<Self, QualityError> {
        config.validate()?;
        let local = config.local_size()?;
        // Размер группы из конфига — против лимитов ЭТОГО устройства (спецификация гарантирует только 128 нитей).
        local.validate_for_device(context)?;
        let slots = config.frame_slots;
        let demosaic = QualityDemosaic::new(context, extent, local, slots)?;
        let temporal = if config.temporal_denoise.enabled {
            Some(TemporalDenoise::new(
                context,
                demosaic.planes().linear_rgb(),
                extent,
                config.temporal_denoise,
                local,
                slots,
            )?)
        } else {
            None
        };
        let frame_parameters = (0..slots)
            .map(|_| HostBuffer::new(context, FRAME_PARAMETERS_BYTES, HostBufferUsage::UniformParameters))
            .collect::<Result<Vec<_>, _>>()?;
        let style_buffers = (0..slots)
            .map(|_| FrameStyleBuffers::new(context, config.look_lut))
            .collect::<Result<Vec<_>, _>>()?;
        let lens_shading_maps = (0..slots)
            .map(|_| LensShadingMapBuffer::new(context, config.lens_shading_map))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            timestamps: GpuTimestamps::new(context, slots, &point_labels())?,
            output: OutputStage::new(context, local, slots)?,
            levels: LevelsStage::new(context, local, slots)?,
            temporal,
            demosaic,
            frame_parameters,
            style_buffers,
            lens_shading_maps,
            config: *config,
            context: context.clone(),
        })
    }

    /// Конфиг.
    pub fn config(&self) -> &VideoHeadroomConfig {
        &self.config
    }

    /// Размер полезной области.
    pub fn extent(&self) -> vk::Extent2D {
        self.demosaic.planes().extent()
    }

    /// Плоскости демозаика (диагностика и тесты: линейный RGB камеры последнего кадра).
    pub fn planes(&self) -> &QualityPlanes {
        self.demosaic.planes()
    }

    /// Шумодав (если включён): история и её состояние.
    pub fn temporal_denoise(&self) -> Option<&TemporalDenoise> {
        self.temporal.as_ref()
    }

    /// Просит шумодав начать историю заново со следующего кадра (сверх автоматического сброса по
    /// смене поколения потока).
    pub fn reset_temporal_history(&mut self) {
        if let Some(temporal) = &mut self.temporal {
            temporal.reset_history();
        }
    }

    /// Записывает кадр в `command_buffer` (см. модуль).
    ///
    /// Все проверки — до первой команды (своей и приёмника): входы, параметры, привязки уровней, лимиты
    /// групп каждого прохода. После `begin_frame` остаётся одна проверка, которую нельзя сделать раньше, —
    /// цель, выданная приёмником; если она не подошла, кадр отменяется (`FrameSink::abort_frame`), а
    /// командный буфер (там только подготовка цели) вызывающий не отправляет.
    pub fn record(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        input: &VideoFrameInput<'_>,
        sink: &mut dyn FrameSink,
    ) -> Result<HeadroomRecordOutcome, QualityError> {
        let extent = self.extent();
        if frame_slot >= self.frame_parameters.len() {
            return Err(QualityError::InvalidInput {
                what: format!("слот {frame_slot} вне 0..{}", self.frame_parameters.len()),
            });
        }
        check_supported(input.params)?;
        let addressing = source_addressing(input.source, input.valid_area, extent)?;
        let sink_description = sink.description();
        if sink_description.extent != extent {
            return Err(QualityError::InvalidInput {
                what: format!(
                    "цель приёмника {:?} не размера конвейера {extent:?}",
                    sink_description.extent
                ),
            });
        }
        let cfa = CfaCode::from_pattern(input.params.pattern);
        // Uniform и наборы слота свободны от GPU — контракт слота; это ещё не команды.
        let parameters = &mut self.frame_parameters[frame_slot];
        parameters.write_bytes(
            0,
            &frame_parameter_bytes(input.params, addressing, OutputWindowOrigin::WHOLE_FRAME),
        )?;
        let parameters_binding = parameters.binding();
        // Основа тона, цвет вида и таблица слота; таблица больше буфера — ошибка кадра ДО команд.
        let style = &mut self.style_buffers[frame_slot];
        style.prepare(input.params)?;
        let style_bindings = style.bindings();
        // Карта больше буфера — ошибка кадра ДО команд (не молчаливый пропуск коррекции). Полезная область
        // конвейера — вся плоскость (`source_addressing` проверил, что размеры совпадают).
        let shading_map = &mut self.lens_shading_maps[frame_slot];
        shading_map.prepare(&input.params.lens_shading, PixelSize::new(extent.width, extent.height))?;
        let shading_binding = shading_map.binding();
        let levels_push = PassPushConstants::geometry(extent, cfa);
        self.levels.bind_and_check(
            frame_slot,
            input.source,
            parameters_binding,
            shading_binding,
            self.demosaic.planes().mosaic(),
            &levels_push,
        )?;
        self.demosaic.check_all(frame_slot, cfa)?;
        if let Some(temporal) = &self.temporal {
            temporal.check(frame_slot)?;
        }
        self.output.check(sink_description.format, extent)?;

        // --- Первая команда: подготовка цели приёмником ---
        let FrameSinkGrant::Ready(target) = sink.begin_frame(command_buffer, frame_slot)? else {
            return Ok(HeadroomRecordOutcome::SkippedSinkBusy);
        };
        if let Err(error) = check_granted_target(&target, sink_description) {
            sink.abort_frame(frame_slot)?;
            return Err(error);
        }
        let recorded = self.record_passes(
            command_buffer,
            frame_slot,
            input,
            &levels_push,
            OutputUniforms {
                frame_parameters: parameters_binding,
                style: style_bindings,
                lens_shading_map: shading_binding,
            },
            &target,
        );
        match recorded {
            Ok(history) => {
                sink.finish_frame(
                    command_buffer,
                    frame_slot,
                    FrameSinkStamp {
                        sequence: input.sequence,
                        presentation_time_ns: input.presentation_time_ns,
                    },
                )?;
                Ok(HeadroomRecordOutcome::Recorded { history })
            }
            Err(error) => {
                // Сюда не попасть при соблюдённых проверках выше; если всё же — приёмник не должен
                // застрять «в кадре», а буфер вызывающий не отправляет.
                sink.abort_frame(frame_slot)?;
                Err(error)
            }
        }
    }

    /// Забирает время проходов слота. Никогда не ждёт GPU (не готово — `NotReady`).
    ///
    /// Звать после fence слота и ОБЯЗАТЕЛЬНО между двумя `record` одного слота: он же подтверждает, что
    /// наборы дескрипторов слота (уровни, выход) свободны — иначе следующий `record` слота с другим кадром
    /// или целью — `InvalidState` (`ComputePipeline::bind`).
    pub fn collect(&mut self, frame_slot: usize) -> Result<HeadroomTimingCollect, QualityError> {
        if frame_slot >= self.frame_parameters.len() {
            return Err(QualityError::InvalidInput {
                what: format!("слот {frame_slot} вне 0..{}", self.frame_parameters.len()),
            });
        }
        self.levels.release_slot_after_fence(frame_slot)?;
        self.output.release_slot_after_fence(frame_slot)?;
        Ok(timings_from_collect(self.timestamps.try_collect(frame_slot)?))
    }

    /// Команды проходов кадра (после `begin_frame`; все dispatch-и проверены заранее).
    fn record_passes(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        input: &VideoFrameInput<'_>,
        levels_push: &PassPushConstants,
        output_uniforms: OutputUniforms,
        target: &FrameSinkTarget,
    ) -> Result<Option<HistoryUse>, QualityError> {
        let extent = self.extent();
        let cfa = levels_push.cfa;
        self.timestamps.record_reset(command_buffer, frame_slot)?;
        // Начало кадра: работа прошлого кадра над общими плоскостями и историей — до новых записей.
        record_compute_pass_dependency(&self.context, command_buffer);
        self.timestamps.record_point(
            command_buffer,
            frame_slot,
            FRAME_START_POINT,
            TimestampStage::ComputeShaderDone,
        )?;

        self.levels
            .record(command_buffer, frame_slot, input.source, levels_push)?;
        self.mark_pass_done(command_buffer, frame_slot, HeadroomPass::Levels)?;

        for pass in MenonPass::ORDER {
            record_compute_pass_dependency(&self.context, command_buffer);
            self.demosaic.record_pass(command_buffer, frame_slot, pass, cfa)?;
            self.mark_pass_done(command_buffer, frame_slot, HeadroomPass::Demosaic(pass))?;
        }

        let history = match &mut self.temporal {
            Some(temporal) => {
                record_compute_pass_dependency(&self.context, command_buffer);
                Some(temporal.record(command_buffer, frame_slot, input.stream)?)
            }
            None => None,
        };
        if history.is_some() {
            self.mark_pass_done(command_buffer, frame_slot, HeadroomPass::TemporalDenoise)?;
        }

        record_compute_pass_dependency(&self.context, command_buffer);
        let linear = match &self.temporal {
            Some(temporal) => temporal.output(),
            None => self.demosaic.planes().linear_rgb(),
        };
        self.output
            .record(command_buffer, frame_slot, linear, output_uniforms, target, extent)?;
        self.mark_pass_done(command_buffer, frame_slot, HeadroomPass::Output)?;
        Ok(history)
    }

    fn mark_pass_done(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        pass: HeadroomPass,
    ) -> Result<(), QualityError> {
        self.timestamps.record_point(
            command_buffer,
            frame_slot,
            pass.point_index(),
            TimestampStage::ComputeShaderDone,
        )?;
        Ok(())
    }
}

/// Цель, выданная приёмником, совпадает с его описанием и пригодна для записи compute-шейдером.
fn check_granted_target(target: &FrameSinkTarget, expected: FrameSinkDescription) -> Result<(), QualityError> {
    if target.description != expected || !target.storage.fits(BindingKind::StorageImageWrite) {
        return Err(QualityError::InvalidInput {
            what: format!(
                "приёмник выдал цель {:?} ({:?}), а описывал {expected:?} для записи compute",
                target.description, target.storage
            ),
        });
    }
    Ok(())
}
