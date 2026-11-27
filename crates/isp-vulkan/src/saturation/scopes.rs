//! [`ScopesSaturation`] — цветовые приборы (`ColorScopes::record`: накопление + показ) под стендом насыщения.
//!
//! # Чему соответствует в продукте
//! Один кадр работы = то, что `render-loop` пишет на каждый показанный кадр
//! (`pipelines/scope_pass.rs::ScopePass::record`): `ColorScopes::record` по картинке видоискателя, правило —
//! `ResolvedScope::resolve(запрос, настройки модели, охват)`.
//!
//! # Как готовится вход
//! Картинка видоискателя считается ОДИН раз в конструкторе тем же `ViewfinderIsp` ([`ViewfinderSaturation`]),
//! дальше в прогоне насыщения считается только прибор: картинка не меняется, значит и счётчики не меняются —
//! это нужно тестам (счётчики прибора == эталон на той же картинке бит в бит).
//!
//! # Откуда умолчания
//! * настройки модели — `ScopeModelSettings::default()` (`ScopesConfig::default().model`);
//! * вид прибора и оси — `ScopeKind::Waveform` и `VectorscopeAxes::YCbCrBt709` (умолчание осей экрана калибровки
//!   `CalibrationConfig`); вид выбирает пользователь, поэтому перебирайте виды отдельными прогонами;
//! * размер картинки прибора `[estimate]` 540×360: реальный размер даёт раскладка экрана калибровки
//!   (`camera-screen/calibration/layout.rs::scope_size_for_zone`) по размеру зоны и плотности экрана; цифра
//!   взята как порядок величины зоны прибора на телефоне 1080 px шириной и проверяется на самом телефоне;
//! * слотов 2 — столько же, сколько у видоискателя (`ColorScopesConfig::default`).

use camera_contract::geometry::PixelSize;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::saturation::{
    DigestPart, FrameRecording, PassTimeLog, PassTimeSummary, SaturationWorkload, SlotId, WorkloadDigest,
};
use isp_model::params::IspParams;
use scope_contract::{HistogramMode, ScopeImageSize, ScopeKind, ScopeRequest, ScopeScan, VectorscopeAxes};
use scope_model::{ResolvedScope, ScopeModelSettings};

use super::error::{FirstFailure, SaturationWorkError};
use super::scene::SceneContent;
use super::scopes_diagnostic::{AccumulateDiagnostic, DiagnosticRule};
use super::source::RawFrameHolder;
use super::timing::log_collect;
use super::viewfinder::{PASS_LOG_CAPACITY_FRAMES, ViewfinderSaturation, ViewfinderSaturationConfig};
use crate::scopes::{ColorScopes, ColorScopesConfig, GpuScopeRule, ScopeFrame, ScopeInput};
use crate::viewfinder::gamut_for_display;

/// Размер картинки прибора по умолчанию `[estimate]`: см. описание модуля.
pub const DEFAULT_PICTURE_SIZE_PX: (u32, u32) = (540, 360);
/// Метки проходов приборов (точки `ColorScopes`: отрезок кончается меткой).
const PASS_LABELS: [&str; 2] = ["accumulate-end", "render-end"];

/// Настройки работы «приборы».
#[derive(Debug, Clone)]
pub struct ScopesSaturationConfig {
    /// Как получить картинку видоискателя (размер кадра, стадии...). Читатель картинки ставится в `Compute`,
    /// содержимое кадра — [`Self::input_content`].
    pub viewfinder: ViewfinderSaturationConfig,
    /// Что показывает кадр: `Natural` — сцена, `UniformGray` — серый (худший случай конкуренции `atomicAdd`).
    pub input_content: SceneContent,
    /// Какой прибор.
    pub kind: ScopeKind,
    /// Оси вектороскопа (для остальных приборов не важны).
    pub vectorscope_axes: VectorscopeAxes,
    /// Режим гистограммы (для остальных приборов не важен).
    pub histogram_mode: HistogramMode,
    /// Шкала диаграммы CIE (для остальных приборов не важна).
    pub cie_diagram: scope_contract::CieDiagram,
    /// Как глаз читает картинку (положение телефона).
    pub scan: ScopeScan,
    /// Размер картинки прибора.
    pub picture_size: ScopeImageSize,
    /// Настройки модели приборов (размеры сеток, прореживание, правило показа).
    pub settings: ScopeModelSettings,
    /// Слотов кадров приборов.
    pub frame_slots: usize,
    /// Диагностический вариант накопления (только стенд; `None` — продуктовый шейдер). Только для приборов с
    /// целочисленной проекцией: waveform, parade, вектороскоп Y'CbCr.
    pub diagnostic: Option<AccumulateDiagnostic>,
}

