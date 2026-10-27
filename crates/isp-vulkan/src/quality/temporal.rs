//! [`TemporalDenoise`] — временной шумодав «на пробу» (стенд V4): рекурсивный фильтр в линейном RGB
//! камеры с адаптацией к движению (формулы — `shaders/quality_temporal_denoise.comp`).
//!
//! # Известное ограничение
//! Компенсации движения нет: при движении камеры или объекта пиксель с разностью выше порога просто
//! теряет историю (шум там не подавляется). Шумодав с компенсацией движения — план `video`.
//!
//! # История
//! Владелец — этот тип: три плоскости f32 (R, G, B), запись на месте. История сбрасывается (кадр
//! пишется в неё как есть), если её ещё нет, если сменилось поколение потока ([`StreamGeneration`])
//! или вызывающий попросил [`TemporalDenoise::reset_history`]. Состояние обновляется при ЗАПИСИ
//! команд: незаотправленный командный буфер требует `reset_history` (то же правило, что у приёмника).

use ash::vk;
use gpu_vulkan::compute::{BindingKind, BindingWrite, LocalSize};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::DeviceBuffer;
use gpu_vulkan::error::GpuError;

use super::config::TemporalDenoiseConfig;
use super::error::QualityError;
use super::frame_params::CfaCode;
use super::input::StreamGeneration;
use super::pass::{PassPushConstants, QualityPass};
use super::planes::{LinearRgbPlanes, plane_bytes};
use crate::shaders::blobs;

/// Слово режима шейдера: история недействительна — записать текущий кадр.
const MODE_RESET_HISTORY: u32 = 0;
/// Слово режима шейдера: смешать с историей.
const MODE_BLEND_WITH_HISTORY: u32 = 1;

/// Привязки шейдера: три плоскости текущего кадра (чтение) и три плоскости истории (чтение-запись).
const BINDINGS: [BindingKind; 6] = [
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadWriteStorageBuffer,
    BindingKind::ReadWriteStorageBuffer,
    BindingKind::ReadWriteStorageBuffer,
];

/// Что сейчас лежит в плоскостях истории.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryState {
    /// Истории нет (создание, сброс): следующий кадр её начнёт.
    Empty,
    /// История накоплена по кадрам потока `stream`.
    Accumulating {
        /// Поколение потока, кадры которого в истории.
        stream: StreamGeneration,
    },
}

/// Что сделает проход с историей на этом кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryUse {
    /// История начата заново этим кадром.
    Restarted,
    /// Кадр смешан с историей.
    Blended,
}

/// Временной шумодав (см. модуль).
///
/// Порядок полей = порядок уничтожения: пайплайн (его наборы ссылаются на историю) → история.
pub struct TemporalDenoise {
    pass: QualityPass,
    history: [DeviceBuffer; 3],
    state: HistoryState,
    settings: TemporalDenoiseConfig,
    extent: vk::Extent2D,
}

impl std::fmt::Debug for TemporalDenoise {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "TemporalDenoise({:?}, {:?})", self.extent, self.state)
    }
}

impl TemporalDenoise {
    /// Создаёт историю размера `extent` и пайплайн, привязанный к плоскостям `input`.
    ///
    /// Контракт: буферы `input` переживают этот объект (наборы дескрипторов ссылаются на них) и имеют
    /// размер `extent` (проверяется по байтам).
    pub fn new(
        context: &GpuContext,
        input: LinearRgbPlanes<'_>,
        extent: vk::Extent2D,
        settings: TemporalDenoiseConfig,
        local: LocalSize,
        frame_slots: usize,
    ) -> Result<Self, QualityError> {
        settings.validate()?;
        let bytes = plane_bytes(extent)?;
        if [input.red, input.green, input.blue]
            .iter()
            .any(|plane| plane.size_bytes() != bytes)
        {
            return Err(QualityError::InvalidInput {
                what: format!("плоскости входа шумодава не размера {}x{}", extent.width, extent.height),
            });
        }
        let history = [
            DeviceBuffer::new(context, bytes)?,
            DeviceBuffer::new(context, bytes)?,
            DeviceBuffer::new(context, bytes)?,
        ];
        let mut pass = QualityPass::new(
            context,
            "isp-quality-temporal-denoise",
            blobs::QUALITY_TEMPORAL_DENOISE_COMP,
            &BINDINGS,
            local,
            frame_slots,
        )?;
        let resources = [
            input.red.read_only_binding(),
            input.green.read_only_binding(),
            input.blue.read_only_binding(),
            history[0].read_write_binding(),
            history[1].read_write_binding(),
            history[2].read_write_binding(),
        ];
        let writes: Vec<BindingWrite> = (0_u32..)
            .zip(resources)
            .map(|(binding, resource)| BindingWrite { binding, resource })
            .collect();
        pass.bind_all_slots(&writes)?;
        Ok(Self {
            pass,
            history,
            state: HistoryState::Empty,
            settings,
            extent,
        })
    }

    /// Состояние истории.
    pub fn history_state(&self) -> HistoryState {
        self.state
    }

    /// Сбрасывает историю: следующий кадр начнёт её заново (без смешивания).
    pub fn reset_history(&mut self) {
        self.state = HistoryState::Empty;
    }

    /// Плоскости истории = выход шумодава (после записи прохода).
    pub fn output(&self) -> LinearRgbPlanes<'_> {
        LinearRgbPlanes {
            red: &self.history[0],
            green: &self.history[1],
            blue: &self.history[2],
        }
    }

    /// Проверяет dispatch прохода без записи команд (звать до первой команды кадра).
    pub(crate) fn check(&self, frame_slot: usize) -> Result<(), GpuError> {
        self.pass.check(frame_slot, &self.push_constants(HistoryUse::Restarted))
    }

    /// Записывает проход. Барьер перед ним (вход записан прошлым проходом) пишет вызывающий.
    pub fn record(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        stream: StreamGeneration,
    ) -> Result<HistoryUse, GpuError> {
        let history_use = match self.state {
            HistoryState::Accumulating { stream: previous } if previous == stream => HistoryUse::Blended,
            HistoryState::Accumulating { .. } | HistoryState::Empty => HistoryUse::Restarted,
        };
        let push = self.push_constants(history_use);
        self.pass.record(command_buffer, frame_slot, &push)?;
        self.state = HistoryState::Accumulating { stream };
        Ok(history_use)
    }

    /// Push-константы прохода для режима истории `history_use`.
    fn push_constants(&self, history_use: HistoryUse) -> PassPushConstants {
        PassPushConstants {
            extent: self.extent,
            // Шумодав не смотрит на CFA (вход уже RGB); код любой — RGGB.
            cfa: CfaCode::from_pattern(isp_model::cfa::BayerPattern::RGGB),
            pass_mode: match history_use {
                HistoryUse::Restarted => MODE_RESET_HISTORY,
                HistoryUse::Blended => MODE_BLEND_WITH_HISTORY,
            },
            pass_parameters: [
                self.settings.strength,
                self.settings.motion_threshold_sigmas,
                self.settings.noise_floor,
                self.settings.shot_noise_gain,
            ],
        }
    }
}
