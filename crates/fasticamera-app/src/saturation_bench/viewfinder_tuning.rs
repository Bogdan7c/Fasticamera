//! Опыты над слитым проходом видоискателя: размер рабочей группы, каденция и способ сведения AWB, набор стадий.
//!
//! Это ДАННЫЕ каталога ([`ViewfinderTuning`]), GPU они не знают: в настройки работы их превращает
//! `viewfinder_variant::configure`. Продукт ничего из этого не использует — только стенд насыщения.

use serde::Serialize;

/// Размер рабочей группы (нитей по X и Y).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct WorkgroupShape {
    pub x: u32,
    pub y: u32,
}

/// Как часто считается статистика AWB в варианте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AwbSchedule {
    /// Продуктовая: каждый 4-й кадр.
    Product,
    /// На каждом кадре (цена кадра со статистикой видна целиком).
    EveryFrame,
    /// Никогда (цена прохода без статистики).
    Never,
}

/// Способ сведения статистики AWB внутри группы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AwbReductionChoice {
    /// Продуктовый (shared-память с коротким путём для выровненных групп).
    Default,
    /// Только shared-память (то же, что продуктовый; оставлено для явных записей каталога).
    SharedMemoryOnly,
    /// Прежний продукт до пакета I: через subgroup, если устройство умеет (A/B со старым).
    SubgroupWhenSupported,
}

/// Набор опытных настроек поверх продуктового видоискателя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ViewfinderTuning {
    /// Размер группы (`None` — продуктовый).
    pub workgroup: Option<WorkgroupShape>,
    /// Каденция статистики AWB.
    pub awb_schedule: AwbSchedule,
    /// Способ сведения AWB.
    pub awb_reduction: AwbReductionChoice,
    /// Только стадии «уровни + демозаик» (пол цены, как работа `viewfinder_minimal`).
    pub minimal_stages: bool,
}

impl ViewfinderTuning {
    /// Продукт без изменений.
    pub const PRODUCT: Self = Self {
        workgroup: None,
        awb_schedule: AwbSchedule::Product,
        awb_reduction: AwbReductionChoice::Default,
        minimal_stages: false,
    };

    /// Изменить каденцию AWB.
    pub const fn schedule(self, awb_schedule: AwbSchedule) -> Self {
        Self { awb_schedule, ..self }
    }

    /// Изменить размер группы.
    pub const fn workgroup(self, x: u32, y: u32) -> Self {
        Self {
            workgroup: Some(WorkgroupShape { x, y }),
            ..self
        }
    }

    /// Оставить только стадии «уровни + демозаик».
    pub const fn minimal(self) -> Self {
        Self {
            minimal_stages: true,
            ..self
        }
    }

    /// Сведение AWB через subgroup (прежний продукт).
    pub const fn subgroup_awb(self) -> Self {
        Self {
            awb_reduction: AwbReductionChoice::SubgroupWhenSupported,
            ..self
        }
    }

    /// Только shared-окно для AWB.
    pub const fn shared_memory_awb(self) -> Self {
        Self {
            awb_reduction: AwbReductionChoice::SharedMemoryOnly,
            ..self
        }
    }
}