impl Default for ScopesSaturationConfig {
    fn default() -> Self {
        Self {
            viewfinder: ViewfinderSaturationConfig::default(),
            input_content: SceneContent::default(),
            kind: ScopeKind::Waveform,
            vectorscope_axes: VectorscopeAxes::YCbCrBt709,
            histogram_mode: HistogramMode::default(),
            cie_diagram: scope_contract::CieDiagram::default(),
            scan: ScopeScan::Upright,
            picture_size: ScopeImageSize::try_new(DEFAULT_PICTURE_SIZE_PX.0, DEFAULT_PICTURE_SIZE_PX.1)
                .unwrap_or_else(|_| unreachable!("540x360 внутри предела словаря приборов")),
            settings: ScopeModelSettings::default(),
            frame_slots: ColorScopesConfig::default().frame_slots,
            diagnostic: None,
        }
    }
}

impl ScopesSaturationConfig {
    /// Проверка без GPU.
    pub fn validate(&self) -> Result<(), SaturationWorkError> {
        self.viewfinder.validate()?;
        if self.frame_slots == 0 {
            return Err(SaturationWorkError::invalid_config(
                "frame_slots = 0: нужен хотя бы один слот",
            ));
        }
        if self.diagnostic.is_some()
            && ((self.kind == ScopeKind::Vectorscope && self.vectorscope_axes == VectorscopeAxes::OklabAb)
                || self.kind == ScopeKind::Cie)
        {
            return Err(SaturationWorkError::invalid_config(
                "диагностика накопления есть только у целочисленных приборов (не OKLab и не CIE)",
            ));
        }
        self.settings
            .validate()
            .map_err(|error| SaturationWorkError::invalid_config(format!("настройки модели приборов: {error}")))?;
        Ok(())
    }

    /// Конфиг видоискателя-источника: содержимое кадра и читатель картинки — как нужно приборам.
    fn viewfinder_for_scopes(&self) -> ViewfinderSaturationConfig {
        ViewfinderSaturationConfig {
            content: self.input_content,
            reader: ConsumerStage::Compute,
            ..self.viewfinder
        }
    }
}

/// Счётчики работы.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScopesCounters {
    /// Кадров приборов записано.
    pub frames_recorded: u64,
    /// Кадров освобождено после fence.
    pub frames_released: u64,
}

/// Работа «цветовые приборы» (см. описание модуля).
pub struct ScopesSaturation {
    scopes: ColorScopes,
    rule: ResolvedScope,
    config: ScopesSaturationConfig,
    /// Источник картинки: живёт дольше приборов (они читают его картинку).
    input: ViewfinderSaturation,
    times: PassTimeLog,
    counters: ScopesCounters,
    failure: FirstFailure,
}

impl ScopesSaturation {
    /// Создаёт работу: считает картинку видоискателя один раз, готовит приборы и их картинку.
    pub fn new(
        context: &GpuContext,
        config: ScopesSaturationConfig,
        params: IspParams,
        holder: Box<dyn RawFrameHolder>,
    ) -> Result<Self, SaturationWorkError> {
        config.validate()?;
        let gamut = gamut_for_display(config.viewfinder.display);
        let request = ScopeRequest {
            kind: config.kind,
            vectorscope_axes: config.vectorscope_axes,
            vectorscope_zoom: scope_contract::VectorscopeZoom::Normal,
            cie_diagram: config.cie_diagram,
            histogram_mode: config.histogram_mode,
            scan: config.scan,
            image_size: config.picture_size,
        };
        let rule = ResolvedScope::resolve(&request, &config.settings, gamut)
            .map_err(|error| SaturationWorkError::invalid_config(format!("правило прибора: {error}")))?;
        let input = ViewfinderSaturation::new(context, config.viewfinder_for_scopes(), params, holder)?;
        let extra_shaders: Vec<_> = config.diagnostic.iter().map(|diagnostic| diagnostic.shader()).collect();
        let mut scopes = ColorScopes::with_extra_accumulators(
            context,
            &ColorScopesConfig {
                frame_slots: config.frame_slots,
                counter_sizing: config.settings.clone(),
            },
            &extra_shaders,
        )?;
        // Картинка готовится МЕЖДУ кадрами (как в продукте): `record` её не создаёт.
        scopes.prepare_picture(config.picture_size)?;
        Ok(Self {
            scopes,
            rule,
            config,
            input,
            times: PassTimeLog::new(&PASS_LABELS, PASS_LOG_CAPACITY_FRAMES),
            counters: ScopesCounters::default(),
            failure: FirstFailure::default(),
        })
    }

    /// Сколько слотов кадров у работы.
    pub fn slots(&self) -> usize {
        self.scopes.frame_slots()
    }

