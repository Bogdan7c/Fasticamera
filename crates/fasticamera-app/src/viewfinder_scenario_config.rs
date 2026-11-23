//! Секция `camera.scenarios.viewfinder`: параметры сценариев видоискателя сессии 07
//! (`viewfinder`, `viewfinder-latency`, `viewfinder-stage-budget`, `viewfinder-endurance`,
//! `video-headroom-bench`). Значения по умолчанию документируют источник: план сессии 07
//! (`user/foundation/07-gpu-viewfinder-isp.md`) и решение 45 README.
//!
//! Ток (A/B-окна, знак, пропуски) и длительность длинного прогона берутся из соседней секции
//! `camera.scenarios.power` — здесь их не дублируем.

use camera_contract::ids::CameraTarget;
use camera_contract::stream::{InvalidFrameRate, StreamMode, TargetFrameRate};
use isp_vulkan::viewfinder::IspStage;
use serde::{Deserialize, Serialize};
use shell_contract::thermal::ThermalStatus;

use crate::camera_scenario_config::{LensRoleSetting, ReadoutSetting, ResolvedStep};
use crate::viewfinder_autostart::ViewfinderModeConfig;
use crate::viewfinder_latency_profile::{
    LatencyProfileConfig, LatencyProfilesError, default_latency_profiles, validate_latency_profiles,
};

/// Стадия ISP в конфиге (имена — snake_case, как в отчётах).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IspStageSetting {
    Levels,
    /// Коррекция виньетирования своей таблицей (14f); идёт сразу после уровней.
    LensShading,
    WhiteBalance,
    Demosaic,
    Color,
    GamutCompression,
    Tone,
    /// Цвет вида (насыщенность, сочность, полосы оттенка) над светом экрана после основы (14a).
    LookColor,
    /// Таблица вида (LUT; 14a): тетраэдральная выборка по буферу.
    LookLut,
    /// Мягкое сжатие охвата выхода основы AgX (у прежней основы стадия пуста, цена нулевая).
    OutputGamutCompression,
    Oetf,
    AwbStatistics,
}

impl IspStageSetting {
    /// Все стадии в порядке прохода.
    pub const ALL: [Self; 12] = [
        Self::Levels,
        Self::LensShading,
        Self::WhiteBalance,
        Self::Demosaic,
        Self::Color,
        Self::GamutCompression,
        Self::Tone,
        Self::LookColor,
        Self::LookLut,
        Self::OutputGamutCompression,
        Self::Oetf,
        Self::AwbStatistics,
    ];

    pub fn to_stage(self) -> IspStage {
        match self {
            Self::Levels => IspStage::Levels,
            Self::LensShading => IspStage::LensShading,
            Self::WhiteBalance => IspStage::WhiteBalance,
            Self::Demosaic => IspStage::Demosaic,
            Self::Color => IspStage::Color,
            Self::GamutCompression => IspStage::GamutCompression,
            Self::Tone => IspStage::Tone,
            Self::LookColor => IspStage::LookColor,
            Self::LookLut => IspStage::LookLut,
            Self::OutputGamutCompression => IspStage::OutputGamutCompression,
            Self::Oetf => IspStage::Oetf,
            Self::AwbStatistics => IspStage::AwbStatistics,
        }
    }

    /// Имя для отчёта (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            Self::Levels => "levels",
            Self::LensShading => "lens_shading",
            Self::WhiteBalance => "white_balance",
            Self::Demosaic => "demosaic",
            Self::Color => "color",
            Self::GamutCompression => "gamut_compression",
            Self::Tone => "tone",
            Self::LookColor => "look_color",
            Self::LookLut => "look_lut",
            Self::OutputGamutCompression => "output_gamut_compression",
            Self::Oetf => "oetf",
            Self::AwbStatistics => "awb_statistics",
        }
    }
}

