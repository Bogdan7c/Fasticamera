//! Строки A/B «цена вида» сценария `viewfinder-stage-budget` (сессия 14o, решения 312–316).
//!
//! Строка вида — пара «вид в окне A» и «вид в окне B»; сами рецепты строит [`MeasurementLook`] из рецепта продукта,
//! так что разница окон — ровно одна вещь. Модуль чистый: рендеру ничего не шлёт (это делает заявка сценария
//! `GpuServices::show_look_for_scenario`), библиотеку пресетов не знает.
//!
//! Имена строк (`look_tone_base_agx`, …) — контракт с `scripts/` и конфигом `viewfinder.stage_budget.rows`.

use look_contract::ToneBaseModel;

use crate::measurement_look::MeasurementLook;

/// Строка вида.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LookRow {
    /// Основа AgX против прежней S-кривой (`legacy`).
    ToneBaseAgx,
    /// Основа ACES против `legacy`.
    ToneBaseAces,
    /// Основа filmic против `legacy`.
    ToneBaseFilmic,
    /// Слой вида целиком: вид продукта против нейтрали на той же основе.
    Layer,
    /// Колёса цвета: образец колёс против вида без колёс.
    Wheels,
    /// Стадия тона 14l: образец ручек тона против вида без них.
    Tone,
}

impl LookRow {
    /// Все строки вида в порядке таблицы.
    pub const ALL: [LookRow; 6] = [
        LookRow::ToneBaseAgx,
        LookRow::ToneBaseAces,
        LookRow::ToneBaseFilmic,
        LookRow::Layer,
        LookRow::Wheels,
        LookRow::Tone,
    ];

    /// Имя строки для отчёта, конфига и intent-параметра `stages`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::ToneBaseAgx => "look_tone_base_agx",
            Self::ToneBaseAces => "look_tone_base_aces",
            Self::ToneBaseFilmic => "look_tone_base_filmic",
            Self::Layer => "look_layer",
            Self::Wheels => "look_wheels",
            Self::Tone => "look_tone",
        }
    }

    /// Вид окна A («работа включена»).
    pub const fn on_look(self) -> MeasurementLook {
        match self {
            Self::ToneBaseAgx => MeasurementLook::ToneBase(ToneBaseModel::Agx),
            Self::ToneBaseAces => MeasurementLook::ToneBase(ToneBaseModel::Aces),
            Self::ToneBaseFilmic => MeasurementLook::ToneBase(ToneBaseModel::Filmic),
            Self::Layer => MeasurementLook::Product,
            Self::Wheels => MeasurementLook::WheelsSample,
            Self::Tone => MeasurementLook::ToneSample,
        }
    }

    /// Вид окна B («работа выключена»).
    pub const fn off_look(self) -> MeasurementLook {
        match self {
            Self::ToneBaseAgx | Self::ToneBaseAces | Self::ToneBaseFilmic => {
                MeasurementLook::ToneBase(ToneBaseModel::Legacy)
            }
            Self::Layer => MeasurementLook::NeutralOnProductBase,
            Self::Wheels => MeasurementLook::WithoutWheels,
            Self::Tone => MeasurementLook::WithoutTone,
        }
    }
}

#[cfg(test)]
#[path = "look_row_tests.rs"]
mod tests;