    /// Настройки.
    pub fn config(&self) -> &ScopesSaturationConfig {
        &self.config
    }

    /// Счётчики.
    pub fn counters(&self) -> ScopesCounters {
        self.counters
    }

    /// Итог по проходам: «накопление», «показ».
    pub fn pass_times(&self) -> PassTimeSummary {
        self.times.summary()
    }

    /// Обнуляет журнал времён и счётчики.
    pub fn reset_statistics(&mut self) {
        self.times.clear();
        self.counters = ScopesCounters::default();
    }

    /// Первая ошибка, случившаяся внутри `record_frame`/`release_slot_after_fence`.
    pub fn take_failure(&mut self) -> Option<SaturationWorkError> {
        self.failure.take()
    }

    /// Источник картинки (видоискатель, посчитанный один раз).
    pub fn input(&self) -> &ViewfinderSaturation {
        &self.input
    }

    /// Приборы (чтение: счётчики диагностики).
    pub fn scopes(&self) -> &ColorScopes {
        &self.scopes
    }

    /// Размер содержимого картинки видоискателя, с которой считались приборы.
    pub fn input_size(&self) -> Option<PixelSize> {
        self.input.image().map(|image| {
            let content = image.content_extent();
            PixelSize::new(content.width, content.height)
        })
    }

    fn record_inner(&mut self, recording: FrameRecording) -> Result<(), SaturationWorkError> {
        let slot = recording.slot().index();
        let slots = self.scopes.frame_slots();
        if slot >= slots {
            return Err(SaturationWorkError::SlotOutOfRange { slot, slots });
        }
        let image = self.input.image().ok_or_else(|| SaturationWorkError::EmptyFrame {
            what: "картинка видоискателя не создана".to_owned(),
        })?;
        let (view, window) = (image.sampled_view(), image.content_window());
        let diagnostic_rule = self.config.diagnostic.map(|diagnostic| DiagnosticRule {
            inner: &self.rule,
            diagnostic,
        });
        let rule: &dyn GpuScopeRule = match &diagnostic_rule {
            Some(diagnostic_rule) => diagnostic_rule,
            None => &self.rule,
        };
        self.scopes.record(
            recording.command_buffer(),
            &ScopeFrame {
                input: ScopeInput { view, window },
                rule,
                settings: &self.config.settings,
                picture_size: self.config.picture_size,
                frame_slot: slot,
            },
        )?;
        self.counters.frames_recorded += 1;
        Ok(())
    }

    fn release_inner(&mut self, slot: SlotId) -> Result<(), SaturationWorkError> {
        let report = self.scopes.release_slot_after_fence(slot.index())?;
        self.counters.frames_released += 1;
        log_collect(&mut self.times, &report.gpu_time, &PASS_LABELS);
        Ok(())
    }
}

impl ScopesSaturation {
    /// Дайджест выхода приборов: две части — `counters` (ячейки сетки последней записи) и `picture` (картинка
    /// прибора). Звать после конца прогона стенда и `wait_idle`. Работает в любой сборке, в том числе на телефоне
    /// (`ColorScopes::digest_last_output_blocking`).
    pub fn output_digest(&self) -> Result<WorkloadDigest, SaturationWorkError> {
        let digest =
            self.scopes
                .digest_last_output_blocking()
                .map_err(|error| SaturationWorkError::DigestUnavailable {
                    what: error.to_string(),
                })?;
        Ok(WorkloadDigest::from_parts(vec![
            DigestPart {
                label: "counters",
                digest: digest.counters,
            },
            DigestPart {
                label: "picture",
                digest: digest.picture,
            },
        ]))
    }
}

impl SaturationWorkload for ScopesSaturation {
    fn record_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError> {
        self.record_inner(recording).map_err(|error| self.failure.note(error))
    }

    fn release_slot_after_fence(&mut self, slot: SlotId) {
        if let Err(error) = self.release_inner(slot) {
            self.failure.note(error);
        }
    }
}

impl ScopesSaturation {
    /// Работа на синтетическом RAW10 (ПК, фича `test-support`).
    pub fn new_synthetic(
        context: &GpuContext,
        config: ScopesSaturationConfig,
        params: IspParams,
    ) -> Result<Self, SaturationWorkError> {
        config.validate()?;
        let viewfinder = config.viewfinder_for_scopes();
        let frame = super::scene::synthesize_raw10(
            viewfinder.frame_size,
            viewfinder.content,
            viewfinder.raw10_row_padding_bytes,
        )?;
        let holder = super::source::synthetic_raw10_holder(context, &frame)?;
        Self::new(context, config, params, holder)
    }
}