/// Строка таблицы «стадия → мс GPU → мА» (решение 45).
///
/// В JSON — строка с именем: `"levels"`, …, `"awb_statistics"`, `"whole_viewfinder"` (так её проще
/// передать intent-параметром `stages` через Kotlin).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum StageBudgetRowSetting {
    /// Цена одной стадии: A = все стадии, B = все без этой.
    Stage(IspStageSetting),
    /// «Весь видоискатель»: A = ISP со всеми стадиями, B = отладочная мозаика 05. Строка «камера без
    /// ISP» (абсолютные числа мозаики) берётся из B-окон этой же строки — отдельного замера не нужно.
    WholeViewfinder,
}

impl StageBudgetRowSetting {
    /// Порядок по умолчанию: все стадии прохода, затем весь видоискатель.
    pub fn default_rows() -> Vec<Self> {
        IspStageSetting::ALL
            .into_iter()
            .map(Self::Stage)
            .chain(std::iter::once(Self::WholeViewfinder))
            .collect()
    }

    /// Имя для отчёта и для intent-параметра `stages`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stage(stage) => stage.name(),
            Self::WholeViewfinder => "whole_viewfinder",
        }
    }

    /// Строка по имени из отчёта.
    pub fn from_name(name: &str) -> Result<Self, ViewfinderScenarioConfigError> {
        Self::default_rows()
            .into_iter()
            .find(|row| row.name() == name)
            .ok_or_else(|| ViewfinderScenarioConfigError::UnknownStageRow { name: name.to_owned() })
    }
}

impl TryFrom<String> for StageBudgetRowSetting {
    type Error = ViewfinderScenarioConfigError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::from_name(&name)
    }
}

impl From<StageBudgetRowSetting> for String {
    fn from(row: StageBudgetRowSetting) -> Self {
        row.name().to_owned()
    }
}

/// Порог теплового статуса в конфиге (`ThermalStatus` без serde-чтения).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThermalStatusSetting {
    None,
    Light,
    Moderate,
    Severe,
    Critical,
    Emergency,
    Shutdown,
}

impl ThermalStatusSetting {
    pub fn to_status(self) -> ThermalStatus {
        match self {
            Self::None => ThermalStatus::None,
            Self::Light => ThermalStatus::Light,
            Self::Moderate => ThermalStatus::Moderate,
            Self::Severe => ThermalStatus::Severe,
            Self::Critical => ThermalStatus::Critical,
            Self::Emergency => ThermalStatus::Emergency,
            Self::Shutdown => ThermalStatus::Shutdown,
        }
    }
}

/// `viewfinder`: минутный smoke продуктового видоискателя.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderSmokeConfig {
    /// Длина замера, мс. Минута — как `raw-stream` (3600 кадров).
    pub measure_ms: u64,
    /// Цель прохода ISP (`isp_dispatch`, p95), мс; план 07 — меньше 3 мс на 4K-входе. Только пометка
    /// `isp_dispatch_target` в отчёте, не провал (решение 56: на Adreno мс GPU растягивает DVFS).
    pub isp_dispatch_budget_ms: f64,
    /// Нижняя граница частоты сенсора по `SENSOR_TIMESTAMP`, к/с: 95 % от 60, как шаги `camera-stream`.
    pub min_sensor_fps: f64,
    /// Допустимое относительное расхождение «кадров через ISP в секунду» и частоты сенсора, а также
    /// «кадров через ISP» и «показанных новых кадров». 3 %: окно замера и счётчики рендера сдвинуты на
    /// тик маршрутизатора (50 мс из 60 с) плюс кадры в полёте — доли процента; больше — потеря кадров.
    pub frame_rate_tolerance: f64,
    /// Сколько переполнений джиттер-буфера в минуту считается дрейфом часов камеры против экрана
    /// (решение 53). Камера 60,03 Гц против экрана 60,00 Гц даёт один лишний кадр за ~33 с, то есть
    /// ~1,8 в минуту; 4 — запас вдвое на разброс частот и фазу. Больше — потеря кадров, провал.
    pub max_jitter_overflow_per_minute: f64,
    /// Какую долю ожидаемых публикаций статистики AWB обязан дать замер. Ожидаемое число — обработанные кадры,
    /// делённые на `statistics_every_n_frames` рендера (прореживание: 60 к/с → 15 Гц). Не 1: исполнитель оценки
    /// бывает занят (кадр статистики пропускается со счётчиком), а окно замера режет период на краях.
    pub min_awb_publish_fraction: f64,
}

