//! Пары состояний работы для строк `viewfinder-stage-budget` (сессия 16a).
//!
//! Строка таблицы — A/B-замер: «работа включена» (окно A) против «работа выключена» (окно B). С 16a «работа» — не
//! только стадия ISP, а любое полное состояние рычагов, которое сценарий умеет задать:
//! * рисовальщик видоискателя (ISP или отладочная мозаика);
//! * набор стадий ISP;
//! * набор подавленных работ GPU (цветовые приборы, рисование UI);
//! * частота экрана и удерживаемое касание.
//!
//! Модуль чистый: имя строки → пара [`WorkState`] (что в A и что в B) и метрика цены в мс GPU ([`CostMetric`]).
//! Ни render, ни сервисов он не знает: применяет состояние `stage_budget_apply`, проверяет, что оно действует,
//! `stage_budget_window`. Старые строки выражены тем же способом (подавление [`GpuWorkSuppression::NONE`], частота
//! «не трогать»), поэтому их поведение не изменилось.

use isp_vulkan::viewfinder::IspStageSet;
use render_loop::viewfinder::{GpuWorkSuppression, SuppressibleGpuWork, ViewfinderPainter};

use crate::measurement_look::MeasurementLook;
use crate::power_measurement::{AbPowerExperiment, StageSetting};
use crate::viewfinder_scenario_config::StageBudgetRowSetting;

/// Частота экрана в окне, как её задаёт строка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayDemand {
    /// Частоту не трогаем (все строки, кроме `display_120hz`).
    Untouched,
    /// Запрошено 120 Гц и на всё окно удерживается касание (OxygenOS сама поднимает экран, пока идут касания).
    Hz120WithHeldTouch,
    /// Запрошено 60 Гц, касаний нет.
    Hz60,
}

impl DisplayDemand {
    /// Какую частоту запрашиваем у платформы и ждём в окне, Гц; `None` — не трогаем.
    pub const fn target_hz(self) -> Option<f32> {
        match self {
            Self::Untouched => None,
            Self::Hz120WithHeldTouch => Some(120.0),
            Self::Hz60 => Some(60.0),
        }
    }

    /// Нужно ли в начале окна просить у драйвера скрипта удерживаемое касание.
    pub const fn holds_touch(self) -> bool {
        matches!(self, Self::Hz120WithHeldTouch)
    }
}

/// Полное состояние рычагов в одном окне.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkState {
    pub painter: ViewfinderPainter,
    pub stages: IspStageSet,
    pub suppression: GpuWorkSuppression,
    pub display: DisplayDemand,
    /// Какой вид показать на рендере (14o). `None` — строка вида не трогает: рендер показывает вид продукта и
    /// заявка сценария на вид не ставится.
    pub look: Option<MeasurementLook>,
}

impl WorkState {
    /// Продукт: рисовальщик ISP, все стадии, ничего не подавлено, частоту не трогаем.
    pub const PRODUCT: Self = Self {
        painter: ViewfinderPainter::Isp,
        stages: IspStageSet::ALL,
        suppression: GpuWorkSuppression::NONE,
        display: DisplayDemand::Untouched,
        look: None,
    };

    fn with_look(look: MeasurementLook) -> Self {
        Self {
            look: Some(look),
            ..Self::PRODUCT
        }
    }

    fn with_stages(stages: IspStageSet) -> Self {
        Self {
            stages,
            ..Self::PRODUCT
        }
    }
}

/// Чем считается цена строки в мс GPU (колонка «мс GPU» таблицы).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostMetric {
    /// Проход ISP: цена одной стадии.
    IspDispatch,
    /// Кадр целиком: цена замены рисовальщика, всего продукта, частоты экрана.
    FrameTotal,
    /// Проход UI поверх видоискателя. В окне B он подавлен, то есть НЕ выполнялся: его время там — ноль по
    /// определению, а не последнее скользящее окно рендера (оно при подавлении залипает на старых числах).
    UiOverlay,
    /// Проход цветовых приборов; в окне B подавлен (как у UI).
    ColorScopes,
}

