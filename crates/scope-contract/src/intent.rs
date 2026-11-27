//! Намерения экрана: экран ничего не считает, он только просит «покажи такой прибор такого размера»
//! или «скрой приборы». Где картинка прибора стоит на экране, решает экран сам (решение 93) —
//! рендеру это знать не нужно.

use serde::{Deserialize, Serialize};

use crate::kind::{ScopeKind, VectorscopeAxes, VectorscopeZoom};
use crate::scan::ScopeScan;
use crate::size::ScopeImageSize;

/// Что показать: тип прибора, оси вектороскопа и размер картинки.
///
/// Оси хранятся всегда, даже когда тип — не вектороскоп: смена типа свайпом не должна терять
/// выбранный в меню стандарт осей.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeRequest {
    /// Какой прибор.
    pub kind: ScopeKind,
    /// Стандарт осей вектороскопа.
    pub vectorscope_axes: VectorscopeAxes,
    /// Увеличение вектороскопа (решение 325). Хранится, как и оси, при любом типе прибора: свайп не теряет выбор.
    /// Старые записи без поля читаются как обычный масштаб.
    #[serde(default)]
    pub vectorscope_zoom: VectorscopeZoom,
    /// Как глаз читает показанную картинку слева направо (положение телефона): горизонталь waveform и
    /// parade. Вектороскоп от поля не зависит; смена поля — только параметры, ресурсы не пересоздаются.
    pub scan: ScopeScan,
    /// Размер картинки прибора в пикселях экрана.
    pub image_size: ScopeImageSize,
}

/// Намерение экрана о приборах.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeIntent {
    /// Показывать прибор (заменяет прежний запрос целиком: смена типа, осей или размера).
    Show(ScopeRequest),
    /// Скрыть приборы: расчёт прекращается, ресурсы отпускаются (закрытые приборы ничего не стоят).
    Hide,
}