impl Default for ViewfinderSmokeConfig {
    fn default() -> Self {
        Self {
            measure_ms: 60_000,
            isp_dispatch_budget_ms: 3.0,
            min_sensor_fps: 57.0,
            frame_rate_tolerance: 0.03,
            max_jitter_overflow_per_minute: 4.0,
            min_awb_publish_fraction: 0.5,
        }
    }
}

/// `viewfinder-latency`: A/B политик линии кадра.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderLatencyConfig {
    /// Длина окна одной политики, мс. 20 с — ~1200 кадров: p95 задержки устойчив, а 3 пары ABBA
    /// укладываются в 2 минуты.
    pub window_ms: u64,
    /// Сколько пар (ABBA, как у тока). Не меньше трёх — виден разброс, не больше
    /// [`MAX_LATENCY_PAIRS`] — окна и их нарушения растут с числом пар, а прогон длиннее
    /// получаса уже не A/B на одном нагреве.
    pub pairs: u32,
    /// Сколько первых миллисекунд окна не считается: оценка стоимости кадра и окна статистики
    /// перестраиваются после смены политики.
    pub warmup_ms: u64,
    /// Ровно два профиля A/B: первый — база (`baseline`), второй — проверяемый (`tuned`). Профиль =
    /// политика линии кадра + поздний захват + сход на ближнюю линию (`depth_reduction`) + режим ADPF +
    /// подача кадров видоискателя (`viewfinder_pacing`: `latest_wins` | `jitter_buffer`). Имена — подписи отчёта (непустые, до 16
    /// символов, разные). По умолчанию: база = поведение до решений 53–54, проверяемый = всё новое сразу.
    pub profiles: Vec<LatencyProfileConfig>,
    /// Минимум замеров задержки в окне: меньше — окно не годится.
    pub min_samples_per_window: u64,
    /// На сколько мс p50 задержки проверяемого профиля должен отличаться от базы, чтобы разница
    /// считалась значимой (меньше — шум сопоставления меток). Ниже базы — улучшение, выше — ухудшение.
    pub min_latency_gain_ms: f64,
    /// На сколько (абсолютная доля) доля опозданий проверяемого профиля должна отличаться от базы,
    /// чтобы разница считалась значимой: меньше — улучшение, больше — ухудшение.
    pub max_miss_ratio_increase: f64,
    /// На сколько кадров/с ISP-частота проверяемого профиля должна отличаться от базы, чтобы разница
    /// считалась значимой (0,5 к/с — шум счёта кадров на краях 20-секундного окна).
    pub min_fps_delta: f64,
}

impl Default for ViewfinderLatencyConfig {
    fn default() -> Self {
        Self {
            window_ms: 20_000,
            pairs: 3,
            warmup_ms: 3_000,
            profiles: default_latency_profiles(),
            min_samples_per_window: 100,
            min_latency_gain_ms: 1.0,
            max_miss_ratio_increase: 0.01,
            min_fps_delta: 0.5,
        }
    }
}

/// `viewfinder-stage-budget`: таблица решения 45.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StageBudgetConfig {
    /// Какие строки мерить и в каком порядке. Каждая строка — A/B по `power.ab` (по умолчанию
    /// 3 пары × 2 окна × 60 с ≈ 6 мин). Подмножество на один запуск — intent-параметр `stages`.
    pub rows: Vec<StageBudgetRowSetting>,
}

impl Default for StageBudgetConfig {
    fn default() -> Self {
        Self {
            rows: StageBudgetRowSetting::default_rows(),
        }
    }
}