impl CostMetric {
    /// В окне B эта работа не выполнялась вовсе, и её время там считается нулём.
    pub const fn absent_when_off_means_zero(self) -> bool {
        matches!(self, Self::UiOverlay | Self::ColorScopes)
    }
}

/// Пара состояний строки и метрика цены.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowStates {
    /// Окно A: работа включена.
    pub on: WorkState,
    /// Окно B: работа выключена.
    pub off: WorkState,
    pub cost: CostMetric,
}

impl RowStates {
    /// Пара состояний строки.
    pub fn of(row: StageBudgetRowSetting) -> Self {
        let both_suppressed = GpuWorkSuppression::NONE
            .suppressing(SuppressibleGpuWork::ColorScopes)
            .suppressing(SuppressibleGpuWork::UiOverlayDrawing);
        match row {
            StageBudgetRowSetting::Stage(stage) => Self {
                on: WorkState::PRODUCT,
                off: WorkState::with_stages(IspStageSet::ALL.without(stage.to_stage())),
                cost: CostMetric::IspDispatch,
            },
            StageBudgetRowSetting::WholeViewfinder => Self {
                on: WorkState::PRODUCT,
                off: WorkState {
                    painter: ViewfinderPainter::DebugMosaic,
                    ..WorkState::PRODUCT
                },
                cost: CostMetric::FrameTotal,
            },
            StageBudgetRowSetting::ColorScopes => Self {
                on: WorkState::PRODUCT,
                off: WorkState {
                    suppression: GpuWorkSuppression::NONE.suppressing(SuppressibleGpuWork::ColorScopes),
                    ..WorkState::PRODUCT
                },
                cost: CostMetric::ColorScopes,
            },
            StageBudgetRowSetting::UiOverlay => Self {
                on: WorkState::PRODUCT,
                off: WorkState {
                    suppression: GpuWorkSuppression::NONE.suppressing(SuppressibleGpuWork::UiOverlayDrawing),
                    ..WorkState::PRODUCT
                },
                cost: CostMetric::UiOverlay,
            },
            StageBudgetRowSetting::WholeProduct => Self {
                on: WorkState::PRODUCT,
                off: WorkState {
                    painter: ViewfinderPainter::DebugMosaic,
                    suppression: both_suppressed,
                    ..WorkState::PRODUCT
                },
                cost: CostMetric::FrameTotal,
            },
            StageBudgetRowSetting::Look(row) => Self {
                on: WorkState::with_look(row.on_look()),
                off: WorkState::with_look(row.off_look()),
                cost: CostMetric::FrameTotal,
            },
            StageBudgetRowSetting::Display120Hz => Self {
                on: WorkState {
                    display: DisplayDemand::Hz120WithHeldTouch,
                    ..WorkState::PRODUCT
                },
                off: WorkState {
                    display: DisplayDemand::Hz60,
                    ..WorkState::PRODUCT
                },
                cost: CostMetric::FrameTotal,
            },
        }
    }

    /// Состояние для положения «работа включена / выключена» (A / B).
    pub fn state_for(&self, stage: StageSetting) -> WorkState {
        match stage {
            StageSetting::On => self.on,
            StageSetting::Off => self.off,
        }
    }

    /// Трогает ли строка рычаг вида (заявка сценария на вид рендера).
    pub fn drives_look(&self) -> bool {
        self.on.look.is_some() || self.off.look.is_some()
    }

    /// Трогает ли строка рычаг подавления GPU (старые строки — нет, и команда `SuppressGpuWork` для них не уходит).
    pub fn drives_suppression(&self) -> bool {
        !self.on.suppression.is_none() || !self.off.suppression.is_none()
    }
}

/// Сколько окон подряд, начиная с `first`, идут окна A (ABBA: соседние A бывают на стыке пар — `A B | B A | A B`).
/// Одно удерживаемое касание покрывает их все, чтобы второе нажатие не оборвало первое. Окон всего `total`.
pub fn consecutive_a_windows(first: usize, total: usize) -> usize {
    (first..total)
        .take_while(|index| AbPowerExperiment::stage_of(*index) == StageSetting::On)
        .count()
}

#[cfg(test)]
#[path = "stage_budget_states_tests.rs"]
mod tests;
