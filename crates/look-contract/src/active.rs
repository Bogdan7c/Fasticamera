//! Что библиотека пресетов отдаёт наружу: текущий вид и откуда он взялся.

use crate::preset::PresetId;
use crate::settings::LookSettings;

/// Совпадает ли текущий вид со значениями активного пресета.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditState {
    /// Значения ровно как у активного пресета.
    Clean,
    /// Владелец покрутил ручки поверх активного пресета и пока не сохранил.
    EditedOnTop,
}

/// Текущий вид: активный пресет, итоговые значения ручек и признак правок поверх.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveLook {
    /// Активный пресет.
    pub preset: PresetId,
    /// Итоговые значения (с учётом правок). Именно они уходят рендеру и в запрос снимка.
    pub settings: LookSettings,
    /// Есть ли правки поверх пресета.
    pub edits: EditState,
}