/// `viewfinder-endurance`: пороги «нашли троттлинг».
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderEnduranceConfig {
    /// Тепловой статус, начиная с которого это троттлинг. `moderate` — Android с него начинает
    /// ограничивать производительность (`PowerManager.THERMAL_STATUS_MODERATE`).
    pub throttling_thermal_status: ThermalStatusSetting,
    /// Частота сенсора ниже этой в любой минуте — просадка, к/с.
    pub min_sensor_fps: f64,
    /// Во сколько раз медиана времени ISP последней минуты может превысить первую. 1,3: DVFS
    /// колеблется на десяток процентов, рост на треть — снижение частоты GPU.
    pub max_isp_time_growth: f64,
}

impl Default for ViewfinderEnduranceConfig {
    fn default() -> Self {
        Self {
            throttling_thermal_status: ThermalStatusSetting::Moderate,
            min_sensor_fps: 57.0,
            max_isp_time_growth: 1.3,
        }
    }
}

/// `video-headroom-bench`: стенд запаса под видео (V4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct VideoHeadroomScenarioConfig {
    /// Режим камеры: основная 16:9@60 (4096×2304) — видео-режим плана `video`.
    pub readout: ReadoutSetting,
    pub frame_rate: u32,
    /// Частота экрана во время стенда, Гц: UI на 120 Гц (план 07, «Замер»).
    pub display_refresh_hz: f32,
    /// Длина стенда, мс. 10 минут — план 07; для быстрой проверки уменьшить здесь.
    pub duration_ms: u64,
    /// Шаг снимков (тепло, время GPU), мс.
    pub snapshot_interval_ms: u64,
}

impl Default for VideoHeadroomScenarioConfig {
    fn default() -> Self {
        Self {
            readout: ReadoutSetting::SixteenByNine,
            frame_rate: 60,
            display_refresh_hz: 120.0,
            duration_ms: 600_000,
            snapshot_interval_ms: 60_000,
        }
    }
}

impl VideoHeadroomScenarioConfig {
    /// Бюджет кадра при частоте сенсора, мс (16,7 мс при 60 к/с).
    pub fn frame_budget_ms(&self) -> f64 {
        1_000.0 / f64::from(self.frame_rate.max(1))
    }
}

/// Секция `camera.scenarios.viewfinder`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderScenarioConfig {
    /// Камера по роли. Основная (`wide`) — как у продукта (автозапуск 07).
    pub role: LensRoleSetting,
    /// Режим продуктового видоискателя (4:3@60 RAW10 — как автозапуск и `raw-stream`).
    pub mode: ViewfinderModeConfig,
    /// Сколько ждать `Streaming`, мс (как `raw-stream`).
    pub streaming_timeout_ms: u64,
    /// Сколько ждать первого показанного кадра камеры после `Streaming`, мс.
    pub render_live_timeout_ms: u64,
    /// Сколько ждать после первого кадра до замера, мс: автоэкспозиция HAL сходится секунды (05).
    pub viewfinder_settle_ms: u64,
    /// Шаг стадии `progress`, мс.
    pub progress_interval_ms: u64,
    /// Потолок выборки задержки «сенсор → экран» (как `raw-stream`).
    pub latency_samples_limit: usize,
    /// Задержка выше этой — ошибка сопоставления, а не замер, мс.
    pub latency_plausible_max_ms: u64,
    pub smoke: ViewfinderSmokeConfig,
    pub latency: ViewfinderLatencyConfig,
    pub stage_budget: StageBudgetConfig,
    pub endurance: ViewfinderEnduranceConfig,
    pub headroom: VideoHeadroomScenarioConfig,
}

impl Default for ViewfinderScenarioConfig {
    fn default() -> Self {
        Self {
            role: LensRoleSetting::Wide,
            mode: ViewfinderModeConfig::default(),
            streaming_timeout_ms: 15_000,
            render_live_timeout_ms: 10_000,
            viewfinder_settle_ms: 2_000,
            progress_interval_ms: 10_000,
            latency_samples_limit: 20_000,
            latency_plausible_max_ms: 500,
            smoke: ViewfinderSmokeConfig::default(),
            latency: ViewfinderLatencyConfig::default(),
            stage_budget: StageBudgetConfig::default(),
            endurance: ViewfinderEnduranceConfig::default(),
            headroom: VideoHeadroomScenarioConfig::default(),
        }
    }
}

/// Неверная секция `camera.scenarios.viewfinder`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ViewfinderScenarioConfigError {
    #[error("viewfinder.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("viewfinder.{field} = {value}: нужно конечное положительное число")]
    NotPositive { field: &'static str, value: f64 },
    #[error("viewfinder.smoke.frame_rate_tolerance = {value}: доля от 0 до 1 (не включая 1)")]
    ToleranceOutOfRange { value: f64 },
    #[error("viewfinder.latency.pairs = {pairs}: нужно не меньше 3 пар")]
    TooFewLatencyPairs { pairs: u32 },
    #[error("viewfinder.latency.pairs = {pairs}: больше {MAX_LATENCY_PAIRS} пар не бывает")]
    TooManyLatencyPairs { pairs: u32 },
    #[error("viewfinder.latency.warmup_ms ({warmup_ms}) должно быть меньше window_ms ({window_ms})")]
    WarmupNotShorterThanWindow { warmup_ms: u64, window_ms: u64 },
    #[error("viewfinder.headroom.snapshot_interval_ms больше duration_ms")]
    HeadroomIntervalLongerThanRun,
    #[error("viewfinder.stage_budget: строк нет — мерить нечего")]
    NoStageRows,
    #[error("viewfinder.stage_budget: неизвестная строка {name:?}")]
    UnknownStageRow { name: String },
    #[error("viewfinder.stage_budget: строка {name} повторяется")]
    DuplicateStageRow { name: &'static str },
    #[error("{0}")]
    LatencyProfiles(#[from] LatencyProfilesError),
    #[error("частота: {0}")]
    InvalidFrameRate(#[from] InvalidFrameRate),
}

/// Нижняя граница пар A/B задержки (как решение 45 для тока).
pub const MIN_LATENCY_PAIRS: u32 = 3;
/// Верхняя граница пар A/B задержки: 12 пар × 2 окна × 20 с ≈ 8 мин по умолчанию; разности пар
/// идут в строку результата, их число должно быть ограничено.
pub const MAX_LATENCY_PAIRS: u32 = 12;

impl ViewfinderScenarioConfig {
    /// Продуктовый режим: цель и режим потока в словаре контракта.
    pub fn product_step(&self) -> Result<ResolvedStep, ViewfinderScenarioConfigError> {
        self.step_with(self.mode.readout, self.mode.frame_rate)
    }

    /// Режим стенда запаса под видео (16:9@60 по умолчанию).
    pub fn headroom_step(&self) -> Result<ResolvedStep, ViewfinderScenarioConfigError> {
        self.step_with(self.headroom.readout, self.headroom.frame_rate)
    }

    fn step_with(
        &self,
        readout: ReadoutSetting,
        frame_rate: u32,
    ) -> Result<ResolvedStep, ViewfinderScenarioConfigError> {
        Ok(ResolvedStep {
            target: CameraTarget::Role(self.role.to_contract()),
            mode: StreamMode {
                raw_format: self.mode.raw_format.to_contract(),
                readout: readout.to_contract(),
                frame_rate: TargetFrameRate::new(frame_rate)?,
            },
        })
    }

    pub fn validate(&self) -> Result<(), ViewfinderScenarioConfigError> {
        for (field, value) in [
            ("streaming_timeout_ms", self.streaming_timeout_ms),
            ("render_live_timeout_ms", self.render_live_timeout_ms),
            ("progress_interval_ms", self.progress_interval_ms),
            ("latency_plausible_max_ms", self.latency_plausible_max_ms),
            ("smoke.measure_ms", self.smoke.measure_ms),
            ("latency.window_ms", self.latency.window_ms),
            ("latency.min_samples_per_window", self.latency.min_samples_per_window),
            ("headroom.duration_ms", self.headroom.duration_ms),
            ("headroom.snapshot_interval_ms", self.headroom.snapshot_interval_ms),
        ] {
            if value == 0 {
                return Err(ViewfinderScenarioConfigError::Zero { field });
            }
        }
        if self.latency_samples_limit == 0 {
            return Err(ViewfinderScenarioConfigError::Zero {
                field: "latency_samples_limit",
            });
        }
        for (field, value) in [
            ("smoke.isp_dispatch_budget_ms", self.smoke.isp_dispatch_budget_ms),
            ("smoke.min_sensor_fps", self.smoke.min_sensor_fps),
            ("smoke.min_awb_publish_fraction", self.smoke.min_awb_publish_fraction),
            (
                "smoke.max_jitter_overflow_per_minute",
                self.smoke.max_jitter_overflow_per_minute,
            ),
            ("latency.min_latency_gain_ms", self.latency.min_latency_gain_ms),
            ("latency.min_fps_delta", self.latency.min_fps_delta),
            ("endurance.min_sensor_fps", self.endurance.min_sensor_fps),
            ("endurance.max_isp_time_growth", self.endurance.max_isp_time_growth),
            (
                "headroom.display_refresh_hz",
                f64::from(self.headroom.display_refresh_hz),
            ),
        ] {
            // Через сравнение: NaN не проходит и отвергается.
            if !(value.is_finite() && value > 0.0) {
                return Err(ViewfinderScenarioConfigError::NotPositive { field, value });
            }
        }
        let increase = self.latency.max_miss_ratio_increase;
        if !(increase.is_finite() && (0.0..1.0).contains(&increase)) {
            return Err(ViewfinderScenarioConfigError::NotPositive {
                field: "latency.max_miss_ratio_increase",
                value: increase,
            });
        }
        let awb_fraction = self.smoke.min_awb_publish_fraction;
        if awb_fraction > 1.0 {
            return Err(ViewfinderScenarioConfigError::NotPositive {
                field: "smoke.min_awb_publish_fraction (доля не больше 1)",
                value: awb_fraction,
            });
        }
        let tolerance = self.smoke.frame_rate_tolerance;
        if !(tolerance.is_finite() && (0.0..1.0).contains(&tolerance)) {
            return Err(ViewfinderScenarioConfigError::ToleranceOutOfRange { value: tolerance });
        }
        if self.latency.pairs < MIN_LATENCY_PAIRS {
            return Err(ViewfinderScenarioConfigError::TooFewLatencyPairs {
                pairs: self.latency.pairs,
            });
        }
        if self.latency.pairs > MAX_LATENCY_PAIRS {
            return Err(ViewfinderScenarioConfigError::TooManyLatencyPairs {
                pairs: self.latency.pairs,
            });
        }
        if self.latency.warmup_ms >= self.latency.window_ms {
            return Err(ViewfinderScenarioConfigError::WarmupNotShorterThanWindow {
                warmup_ms: self.latency.warmup_ms,
                window_ms: self.latency.window_ms,
            });
        }
        validate_latency_profiles(&self.latency.profiles)?;
        if self.headroom.snapshot_interval_ms > self.headroom.duration_ms {
            return Err(ViewfinderScenarioConfigError::HeadroomIntervalLongerThanRun);
        }
        validate_rows(&self.stage_budget.rows)?;
        self.product_step()?;
        self.headroom_step()?;
        Ok(())
    }
}

/// Строки таблицы: не пусто, без повторов.
pub fn validate_rows(rows: &[StageBudgetRowSetting]) -> Result<(), ViewfinderScenarioConfigError> {
    if rows.is_empty() {
        return Err(ViewfinderScenarioConfigError::NoStageRows);
    }
    for (index, row) in rows.iter().enumerate() {
        if rows[..index].contains(row) {
            return Err(ViewfinderScenarioConfigError::DuplicateStageRow { name: row.name() });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "viewfinder_scenario_config_tests.rs"]
mod tests;
